#![allow(
    clippy::wildcard_imports,
    reason = "this file is an implementation partition of the parent Dioxus DOM renderer"
)]

use super::*;
use crate::render::frame::NativeFramePresenter;

fn editor_text_input_state(
    document: &DioxusDocument,
    interactive: bool,
) -> Option<scorepeek_overlay_wayland_handles::TextInputState> {
    if !interactive {
        return None;
    }
    let document = document.inner.borrow();
    let node = document.get_focussed_node_id().filter(|node| {
        document.get_node(*node).is_some_and(|node| {
            node.element_data()
                .is_some_and(|element| element.text_input_data().is_some())
        })
    });
    node.and_then(|node| {
        let rect = document.get_client_bounding_rect(node)?;
        let input = document.get_node(node)?.element_data()?.text_input_data()?;
        let selection = input.editor.raw_selection().text_range();
        Some(scorepeek_overlay_wayland_handles::TextInputState {
            from_ime: input.editor.raw_compose().is_some(),
            text: input.editor.raw_text().to_owned(),
            cursor: i32::try_from(selection.end).unwrap_or(i32::MAX),
            anchor: i32::try_from(selection.start).unwrap_or(i32::MAX),
            rectangle: [
                snap_i32(rect.x),
                snap_i32(rect.y),
                snap_i32(rect.width).max(1),
                snap_i32(rect.height).max(1),
            ],
        })
    })
}

#[derive(Clone, Copy)]
pub(super) enum NativeFrameBoundary {
    Deferred,
    Frame,
}

impl NativeFrameBoundary {
    const fn is_frame(self) -> bool {
        matches!(self, Self::Frame)
    }
}

#[derive(Clone, Copy)]
pub(super) enum NativeSurfaceReadiness {
    Pending,
    Configured,
}

