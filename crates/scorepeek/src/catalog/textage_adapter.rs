use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use encoding_rs::SHIFT_JIS;
use sha2::{Digest, Sha256};

use super::adapter::{
    AdapterError, SourceRevision, snapshot_from_parts, validate_source_id, validate_text,
};
use super::federation::{
    Chart, ChartKey, Difficulty, DisplayVariantKind, PlayType, SourceChartObservation,
    SourceObservation, SourcePolicy, SourceSnapshot, TextageObservation,
};

pub(super) const MAX_TEXTAGE_FILE_BYTES: usize = 1024 * 1024;
const MAX_TEXTAGE_BUNDLE_BYTES: usize = 3 * MAX_TEXTAGE_FILE_BYTES;
const MAX_TEXTAGE_SONGS: usize = 10_000;
const MAX_TEXTAGE_ROWS: usize = 50_000;
const MAX_JS_TEXT_BYTES: usize = 512;

const TITLE_CONSTANTS: &[(&str, JsAtom)] = &[
    ("VERINDEX", JsAtom::Integer(0)),
    ("IDINDEX", JsAtom::Integer(1)),
    ("OPTINDEX", JsAtom::Integer(2)),
    ("GENREINDEX", JsAtom::Integer(3)),
    ("ARTISTINDEX", JsAtom::Integer(4)),
    ("TITLEINDEX", JsAtom::Integer(5)),
    ("SUBTITLEINDEX", JsAtom::Integer(6)),
];
const AVAILABILITY_CONSTANTS: &[(&str, JsAtom)] = &[
    ("A", JsAtom::Integer(10)),
    ("B", JsAtom::Integer(11)),
    ("C", JsAtom::Integer(12)),
    ("D", JsAtom::Integer(13)),
    ("E", JsAtom::Integer(14)),
    ("F", JsAtom::Integer(15)),
];

pub(super) struct TextageLiveAdapter;

impl TextageLiveAdapter {
    /// Parses the three mutable Textage tables without executing JavaScript.
    ///
    /// # Errors
    ///
    /// Returns an error when decoding would replace a Windows-31J byte, the bounded assignment
    /// grammar drifts, joined rows are missing or inconsistent, or the supplied revision does not
    /// match the framed digest of all three exact inputs.
    pub(super) fn parse(
        title_bytes: &[u8],
        availability_bytes: &[u8],
        chart_bytes: &[u8],
        revision: SourceRevision,
    ) -> Result<SourceSnapshot, AdapterError> {
        let byte_size = validate_bundle_size([title_bytes, availability_bytes, chart_bytes])?;

        let titles = parse_table(
            &decode_windows_31j(title_bytes, "Textage titletbl.js")?,
            "Textage titletbl.js",
            "titletbl",
            TITLE_CONSTANTS,
            &[],
            &["SS"],
            true,
        )?;
        let availability = parse_table(
            &decode_windows_31j(availability_bytes, "Textage actbl.js")?,
            "Textage actbl.js",
            "actbl",
            AVAILABILITY_CONSTANTS,
            &["pspver"],
            &[],
            false,
        )?;
        let chart_data = parse_table(
            &decode_windows_31j(chart_bytes, "Textage datatbl.js")?,
            "Textage datatbl.js",
            "datatbl",
            &[],
            &[],
            &[],
            false,
        )?;
        if availability.is_empty() || availability.len() > MAX_TEXTAGE_SONGS {
            return Err(AdapterError::TooManyRecords {
                actual: availability.len(),
                maximum: MAX_TEXTAGE_SONGS,
            });
        }

        let mut observations = Vec::with_capacity(availability.len());
        let mut accepted_chart_count = 0_usize;
        for (source_slug, availability_row) in availability {
            validate_textage_slug(&source_slug)?;
            let title_row = titles.get(&source_slug).ok_or_else(|| {
                invalid_field(
                    "titletbl",
                    format!("missing row for actbl source ID {source_slug:?}"),
                )
            })?;
            let data_row = chart_data.get(&source_slug).ok_or_else(|| {
                invalid_field(
                    "datatbl",
                    format!("missing row for actbl source ID {source_slug:?}"),
                )
            })?;
            let title = parse_title_row(title_row)
                .map_err(|error| with_source_context(error, &source_slug))?;
            let availability = parse_availability_row(&availability_row)
                .map_err(|error| with_source_context(error, &source_slug))?;
            let data = parse_chart_data_row(data_row)
                .map_err(|error| with_source_context(error, &source_slug))?;
            let source_song_id = format!("{source_slug}#{}", title.numeric_id);
            validate_source_id(&source_song_id)?;
            let charts = build_charts(
                &source_song_id,
                availability.status,
                &availability.levels,
                &availability.options,
                &data.notes,
            )?;
            accepted_chart_count = accepted_chart_count.saturating_add(charts.len());
            observations.push(SourceObservation::Textage(TextageObservation {
                source_song_id,
                title: title.title,
                artist: title.artist,
                version: title.version,
                title_kind: DisplayVariantKind::OfficialDisplay,
                charts,
                infinitas_flag: availability.status & 0b0010 != 0,
                bpm_min: data.bpm_min,
                bpm_max: data.bpm_max,
            }));
        }
        let record_count = observations.len().saturating_add(accepted_chart_count);
        if record_count > MAX_TEXTAGE_ROWS {
            return Err(AdapterError::TooManyRecords {
                actual: record_count,
                maximum: MAX_TEXTAGE_ROWS,
            });
        }
        let content_sha256 = textage_bundle_digest([
            ("titletbl.js", title_bytes),
            ("actbl.js", availability_bytes),
            ("datatbl.js", chart_bytes),
        ]);
        snapshot_from_parts(
            SourcePolicy::textage(),
            revision,
            content_sha256,
            byte_size,
            record_count,
            observations,
        )
    }
}

