use crate::{ApplicationSnapshot, ModelDownload};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InspectionHeader {
    pub run_id: String,
    pub active: bool,
    pub partial: bool,
    pub oldest_sequence: u64,
    pub next_sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InspectionRecord {
    pub sequence: u64,
    pub operation: String,
    pub data: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OperationalWarning {
    ReplayTruncated {
        requested_seconds: u64,
        available_us: u64,
    },
    DiagnosticPersistenceUnavailable {
        error: String,
    },
    DiagnosticPersistenceLagged,
    DiagnosticPersistenceWriteFailed,
    DiagnosticSocketUnavailable {
        error: String,
    },
    BackgroundCatalogWorkerStartFailed {
        error: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum FrontendEvent {
    ModelDownload { state: ModelDownload },
    Output { stream: OutputStream, text: String },
    InspectionHeader { header: InspectionHeader },
    InspectionRecord { record: InspectionRecord },
    Warning { warning: OperationalWarning },
    Snapshot { snapshot: Box<ApplicationSnapshot> },
}