impl NativeSurfaceReadiness {
    const fn is_configured(self) -> bool {
        matches!(self, Self::Configured)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NativeDisplaySurfaceState {
    Unmapped,
    AwaitingConfigure,
    Mapped,
}

impl NativeDisplaySurfaceState {
    pub(super) const fn is_mapped(self) -> bool {
        matches!(self, Self::Mapped)
    }

    pub(super) const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::Unmapped => "unmapped",
            Self::AwaitingConfigure => "awaiting_configure",
            Self::Mapped => "mapped",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct NativeEditorStageTurnInput {
    pub(super) frame: NativeFrameBoundary,
    pub(super) surface: NativeSurfaceReadiness,
    pub(super) seconds: f64,
}

#[derive(Default)]
#[cfg_attr(not(test), allow(dead_code))]
#[allow(clippy::struct_excessive_bools)]
pub(super) struct NativeEditorStageTurnResult {
    pub(super) dioxus_changed: bool,
    pub(super) reconciled: bool,
    pub(super) reconciliation: EditorSkinReconciliation,
    pub(super) painted: bool,
    pub(super) text_input_active: bool,
}

#[derive(Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub(super) struct NativeDisplayTurnInput {
    pub(super) frame: NativeFrameBoundary,
    pub(super) surface: NativeSurfaceReadiness,
    pub(super) visible: bool,
    pub(super) live_widgets: u64,
    pub(super) seconds: f64,
}

#[derive(Default)]
pub(super) struct NativeDisplayTurnResult {
    pub(super) dioxus_changed: bool,
    pub(super) painted: bool,
    pub(super) unmapped: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_native_display_turn(
    document: &mut DioxusDocument,
    assets: &Arc<SkinAssetCache>,
    full_layout_pending: &mut bool,
    resolve_pending: &mut bool,
    surface_state: &mut NativeDisplaySurfaceState,
    work: &mut FrameWorkProfile,
    waker: &Waker,
    frame_start: &FrameWorkSample,
    input: NativeDisplayTurnInput,
    presenter: &mut impl NativeFramePresenter,
) -> Result<NativeDisplayTurnResult, String> {
    let frame = input.frame.is_frame();
    presenter.set_text_input(None);
    if !input.visible {
        let unmapped = if *surface_state != NativeDisplaySurfaceState::Unmapped
            || input.surface.is_configured()
        {
            presenter
                .unmap()
                .map_err(|error| format!("unmap Wayland surface: {error}"))?;
            *surface_state = NativeDisplaySurfaceState::Unmapped;
            true
        } else {
            false
        };
        return Ok(NativeDisplayTurnResult {
            dioxus_changed: false,
            painted: false,
            unmapped,
        });
    }
    let boundary = match *surface_state {
        NativeDisplaySurfaceState::Unmapped => false,
        NativeDisplaySurfaceState::AwaitingConfigure => input.surface.is_configured(),
        NativeDisplaySurfaceState::Mapped => input.surface.is_configured() || frame,
    };
    if !boundary || !presenter.is_active() {
        return Ok(NativeDisplayTurnResult::default());
    }
    let dioxus_changed = poll_native_document_for_frame(document, waker, full_layout_pending, work);
    *resolve_pending |= dioxus_changed;
    render_native_frame(
        &mut document.inner.borrow_mut(),
        input.seconds,
        full_layout_pending,
        resolve_pending,
        presenter,
        assets,
        work,
    )?;
    *surface_state = NativeDisplaySurfaceState::Mapped;
    if frame {
        work.finish_frame(
            frame_start,
            u64::from(input.visible),
            if input.visible { input.live_widgets } else { 0 },
        );
    }
    Ok(NativeDisplayTurnResult {
        dioxus_changed,
        painted: true,
        unmapped: false,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_native_editor_stage_turn(
    document: &mut DioxusDocument,
    projection: Reactive<NativeDocumentProjection>,
    previews: &mut std::collections::BTreeMap<String, EditorSkinPreview>,
    assets: &Arc<SkinAssetCache>,
    report: &Rc<RefCell<RunReport>>,
    output: &str,
    editor_state: &OverlayState,
    updates: &mut EditorSkinUpdates,
    runtime_create_count: &mut u64,
    next_skin_render: &mut Option<Instant>,
    full_layout_pending: &mut bool,
    resolve_pending: &mut bool,
    work: &mut FrameWorkProfile,
    waker: &Waker,
    frame_start: &FrameWorkSample,
    input: NativeEditorStageTurnInput,
    presenter: &mut impl NativeFramePresenter,
) -> Result<NativeEditorStageTurnResult, String> {
    let frame = input.frame.is_frame();
    if next_skin_render.is_some_and(|deadline| Instant::now() >= deadline) {
        updates.request();
    }
    // Native DOM work is driven by the compositor frame, matching the browser's
    // animation-frame boundary. Input/configure turns only accumulate damage and
    // request the next frame; they must not introduce extra VDOM polls between
    // frame callbacks.
    let dioxus_changed =
        frame && poll_native_document_for_frame(document, waker, full_layout_pending, work);
    *resolve_pending |= dioxus_changed;
    let dragging = matches!(&*projection.borrow(), NativeDocumentProjection::Editor(editor_projection) if editor_projection.drag.is_some());
    let mut reconciliation = EditorSkinReconciliation::default();
    let mut skin_changed = false;
    // The projection Signal and its skin roots become current together during
    // the Dioxus frame rebuild. Never reconcile a newly accepted projection on
    // an earlier wake/input turn, where its DOM roots do not exist yet.
    if frame && updates.take_if_ready(frame, dragging) {
        updates.renders = updates.renders.saturating_add(1);
        let canvases = match &*projection.borrow() {
            NativeDocumentProjection::Editor(editor_projection) => {
                editor_projection.canvases.clone()
            }
            NativeDocumentProjection::Display { .. } => {
                return Err("editor stage turn received a display projection".into());
            }
        };
        reconciliation = reconcile_editor_skin_previews(
            document,
            previews,
            &canvases,
            assets,
            report,
            Some(output),
            editor_state,
            runtime_create_count,
            work,
        )?;
        *resolve_pending |= reconciliation.tree_changed;
        if reconciliation.retry_owner {
            updates.request();
        }
        *next_skin_render = previews
            .values()
            .filter_map(|preview| preview.next_render)
            .min();
        skin_changed = true;
    }
    let interactive = matches!(&*projection.borrow(), NativeDocumentProjection::Editor(editor_projection) if editor_projection.interactive);
    let text_input = editor_text_input_state(document, interactive);
    let text_input_active = text_input.is_some();
    presenter.set_text_input(text_input);
    let boundary = frame || input.surface.is_configured();
    let painted = boundary && presenter.is_active();
    if painted {
        render_native_frame(
            &mut document.inner.borrow_mut(),
            input.seconds,
            full_layout_pending,
            resolve_pending,
            presenter,
            assets,
            work,
        )?;
    }
    if frame {
        let live_canvases = u64::try_from(previews.len()).unwrap_or(u64::MAX);
        let live_widgets = previews.values().fold(0_u64, |count, preview| {
            count.saturating_add(u64::try_from(preview.canvas.widgets.len()).unwrap_or(u64::MAX))
        });
        work.finish_frame(frame_start, live_canvases, live_widgets);
    }
    Ok(NativeEditorStageTurnResult {
        dioxus_changed,
        reconciled: skin_changed,
        reconciliation,
        painted,
        text_input_active,
    })
}

fn render_native_frame(
    document: &mut blitz_dom::BaseDocument,
    seconds: f64,
    full_layout_pending: &mut bool,
    resolve_pending: &mut bool,
    presenter: &mut impl NativeFramePresenter,
    assets: &SkinAssetCache,
    work: &mut FrameWorkProfile,
) -> Result<(), String> {
    let package_open_before = WorkStat {
        calls: assets.open_count.load(std::sync::atomic::Ordering::Relaxed),
        total_ns: assets.open_ns.load(std::sync::atomic::Ordering::Relaxed),
    };
    let package_clone_before = WorkStat {
        calls: assets
            .clone_count
            .load(std::sync::atomic::Ordering::Relaxed),
        total_ns: assets.clone_ns.load(std::sync::atomic::Ordering::Relaxed),
    };
    let lookup_before = WorkStat {
        calls: assets
            .resource_lookup_count
            .load(std::sync::atomic::Ordering::Relaxed),
        total_ns: assets
            .resource_lookup_ns
            .load(std::sync::atomic::Ordering::Relaxed),
    };
    if *resolve_pending || document.is_animating() {
        let incremental_layout = document.incremental_layout();
        if *full_layout_pending {
            document.set_incremental_layout(false);
        }
        *resolve_pending = false;
        work.measure("blitz_layout", || document.resolve(seconds));
        if *full_layout_pending {
            document.set_incremental_layout(incremental_layout);
            *full_layout_pending = false;
        }
        if assets
            .resource_lookup_count
            .load(std::sync::atomic::Ordering::Relaxed)
            > lookup_before.calls
        {
            // The embedded provider completes synchronously, but Blitz reads its response at
            // the start of resolve. Keep its image/layout damage for the next compositor frame.
            work.measure("resource_decode", || document.handle_messages());
            *resolve_pending = true;
        }
    }
    let (width, height) = document.viewport().window_size;
    let scale = document.viewport().scale_f64();
    let result = presenter.present(document, scale, width, height, work);
    let lookup_after = WorkStat {
        calls: assets
            .resource_lookup_count
            .load(std::sync::atomic::Ordering::Relaxed),
        total_ns: assets
            .resource_lookup_ns
            .load(std::sync::atomic::Ordering::Relaxed),
    };
    let package_open_after = WorkStat {
        calls: assets.open_count.load(std::sync::atomic::Ordering::Relaxed),
        total_ns: assets.open_ns.load(std::sync::atomic::Ordering::Relaxed),
    };
    let package_clone_after = WorkStat {
        calls: assets
            .clone_count
            .load(std::sync::atomic::Ordering::Relaxed),
        total_ns: assets.clone_ns.load(std::sync::atomic::Ordering::Relaxed),
    };
    work.record_stat(
        "package_open",
        WorkStat {
            calls: package_open_after
                .calls
                .saturating_sub(package_open_before.calls),
            total_ns: package_open_after
                .total_ns
                .saturating_sub(package_open_before.total_ns),
        },
    );
    work.record_stat(
        "package_clone",
        WorkStat {
            calls: package_clone_after
                .calls
                .saturating_sub(package_clone_before.calls),
            total_ns: package_clone_after
                .total_ns
                .saturating_sub(package_clone_before.total_ns),
        },
    );
    work.record_stat(
        "resource_lookup",
        WorkStat {
            calls: lookup_after.calls.saturating_sub(lookup_before.calls),
            total_ns: lookup_after.total_ns.saturating_sub(lookup_before.total_ns),
        },
    );
    result
}
