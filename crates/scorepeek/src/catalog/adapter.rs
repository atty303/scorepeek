use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fmt::Write as _;

use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Deserializer};
use sha2::{Digest, Sha256};

use super::federation::{
    Chart, ChartKey, Difficulty, DisplayVariantKind, DqnObservation, LineageId, PlayType,
    RevisionStrategy, SourceChartObservation, SourceEvidence, SourceId, SourceObservation,
    SourcePolicy, SourceSnapshot, SourceTitleObservation, TachiObservation, TextageObservation,
};

pub(super) const MAX_SOURCE_BYTES: usize = 1024 * 1024;
pub(super) const MAX_TACHI_SONG_BYTES: usize = 2 * 1024 * 1024;
pub(super) const MAX_TACHI_CHART_BYTES: usize = 24 * 1024 * 1024;
pub(super) const MAX_TACHI_BUNDLE_BYTES: usize = 64 * 1024 * 1024;
const MAX_RECORDS: usize = 10_000;
const MAX_TACHI_CHART_RECORDS: usize = 50_000;
const MAX_TACHI_VARIANTS: usize = 64;
const MAX_TEXT_BYTES: usize = 512;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum SourceRevision {
    GitCommit(String),
    ContentSha256(String),
}

impl SourceRevision {
    #[must_use]
    pub fn from_content(bytes: &[u8]) -> Self {
        Self::ContentSha256(hex_digest(&Sha256::digest(bytes)))
    }

    /// Creates a revision pinned to an exact Git commit.
    ///
    /// # Errors
    ///
    /// Returns an error unless `value` contains exactly 40 hexadecimal characters.
    pub fn git_commit(value: impl Into<String>) -> Result<Self, AdapterError> {
        let value = value.into().to_ascii_lowercase();
        validate_hex(&value, 40, "git commit")?;
        Ok(Self::GitCommit(value))
    }

    /// Creates a revision pinned to a content SHA-256 digest.
    ///
    /// # Errors
    ///
    /// Returns an error unless `value` contains exactly 64 hexadecimal characters.
    pub fn content_sha256(value: impl Into<String>) -> Result<Self, AdapterError> {
        let value = value.into().to_ascii_lowercase();
        validate_hex(&value, 64, "content SHA-256")?;
        Ok(Self::ContentSha256(value))
    }

    fn into_value(self) -> String {
        match self {
            Self::GitCommit(value) | Self::ContentSha256(value) => value,
        }
    }

    const fn strategy(&self) -> RevisionStrategy {
        match self {
            Self::GitCommit(_) => RevisionStrategy::GitCommit,
            Self::ContentSha256(_) => RevisionStrategy::ContentSha256,
        }
    }
}

#[derive(Debug)]
pub enum AdapterError {
    SourceTooLarge {
        actual: usize,
        maximum: usize,
    },
    InvalidJson(serde_json::Error),
    InvalidEncoding {
        resource: &'static str,
    },
    InvalidJavaScript {
        resource: &'static str,
        detail: String,
    },
    InvalidSchema {
        expected: &'static str,
        actual: String,
    },
    TooManyRecords {
        actual: usize,
        maximum: usize,
    },
    DuplicateSourceId(String),
    DuplicateRecord(String),
    DuplicateChart {
        source_id: String,
        chart: ChartKey,
    },
    InvalidField {
        field: &'static str,
        detail: String,
    },
    RevisionStrategyMismatch {
        expected: RevisionStrategy,
        actual: RevisionStrategy,
    },
    ContentDigestMismatch {
        expected: String,
        actual: String,
    },
}

