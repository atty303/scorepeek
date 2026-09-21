use crate::{Backend, CanvasPresentation};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Request {
    AcquireBackend {
        backend: Backend,
        editor_id: String,
    },
    KeepAliveBackend {
        backend: Backend,
        editor_id: String,
    },
    ReleaseBackend {
        backend: Backend,
        editor_id: String,
    },
    GetBackend {
        backend: Backend,
    },
    UpdateBackendDraft {
        backend: Backend,
        editor_id: String,
        canvases: Vec<CanvasPresentation>,
    },
    CommitBackend {
        backend: Backend,
        editor_id: String,
        canvases: Vec<CanvasPresentation>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Response {
    pub ok: bool,
    pub readonly: bool,
    pub error: Option<String>,
    #[serde(default)]
    pub canvases: Vec<CanvasPresentation>,
    pub generation: Option<u64>,
    #[serde(default)]
    pub dirty: bool,
}