fn validate_bundle_size<const N: usize>(files: [&[u8]; N]) -> Result<usize, AdapterError> {
    let mut byte_size = 0_usize;
    for bytes in files {
        validate_file_size(bytes)?;
        byte_size = byte_size
            .checked_add(bytes.len())
            .ok_or(AdapterError::SourceTooLarge {
                actual: usize::MAX,
                maximum: MAX_TEXTAGE_BUNDLE_BYTES,
            })?;
    }
    if byte_size > MAX_TEXTAGE_BUNDLE_BYTES {
        return Err(AdapterError::SourceTooLarge {
            actual: byte_size,
            maximum: MAX_TEXTAGE_BUNDLE_BYTES,
        });
    }
    Ok(byte_size)
}

fn validate_file_size(bytes: &[u8]) -> Result<(), AdapterError> {
    if bytes.len() > MAX_TEXTAGE_FILE_BYTES {
        return Err(AdapterError::SourceTooLarge {
            actual: bytes.len(),
            maximum: MAX_TEXTAGE_FILE_BYTES,
        });
    }
    Ok(())
}

fn decode_windows_31j<'a>(
    bytes: &'a [u8],
    resource: &'static str,
) -> Result<Cow<'a, str>, AdapterError> {
    SHIFT_JIS
        .decode_without_bom_handling_and_without_replacement(bytes)
        .ok_or(AdapterError::InvalidEncoding { resource })
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum JsAtom {
    Integer(u32),
    String(String),
}

impl JsAtom {
    fn integer(&self, field: &'static str) -> Result<u32, AdapterError> {
        match self {
            Self::Integer(value) => Ok(*value),
            Self::String(_) => Err(invalid_field(field, "must be an integer")),
        }
    }

    fn string(&self, field: &'static str) -> Result<String, AdapterError> {
        match self {
            Self::String(value) => Ok(value.clone()),
            Self::Integer(_) => Err(invalid_field(field, "must be a string")),
        }
    }
}

struct TitleRow {
    numeric_id: u32,
    version: String,
    artist: String,
    title: String,
}

fn parse_title_row(row: &[JsAtom]) -> Result<TitleRow, AdapterError> {
    if !(6..=8).contains(&row.len()) {
        return Err(invalid_field(
            "titletbl row",
            "must contain between 6 and 8 fields",
        ));
    }
    let version = row[0].integer("titletbl version")?;
    let numeric_id = row[1].integer("titletbl numeric ID")?;
    if numeric_id == 0 {
        return Err(invalid_field("titletbl numeric ID", "must be positive"));
    }
    let option = row[2].integer("titletbl option")?;
    if option > 1 {
        return Err(invalid_field("titletbl option", "must be zero or one"));
    }
    textage_display_text("genre", &row[3].string("titletbl genre")?)?;
    let artist = textage_display_text("artist", &row[4].string("titletbl artist")?)?;
    let title = textage_display_text("title", &row[5].string("titletbl title")?)?;
    for value in &row[6..] {
        let optional = value.string("titletbl optional display fragment")?;
        if optional.len() > MAX_JS_TEXT_BYTES || optional.chars().any(char::is_control) {
            return Err(invalid_field(
                "titletbl optional display fragment",
                "must be bounded and contain no control characters",
            ));
        }
    }
    Ok(TitleRow {
        numeric_id,
        version: version.to_string(),
        artist,
        title,
    })
}