impl fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "source input has {actual} bytes; maximum is {maximum}"
                )
            }
            Self::InvalidJson(error) => write!(formatter, "invalid source JSON: {error}"),
            Self::InvalidEncoding { resource } => {
                write!(formatter, "{resource} is not valid Windows-31J")
            }
            Self::InvalidJavaScript { resource, detail } => {
                write!(formatter, "invalid {resource} assignment data: {detail}")
            }
            Self::InvalidSchema { expected, actual } => {
                write!(
                    formatter,
                    "fixture schema is {actual:?}; expected {expected:?}"
                )
            }
            Self::TooManyRecords { actual, maximum } => {
                write!(
                    formatter,
                    "fixture has {actual} records; maximum is {maximum}"
                )
            }
            Self::DuplicateSourceId(source_id) => {
                write!(formatter, "duplicate source ID {source_id:?}")
            }
            Self::DuplicateRecord(record) => write!(formatter, "duplicate source record {record}"),
            Self::DuplicateChart { source_id, chart } => {
                write!(
                    formatter,
                    "duplicate chart {chart:?} for source ID {source_id:?}"
                )
            }
            Self::InvalidField { field, detail } => {
                write!(formatter, "invalid {field}: {detail}")
            }
            Self::RevisionStrategyMismatch { expected, actual } => write!(
                formatter,
                "revision strategy is {actual:?}; expected {expected:?}"
            ),
            Self::ContentDigestMismatch { expected, actual } => write!(
                formatter,
                "content SHA-256 is {actual}; expected pinned digest {expected}"
            ),
        }
    }
}

impl Error for AdapterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidJson(error) => Some(error),
            _ => None,
        }
    }
}

pub struct TachiFixtureAdapter;
pub struct TachiLiveAdapter;
pub struct TextageFixtureAdapter;
pub struct DqnLiveAdapter;

impl TachiFixtureAdapter {
    /// Parses the bounded, synthetic Tachi fixture contract.
    ///
    /// # Errors
    ///
    /// Returns an error when the fixture exceeds a bound, violates its versioned schema, or
    /// contains an invalid or duplicate record.
    pub fn parse(bytes: &[u8], revision: SourceRevision) -> Result<SourceSnapshot, AdapterError> {
        let fixture: TachiFixture = parse_fixture(bytes, "scorepeek-tachi-fixture-v1")?;
        validate_record_count(fixture.records.len())?;
        let mut source_ids = BTreeSet::new();
        let mut observations = Vec::with_capacity(fixture.records.len());
        for record in fixture.records {
            validate_source_id(&record.source_song_id)?;
            if !source_ids.insert(record.source_song_id.clone()) {
                return Err(AdapterError::DuplicateSourceId(record.source_song_id));
            }
            observations.push(SourceObservation::Tachi(TachiObservation {
                source_song_id: record.source_song_id,
                title_variants: BTreeSet::from([SourceTitleObservation {
                    value: validate_text("title", record.title)?,
                    kind: record.title_kind,
                }]),
                artist: validate_text("artist", record.artist)?,
                version: validate_text("version", record.version)?,
                charts: validate_charts(&record.charts)?,
                primary_infinitas: record.primary_infinitas,
            }));
        }
        snapshot(SourcePolicy::tachi(), revision, bytes, observations)
    }
}

