//! Portable diagnostic record.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiagnosticRecord {
    pub operation: String,
    pub status: String,
    #[serde(default)]
    pub detail: serde_json::Value,
}