struct AvailabilityRow {
    status: u8,
    levels: [u8; 11],
    options: [u8; 11],
}

fn parse_availability_row(row: &[JsAtom]) -> Result<AvailabilityRow, AdapterError> {
    if !(23..=24).contains(&row.len()) {
        return Err(invalid_field(
            "actbl row",
            "must contain 23 numeric fields and at most one display suffix",
        ));
    }
    let status = u8::try_from(row[0].integer("actbl status")?)
        .map_err(|_| invalid_field("actbl status", "must fit in one byte"))?;
    if status > 15 {
        return Err(invalid_field("actbl status", "must be a four-bit value"));
    }
    let mut levels = [0_u8; 11];
    let mut options = [0_u8; 11];
    for chart_type in 0..11 {
        levels[chart_type] = u8::try_from(row[chart_type * 2 + 1].integer("actbl chart level")?)
            .map_err(|_| invalid_field("actbl chart level", "must fit in one byte"))?;
        options[chart_type] = u8::try_from(row[chart_type * 2 + 2].integer("actbl chart option")?)
            .map_err(|_| invalid_field("actbl chart option", "must fit in one byte"))?;
        if levels[chart_type] > 15 || options[chart_type] > 15 {
            return Err(invalid_field(
                "actbl chart pair",
                "level and option must be four-bit values",
            ));
        }
    }
    if let Some(suffix) = row.get(23) {
        let suffix = suffix.string("actbl display suffix")?;
        if suffix.len() > MAX_JS_TEXT_BYTES || suffix.chars().any(char::is_control) {
            return Err(invalid_field(
                "actbl display suffix",
                "must be bounded and contain no control characters",
            ));
        }
    }
    Ok(AvailabilityRow {
        status,
        levels,
        options,
    })
}

struct ChartDataRow {
    notes: [u32; 11],
    bpm_min: u16,
    bpm_max: u16,
}

fn parse_chart_data_row(row: &[JsAtom]) -> Result<ChartDataRow, AdapterError> {
    if row.len() != 12 {
        return Err(invalid_field(
            "datatbl row",
            "must contain 11 note counts and one BPM string",
        ));
    }
    let mut notes = [0_u32; 11];
    for (index, target) in notes.iter_mut().enumerate() {
        *target = row[index].integer("datatbl note count")?;
    }
    let bpm = row[11].string("datatbl BPM")?;
    let (minimum, maximum) = bpm.split_once('～').map_or((&*bpm, &*bpm), |parts| parts);
    if maximum.contains('～') {
        return Err(invalid_field("datatbl BPM", "contains multiple ranges"));
    }
    let bpm_min = minimum
        .parse::<u16>()
        .map_err(|_| invalid_field("datatbl BPM", "minimum is not a positive integer"))?;
    let bpm_max = maximum
        .parse::<u16>()
        .map_err(|_| invalid_field("datatbl BPM", "maximum is not a positive integer"))?;
    if bpm_min == 0 || bpm_min > bpm_max {
        return Err(invalid_field(
            "datatbl BPM",
            "minimum must be positive and no greater than maximum",
        ));
    }
    Ok(ChartDataRow {
        notes,
        bpm_min,
        bpm_max,
    })
}