impl TachiLiveAdapter {
    /// Parses the three pinned Tachi IIDX seed collections without executing repository code.
    ///
    /// # Errors
    ///
    /// Returns an error when a collection exceeds its bound, violates the strict live schema,
    /// contains invalid or duplicate IDs, has inconsistent primary charts, references an absent
    /// song, or is not pinned to a Git commit.
    pub fn parse(
        songs_bytes: &[u8],
        single_charts_bytes: &[u8],
        double_charts_bytes: &[u8],
        revision: SourceRevision,
    ) -> Result<SourceSnapshot, AdapterError> {
        let byte_size = songs_bytes
            .len()
            .checked_add(single_charts_bytes.len())
            .and_then(|size| size.checked_add(double_charts_bytes.len()))
            .ok_or(AdapterError::SourceTooLarge {
                actual: usize::MAX,
                maximum: MAX_TACHI_BUNDLE_BYTES,
            })?;
        if byte_size > MAX_TACHI_BUNDLE_BYTES {
            return Err(AdapterError::SourceTooLarge {
                actual: byte_size,
                maximum: MAX_TACHI_BUNDLE_BYTES,
            });
        }
        let songs: Vec<TachiLiveSong> = parse_bounded_json(songs_bytes, MAX_TACHI_SONG_BYTES)?;
        validate_record_count(songs.len())?;
        let single_charts: Vec<TachiLiveChart> =
            parse_bounded_json(single_charts_bytes, MAX_TACHI_CHART_BYTES)?;
        let double_charts: Vec<TachiLiveChart> =
            parse_bounded_json(double_charts_bytes, MAX_TACHI_CHART_BYTES)?;
        validate_tachi_chart_count(single_charts.len())?;
        validate_tachi_chart_count(double_charts.len())?;

        let mut source_chart_ids = BTreeSet::new();
        let mut referenced_song_ids = BTreeSet::new();
        let mut chart_keys = BTreeSet::new();
        let mut charts_by_song = BTreeMap::<String, Vec<SourceChartObservation>>::new();
        let mut accepted_chart_count = 0_usize;
        append_tachi_charts(
            single_charts,
            PlayType::Single,
            &mut source_chart_ids,
            &mut referenced_song_ids,
            &mut chart_keys,
            &mut charts_by_song,
            &mut accepted_chart_count,
        )?;
        append_tachi_charts(
            double_charts,
            PlayType::Double,
            &mut source_chart_ids,
            &mut referenced_song_ids,
            &mut chart_keys,
            &mut charts_by_song,
            &mut accepted_chart_count,
        )?;

        let song_count = songs.len();
        let observations = build_tachi_observations(songs, charts_by_song, &referenced_song_ids)?;

        let content_sha256 = tachi_bundle_digest([
            ("db/seeds/songs-iidx.json", songs_bytes),
            ("db/seeds/charts-iidx-sp.json", single_charts_bytes),
            ("db/seeds/charts-iidx-dp.json", double_charts_bytes),
        ]);
        snapshot_from_parts(
            SourcePolicy::tachi(),
            revision,
            content_sha256,
            byte_size,
            song_count + accepted_chart_count,
            observations,
        )
    }
}

fn build_tachi_observations(
    songs: Vec<TachiLiveSong>,
    mut charts_by_song: BTreeMap<String, Vec<SourceChartObservation>>,
    referenced_song_ids: &BTreeSet<String>,
) -> Result<Vec<SourceObservation>, AdapterError> {
    let mut source_song_ids = BTreeSet::new();
    let mut observations = Vec::with_capacity(songs.len());
    for song in songs {
        validate_tachi_id("source_song_id", &song.id, b'S')?;
        if !source_song_ids.insert(song.id.clone()) {
            return Err(AdapterError::DuplicateSourceId(song.id));
        }
        if song.legacy_song_id == 0 {
            return Err(AdapterError::InvalidField {
                field: "legacySongID",
                detail: "must be positive".to_owned(),
            });
        }
        validate_string_list("altTitles", &song.alt_titles, MAX_TACHI_VARIANTS)?;
        validate_string_list("searchTerms", &song.search_terms, MAX_TACHI_VARIANTS)?;
        let title_variants =
            tachi_title_variants(song.title, song.alt_titles, song.data.eamusement_csv_title)?;
        validate_text("genre", song.data.genre)?;
        let mut charts = charts_by_song.remove(&song.id).unwrap_or_default();
        charts.sort_by(|left, right| left.chart.cmp(&right.chart));
        let primary_infinitas = charts
            .iter()
            .any(|chart| chart.product_versions.contains("inf"));
        observations.push(SourceObservation::Tachi(TachiObservation {
            source_song_id: song.id,
            title_variants,
            artist: validate_text("artist", song.artist)?,
            version: validate_text("displayVersion", song.data.display_version)?,
            charts,
            primary_infinitas,
        }));
    }
    if !charts_by_song.is_empty() || !referenced_song_ids.is_subset(&source_song_ids) {
        return Err(AdapterError::InvalidField {
            field: "chart.songID",
            detail: "references an absent song".to_owned(),
        });
    }
    Ok(observations)
}

