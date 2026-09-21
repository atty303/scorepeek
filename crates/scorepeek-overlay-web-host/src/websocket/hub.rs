//! Stage broadcast snapshots.

use crate::server::state::Shared;

pub(crate) struct StageSnapshot {
    pub(crate) state: scorepeek_overlay::OverlayState,
    pub(crate) canvases: Vec<scorepeek_overlay::CanvasPresentation>,
}

pub(crate) fn stage_snapshot(shared: &Shared) -> StageSnapshot {
    let state = shared
        .feed
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let canvases = shared
        .canvases
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .map(crate::config::Canvas::presentation)
        .collect();
    StageSnapshot { state, canvases }
}
