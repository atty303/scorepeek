//! Versioned corpus manifests and immutable content references.

use serde::{Deserialize, Serialize};

pub(crate) const SOURCE_MANIFEST_SCHEMA: &str = "scorepeek-private-corpus-source-v2";
pub(crate) const GENERATION_SCHEMA: &str = "scorepeek-private-corpus-generation-v1";
pub(crate) const CANONICAL_FRAME_CONTRACT_ID: &str = "scorepeek-canonical-rgb8-1920x1080-v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalFrameBinding {
    pub normalizer_artifact_sha256: String,
    pub canonical_frame_contract_id: String,
    pub canonical_layout_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentRef {
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceManifest {
    pub schema: String,
    pub fixture_id: String,
    pub session_id: String,
    pub capture_profile_id: String,
    pub source: ContentRef,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusGeneration {
    pub schema: String,
    pub generation_id: String,
    pub sources: Vec<GenerationSource>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationSource {
    pub fixture_id: String,
    pub source_manifest_sha256: String,
}