fn tachi_title_variants(
    title: String,
    alternate_titles: Vec<String>,
    eamusement_csv_title: Option<String>,
) -> Result<BTreeSet<SourceTitleObservation>, AdapterError> {
    let mut variants = BTreeSet::from([SourceTitleObservation {
        value: validate_text("title", title)?,
        kind: DisplayVariantKind::InGameDisplay,
    }]);
    for title in alternate_titles {
        variants.insert(SourceTitleObservation {
            value: validate_text("altTitles", title)?,
            kind: DisplayVariantKind::AlternateDisplay,
        });
    }
    if let Some(title) = eamusement_csv_title {
        variants.insert(SourceTitleObservation {
            value: validate_text("eamusementCsvTitle", title)?,
            kind: DisplayVariantKind::EamusementCsv,
        });
    }
    Ok(variants)
}

impl TextageFixtureAdapter {
    /// Parses the bounded, synthetic Textage fixture contract.
    ///
    /// # Errors
    ///
    /// Returns an error when the fixture exceeds a bound, violates its versioned schema, or
    /// contains an invalid or duplicate record.
    pub fn parse(bytes: &[u8], revision: SourceRevision) -> Result<SourceSnapshot, AdapterError> {
        let fixture: TextageFixture = parse_fixture(bytes, "scorepeek-textage-fixture-v1")?;
        validate_record_count(fixture.records.len())?;
        let mut source_ids = BTreeSet::new();
        let mut observations = Vec::with_capacity(fixture.records.len());
        for record in fixture.records {
            validate_source_id(&record.source_song_id)?;
            if !source_ids.insert(record.source_song_id.clone()) {
                return Err(AdapterError::DuplicateSourceId(record.source_song_id));
            }
            if record.bpm_min == 0 || record.bpm_min > record.bpm_max {
                return Err(AdapterError::InvalidField {
                    field: "bpm",
                    detail: "minimum must be positive and no greater than maximum".to_owned(),
                });
            }
            observations.push(SourceObservation::Textage(TextageObservation {
                source_song_id: record.source_song_id,
                title: validate_text("title", record.title)?,
                title_kind: record.title_kind,
                artist: validate_text("artist", record.artist)?,
                version: validate_text("version", record.version)?,
                charts: validate_charts(&record.charts)?,
                infinitas_flag: record.infinitas_flag,
                bpm_min: record.bpm_min,
                bpm_max: record.bpm_max,
            }));
        }
        snapshot(SourcePolicy::textage(), revision, bytes, observations)
    }
}

impl DqnLiveAdapter {
    /// Parses pinned bytes from the public dqn/iidxapi INFINITAS music endpoint.
    ///
    /// This boundary is deliberately independent of HTTP clients, credentials, response headers,
    /// and caches. Acquisition code must supply the exact response body and its expected SHA-256
    /// revision.
    ///
    /// # Errors
    ///
    /// Returns an error when the source exceeds a bound, violates the strict live schema, has a
    /// duplicate row, contains an invalid record, or does not match its pinned content revision.
    pub fn parse(bytes: &[u8], revision: SourceRevision) -> Result<SourceSnapshot, AdapterError> {
        validate_source_size(bytes)?;
        let records: Vec<DqnLiveRecord> =
            serde_json::from_slice(bytes).map_err(AdapterError::InvalidJson)?;
        validate_record_count(records.len())?;
        let mut unique_records = BTreeSet::new();
        let mut observations = Vec::with_capacity(records.len());
        for record in records {
            let title = validate_text("title", record.title)?;
            let artist = validate_text("artist", record.artist)?;
            let pack = record
                .pack_name
                .map(|pack| validate_text("packName", pack))
                .transpose()?;
            if !unique_records.insert((title.clone(), artist.clone(), pack.clone())) {
                return Err(AdapterError::DuplicateRecord(format!(
                    "({title:?}, {artist:?}, {pack:?})"
                )));
            }
            observations.push(SourceObservation::Dqn(DqnObservation {
                title,
                artist,
                pack,
            }));
        }
        snapshot(SourcePolicy::dqn(), revision, bytes, observations)
    }
}

fn snapshot(
    policy: SourcePolicy,
    revision: SourceRevision,
    bytes: &[u8],
    observations: Vec<SourceObservation>,
) -> Result<SourceSnapshot, AdapterError> {
    snapshot_from_parts(
        policy,
        revision,
        hex_digest(&Sha256::digest(bytes)),
        bytes.len(),
        observations.len(),
        observations,
    )
}

