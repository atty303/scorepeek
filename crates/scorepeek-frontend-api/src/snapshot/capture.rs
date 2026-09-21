use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CaptureSnapshot {
    pub backend: Option<String>,
    pub connected: bool,
}
