//! Typed browser transport messages.

use scorepeek_overlay::{CanvasPresentation, OverlayState};
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct Reply {
    pub(crate) ok: bool,
    pub(crate) readonly: bool,
    pub(crate) error: Option<String>,
    #[serde(default)]
    pub(crate) canvases: Vec<CanvasPresentation>,
    #[serde(default)]
    pub(crate) dirty: bool,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Message {
    VersionMismatch,
    Stage {
        #[serde(default)]
        asset_version: String,
        state: Box<OverlayState>,
        canvases: Vec<CanvasPresentation>,
    },
    Control {
        request_id: u64,
        response: Reply,
    },
}