pub(super) fn snapshot_from_parts(
    policy: SourcePolicy,
    revision: SourceRevision,
    content_sha256: String,
    byte_size: usize,
    record_count: usize,
    observations: Vec<SourceObservation>,
) -> Result<SourceSnapshot, AdapterError> {
    let actual_strategy = revision.strategy();
    if actual_strategy != policy.revision_strategy {
        return Err(AdapterError::RevisionStrategyMismatch {
            expected: policy.revision_strategy,
            actual: actual_strategy,
        });
    }
    let revision = revision.into_value();
    if actual_strategy == RevisionStrategy::ContentSha256 && revision != content_sha256 {
        return Err(AdapterError::ContentDigestMismatch {
            expected: revision,
            actual: content_sha256,
        });
    }
    let mut field_authority: Vec<_> = policy
        .field_authority
        .iter()
        .map(|field| (*field).to_owned())
        .collect();
    field_authority.sort();
    let evidence = SourceEvidence {
        source_id: policy.source_id,
        lineage_id: policy.lineage_id,
        revision_strategy: policy.revision_strategy,
        revision,
        content_sha256,
        byte_size,
        record_count,
        parser_version: policy.parser_version.to_owned(),
        declared_scope: policy.declared_scope.to_owned(),
        completeness: policy.completeness,
        field_authority,
        freshness: policy.freshness.to_owned(),
        rights_and_provenance: policy.rights_and_provenance.to_owned(),
    };
    Ok(SourceSnapshot {
        policy,
        evidence,
        observations,
    })
}

fn parse_bounded_json<T: DeserializeOwned>(
    bytes: &[u8],
    maximum: usize,
) -> Result<T, AdapterError> {
    if bytes.len() > maximum {
        return Err(AdapterError::SourceTooLarge {
            actual: bytes.len(),
            maximum,
        });
    }
    serde_json::from_slice(bytes).map_err(AdapterError::InvalidJson)
}

fn append_tachi_charts(
    records: Vec<TachiLiveChart>,
    play_type: PlayType,
    source_chart_ids: &mut BTreeSet<String>,
    referenced_song_ids: &mut BTreeSet<String>,
    chart_keys: &mut BTreeSet<(String, ChartKey)>,
    charts_by_song: &mut BTreeMap<String, Vec<SourceChartObservation>>,
    accepted_chart_count: &mut usize,
) -> Result<(), AdapterError> {
    for record in records {
        validate_tachi_id("chart.id", &record.id, b'C')?;
        validate_tachi_id("chart.songID", &record.song_id, b'S')?;
        if !source_chart_ids.insert(record.id.clone()) {
            return Err(AdapterError::DuplicateSourceId(record.id));
        }
        referenced_song_ids.insert(record.song_id.clone());
        validate_text("chart.legacyChartID", record.legacy_chart_id)?;
        validate_tachi_level(&record.level, record.level_num)?;
        if record.data.notecount == 0 {
            return Err(AdapterError::InvalidField {
                field: "chart.data.notecount",
                detail: "must be positive".to_owned(),
            });
        }
        if record.versions.is_empty() || record.versions.len() > MAX_TACHI_VARIANTS {
            return Err(AdapterError::InvalidField {
                field: "chart.versions",
                detail: "must contain between 1 and 64 values".to_owned(),
            });
        }
        let product_versions: BTreeSet<_> = record
            .versions
            .into_iter()
            .map(|version| validate_text("chart.version", version))
            .collect::<Result<_, _>>()?;
        if product_versions.len() > MAX_TACHI_VARIANTS {
            return Err(AdapterError::InvalidField {
                field: "chart.versions",
                detail: "contains too many unique values".to_owned(),
            });
        }
        let Some(difficulty) = record.difficulty.standard() else {
            continue;
        };
        if !record.is_primary {
            continue;
        }
        let key = ChartKey {
            play_type,
            difficulty,
        };
        if !chart_keys.insert((record.song_id.clone(), key)) {
            return Err(AdapterError::DuplicateChart {
                source_id: record.song_id,
                chart: key,
            });
        }
        if !(1..=12).contains(&record.level_num) {
            return Err(AdapterError::InvalidField {
                field: "chart.levelNum",
                detail: "primary standard chart level must be between 1 and 12".to_owned(),
            });
        }
        charts_by_song
            .entry(record.song_id)
            .or_default()
            .push(SourceChartObservation {
                chart: Chart {
                    key,
                    level: record.level_num,
                    notes: record.data.notecount,
                },
                source_chart_id: record.id,
                product_versions,
                primary: true,
            });
        *accepted_chart_count = accepted_chart_count.saturating_add(1);
    }
    Ok(())
}

