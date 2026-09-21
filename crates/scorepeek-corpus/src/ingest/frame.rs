//! Canonical replay-frame ingest contracts.

use crate::store::{
    CorpusError, CorpusSplit, ErrorContext, ScreenClass, SplitGroups, validate_opaque_id,
    validate_sha256, validate_token,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractorIdentity {
    pub tool_id: String,
    pub tool_version: String,
    pub extractor_manifest_sha256: String,
    pub parameters_sha256: String,
}

impl ExtractorIdentity {
    pub(crate) fn validate(&self) -> Result<(), CorpusError> {
        validate_token(&self.tool_id, "extractor tool_id", ErrorContext::Replay)?;
        validate_token(
            &self.tool_version,
            "extractor tool_version",
            ErrorContext::Replay,
        )?;
        validate_sha256(
            &self.extractor_manifest_sha256,
            "extractor_manifest_sha256",
            ErrorContext::Replay,
        )?;
        validate_sha256(
            &self.parameters_sha256,
            "extractor parameters_sha256",
            ErrorContext::Replay,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimeBase {
    pub numerator: u32,
    pub denominator: u32,
}

impl TimeBase {
    pub(crate) fn validate(self) -> Result<(), CorpusError> {
        if self.numerator == 0 || self.denominator == 0 {
            return Err(CorpusError::InvalidReplay(
                "source_time_base values must be positive".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayFrame {
    pub frame_id: String,
    pub source_pts: i64,
    pub decode_index: u64,
    pub frame_sha256: String,
    pub episode_id: String,
    pub screen_class: ScreenClass,
    pub split: CorpusSplit,
    pub groups: SplitGroups,
    pub annotation_revision: String,
    pub labels_sha256: String,
}

impl ReplayFrame {
    pub(crate) fn validate(&self) -> Result<(), CorpusError> {
        validate_opaque_id(&self.frame_id, "frame_id", ErrorContext::Replay)?;
        validate_opaque_id(&self.episode_id, "episode_id", ErrorContext::Replay)?;
        validate_sha256(&self.frame_sha256, "frame_sha256", ErrorContext::Replay)?;
        self.groups.validate()?;
        validate_token(
            &self.annotation_revision,
            "annotation_revision",
            ErrorContext::Replay,
        )?;
        validate_sha256(&self.labels_sha256, "labels_sha256", ErrorContext::Replay)
    }
}
