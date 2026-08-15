use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fmt::Write as _;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use sha2::{Digest, Sha256};

use super::federation::{
    Chart, ChartKey, Difficulty, DisplayVariantKind, DqnObservation, LineageId, PlayType,
    RevisionStrategy, SourceChartObservation, SourceEvidence, SourceId, SourceObservation,
    SourcePolicy, SourceSnapshot, TachiObservation, TextageObservation,
};

const MAX_SOURCE_BYTES: usize = 1024 * 1024;
const MAX_RECORDS: usize = 10_000;
const MAX_TEXT_BYTES: usize = 512;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum SourceRevision {
    GitCommit(String),
    ContentSha256(String),
}

impl SourceRevision {
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
                title: validate_text("title", record.title)?,
                title_kind: record.title_kind,
                artist: validate_text("artist", record.artist)?,
                version: validate_text("version", record.version)?,
                charts: validate_charts(&record.charts)?,
                primary_infinitas: record.primary_infinitas,
            }));
        }
        snapshot(SourcePolicy::tachi(), revision, bytes, observations)
    }
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
    let actual_strategy = revision.strategy();
    if actual_strategy != policy.revision_strategy {
        return Err(AdapterError::RevisionStrategyMismatch {
            expected: policy.revision_strategy,
            actual: actual_strategy,
        });
    }
    let content_sha256 = hex_digest(&Sha256::digest(bytes));
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
        byte_size: bytes.len(),
        record_count: observations.len(),
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

fn validate_source_id(source_id: &str) -> Result<(), AdapterError> {
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

fn validate_text(field: &'static str, value: String) -> Result<String, AdapterError> {
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