fn validate_tachi_chart_count(actual: usize) -> Result<(), AdapterError> {
    if actual > MAX_TACHI_CHART_RECORDS {
        return Err(AdapterError::TooManyRecords {
            actual,
            maximum: MAX_TACHI_CHART_RECORDS,
        });
    }
    Ok(())
}

fn validate_tachi_level(level: &str, level_num: u8) -> Result<(), AdapterError> {
    if (level == "?" && level_num == 0) || (level_num > 0 && level.parse::<u8>() == Ok(level_num)) {
        return Ok(());
    }
    Err(AdapterError::InvalidField {
        field: "chart.level",
        detail: "must agree with levelNum or use ? with zero".to_owned(),
    })
}

fn validate_tachi_id(field: &'static str, value: &str, prefix: u8) -> Result<(), AdapterError> {
    if value.len() == 20
        && value.as_bytes().first() == Some(&prefix)
        && value.as_bytes()[1..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Ok(());
    }
    Err(AdapterError::InvalidField {
        field,
        detail: "must be a Tachi prefixed 19-digit lowercase hexadecimal ID".to_owned(),
    })
}

fn validate_string_list(
    field: &'static str,
    values: &[String],
    maximum: usize,
) -> Result<(), AdapterError> {
    if values.len() > maximum {
        return Err(AdapterError::InvalidField {
            field,
            detail: format!("must contain at most {maximum} values"),
        });
    }
    let mut unique = BTreeSet::new();
    for value in values {
        validate_text(field, value.clone())?;
        if !unique.insert(value) {
            return Err(AdapterError::InvalidField {
                field,
                detail: "must not contain duplicates".to_owned(),
            });
        }
    }
    Ok(())
}