fn build_charts(
    source_song_id: &str,
    status: u8,
    levels: &[u8; 11],
    options: &[u8; 11],
    notes: &[u32; 11],
) -> Result<Vec<SourceChartObservation>, AdapterError> {
    const CHART_TYPES: &[(usize, PlayType, Difficulty, &str)] = &[
        (1, PlayType::Single, Difficulty::Beginner, "sp_beginner"),
        (2, PlayType::Single, Difficulty::Normal, "sp_normal"),
        (3, PlayType::Single, Difficulty::Hyper, "sp_hyper"),
        (4, PlayType::Single, Difficulty::Another, "sp_another"),
        (
            5,
            PlayType::Single,
            Difficulty::Leggendaria,
            "sp_leggendaria",
        ),
        (7, PlayType::Double, Difficulty::Normal, "dp_normal"),
        (8, PlayType::Double, Difficulty::Hyper, "dp_hyper"),
        (9, PlayType::Double, Difficulty::Another, "dp_another"),
        (
            10,
            PlayType::Double,
            Difficulty::Leggendaria,
            "dp_leggendaria",
        ),
    ];
    let mut charts = Vec::new();
    for &(chart_type, play_type, difficulty, suffix) in CHART_TYPES {
        let level = levels[chart_type];
        let note_count = notes[chart_type];
        if level == 0 || note_count == 0 {
            continue;
        }
        if !(1..=12).contains(&level) {
            return Err(invalid_field(
                "Textage chart",
                format!("{source_song_id:?} {suffix} has invalid level {level}"),
            ));
        }
        let mut product_versions = BTreeSet::new();
        if status & 0b0001 != 0 && options[chart_type] & 0b0100 != 0 {
            product_versions.insert("ac".to_owned());
        }
        let infinitas_chart = match chart_type {
            1 => status & 0b0100 != 0,
            5 | 10 => status & 0b1000 != 0,
            _ => options[chart_type] & 0b0100 != 0 || status & 0b1000 != 0,
        };
        if status & 0b0010 != 0 && infinitas_chart {
            product_versions.insert("inf".to_owned());
        }
        if product_versions.is_empty() {
            product_versions.insert("cs".to_owned());
        }
        charts.push(SourceChartObservation {
            chart: Chart {
                key: ChartKey {
                    play_type,
                    difficulty,
                },
                level,
                notes: note_count,
            },
            source_chart_id: format!("{source_song_id}:{suffix}"),
            product_versions,
            primary: true,
        });
    }
    Ok(charts)
}

fn parse_table(
    input: &str,
    resource: &'static str,
    table_name: &'static str,
    expected_constants: &[(&str, JsAtom)],
    ignored_string_assignments: &[&str],
    dynamic_integer_assignments: &[&str],
    allow_static_fontcolor: bool,
) -> Result<BTreeMap<String, Vec<JsAtom>>, AdapterError> {
    let constants: BTreeMap<_, _> = expected_constants
        .iter()
        .map(|(name, value)| ((*name).to_owned(), value.clone()))
        .collect();
    let mut observed = BTreeMap::new();
    let mut observed_ignored = BTreeSet::new();
    let mut parser = Parser::new(input, resource);
    loop {
        parser.skip_trivia()?;
        let name = parser.identifier()?;
        parser.skip_trivia()?;
        parser.expect_char('=')?;
        parser.skip_trivia()?;
        if name == table_name {
            let fixed_constants_match = constants
                .iter()
                .all(|(name, value)| observed.get(name) == Some(value));
            if !fixed_constants_match
                || observed.len() != constants.len() + dynamic_integer_assignments.len()
                || observed_ignored.len() != ignored_string_assignments.len()
            {
                return Err(parser.error(format!(
                    "constant assignments before {table_name} do not match the declared contract"
                )));
            }
            let table = parser.object(&observed, allow_static_fontcolor)?;
            parser.skip_trivia()?;
            parser.expect_char(';')?;
            return Ok(table);
        }
        let value = parser.atom(&constants, false)?;
        if ignored_string_assignments.contains(&name.as_str()) {
            if !matches!(value, JsAtom::String(_)) || !observed_ignored.insert(name.clone()) {
                return Err(parser.error(format!("{name} must be a string")));
            }
        } else if dynamic_integer_assignments.contains(&name.as_str()) {
            if !matches!(value, JsAtom::Integer(0..=255))
                || observed.insert(name.clone(), value).is_some()
            {
                return Err(parser.error(format!("invalid or duplicate constant {name:?}")));
            }
        } else {
            let Some(expected) = constants.get(&name) else {
                return Err(parser.error(format!("unexpected assignment {name:?}")));
            };
            if expected != &value || observed.insert(name.clone(), value).is_some() {
                return Err(parser.error(format!("invalid or duplicate constant {name:?}")));
            }
        }
        parser.skip_trivia()?;
        match parser.peek_char() {
            Some(',') => parser.expect_char(',')?,
            Some(';') => parser.expect_char(';')?,
            _ => return Err(parser.error("assignment must end with comma or semicolon")),
        }
    }
}

