//! Immutable resources bound to a diagnostic run.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiagnosticBinding {
    pub capture_profile_sha256: String,
    pub canonical_layout_sha256: String,
    pub catalog_sha256: String,
    pub model_sha256: String,
    pub runtime_sha256: String,
}