fn tachi_bundle_digest<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> String {
    let mut digest = Sha256::new();
    digest.update(b"scorepeek-tachi-live-bundle-v1\0");
    for (path, bytes) in files {
        digest.update((path.len() as u64).to_be_bytes());
        digest.update(path.as_bytes());
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    hex_digest(&digest.finalize())
}

fn parse_fixture<T>(bytes: &[u8], expected_schema: &'static str) -> Result<T, AdapterError>
where
    T: DeserializeOwned + Fixture,
{
    validate_source_size(bytes)?;
    let fixture: T = serde_json::from_slice(bytes).map_err(AdapterError::InvalidJson)?;
    if fixture.schema() != expected_schema {
        return Err(AdapterError::InvalidSchema {
            expected: expected_schema,
            actual: fixture.schema().to_owned(),
        });
    }
    Ok(fixture)
}

fn validate_source_size(bytes: &[u8]) -> Result<(), AdapterError> {
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(AdapterError::SourceTooLarge {
            actual: bytes.len(),
            maximum: MAX_SOURCE_BYTES,
        });
    }
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn validate_record_count(actual: usize) -> Result<(), AdapterError> {
    if actual > MAX_RECORDS {
        return Err(AdapterError::TooManyRecords {
            actual,
            maximum: MAX_RECORDS,
        });
    }
    Ok(())
}

pub(super) fn validate_source_id(source_id: &str) -> Result<(), AdapterError> {
    if source_id.is_empty()
        || source_id.len() > MAX_TEXT_BYTES
        || source_id.chars().any(char::is_control)
    {
        return Err(AdapterError::InvalidField {
            field: "source_song_id",
            detail: "must be non-empty, bounded, and contain no control characters".to_owned(),
        });
    }
    Ok(())
}

pub(super) fn validate_text(field: &'static str, value: String) -> Result<String, AdapterError> {
    if value.is_empty() || value.len() > MAX_TEXT_BYTES || value.chars().any(char::is_control) {
        return Err(AdapterError::InvalidField {
            field,
            detail: "must be non-empty, bounded, and contain no control characters".to_owned(),
        });
    }
    Ok(value)
}

fn validate_charts(records: &[FixtureChart]) -> Result<Vec<SourceChartObservation>, AdapterError> {
    let mut keys = BTreeSet::new();
    let mut charts = Vec::with_capacity(records.len());
    for record in records {
        if !(1..=12).contains(&record.level) {
            return Err(AdapterError::InvalidField {
                field: "chart.level",
                detail: "must be between 1 and 12".to_owned(),
            });
        }
        if record.notes == 0 {
            return Err(AdapterError::InvalidField {
                field: "chart.notes",
                detail: "must be positive".to_owned(),
            });
        }
        let key = ChartKey {
            play_type: record.play_type,
            difficulty: record.difficulty,
        };
        if !keys.insert(key) {
            return Err(AdapterError::DuplicateChart {
                source_id: "current record".to_owned(),
                chart: key,
            });
        }
        let source_chart_id =
            validate_text("chart.source_chart_id", record.source_chart_id.clone())?;
        let product_versions: BTreeSet<_> = record
            .product_versions
            .iter()
            .cloned()
            .map(|version| validate_text("chart.product_version", version))
            .collect::<Result<_, _>>()?;
        if product_versions.is_empty() {
            return Err(AdapterError::InvalidField {
                field: "chart.product_versions",
                detail: "must contain at least one product version".to_owned(),
            });
        }
        charts.push(SourceChartObservation {
            chart: Chart {
                key,
                level: record.level,
                notes: record.notes,
            },
            source_chart_id,
            product_versions,
            primary: record.primary,
        });
    }
    charts.sort_by(|left, right| left.chart.cmp(&right.chart));
    Ok(charts)
}

fn validate_hex(value: &str, length: usize, label: &'static str) -> Result<(), AdapterError> {
    if value.len() != length || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AdapterError::InvalidField {
            field: "revision",
            detail: format!("{label} must contain exactly {length} hexadecimal characters"),
        });
    }
    Ok(())
}