struct Parser<'a> {
    input: &'a str,
    resource: &'static str,
    position: usize,
}

impl<'a> Parser<'a> {
    const fn new(input: &'a str, resource: &'static str) -> Self {
        Self {
            input,
            resource,
            position: 0,
        }
    }

    fn object(
        &mut self,
        constants: &BTreeMap<String, JsAtom>,
        allow_static_fontcolor: bool,
    ) -> Result<BTreeMap<String, Vec<JsAtom>>, AdapterError> {
        self.expect_char('{')?;
        let mut rows = BTreeMap::new();
        loop {
            self.skip_trivia()?;
            if self.peek_char() == Some('}') {
                self.expect_char('}')?;
                return Ok(rows);
            }
            let key = self.string()?;
            if key.is_empty() || key.len() > MAX_JS_TEXT_BYTES || key.chars().any(char::is_control)
            {
                return Err(self.error("object key must be non-empty, bounded, and printable"));
            }
            self.skip_trivia()?;
            self.expect_char(':')?;
            self.skip_trivia()?;
            let row = self.array(constants, allow_static_fontcolor)?;
            if rows.insert(key.clone(), row).is_some() {
                return Err(self.error(format!("duplicate object key {key:?}")));
            }
            self.skip_trivia()?;
            match self.peek_char() {
                Some(',') => self.expect_char(',')?,
                Some('}') => {}
                _ => return Err(self.error("object row must end with comma or closing brace")),
            }
        }
    }

    fn array(
        &mut self,
        constants: &BTreeMap<String, JsAtom>,
        allow_static_fontcolor: bool,
    ) -> Result<Vec<JsAtom>, AdapterError> {
        self.expect_char('[')?;
        let mut values = Vec::new();
        loop {
            self.skip_trivia()?;
            if self.peek_char() == Some(']') {
                self.expect_char(']')?;
                return Ok(values);
            }
            values.push(self.atom(constants, allow_static_fontcolor)?);
            if values.len() > 32 {
                return Err(self.error("array contains more than 32 values"));
            }
            self.skip_trivia()?;
            match self.peek_char() {
                Some(',') => self.expect_char(',')?,
                Some(']') => {}
                _ => return Err(self.error("array value must end with comma or closing bracket")),
            }
        }
    }

    fn atom(
        &mut self,
        constants: &BTreeMap<String, JsAtom>,
        allow_fontcolor: bool,
    ) -> Result<JsAtom, AdapterError> {
        self.skip_trivia()?;
        match self.peek_char() {
            Some('\'' | '"') => {
                let value = self.string()?;
                self.skip_trivia()?;
                if self.peek_char() == Some('.') {
                    if !allow_fontcolor {
                        return Err(self.error("string methods are not allowed here"));
                    }
                    self.expect_char('.')?;
                    if self.identifier()? != "fontcolor" {
                        return Err(self.error("only the static fontcolor wrapper is allowed"));
                    }
                    self.skip_trivia()?;
                    self.expect_char('(')?;
                    self.skip_trivia()?;
                    let color = self.string()?;
                    if color.is_empty()
                        || color.len() > MAX_JS_TEXT_BYTES
                        || color.chars().any(char::is_control)
                    {
                        return Err(self.error("fontcolor argument must be bounded and printable"));
                    }
                    self.skip_trivia()?;
                    self.expect_char(')')?;
                }
                Ok(JsAtom::String(value))
            }
            Some(character) if character.is_ascii_digit() => self.integer().map(JsAtom::Integer),
            Some(character) if is_identifier_start(character) => {
                let identifier = self.identifier()?;
                constants
                    .get(&identifier)
                    .cloned()
                    .ok_or_else(|| self.error(format!("unknown literal constant {identifier:?}")))
            }
            _ => Err(self.error("expected a string, integer, or declared constant")),
        }
    }

    fn integer(&mut self) -> Result<u32, AdapterError> {
        let start = self.position;
        while self.peek_char().is_some_and(|value| value.is_ascii_digit()) {
            self.next_char();
        }
        self.input[start..self.position]
            .parse()
            .map_err(|_| self.error("integer literal is out of range"))
    }

