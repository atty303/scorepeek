use crate::CanvasPresentation;
use crate::editor::EditorView;
use crate::editor_model::{EditorEffect, EditorInput, EditorSession, StageProjection};
use dioxus::prelude::*;

#[derive(Clone, Copy)]
pub struct EditorRuntime {
    pub session: Signal<EditorSession>,
    pub active_output: Memo<Option<String>>,
    pub selection: Memo<(Option<String>, Option<String>)>,
    pub inspector: Memo<EditorView>,
    pub stages: Memo<Vec<StageProjection>>,
    pub selected_canvas: Memo<Option<CanvasPresentation>>,
    pub dispatch: Callback<EditorInput, Vec<EditorEffect>>,
}

/// Creates the sole writable editor authority and its read-only derived views.
pub fn use_editor_runtime(initialize: impl FnOnce() -> EditorSession + 'static) -> EditorRuntime {
    let mut session = use_signal(initialize);
    let active_output = use_memo(move || session.read().active_output.clone());
    let selection = use_memo(move || {
        let session = session.read();
        (
            session.selected_canvas.clone(),
            session.selected_widget.clone(),
        )
    });
    let inspector = use_memo(move || session.read().view());
    let stages = use_memo(move || session.read().stage_projections());
    let selected_canvas = use_memo(move || {
        let session = session.read();
        session
            .current()
            .filter(|canvas| session.visible(canvas))
            .cloned()
    });
    let dispatch = Callback::new(move |input| {
        if matches!(
            input,
            EditorInput::Surface(crate::editor_surface::SurfaceAction::Move(_))
        ) {
            let current = session.read();
            if current.drag.is_none() && current.placing.is_none() {
                return Vec::new();
            }
        }
        session.write().reduce(input)
    });
    EditorRuntime {
        session,
        active_output,
        selection,
        inspector,
        stages,
        selected_canvas,
        dispatch,
    }
}