trait Fixture {
    fn schema(&self) -> &str;
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TachiFixture {
    schema: String,
    records: Vec<TachiRecord>,
}

impl Fixture for TachiFixture {
    fn schema(&self) -> &str {
        &self.schema
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TachiRecord {
    source_song_id: String,
    title: String,
    title_kind: DisplayVariantKind,
    artist: String,
    version: String,
    charts: Vec<FixtureChart>,
    primary_infinitas: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TachiLiveSong {
    #[serde(rename = "altTitles")]
    alt_titles: Vec<String>,
    artist: String,
    data: TachiLiveSongData,
    id: String,
    #[serde(rename = "legacySongID")]
    legacy_song_id: u64,
    #[serde(rename = "searchTerms")]
    search_terms: Vec<String>,
    title: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TachiLiveSongData {
    #[serde(rename = "displayVersion")]
    display_version: String,
    #[serde(
        default,
        rename = "eamusementCsvTitle",
        deserialize_with = "deserialize_optional_nonnull_string"
    )]
    eamusement_csv_title: Option<String>,
    genre: String,
}

fn deserialize_optional_nonnull_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TachiLiveChart {
    data: TachiLiveChartData,
    difficulty: TachiDifficulty,
    id: String,
    #[serde(rename = "isPrimary")]
    is_primary: bool,
    #[serde(rename = "legacyChartID")]
    legacy_chart_id: String,
    level: String,
    #[serde(rename = "levelNum")]
    level_num: u8,
    #[serde(rename = "songID")]
    song_id: String,
    versions: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TachiLiveChartData {
    #[serde(default, rename = "2dxtraSet")]
    _two_dxtra_set: Option<IgnoredAny>,
    #[serde(default, rename = "bpiCoefficient")]
    _bpi_coefficient: Option<IgnoredAny>,
    #[serde(default, rename = "dpTier")]
    _dp_tier: Option<IgnoredAny>,
    #[serde(default, rename = "exhcTier")]
    _exhc_tier: Option<IgnoredAny>,
    #[serde(default, rename = "hashSHA256")]
    _hash_sha256: Option<IgnoredAny>,
    #[serde(default, rename = "hcTier")]
    _hc_tier: Option<IgnoredAny>,
    #[serde(default, rename = "inGameID")]
    _in_game_id: Option<IgnoredAny>,
    #[serde(default, rename = "kaidenAverage")]
    _kaiden_average: Option<IgnoredAny>,
    #[serde(default, rename = "ncTier")]
    _nc_tier: Option<IgnoredAny>,
    notecount: u32,
    #[serde(default, rename = "worldRecord")]
    _world_record: Option<IgnoredAny>,
}

#[derive(Deserialize)]
enum TachiDifficulty {
    #[serde(rename = "NORMAL")]
    Normal,
    #[serde(rename = "HYPER")]
    Hyper,
    #[serde(rename = "ANOTHER")]
    Another,
    #[serde(rename = "LEGGENDARIA")]
    Leggendaria,
    #[serde(rename = "All Scratch NORMAL")]
    AllScratchNormal,
    #[serde(rename = "All Scratch HYPER")]
    AllScratchHyper,
    #[serde(rename = "All Scratch ANOTHER")]
    AllScratchAnother,
    #[serde(rename = "All Scratch LEGGENDARIA")]
    AllScratchLeggendaria,
    #[serde(rename = "Kichiku NORMAL")]
    KichikuNormal,
    #[serde(rename = "Kichiku HYPER")]
    KichikuHyper,
    #[serde(rename = "Kichiku ANOTHER")]
    KichikuAnother,
    #[serde(rename = "Kichiku LEGGENDARIA")]
    KichikuLeggendaria,
    #[serde(rename = "Kiraku NORMAL")]
    KirakuNormal,
    #[serde(rename = "Kiraku HYPER")]
    KirakuHyper,
    #[serde(rename = "Kiraku ANOTHER")]
    KirakuAnother,
    #[serde(rename = "Kiraku LEGGENDARIA")]
    KirakuLeggendaria,
}

impl TachiDifficulty {
    const fn standard(&self) -> Option<Difficulty> {
        match self {
            Self::Normal => Some(Difficulty::Normal),
            Self::Hyper => Some(Difficulty::Hyper),
            Self::Another => Some(Difficulty::Another),
            Self::Leggendaria => Some(Difficulty::Leggendaria),
            Self::AllScratchNormal
            | Self::AllScratchHyper
            | Self::AllScratchAnother
            | Self::AllScratchLeggendaria
            | Self::KichikuNormal
            | Self::KichikuHyper
            | Self::KichikuAnother
            | Self::KichikuLeggendaria
            | Self::KirakuNormal
            | Self::KirakuHyper
            | Self::KirakuAnother
            | Self::KirakuLeggendaria => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextageFixture {
    schema: String,
    records: Vec<TextageRecord>,
}

impl Fixture for TextageFixture {
    fn schema(&self) -> &str {
        &self.schema
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextageRecord {
    source_song_id: String,
    title: String,
    title_kind: DisplayVariantKind,
    artist: String,
    version: String,
    charts: Vec<FixtureChart>,
    infinitas_flag: bool,
    bpm_min: u16,
    bpm_max: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DqnLiveRecord {
    title: String,
    artist: String,
    #[serde(
        rename = "packName",
        deserialize_with = "deserialize_nullable_pack_name"
    )]
    pack_name: Option<String>,
}

fn deserialize_nullable_pack_name<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::deserialize(deserializer)
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureChart {
    play_type: PlayType,
    difficulty: Difficulty,
    level: u8,
    notes: u32,
    source_chart_id: String,
    product_versions: Vec<String>,
    primary: bool,
}

const _: [(); 3] = [(); SourceId::COUNT];
const _: [(); 3] = [(); LineageId::COUNT];