    fn identifier(&mut self) -> Result<String, AdapterError> {
        let Some(first) = self.peek_char() else {
            return Err(self.error("expected an identifier"));
        };
        if !is_identifier_start(first) {
            return Err(self.error("expected an identifier"));
        }
        let start = self.position;
        self.next_char();
        while self.peek_char().is_some_and(is_identifier_continue) {
            self.next_char();
        }
        Ok(self.input[start..self.position].to_owned())
    }

    fn string(&mut self) -> Result<String, AdapterError> {
        let quote = self
            .next_char()
            .filter(|quote| matches!(quote, '\'' | '"'))
            .ok_or_else(|| self.error("expected a quoted string"))?;
        let mut value = String::new();
        loop {
            let character = self
                .next_char()
                .ok_or_else(|| self.error("unterminated string literal"))?;
            if character == quote {
                break;
            }
            if character == '\n' || character == '\r' {
                return Err(self.error("string literal contains an unescaped newline"));
            }
            if character == '\\' {
                let escaped = self
                    .next_char()
                    .ok_or_else(|| self.error("unterminated string escape"))?;
                match escaped {
                    '\'' | '"' | '\\' | '/' => value.push(escaped),
                    'b' => value.push('\u{0008}'),
                    'f' => value.push('\u{000c}'),
                    'n' => value.push('\n'),
                    'r' => value.push('\r'),
                    't' => value.push('\t'),
                    'x' => value.push(self.hex_escape(2)?),
                    'u' => value.push(self.hex_escape(4)?),
                    _ => return Err(self.error(format!("unsupported string escape \\{escaped}"))),
                }
            } else {
                value.push(character);
            }
            if value.len() > MAX_JS_TEXT_BYTES {
                return Err(self.error("string literal exceeds 512 UTF-8 bytes"));
            }
        }
        Ok(value)
    }

    fn hex_escape(&mut self, digits: usize) -> Result<char, AdapterError> {
        let start = self.position;
        for _ in 0..digits {
            let digit = self
                .next_char()
                .ok_or_else(|| self.error("truncated hexadecimal escape"))?;
            if !digit.is_ascii_hexdigit() {
                return Err(self.error("invalid hexadecimal escape"));
            }
        }
        let codepoint = u32::from_str_radix(&self.input[start..self.position], 16)
            .map_err(|_| self.error("invalid hexadecimal escape"))?;
        char::from_u32(codepoint).ok_or_else(|| self.error("invalid Unicode scalar escape"))
    }

    fn skip_trivia(&mut self) -> Result<(), AdapterError> {
        loop {
            while self.peek_char().is_some_and(char::is_whitespace) {
                self.next_char();
            }
            let remaining = &self.input[self.position..];
            if remaining.starts_with("//") {
                self.position += 2;
                while self.peek_char().is_some_and(|value| value != '\n') {
                    self.next_char();
                }
            } else if remaining.starts_with("/*") {
                self.position += 2;
                let Some(end) = self.input[self.position..].find("*/") else {
                    return Err(self.error("unterminated block comment"));
                };
                self.position += end + 2;
            } else {
                return Ok(());
            }
        }
    }

