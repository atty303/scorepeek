use crate::bundle::embedded::ASSET_VERSION;

pub use crate::control::{Request, Response};

#[derive(serde::Deserialize)]
pub(crate) struct StageControlEnvelope {
    pub(crate) request_id: u64,
    #[serde(default)]
    pub(crate) asset_version: String,
    pub(crate) request: serde_json::Value,
}

pub(crate) fn version_mismatch() -> String {
    serde_json::json!({"type":"version_mismatch", "asset_version":ASSET_VERSION}).to_string()
}
