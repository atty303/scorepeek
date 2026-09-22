use crate::{ApplicationSnapshot, ModelDownload};
use serde::{Deserialize, Serialize};

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
    Snapshot { snapshot: Box<ApplicationSnapshot> },
}