    fn expect_char(&mut self, expected: char) -> Result<(), AdapterError> {
        let actual = self.next_char();
        if actual == Some(expected) {
            Ok(())
        } else {
            Err(self.error(format!("expected {expected:?}, found {actual:?}")))
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.input[self.position..].chars().next()
    }

    fn next_char(&mut self) -> Option<char> {
        let character = self.peek_char()?;
        self.position += character.len_utf8();
        Some(character)
    }

    fn error(&self, detail: impl Into<String>) -> AdapterError {
        AdapterError::InvalidJavaScript {
            resource: self.resource,
            detail: format!("{} at UTF-8 byte {}", detail.into(), self.position),
        }
    }
}

fn is_identifier_start(character: char) -> bool {
    character.is_ascii_alphabetic() || matches!(character, '_' | '$')
}

fn is_identifier_continue(character: char) -> bool {
    is_identifier_start(character) || character.is_ascii_digit()
}

pub(super) fn textage_bundle_digest<'a>(
    files: impl IntoIterator<Item = (&'a str, &'a [u8])>,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"scorepeek-textage-live-bundle-v1\0");
    for (path, bytes) in files {
        digest.update((path.len() as u64).to_be_bytes());
        digest.update(path.as_bytes());
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    let mut encoded = String::with_capacity(64);
    for byte in digest.finalize() {
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn invalid_field(field: &'static str, detail: impl Into<String>) -> AdapterError {
    AdapterError::InvalidField {
        field,
        detail: detail.into(),
    }
}

fn textage_display_text(field: &'static str, value: &str) -> Result<String, AdapterError> {
    validate_text(
        field,
        value
            .trim_matches(|character| matches!(character, ' ' | '\t' | '\r' | '\n'))
            .to_owned(),
    )
}

fn validate_textage_slug(source_slug: &str) -> Result<(), AdapterError> {
    if source_slug.is_empty()
        || source_slug.len() > 64
        || !source_slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(invalid_field(
            "Textage source slug",
            "must contain 1 to 64 lowercase ASCII letters, digits, or underscores",
        ));
    }
    Ok(())
}

fn with_source_context(error: AdapterError, source_song_id: &str) -> AdapterError {
    match error {
        AdapterError::InvalidField { field, detail } => AdapterError::InvalidField {
            field,
            detail: format!("source {source_song_id:?}: {detail}"),
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_bounded_assignment_contract_without_executing_javascript() {
        let titles = br#"VERINDEX=0;IDINDEX=1;OPTINDEX=2;GENREINDEX=3;ARTISTINDEX=4;TITLEINDEX=5;SUBTITLEINDEX=6;SS=35;titletbl={'alpha':[1,10,0,"GENRE","ARTIST","\tALPHA".fontcolor("red")]};alert('not parsed');"#;
        let availability = br#"pspver="version";A=10,B=11,C=12,D=13,E=14,F=15;actbl={'alpha':[3,0,0,1,7,4,7,8,7,A,7,0,0,0,0,4,7,8,7,A,7,0,0]};"#;
        let charts = "datatbl={'alpha':[0,100,200,300,400,0,0,210,310,410,0,\"120\"]};function ignored(){return 1;}";
        let digest = textage_bundle_digest([
            ("titletbl.js", titles.as_slice()),
            ("actbl.js", availability.as_slice()),
            ("datatbl.js", charts.as_bytes()),
        ]);
        let snapshot = TextageLiveAdapter::parse(
            titles,
            availability,
            charts.as_bytes(),
            SourceRevision::content_sha256(digest).unwrap(),
        )
        .unwrap();

        assert_eq!(snapshot.evidence().record_count(), 8);
        let SourceObservation::Textage(record) = &snapshot.observations[0] else {
            panic!("expected Textage observation");
        };
        assert_eq!(record.source_song_id, "alpha#10");
        assert_eq!(record.title, "ALPHA");
        assert_eq!((record.bpm_min, record.bpm_max), (120, 120));
        assert!(record.infinitas_flag);
        assert_eq!(record.charts.len(), 7);
    }

    #[test]
    fn rejects_encoding_replacement_code_and_join_or_chart_drift() {
        let invalid_encoding = [0x81_u8];
        assert!(matches!(
            decode_windows_31j(&invalid_encoding, "test"),
            Err(AdapterError::InvalidEncoding { .. })
        ));

        let constants = BTreeMap::new();
        let mut executable = Parser::new("{'a':[call()]}", "test");
        assert!(executable.object(&constants, false).is_err());

        let mut duplicate = Parser::new("{'a':[1],'a':[2]}", "test");
        assert!(matches!(
            duplicate.object(&constants, false),
            Err(AdapterError::InvalidJavaScript { .. })
        ));

        let mut unexpected_method = Parser::new("{'a':['value'.fontcolor('red')]}", "test");
        assert!(unexpected_method.object(&constants, false).is_err());

        let incomplete = build_charts(
            "alpha",
            1,
            &[0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            &[0; 11],
            &[0; 11],
        )
        .unwrap();
        assert!(incomplete.is_empty());

        let invalid_level = build_charts(
            "alpha",
            1,
            &[0, 13, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            &[0; 11],
            &[0, 100, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        assert!(matches!(
            invalid_level,
            Err(AdapterError::InvalidField { .. })
        ));
    }
}
