//! Shared source revision, validation, and snapshot assembly contracts.

#[cfg(test)]
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fmt::Write as _;

#[cfg(test)]
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

#[cfg(test)]
use scorepeek_core::catalog::{Chart, Difficulty, PlayType, SourceChartObservation};
use scorepeek_core::catalog::{
    ChartKey, LineageId, RevisionStrategy, SourceEvidence, SourceId, SourceObservation,
    SourcePolicy, SourceSnapshot,
};

pub(crate) const MAX_SOURCE_BYTES: usize = 1024 * 1024;
const MAX_RECORDS: usize = 10_000;
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
    #[cfg(test)]
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
            #[cfg(test)]
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

pub(crate) fn snapshot(
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

pub(crate) fn snapshot_from_parts(
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

pub(crate) fn parse_bounded_json<T: DeserializeOwned>(
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

#[cfg(test)]
pub(crate) fn parse_fixture<T>(
    bytes: &[u8],
    expected_schema: &'static str,
) -> Result<T, AdapterError>
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

pub(crate) fn validate_source_size(bytes: &[u8]) -> Result<(), AdapterError> {
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(AdapterError::SourceTooLarge {
            actual: bytes.len(),
            maximum: MAX_SOURCE_BYTES,
        });
    }
    Ok(())
}

pub(crate) fn hex_digest(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

pub(crate) fn validate_record_count(actual: usize) -> Result<(), AdapterError> {
    if actual > MAX_RECORDS {
        return Err(AdapterError::TooManyRecords {
            actual,
            maximum: MAX_RECORDS,
        });
    }
    Ok(())
}

pub(crate) fn validate_source_id(source_id: &str) -> Result<(), AdapterError> {
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

pub(crate) fn validate_text(field: &'static str, value: String) -> Result<String, AdapterError> {
    if value.is_empty() || value.len() > MAX_TEXT_BYTES || value.chars().any(char::is_control) {
        return Err(AdapterError::InvalidField {
            field,
            detail: "must be non-empty, bounded, and contain no control characters".to_owned(),
        });
    }
    Ok(value)
}

#[cfg(test)]
pub(crate) fn validate_charts(
    records: &[FixtureChart],
) -> Result<Vec<SourceChartObservation>, AdapterError> {
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

#[cfg(test)]
pub(crate) trait Fixture {
    fn schema(&self) -> &str;
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg(test)]
pub(crate) struct FixtureChart {
    pub(crate) play_type: PlayType,
    pub(crate) difficulty: Difficulty,
    pub(crate) level: u8,
    pub(crate) notes: u32,
    pub(crate) source_chart_id: String,
    pub(crate) product_versions: Vec<String>,
    pub(crate) primary: bool,
}

const _: [(); 3] = [(); SourceId::COUNT];
const _: [(); 3] = [(); LineageId::COUNT];
