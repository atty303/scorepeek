use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiagnosticsSnapshot {
    pub run_id: Option<String>,
    pub degraded: bool,
}
