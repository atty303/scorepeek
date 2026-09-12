mod text;
#[cfg(test)]
use scorepeek_overlay_ui::editor_model::EditorEffectKind;
use scorepeek_overlay_ui::editor_model::{
    EditorBackendReply, EditorEffect, EditorInput, EditorSession, StageProjection,
};
use scorepeek_overlay_ui::editor_runtime::{EditorRuntime, use_editor_runtime};
use scorepeek_overlay_ui::editor_surface::{
    EditorCanvas, EditorSelectionMetrics, EditorSurface, PlacementPreview, SurfaceAction,
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::Arc,
    task::{Context as TaskContext, Wake, Waker},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::runtime::{Config, Feed};
use anyrender::{CompositeAlphaMode, ImageRenderer, PaintScene, WindowRenderer};
use anyrender_vello::{VelloRendererOptions, VelloWindowRenderer};
use blitz_dom::{Document, DocumentConfig};
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};
use dioxus::prelude::*;
use dioxus_core::VirtualDom;
use dioxus_native_dom::DioxusDocument;
use scorepeek_overlay_handles::{Event, OutputDescription, Shell};
use scorepeek_overlay_ui::editor::{EditorAction, EditorOutput, EditorPanel};
use scorepeek_overlay_ui::{Appearance, OXANIUM, OverlayState, WidgetLayout};
use serde::{Deserialize, Serialize};
use smithay_client_toolkit::reexports::calloop::ping::{Ping, make_ping};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum PaintReason {
    Steady,
    InitialConfigure,
    Reconfigure,
    VisibilityClear,
    Editor,
}

impl PaintReason {
    const fn name(self) -> &'static str {
        match self {
            Self::Steady => "steady",
            Self::InitialConfigure => "initial_configure",
            Self::Reconfigure => "reconfigure",
            Self::VisibilityClear => "visibility_clear",
            Self::Editor => "editor",
        }
    }

    const fn bypasses_cap(self) -> bool {
        !matches!(self, Self::Steady)
    }
}

#[derive(Default)]
struct FrameCadence {
    last_paint: Option<Duration>,
}

#[derive(Default)]
struct EditorSkinUpdates {
    pending: bool,
    requests: u64,
    renders: u64,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct WorkStat {
    calls: u64,
    total_ns: u64,
}

#[derive(Clone, Default, Serialize)]
struct FrameWorkProfile {
    phases: std::collections::BTreeMap<&'static str, WorkStat>,
    unmeasured_calls: std::collections::BTreeMap<&'static str, u64>,
    frames: std::collections::VecDeque<FrameWorkSample>,
    dropped_frames: u64,
}

#[derive(Clone, Default, Serialize)]
struct FrameWorkSample {
    sequence: u64,
    phases: std::collections::BTreeMap<&'static str, WorkStat>,
    unmeasured_calls: std::collections::BTreeMap<&'static str, u64>,
    live_canvases: u64,
    live_widgets: u64,
}

impl FrameWorkProfile {
    const FRAME_CAPACITY: usize = 256;
    const REQUIRED_PHASES: [&'static str; 16] = [
        "dioxus_poll",
        "projection_rebuild",
        "canvas_config",
        "package_open",
        "package_clone",
        "wasm_runtime_create",
        "skin_input",
        "wasm_render",
        "json_tree",
        "tree_reconciliation",
        "resource_lookup",
        "resource_decode",
        "blitz_layout",
        "scene",
        "gpu_present",
        "surface_commit",
    ];

    fn record(&mut self, phase: &'static str, elapsed: Duration) {
        self.record_stat(
            phase,
            WorkStat {
                calls: 1,
                total_ns: u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX),
            },
        );
    }

    fn record_stat(&mut self, phase: &'static str, delta: WorkStat) {
        let stat = self.phases.entry(phase).or_default();
        stat.calls = stat.calls.saturating_add(delta.calls);
        stat.total_ns = stat.total_ns.saturating_add(delta.total_ns);
    }

    fn measure<T>(&mut self, phase: &'static str, operation: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let result = operation();
        self.record(phase, started.elapsed());
        result
    }

    fn unmeasured(&mut self, phase: &'static str) {
        let calls = self.unmeasured_calls.entry(phase).or_default();
        *calls = calls.saturating_add(1);
    }

    fn snapshot(&self) -> FrameWorkSample {
        FrameWorkSample {
            sequence: self
                .dropped_frames
                .saturating_add(u64::try_from(self.frames.len()).unwrap_or(u64::MAX))
                .saturating_add(1),
            phases: self.phases.clone(),
            unmeasured_calls: self.unmeasured_calls.clone(),
            live_canvases: 0,
            live_widgets: 0,
        }
    }

    fn finish_frame(&mut self, before: &FrameWorkSample, live_canvases: u64, live_widgets: u64) {
        let mut phases = self
            .phases
            .iter()
            .filter_map(|(phase, after)| {
                let before = before.phases.get(phase).copied().unwrap_or_default();
                let delta = WorkStat {
                    calls: after.calls.saturating_sub(before.calls),
                    total_ns: after.total_ns.saturating_sub(before.total_ns),
                };
                (delta.calls > 0).then_some((*phase, delta))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        for phase in Self::REQUIRED_PHASES {
            phases.entry(phase).or_default();
        }
        let unmeasured_calls = self
            .unmeasured_calls
            .iter()
            .filter_map(|(phase, after)| {
                let delta = after.saturating_sub(*before.unmeasured_calls.get(phase).unwrap_or(&0));
                (delta > 0).then_some((*phase, delta))
            })
            .collect();
        if self.frames.len() == Self::FRAME_CAPACITY {
            self.frames.pop_front();
            self.dropped_frames = self.dropped_frames.saturating_add(1);
        }
        self.frames.push_back(FrameWorkSample {
            sequence: before.sequence,
            phases,
            unmeasured_calls,
            live_canvases,
            live_widgets,
        });
    }

    #[cfg(test)]
    fn calls(&self, phase: &'static str) -> u64 {
        self.phases.get(phase).map_or(0, |stat| stat.calls)
    }
}

trait NativeFramePresenter {
    fn is_active(&self) -> bool;

    fn set_text_input(&mut self, input: Option<scorepeek_overlay_handles::TextInputState>);

    fn request_frame_commit(&mut self);

    fn present(
        &mut self,
        document: &mut blitz_dom::BaseDocument,
        scale: f64,
        width: u32,
        height: u32,
        work: &mut FrameWorkProfile,
    ) -> Result<(), String>;
}

struct WindowPresenter<'a> {
    shell: &'a mut Shell,
    renderer: &'a mut VelloWindowRenderer,
}

impl NativeFramePresenter for WindowPresenter<'_> {
    fn is_active(&self) -> bool {
        self.renderer.is_active()
    }

    fn set_text_input(&mut self, input: Option<scorepeek_overlay_handles::TextInputState>) {
        self.shell.set_text_input(input);
    }

    fn request_frame_commit(&mut self) {
        self.shell.request_frame_and_commit();
    }

    fn present(
        &mut self,
        document: &mut blitz_dom::BaseDocument,
        scale: f64,
        width: u32,
        height: u32,
        work: &mut FrameWorkProfile,
    ) -> Result<(), String> {
        self.shell.request_frame();
        let started = Instant::now();
        let mut scene_elapsed = Duration::ZERO;
        self.renderer.render(|scene| {
            let scene_started = Instant::now();
            paint_native_scene(scene, document, scale, width, height);
            scene_elapsed = scene_started.elapsed();
            work.record("scene", scene_elapsed);
        });
        // anyrender-vello exposes scene construction and a combined renderer/present call, but
        // does not expose the wl_surface commit as a separately timed operation. Keep scene time
        // disjoint and mark commit as unavailable instead of publishing a fabricated zero.
        work.record(
            "gpu_present",
            started.elapsed().saturating_sub(scene_elapsed),
        );
        work.unmeasured("surface_commit");
        Ok(())
    }
}

impl EditorSkinUpdates {
    fn request(&mut self) {
        self.pending = true;
        self.requests = self.requests.saturating_add(1);
    }

    fn take_if_ready(&mut self, frame: bool, dragging: bool) -> bool {
        if !self.pending || (!frame && dragging) {
            return false;
        }
        self.pending = false;
        true
    }
}

impl FrameCadence {
    fn permits(
        &self,
        now: Duration,
        rate: scorepeek_overlay_ui::WaylandRefreshRate,
        reason: PaintReason,
    ) -> bool {
        if reason.bypasses_cap() {
            return true;
        }
        let Some(hz) = rate.hz() else {
            return true;
        };
        self.last_paint.is_none_or(|last| {
            now.saturating_sub(last) >= Duration::from_secs_f64(1.0 / f64::from(hz))
        })
    }

    fn record(&mut self, now: Duration) {
        self.last_paint = Some(now);
    }
}

#[derive(Default)]
struct PointerInput {
    buttons: blitz_traits::events::MouseEventButtons,
}

impl PointerInput {
    fn focus_clicked_button(document: &mut DioxusDocument, point: [f32; 2]) {
        // Browsers focus an enabled button on primary click. Blitz currently only performs its
        // pointer-down focus default for text inputs, so keep this proven renderer difference at
        // the native event adapter rather than teaching shared Dioxus components about native.
        let button = {
            let inner = document.inner.borrow();
            let mut candidate = inner.element_from_point(point[0], point[1]);
            loop {
                let Some(id) = candidate else { break None };
                let Some(node) = inner.get_node(id) else {
                    break None;
                };
                if node.is_focussable()
                    && node.element_data().is_some_and(|element| {
                        element.name.local.as_ref().eq_ignore_ascii_case("button")
                    })
                {
                    break Some(id);
                }
                candidate = node.parent;
            }
        };
        if let Some(button) = button {
            document.inner.borrow_mut().set_focus_to(button);
        }
    }

    fn dispatch_blitz(
        &mut self,
        document: &mut DioxusDocument,
        point: [f64; 2],
        button: u32,
        pressed: Option<bool>,
    ) {
        use blitz_traits::events::{
            BlitzPointerEvent, BlitzPointerId, MouseEventButton, Point, PointerCoords,
            PointerDetails, UiEvent,
        };
        let point = dioxus::html::geometry::ClientPoint::new(point[0], point[1]).to_f32();
        let (x, y) = (point.x, point.y);
        let button = if button == 0x111 {
            MouseEventButton::Secondary
        } else {
            MouseEventButton::Main
        };
        if let Some(pressed) = pressed {
            self.buttons.set(button.into(), pressed);
        }
        let event = BlitzPointerEvent {
            id: BlitzPointerId::Mouse,
            is_primary: true,
            coords: PointerCoords {
                page_x: x,
                page_y: y,
                screen_x: x,
                screen_y: y,
                client_x: x,
                client_y: y,
            },
            button,
            buttons: self.buttons,
            mods: dioxus::html::Modifiers::default(),
            details: PointerDetails::default(),
            element: Point::default(),
            active_pointers: Arc::default(),
        };
        document.handle_ui_event(match pressed {
            Some(true) => UiEvent::PointerDown(event),
            Some(false) => UiEvent::PointerUp(event),
            None => UiEvent::PointerMove(event),
        });
    }

    fn dispatch(
        &mut self,
        document: &mut DioxusDocument,
        point: [f64; 2],
        button: u32,
        pressed: Option<bool>,
    ) {
        self.dispatch_blitz(document, point, button, pressed);
        if pressed == Some(false) && button == 0x110 {
            let point = dioxus::html::geometry::ClientPoint::new(point[0], point[1]).to_f32();
            Self::focus_clicked_button(document, [point.x, point.y]);
        }
    }
    fn click(&mut self, document: &mut DioxusDocument, point: [f64; 2]) {
        self.dispatch(document, point, 0x110, None);
        self.dispatch(document, point, 0x110, Some(true));
        self.dispatch(document, point, 0x110, Some(false));
    }

    fn wheel(&mut self, document: &mut DioxusDocument, point: [f64; 2], delta: [f64; 2]) {
        use blitz_traits::events::{
            BlitzWheelDelta, BlitzWheelEvent, Point, PointerCoords, UiEvent,
        };
        // Browser wheel targeting is resolved from the wheel event's coordinates. Blitz currently
        // routes the default scroll action through its previously stored hover chain, so reproduce
        // the browser contract at this adapter boundary without synthesizing a Dioxus pointermove.
        let point = dioxus::html::geometry::ClientPoint::new(point[0], point[1]).to_f32();
        document.inner.borrow_mut().set_hover_to(point.x, point.y);
        document.handle_ui_event(UiEvent::Wheel(BlitzWheelEvent {
            delta: BlitzWheelDelta::Pixels(delta[0], delta[1]),
            coords: PointerCoords {
                page_x: point.x,
                page_y: point.y,
                screen_x: point.x,
                screen_y: point.y,
                client_x: point.x,
                client_y: point.y,
            },
            buttons: self.buttons,
            mods: dioxus::html::Modifiers::default(),
            element: Point::default(),
        }));
    }
}

fn poll_native_document(document: &mut DioxusDocument, waker: &Waker) -> bool {
    // Browser DOM reconciliation retains the focused control's selection. Blitz currently resets
    // it while applying an otherwise unrelated Dioxus rebuild, so preserve that renderer fact at
    // the native DOM adapter boundary without interpreting the editor field or its value.
    let selection = text::focused_selection(document);
    let changed = document.poll(Some(TaskContext::from_waker(waker)));
    if changed && let Some(selection) = selection {
        text::restore_focused_selection(document, &selection);
    }
    changed
}

fn poll_native_document_for_frame(
    document: &mut DioxusDocument,
    waker: &Waker,
    full_layout_pending: &mut bool,
    work: &mut FrameWorkProfile,
) -> bool {
    let started = Instant::now();
    let mut changed = false;
    while poll_native_document(document, waker) {
        changed = true;
    }
    *full_layout_pending |= changed;
    work.record("dioxus_poll", started.elapsed());
    changed
}

#[derive(Clone, Copy, Default)]
#[allow(clippy::struct_excessive_bools)]
struct NativeEventOutcome {
    frame: bool,
    configured: bool,
    input_damage: bool,
    closed: bool,
}

trait NativeEventConsumer {
    fn configure_event(
        &mut self,
        logical: [u32; 2],
        physical: [u32; 2],
        scale_120: u32,
    ) -> Result<bool, String>;
    fn pointer_motion_event(&mut self, point: [f64; 2]);
    fn pointer_button_event(&mut self, button: u32, pressed: bool, point: [f64; 2]);
    fn pointer_scroll_event(&mut self, delta: [f64; 2], point: [f64; 2]);
    fn text_event(&mut self, command: &scorepeek_overlay_handles::TextCommand);
    fn ime_event(&mut self, update: scorepeek_overlay_handles::TextUpdate);
    fn keyboard_focus_event(&mut self, focused: bool);
}

fn dispatch_native_event(
    consumer: &mut impl NativeEventConsumer,
    event: Event,
) -> Result<NativeEventOutcome, String> {
    let mut outcome = NativeEventOutcome::default();
    match event {
        Event::Configure {
            logical,
            physical,
            scale_120,
        } => outcome.configured = consumer.configure_event(logical, physical, scale_120)?,
        Event::Wake => {}
        Event::PointerMotion { x, y } => {
            consumer.pointer_motion_event([x, y]);
            outcome.input_damage = true;
        }
        Event::PointerButton {
            button,
            pressed,
            x,
            y,
        } => {
            consumer.pointer_button_event(button, pressed, [x, y]);
            outcome.input_damage = true;
        }
        Event::PointerScroll { dx, dy, x, y } => {
            consumer.pointer_scroll_event([dx, dy], [x, y]);
            outcome.input_damage = true;
        }
        Event::Text(command) => consumer.text_event(&command),
        Event::Ime(update) => consumer.ime_event(update),
        Event::KeyboardFocus(focused) => consumer.keyboard_focus_event(focused),
        Event::Frame => outcome.frame = true,
        Event::Closed => outcome.closed = true,
    }
    Ok(outcome)
}

#[cfg(test)]
fn editor_geometry(
    position: [i32; 2],
    canvas_size: [u32; 2],
    output_size: Option<[u32; 2]>,
) -> ([i32; 2], [u32; 2]) {
    if let Some(output) = output_size {
        return ([0, 0], output);
    }
    (position, canvas_size)
}

#[cfg(test)]
fn editor_panel_width(output_width: Option<u32>) -> u32 {
    match output_width {
        Some(width) => (width / 5).clamp(360, 480),
        None => 400,
    }
}

fn editor_skin_root_id(canvas_id: &str) -> String {
    use std::fmt::Write as _;
    let mut id = String::from("scorepeek-editor-skin-");
    for byte in canvas_id.bytes() {
        let _ = write!(id, "{byte:02x}");
    }
    id
}

fn worker_needs_replacement(
    id: &str,
    output: Option<&str>,
    desired_ids: &std::collections::BTreeSet<String>,
    projected: &[crate::config::Canvas],
    finished: bool,
) -> bool {
    !desired_ids.contains(id)
        || projected
            .iter()
            .find(|canvas| canvas.id == id)
            .is_some_and(|canvas| Some(canvas.output.as_str()) != output)
        || finished
}

#[derive(Debug, Default, PartialEq, Eq)]
struct WorkerReconciliation {
    stop_join: Vec<String>,
    start: Vec<String>,
}

fn reconcile_worker_lifecycle<'a>(
    workers: impl IntoIterator<Item = (&'a str, Option<&'a str>, bool)>,
    projected: &[crate::config::Canvas],
) -> WorkerReconciliation {
    let desired_ids = projected
        .iter()
        .map(|canvas| canvas.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let live = workers
        .into_iter()
        .map(|(id, output, finished)| (id.to_owned(), output.map(str::to_owned), finished))
        .collect::<Vec<_>>();
    let stop_join = live
        .iter()
        .filter(|(id, output, finished)| {
            worker_needs_replacement(id, output.as_deref(), &desired_ids, projected, *finished)
        })
        .map(|(id, _, _)| id.clone())
        .collect::<Vec<_>>();
    let retained = live
        .iter()
        .filter(|(id, _, _)| !stop_join.contains(id))
        .map(|(id, _, _)| id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let start = projected
        .iter()
        .filter(|canvas| !retained.contains(canvas.id.as_str()))
        .map(|canvas| canvas.id.clone())
        .collect();
    WorkerReconciliation { stop_join, start }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProjectionCacheKey {
    editing: bool,
    session_id: u64,
    revision: u64,
}

#[derive(Default)]
struct NativeProjectionCache {
    key: Option<ProjectionCacheKey>,
    canvases: Vec<crate::config::Canvas>,
    rebuilds: u64,
}

impl NativeProjectionCache {
    fn resolve<'a>(
        &'a mut self,
        fallback_skin: Option<scorepeek_overlay_ui::Skin>,
        session: &EditorSession,
        desired: &[crate::config::Canvas],
    ) -> Result<&'a [crate::config::Canvas], String> {
        let key = ProjectionCacheKey {
            editing: session.editing,
            session_id: session.session_id,
            revision: session.revision,
        };
        if self.key == Some(key) {
            return Ok(&self.canvases);
        }
        self.canvases = if session.editing {
            let output_descriptions = session
                .outputs
                .iter()
                .map(|output| OutputDescription {
                    name: output.name.clone(),
                    model: output.model.clone(),
                    logical_size: output.logical_size,
                })
                .collect::<Vec<_>>();
            let draft = session
                .draft
                .iter()
                .map(|presentation| {
                    let mut canvas = crate::config::empty_canvas(
                        presentation.id.clone(),
                        crate::runtime::Backend::Wayland,
                    );
                    canvas.apply_presentation(presentation);
                    canvas
                })
                .collect::<Vec<_>>();
            editor_stage_canvases(fallback_skin, &draft, &output_descriptions)?
        } else {
            desired.to_vec()
        };
        self.key = Some(key);
        self.rebuilds = self.rebuilds.saturating_add(1);
        crate::diagnostics::emit(
            "native_projection_rebuilt",
            &serde_json::json!({
                "session_id": key.session_id,
                "revision": key.revision,
                "editing": key.editing,
                "surface_count": self.canvases.len(),
                "rebuild_count": self.rebuilds,
            }),
        );
        Ok(&self.canvases)
    }
}

fn editor_skin_presentation_changed(
    before: &scorepeek_overlay_ui::CanvasPresentation,
    after: &scorepeek_overlay_ui::CanvasPresentation,
) -> bool {
    before.id != after.id
        || before.skin != after.skin
        || before.skin_properties != after.skin_properties
        || before.width != after.width
        || before.height != after.height
        || before.widgets != after.widgets
}

fn editor_skin_previews_changed(
    previews: &std::collections::BTreeMap<String, EditorSkinPreview>,
    stage: &StageProjection,
) -> bool {
    stage.canvases.len() != previews.len()
        || stage.canvases.iter().any(|canvas| {
            previews.get(&canvas.id).is_none_or(|preview| {
                editor_skin_presentation_changed(&preview.canvas.presentation(), canvas)
            })
        })
}

fn accepts_stage_projection(current: &StageProjection, candidate: &StageProjection) -> bool {
    current.session_id != candidate.session_id || candidate.revision > current.revision
}

fn accept_stage_projection_replica(
    projection: Reactive<NativeDocumentProjection>,
    document: &mut DioxusDocument,
    previews: &mut std::collections::BTreeMap<String, EditorSkinPreview>,
    assets: &SkinAssetCache,
    output: &str,
    candidate: &StageProjection,
) -> bool {
    let accepted = match &*projection.borrow() {
        NativeDocumentProjection::Editor(current) => {
            accepts_stage_projection(current, candidate) && current != candidate
        }
        NativeDocumentProjection::Display { .. } => false,
    };
    if !accepted {
        return false;
    }
    let live = candidate
        .canvases
        .iter()
        .map(|canvas| canvas.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let removed = previews
        .keys()
        .filter(|id| !live.contains(id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    for id in removed {
        if let Some(mut preview) = previews.remove(&id) {
            preview.tree.unmount(&mut document.inner.borrow_mut());
            assets.release_editor_owner(&id, output);
        }
    }
    projection.set(NativeDocumentProjection::Editor(candidate.clone()));
    true
}

#[derive(Clone, Copy)]
enum PaintSignal {
    None,
    Frame,
    Damage,
    DamageAndFrame,
}

impl PaintSignal {
    const fn from_state(frame: bool, pending: bool) -> Self {
        match (frame, pending) {
            (false, false) => Self::None,
            (true, false) => Self::Frame,
            (false, true) => Self::Damage,
            (true, true) => Self::DamageAndFrame,
        }
    }
}

#[derive(Clone, Copy)]
struct PaintState {
    editing: bool,
    visible: bool,
    signal: PaintSignal,
    animating: bool,
}

impl PaintState {
    const fn editor_frame_needed(self) -> bool {
        self.editing && self.visible && matches!(self.signal, PaintSignal::Damage)
    }

    const fn ordinary_reason(self) -> Option<PaintReason> {
        if self.editing && self.visible && matches!(self.signal, PaintSignal::DamageAndFrame) {
            Some(PaintReason::Editor)
        } else if self.visible
            && ((!self.editing
                && matches!(
                    self.signal,
                    PaintSignal::Damage | PaintSignal::DamageAndFrame
                ))
                || (self.animating
                    && matches!(
                        self.signal,
                        PaintSignal::Frame | PaintSignal::DamageAndFrame
                    )))
        {
            Some(PaintReason::Steady)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PaintAdmission {
    Paint(PaintReason),
    RequestFrameCommit,
    None,
}

fn admit_native_paint(
    renderer_active: bool,
    configured: bool,
    visibility_changed: bool,
    state: PaintState,
    now: Duration,
    refresh: scorepeek_overlay_ui::WaylandRefreshRate,
    cadence: &FrameCadence,
) -> PaintAdmission {
    if !renderer_active {
        return PaintAdmission::None;
    }
    let reason = if configured {
        Some(if cadence.last_paint.is_none() {
            PaintReason::InitialConfigure
        } else {
            PaintReason::Reconfigure
        })
    } else if visibility_changed && !state.visible {
        Some(PaintReason::VisibilityClear)
    } else {
        state.ordinary_reason()
    };
    if let Some(reason) = reason {
        if cadence.permits(now, refresh, reason) {
            PaintAdmission::Paint(reason)
        } else if state.visible {
            PaintAdmission::RequestFrameCommit
        } else {
            PaintAdmission::None
        }
    } else if state.editor_frame_needed() {
        PaintAdmission::RequestFrameCommit
    } else {
        PaintAdmission::None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SurfaceRole {
    DisplayCanvas,
    EditorStage,
}

#[derive(Clone, Default)]
struct PublishedStages {
    by_output: std::collections::BTreeMap<String, StageProjection>,
    #[cfg(test)]
    publications: u64,
}

#[derive(Clone)]
struct EditorAuthorityProps {
    initial: Rc<RefCell<Option<EditorSession>>>,
    runtime: Rc<RefCell<Option<EditorRuntime>>>,
}

#[allow(clippy::needless_pass_by_value)]
fn editor_authority(props: EditorAuthorityProps) -> Element {
    let initial = props
        .initial
        .borrow_mut()
        .take()
        .expect("editor authority initializes exactly once");
    let runtime = use_editor_runtime(move || initial);
    *props.runtime.borrow_mut() = Some(runtime);
    rsx! {}
}

struct NativeEditorAuthority {
    document: DioxusDocument,
    runtime: EditorRuntime,
    published: Arc<std::sync::Mutex<PublishedStages>>,
}

impl NativeEditorAuthority {
    fn new(session: EditorSession, published: Arc<std::sync::Mutex<PublishedStages>>) -> Self {
        let initial = Rc::new(RefCell::new(Some(session)));
        let runtime = Rc::new(RefCell::new(None));
        let mut document = DioxusDocument::new(
            VirtualDom::new_with_props(
                editor_authority,
                EditorAuthorityProps {
                    initial,
                    runtime: Rc::clone(&runtime),
                },
            ),
            DocumentConfig::default(),
        );
        document.initial_build();
        let runtime = runtime
            .borrow()
            .as_ref()
            .copied()
            .expect("editor authority publishes its runtime during initial build");
        let mut authority = Self {
            document,
            runtime,
            published,
        };
        authority.publish();
        authority
    }

    fn dispatch(&mut self, input: EditorInput) -> Vec<EditorEffect> {
        let before = {
            let session = self.runtime.session.read();
            (session.session_id, session.revision)
        };
        crate::diagnostics::emit(
            "native_editor_action_received",
            &serde_json::json!({"session_id":before.0,"revision":before.1,"input":input.diagnostic_name()}),
        );
        let effects = self.runtime.dispatch.call(input);
        self.poll();
        let after = {
            let session = self.runtime.session.read();
            (session.session_id, session.revision)
        };
        crate::diagnostics::emit(
            "native_editor_action_reduced",
            &serde_json::json!({"session_id":after.0,"before_revision":before.1,"revision":after.1,"effect_count":effects.len()}),
        );
        if after != before {
            self.publish();
        }
        effects
    }

    fn poll(&mut self) {
        while poll_native_document(&mut self.document, Waker::noop()) {}
    }

    fn publish(&mut self) {
        self.poll();
        let stages = self.runtime.stages.read().clone();
        let session_id = self.runtime.session.read().session_id;
        let revision = self.runtime.session.read().revision;
        let output_count = stages.len();
        let mut published = self
            .published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        published.by_output = stages
            .into_iter()
            .map(|stage| (stage.output.name.clone(), stage))
            .collect();
        #[cfg(test)]
        {
            published.publications = published.publications.saturating_add(1);
        }
        crate::diagnostics::emit(
            "native_editor_projection_published",
            &serde_json::json!({"session_id":session_id,"revision":revision,"output_count":output_count}),
        );
    }

    fn session(&self) -> dioxus::signals::ReadableRef<'_, Signal<EditorSession>> {
        self.runtime.session.read()
    }
}

#[derive(Clone, Debug, PartialEq)]
struct InteractionCorrelation {
    run_id: String,
    interaction_id: u64,
    action: &'static str,
}

#[derive(Debug)]
enum CoordinatorCommand {
    EditorInput {
        input: EditorInput,
        correlation: Option<InteractionCorrelation>,
    },
    Open {
        output: Option<String>,
        canvas: String,
        preview_screen: Option<scorepeek_overlay_ui::ScreenKind>,
    },
    ResolveOutput {
        output_names: Vec<String>,
        output: Option<String>,
        canvas: String,
        preview_screen: Option<scorepeek_overlay_ui::ScreenKind>,
    },
}

fn stop_workers<'a>(
    ids: impl IntoIterator<Item = &'a String>,
    workers: &std::collections::BTreeMap<String, impl WorkerControl>,
    wakes: &std::sync::Mutex<std::collections::BTreeMap<String, Ping>>,
) {
    for id in ids {
        if let Some(worker) = workers.get(id) {
            worker.request_stop();
        }
        if let Some(wake) = wakes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
        {
            wake.ping();
        }
    }
}

trait WorkerControl {
    fn request_stop(&self);
}

struct NativeWorker {
    output: Option<String>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    join: std::thread::JoinHandle<Result<(), String>>,
}

impl WorkerControl for NativeWorker {
    fn request_stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
    }
}

fn new_native_skin_runtime(
    package: &crate::skin::Package,
    report: &Rc<RefCell<RunReport>>,
    canvas_id: &str,
    output: Option<&str>,
    interaction_ids: &[u64],
) -> Result<crate::skin::Runtime, String> {
    let started = Instant::now();
    let result = crate::skin::Runtime::new_measured(package);
    match &result {
        Ok((_, timing)) => crate::diagnostics::emit(
            "native_skin_runtime_timing",
            &serde_json::json!({
                "run_id": report.borrow().run_id,
                "interaction_ids": interaction_ids,
                "canvas_id": canvas_id,
                "output": output,
                "cache_phase": timing.cache_phase,
                "engine_us": timing.engine_us,
                "module_us": timing.module_us,
                "cache_duration_us": timing.duration_us,
                "duration_us": duration_us(started.elapsed()),
                "status": "success",
            }),
        ),
        Err(_) => crate::diagnostics::emit(
            "native_skin_runtime_timing",
            &serde_json::json!({
                "run_id": report.borrow().run_id,
                "interaction_ids": interaction_ids,
                "canvas_id": canvas_id,
                "output": output,
                "duration_us": duration_us(started.elapsed()),
                "status": "error",
                "error_type": "skin_runtime_create_failed",
            }),
        ),
    }
    result.map(|(runtime, _)| runtime)
}

#[derive(Clone)]
struct NativeOverlayProps {
    initial: NativeDocumentProjection,
    published: Rc<RefCell<Option<Reactive<NativeDocumentProjection>>>>,
    port: NativeEditorPort,
}

#[derive(Clone, PartialEq)]
// Keeping both complete projections inline avoids an allocation on each native frame update.
#[allow(clippy::large_enum_variant)]
enum NativeDocumentProjection {
    Display {
        canvas: scorepeek_overlay_ui::CanvasPresentation,
        visible: bool,
    },
    Editor(StageProjection),
}

#[derive(Clone)]
struct NativeEditorPort {
    coordinator: std::sync::mpsc::Sender<CoordinatorCommand>,
    source_output: Option<String>,
    run_id: String,
    sequence: Arc<std::sync::atomic::AtomicU64>,
}

impl NativeEditorPort {
    fn send(&self, input: EditorInput, action: &'static str) {
        let interaction_id = self
            .sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .saturating_add(1);
        let _ = self.coordinator.send(CoordinatorCommand::EditorInput {
            input,
            correlation: Some(InteractionCorrelation {
                run_id: self.run_id.clone(),
                interaction_id,
                action,
            }),
        });
    }
}

struct Reactive<T: 'static>(Signal<T>);

impl<T: 'static> Clone for Reactive<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: 'static> Copy for Reactive<T> {}

impl<T: 'static> Reactive<T> {
    fn borrow(&self) -> dioxus::signals::ReadableRef<'_, Signal<T>> {
        self.0.read()
    }

    fn set(&self, value: T) {
        let mut signal = self.0;
        signal.set(value);
    }
}

#[allow(clippy::cast_precision_loss)]
fn native_overlay(props: NativeOverlayProps) -> Element {
    let initial = props.initial.clone();
    let projection = Reactive(use_signal(move || initial));
    *props.published.borrow_mut() = Some(projection);
    let surface_port = props.port.clone();
    let onsurface = Callback::new(move |action: SurfaceAction| match &action {
        SurfaceAction::Enter(canvas) => {
            let current = projection.borrow();
            if let NativeDocumentProjection::Display {
                canvas: visible, ..
            } = &*current
            {
                let _ = surface_port.coordinator.send(CoordinatorCommand::Open {
                    output: surface_port.source_output.clone(),
                    canvas: canvas.clone().unwrap_or_else(|| visible.id.clone()),
                    preview_screen: None,
                });
            } else {
                surface_port.send(EditorInput::Surface(action), "surface");
            }
        }
        _ => surface_port.send(EditorInput::Surface(action), "surface"),
    });
    let action_port = props.port;
    let onaction = Callback::new(move |action: EditorAction| {
        let name = action.diagnostic_name();
        action_port.send(EditorInput::Action(action), name);
    });
    match &*projection.borrow() {
        NativeDocumentProjection::Display { canvas, visible } => rsx! {
            EditorSurface { onaction:onsurface,
                div { class:"canvas-content",style:format!("display:{};opacity:{}",if *visible{"block"}else{"none"},f32::from(canvas.opacity_percent)/100.0),
                    div { id:"scorepeek-skin-root", class:"scorepeek-skin-scope", "data-backend":"native", style:"position:absolute;inset:0" }
                }
            }
        },
        NativeDocumentProjection::Editor(stage) => {
            let selected = stage.selected_canvas.as_ref();
            rsx! {
                EditorSurface { onaction:onsurface,
                    div { id:"scorepeek-skin-root", style:"display:none" }
                    for canvas in &stage.canvases {
                        Fragment { key:"{canvas.id}",
                            EditorCanvas {canvas:canvas.clone(),editing:stage.interactive,selected:stage.interactive&&selected.is_some_and(|selected|selected.id==canvas.id),selected_widget:stage.selected_widget.clone(),onaction:onsurface,
                                div { id:editor_skin_root_id(&canvas.id), class:"scorepeek-skin-scope", "data-backend":"native", "data-skin-canvas":"{canvas.id}", style:"position:absolute;inset:0" }
                            }
                        }
                    }
                    if stage.interactive {
                        if let Some(canvas)=selected {
                            EditorSelectionMetrics { canvas:canvas.clone(), selected_widget:stage.selected_widget.clone() }
                        }
                        EditorPanel {
                            view:stage.view.clone(),
                            title:stage.title.clone(),
                            onaction,
                        }
                        if let Some(kind)=stage.placing {
                            PlacementPreview {kind,point:stage.point.map(f64::from)}
                        }
                    }
                    if let Some(notice)=&stage.notice {
                        div { id:"notice", class:"show error", role:"alert", "{notice}" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
fn parse_refresh_rate(text: &str) -> Result<scorepeek_overlay_ui::WaylandRefreshRate, String> {
    text.parse::<u16>()
        .map_err(|_| "Enter an integer from 1 through 1000 Hz".to_owned())
        .and_then(|hz| scorepeek_overlay_ui::WaylandRefreshRate::capped(hz).map_err(str::to_owned))
}

struct CalloopWaker(Ping);

fn widget_layout(widget: &crate::config::Widget) -> WidgetLayout {
    WidgetLayout {
        id: widget.id.clone(),
        kind: widget.kind,
        x: widget.x,
        y: widget.y,
        width: widget.width,
        height: widget.height,
        settings: widget.settings.clone(),
        skin_properties: widget.skin_properties.clone(),
    }
}

fn native_skin_input(
    canvas: &crate::config::Canvas,
    state: &OverlayState,
    manifest: &crate::skin::Manifest,
) -> serde_json::Value {
    let mut canvas_properties = manifest.effective_canvas_properties(&canvas.skin_properties);
    canvas_properties.insert(
        "background".into(),
        serde_json::to_value(canvas.background).expect("Background serialization is infallible"),
    );
    serde_json::json!({
        "schema":"scorepeek-skin-input-v1",
        "backend":"native",
        "canvas":{"id":canvas.id,"skin":canvas.skin.name(),"width":canvas.width,"height":canvas.height,"properties":canvas_properties},
        "widgets":canvas.widgets.iter().map(|widget| { let kind = serde_json::to_value(widget.kind).ok().and_then(|value| value.as_str().map(str::to_owned)).unwrap_or_default(); let properties = manifest.effective_widget_properties(&kind,&widget.skin_properties); serde_json::json!({"id":widget.id,"kind":widget.kind,"x":widget.x,"y":widget.y,"width":widget.width,"height":widget.height,"settings":widget.settings,"properties":properties}) }).collect::<Vec<_>>(),
        "state":state,
    })
}

fn native_skin_input_presentation(
    canvas: &scorepeek_overlay_ui::CanvasPresentation,
    state: &OverlayState,
    manifest: &crate::skin::Manifest,
) -> serde_json::Value {
    let mut canvas_properties = manifest.effective_canvas_properties(&canvas.skin_properties);
    canvas_properties.insert(
        "background".into(),
        serde_json::to_value(canvas.background).expect("Background serialization is infallible"),
    );
    serde_json::json!({
        "schema":"scorepeek-skin-input-v1",
        "backend":"native",
        "canvas":{"id":canvas.id,"skin":canvas.skin.name(),"width":canvas.width,"height":canvas.height,"properties":canvas_properties},
        "widgets":canvas.widgets.iter().map(|widget| { let kind = serde_json::to_value(widget.kind).ok().and_then(|value| value.as_str().map(str::to_owned)).unwrap_or_default(); let properties = manifest.effective_widget_properties(&kind,&widget.skin_properties); serde_json::json!({"id":widget.id,"kind":widget.kind,"x":widget.x,"y":widget.y,"width":widget.width,"height":widget.height,"settings":widget.settings,"properties":properties}) }).collect::<Vec<_>>(),
        "state":state,
    })
}

fn skin_deadline(schedule: &crate::skin::Schedule, editing: bool) -> Option<Instant> {
    if editing {
        return None;
    }
    match schedule {
        crate::skin::Schedule::Idle => None,
        crate::skin::Schedule::NextFrame => Some(Instant::now()),
        crate::skin::Schedule::AfterMs { milliseconds } => {
            Instant::now().checked_add(Duration::from_millis(*milliseconds))
        }
    }
}

fn skin_error_type(error: &str) -> &'static str {
    if error.contains("timeout") {
        "hard_timeout"
    } else if error.contains("trap") {
        "trap"
    } else if error.contains("tree") || error.contains("JSON") {
        "invalid_output"
    } else {
        "runtime_error"
    }
}

fn embedded_editor_skins() -> Vec<scorepeek_overlay_ui::editor::EditorSkin> {
    [
        include_str!("../../../skins/cyan-system/skin.toml"),
        include_str!("../../../skins/result-aurora/skin.toml"),
        include_str!("../../../skins/dj-blackbox/skin.toml"),
    ]
    .into_iter()
    .filter_map(|source| {
        let manifest: crate::skin::Manifest = toml::from_str(source).ok()?;
        Some(scorepeek_overlay_ui::editor::EditorSkin {
            id: manifest.id.parse().ok()?,
            name: manifest.name,
            release: manifest.release,
            preview: String::new(),
            preview_video: None,
            canvas_properties: serde_json::from_value(
                serde_json::to_value(manifest.canvas_properties).ok()?,
            )
            .ok()?,
            widget_properties: serde_json::from_value(
                serde_json::to_value(manifest.widget_properties).ok()?,
            )
            .ok()?,
        })
    })
    .collect()
}
#[allow(clippy::cast_possible_truncation)]
fn snap_i32(value: f64) -> i32 {
    ((value / 4.0).round() * 4.0).clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}
const fn grid_floor(value: u32) -> u32 {
    value / 4 * 4
}
fn maximum_grid_position(output: u32, extent: u32) -> i32 {
    i32::try_from(grid_floor(output.saturating_sub(extent))).unwrap_or(i32::MAX)
}

impl Wake for CalloopWaker {
    fn wake(self: Arc<Self>) {
        self.0.ping();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.ping();
    }
}

fn watch_parent(
    mut input: impl std::io::Read + Send + 'static,
    stop: Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("overlay-parent".into())
        .spawn(move || {
            let mut byte = [0];
            let _ = input.read(&mut byte);
            stop.store(true, std::sync::atomic::Ordering::Release);
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn recent_same_failure(
    failure: &(Option<String>, Instant),
    canvas: &crate::config::Canvas,
) -> bool {
    failure.0.as_deref() == Some(canvas.output.as_str())
        && failure.1.elapsed() < Duration::from_secs(5)
}

fn execute_native_editor_effect(
    effect: &EditorEffect,
    control_socket: &std::path::Path,
    editor_id: &str,
    wayland_refresh_hz: scorepeek_overlay_ui::WaylandRefreshRate,
    fallback: &[scorepeek_overlay_ui::CanvasPresentation],
) -> EditorBackendReply {
    let request = match effect {
        EditorEffect::Acquire => crate::control::Request::AcquireBackend {
            backend: crate::runtime::Backend::Wayland,
            editor_id: editor_id.to_owned(),
        },
        EditorEffect::KeepAlive => crate::control::Request::KeepAliveBackend {
            backend: crate::runtime::Backend::Wayland,
            editor_id: editor_id.to_owned(),
        },
        EditorEffect::Update { canvases } => crate::control::Request::UpdateBackendDraft {
            backend: crate::runtime::Backend::Wayland,
            editor_id: editor_id.to_owned(),
            canvases: canvases.clone(),
            wayland_refresh_hz: Some(wayland_refresh_hz),
        },
        EditorEffect::Save { canvases } => crate::control::Request::CommitBackend {
            backend: crate::runtime::Backend::Wayland,
            editor_id: editor_id.to_owned(),
            canvases: canvases.clone(),
            wayland_refresh_hz: Some(wayland_refresh_hz),
        },
        EditorEffect::Discard | EditorEffect::Close => crate::control::Request::ReleaseBackend {
            backend: crate::runtime::Backend::Wayland,
            editor_id: editor_id.to_owned(),
        },
    };
    let started = Instant::now();
    let response = crate::control::request(control_socket, &request);
    let request_name = match effect {
        EditorEffect::Acquire => "acquire_backend",
        EditorEffect::KeepAlive => "keep_alive_backend",
        EditorEffect::Update { .. } => "update_backend_draft",
        EditorEffect::Save { .. } => "commit_backend",
        EditorEffect::Discard | EditorEffect::Close => "release_backend",
    };
    crate::diagnostics::emit(
        "native_editor_effect",
        &serde_json::json!({
            "request": request_name,
            "effect": format!("{:?}", effect.kind()),
            "duration_us": duration_us(started.elapsed()),
            "status": if response.as_ref().is_ok_and(|reply| reply.ok) { "success" } else { "error" },
        }),
    );
    match response {
        Ok(response) => EditorBackendReply {
            ok: response.ok,
            readonly: response.readonly,
            error: response.error,
            canvases: if !response.ok
                || (response.canvases.is_empty()
                    && !matches!(
                        effect,
                        EditorEffect::Acquire
                            | EditorEffect::Update { .. }
                            | EditorEffect::Save { .. }
                    )) {
                fallback.to_vec()
            } else {
                response.canvases
            },
            dirty: response.dirty,
        },
        Err(error) => EditorBackendReply {
            ok: false,
            readonly: true,
            error: Some(error),
            canvases: fallback.to_vec(),
            dirty: true,
        },
    }
}

fn execute_native_editor_effects(
    authority: &mut NativeEditorAuthority,
    effects: Vec<EditorEffect>,
    control_socket: &std::path::Path,
    editor_id: &str,
    wayland_refresh_hz: scorepeek_overlay_ui::WaylandRefreshRate,
) {
    let mut pending = std::collections::VecDeque::from(effects);
    while let Some(effect) = pending.pop_front() {
        let fallback = authority.session().draft.clone();
        let reply = execute_native_editor_effect(
            &effect,
            control_socket,
            editor_id,
            wayland_refresh_hz,
            &fallback,
        );
        pending.extend(authority.dispatch(EditorInput::BackendCompleted {
            effect: effect.kind(),
            requested_draft: effect.requested_draft().map(<[_]>::to_vec),
            reply,
        }));
    }
}

/// Runs until the parent's lifetime lease closes.
/// # Errors
/// Returns Wayland, GPU or event-loop failures.
#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
pub fn run(config: Config, input: impl std::io::Read + Send + 'static) -> Result<(), String> {
    run_with_editor_scenario(config, input, Vec::new())
}

/// A timed editor action used by the checked-in nested compositor scenario.
#[doc(hidden)]
pub struct NativeEditorScenarioStep {
    pub after: Duration,
    pub target_canvas: &'static str,
    pub name: &'static str,
    pub action: EditorAction,
}

/// Runs the production native coordinator with a deterministic scenario input.
///
/// The driver only replaces human timing. Every action still crosses the
/// production coordinator and is reduced by the sole `EditorSession` authority.
/// # Errors
/// Returns Wayland, GPU or event-loop failures.
#[doc(hidden)]
#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
pub fn run_with_editor_scenario(
    config: Config,
    input: impl std::io::Read + Send + 'static,
    scenario: Vec<NativeEditorScenarioStep>,
) -> Result<(), String> {
    use std::collections::BTreeMap;
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    watch_parent(input, Arc::clone(&stop))?;
    let canvas_wakes = Arc::new(std::sync::Mutex::new(BTreeMap::<String, Ping>::new()));
    let waking = Arc::clone(&canvas_wakes);
    let feed = Feed::start(
        config.clone(),
        Arc::new(move || {
            for wake in waking
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .values()
            {
                wake.ping();
            }
        }),
    )
    .map_err(|error| error.to_string())?;
    let feed_state = Arc::clone(&feed.state);
    let feed_stop = Arc::clone(&feed.stop);
    let mut desired = config.canvases.clone();
    let mut workers = BTreeMap::<String, NativeWorker>::new();
    let mut failed = BTreeMap::<String, (Option<String>, Instant)>::new();
    let (coordinator_tx, coordinator_rx) = std::sync::mpsc::channel();
    if !scenario.is_empty() {
        let scenario_tx = coordinator_tx.clone();
        std::thread::Builder::new()
            .name("overlay-wayland-scenario".into())
            .spawn(move || {
                for (interaction_id, step) in scenario.into_iter().enumerate() {
                    std::thread::sleep(step.after);
                    if scenario_tx
                        .send(CoordinatorCommand::EditorInput {
                            input: EditorInput::Action(EditorAction::SelectCanvas(
                                step.target_canvas.into(),
                            )),
                            correlation: Some(InteractionCorrelation {
                                run_id: "nested-editor-setup".into(),
                                interaction_id: u64::try_from(interaction_id).unwrap_or(u64::MAX),
                                action: "target_canvas_selected",
                            }),
                        })
                        .is_err()
                    {
                        break;
                    }
                    if scenario_tx
                        .send(CoordinatorCommand::EditorInput {
                            input: EditorInput::Action(step.action),
                            correlation: Some(InteractionCorrelation {
                                run_id: "nested-editor-scenario".into(),
                                interaction_id: u64::try_from(interaction_id).unwrap_or(u64::MAX),
                                action: step.name,
                            }),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|error| error.to_string())?;
    }
    let editor_id = format!("wayland-{}", std::process::id());
    let wayland_refresh_hz = Arc::new(std::sync::Mutex::new(config.wayland_refresh_hz));
    let start_editing = config.edit_on_start || desired.is_empty();
    let skin_assets = Arc::new(SkinAssetCache::new(crate::skin::StoreRoot::new(
        config.skin_store.clone(),
    )));
    let installed_skins = skin_assets.installed_editor_skins().unwrap_or_default();
    let fallback_skin = installed_skins.first().map(|skin| skin.id);
    let probe = desired.first().cloned().map_or_else(
        || {
            fallback_skin
                .map(editor_bootstrap)
                .ok_or_else(|| String::from("Wayland editor requires at least one installed skin"))
        },
        Ok,
    )?;
    let outputs = discover_editor_outputs(&probe)?;
    let initial = desired
        .iter()
        .map(crate::config::Canvas::presentation)
        .collect::<Vec<_>>();
    let viewport = outputs
        .first()
        .and_then(|output| output.logical_size)
        .unwrap_or([1920, 1080]);
    let mut session = EditorSession::new(initial, viewport, "wayland");
    session.set_session_id(
        u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros(),
        )
        .unwrap_or(u64::MAX),
    );
    session.set_skins(installed_skins);
    session.set_outputs(editor_outputs_from_descriptions(&outputs));
    let published_stages = Arc::new(std::sync::Mutex::new(PublishedStages::default()));
    let mut authority = NativeEditorAuthority::new(session, Arc::clone(&published_stages));
    if start_editing {
        let effects = authority.dispatch(EditorInput::Open {
            output: outputs.first().map(|output| output.name.clone()),
            canvas: desired.first().map(|canvas| canvas.id.clone()),
            preview: scorepeek_overlay_ui::ScreenKind::MusicSelect,
        });
        execute_native_editor_effects(
            &mut authority,
            effects,
            &config.control_socket,
            &editor_id,
            config.wayland_refresh_hz,
        );
    }
    let mut was_editing = authority.session().editing;
    let mut projection_cache = NativeProjectionCache::default();
    let mut coordinator_work = FrameWorkProfile::default();
    let mut next_keepalive = Instant::now() + Duration::from_secs(5);
    while !stop.load(std::sync::atomic::Ordering::Acquire) {
        while let Ok(command) = coordinator_rx.try_recv() {
            match command {
                CoordinatorCommand::EditorInput { input, correlation } => {
                    if let Some(correlation) = correlation.as_ref() {
                        crate::diagnostics::emit(
                            "native_editor_input_transport",
                            &serde_json::json!({
                                "run_id": correlation.run_id,
                                "interaction_id": correlation.interaction_id,
                                "action": correlation.action,
                                "status": "received",
                            }),
                        );
                    }
                    let before_revision = authority.session().revision;
                    let effects = authority.dispatch(input);
                    let refresh = *wayland_refresh_hz
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    execute_native_editor_effects(
                        &mut authority,
                        effects,
                        &config.control_socket,
                        &editor_id,
                        refresh,
                    );
                    if let Some(correlation) = correlation {
                        let session = authority.session();
                        crate::diagnostics::emit(
                            "native_editor_input_applied",
                            &serde_json::json!({
                                "run_id": correlation.run_id,
                                "interaction_id": correlation.interaction_id,
                                "action": correlation.action,
                                "session_id": session.session_id,
                                "revision": session.revision,
                                "changed": session.revision > before_revision,
                                "status": "applied",
                            }),
                        );
                    }
                }
                CoordinatorCommand::Open {
                    output,
                    canvas,
                    preview_screen,
                } => {
                    let effects = authority.dispatch(EditorInput::Open {
                        output,
                        canvas: Some(canvas),
                        preview: preview_screen
                            .unwrap_or(scorepeek_overlay_ui::ScreenKind::MusicSelect),
                    });
                    let refresh = *wayland_refresh_hz
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    execute_native_editor_effects(
                        &mut authority,
                        effects,
                        &config.control_socket,
                        &editor_id,
                        refresh,
                    );
                }
                CoordinatorCommand::ResolveOutput {
                    output_names,
                    output,
                    canvas,
                    preview_screen,
                } => {
                    let result = crate::control::request(
                        &config.control_socket,
                        &crate::control::Request::ResolveWaylandOutputs {
                            outputs: output_names,
                        },
                    );
                    if let Ok(response) = result
                        && response.ok
                    {
                        let _ = authority.dispatch(EditorInput::LegacyOutputsResolved {
                            unresolved_output: crate::config::UNRESOLVED_WAYLAND_OUTPUT_ID.into(),
                            canvases: response.canvases,
                        });
                        let _ = coordinator_tx.send(CoordinatorCommand::Open {
                            output,
                            canvas,
                            preview_screen,
                        });
                    }
                }
            }
            for wake in canvas_wakes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .values()
            {
                wake.ping();
            }
        }
        if authority.session().editing && Instant::now() >= next_keepalive {
            let effects = authority.dispatch(EditorInput::KeepAliveTick);
            let refresh = *wayland_refresh_hz
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            execute_native_editor_effects(
                &mut authority,
                effects,
                &config.control_socket,
                &editor_id,
                refresh,
            );
            next_keepalive = Instant::now() + Duration::from_secs(5);
        }
        let editing = authority.session().editing;
        if was_editing && !editing {
            if let Ok(response) = crate::control::request(
                &config.control_socket,
                &crate::control::Request::GetBackend {
                    backend: crate::runtime::Backend::Wayland,
                },
            ) {
                desired = response
                    .canvases
                    .into_iter()
                    .map(|presentation| {
                        let mut canvas = crate::config::empty_canvas(
                            presentation.id.clone(),
                            crate::runtime::Backend::Wayland,
                        );
                        canvas.apply_presentation(&presentation);
                        canvas
                    })
                    .collect();
            }
            if desired.is_empty() {
                let output = authority
                    .session()
                    .outputs
                    .first()
                    .map(|output| output.name.clone());
                let effects = authority.dispatch(EditorInput::Open {
                    output,
                    canvas: None,
                    preview: scorepeek_overlay_ui::ScreenKind::MusicSelect,
                });
                let refresh = *wayland_refresh_hz
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                execute_native_editor_effects(
                    &mut authority,
                    effects,
                    &config.control_socket,
                    &editor_id,
                    refresh,
                );
            }
        }
        was_editing = authority.session().editing;
        let editing = was_editing;
        let session = authority.session();
        let projection_started = Instant::now();
        let projected = projection_cache.resolve(fallback_skin, &session, &desired)?;
        coordinator_work.record("projection", projection_started.elapsed());
        let lifecycle_started = Instant::now();
        let reconciliation = reconcile_worker_lifecycle(
            workers.iter().map(|(id, worker)| {
                (
                    id.as_str(),
                    worker.output.as_deref(),
                    worker.join.is_finished(),
                )
            }),
            projected,
        );
        coordinator_work.record("surface_lifecycle", lifecycle_started.elapsed());
        stop_workers(reconciliation.stop_join.iter(), &workers, &canvas_wakes);
        for id in reconciliation.stop_join {
            if let Some(worker) = workers.remove(&id) {
                let output = worker.output.clone();
                match worker.join.join() {
                    Ok(Ok(())) => {
                        failed.remove(&id);
                    }
                    Ok(Err(error)) => {
                        if let Some(canvas) = projected.iter().find(|canvas| canvas.id == id) {
                            failed
                                .insert(id.clone(), (Some(canvas.output.clone()), Instant::now()));
                        }
                        crate::diagnostics::emit(
                            "native_canvas_failed",
                            &serde_json::json!({"canvas_id":id,"output":output,"error":error}),
                        );
                    }
                    Err(_) => {
                        crate::diagnostics::emit(
                            "native_canvas_failed",
                            &serde_json::json!({"canvas_id":id,"output":output,"error":"panicked"}),
                        );
                    }
                }
            }
            canvas_wakes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&id);
        }
        for id in reconciliation.start {
            let Some(canvas) = projected.iter().find(|canvas| canvas.id == id) else {
                return Err(format!(
                    "worker reconciliation referenced missing projected canvas {id}"
                ));
            };
            if failed
                .get(&canvas.id)
                .is_some_and(|failure| recent_same_failure(failure, canvas))
            {
                continue;
            }
            let config_started = Instant::now();
            let mut canvas_config = config.clone();
            canvas_config.canvases = vec![canvas.clone()];
            coordinator_work.record("canvas_config", config_started.elapsed());
            let canvas_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stopping = Arc::clone(&canvas_stop);
            let state = Arc::clone(&feed_state);
            let stopped = Arc::clone(&feed_stop);
            let stages = Arc::clone(&published_stages);
            let refresh = Arc::clone(&wayland_refresh_hz);
            let wakes = Arc::clone(&canvas_wakes);
            let coordinator = coordinator_tx.clone();
            let assets = Arc::clone(&skin_assets);
            let role = if editing {
                SurfaceRole::EditorStage
            } else {
                SurfaceRole::DisplayCanvas
            };
            let join = std::thread::Builder::new()
                .name(format!("overlay-wayland-{}", canvas.id))
                .spawn(move || {
                    run_canvas(
                        &canvas_config,
                        stopping,
                        state,
                        stopped,
                        stages,
                        refresh,
                        &wakes,
                        coordinator,
                        role,
                        assets,
                    )
                })
                .map_err(|error| error.to_string())?;
            workers.insert(
                canvas.id.clone(),
                NativeWorker {
                    output: Some(canvas.output.clone()),
                    stop: canvas_stop,
                    join,
                },
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let ids = workers.keys().cloned().collect::<Vec<_>>();
    stop_workers(ids.iter(), &workers, &canvas_wakes);
    for (_, worker) in workers {
        let _ = worker.join.join();
    }
    if authority.session().editing {
        let effects = authority.dispatch(EditorInput::Action(EditorAction::Close));
        execute_native_editor_effects(
            &mut authority,
            effects,
            &config.control_socket,
            &editor_id,
            config.wayland_refresh_hz,
        );
    }
    crate::diagnostics::emit("native_coordinator_work", &coordinator_work);
    Ok(())
}

fn discover_editor_outputs(
    canvas: &crate::config::Canvas,
) -> Result<Vec<OutputDescription>, String> {
    let ping = make_ping().map_err(|error| error.to_string())?;
    let shell = Shell::open(
        Some(canvas.output.as_str()),
        canvas.width,
        canvas.height,
        canvas.x,
        canvas.y,
        false,
        ping.1,
    )?;
    Ok(shell.output_descriptions)
}

fn editor_outputs_from_descriptions(outputs: &[OutputDescription]) -> Vec<EditorOutput> {
    outputs
        .iter()
        .map(|output| EditorOutput {
            name: output.name.clone(),
            model: output.model.clone(),
            logical_size: output.logical_size,
        })
        .collect()
}

fn editor_bootstrap(skin: scorepeek_overlay_ui::Skin) -> crate::config::Canvas {
    let mut bootstrap = crate::config::empty_canvas(
        "__scorepeek-editor-bootstrap".into(),
        crate::runtime::Backend::Wayland,
    );
    bootstrap.skin = skin;
    bootstrap.x = 0;
    bootstrap.y = 0;
    bootstrap.width = 1920;
    bootstrap.height = 1080;
    bootstrap.show_on = Some(Vec::new());
    bootstrap
}

fn editor_stage_canvases(
    fallback_skin: Option<scorepeek_overlay_ui::Skin>,
    canvases: &[crate::config::Canvas],
    outputs: &[OutputDescription],
) -> Result<Vec<crate::config::Canvas>, String> {
    if outputs.is_empty() {
        return fallback_skin
            .map(editor_bootstrap)
            .map(|bootstrap| vec![bootstrap])
            .ok_or_else(|| "Wayland editor requires at least one installed skin".into());
    }
    let skin = if let Some(canvas) = canvases.first() {
        canvas.skin
    } else {
        fallback_skin.ok_or("Wayland editor requires at least one installed skin")?
    };
    Ok(editor_stage_projections(canvases, outputs, skin))
}

fn editor_stage_projections(
    canvases: &[crate::config::Canvas],
    outputs: &[OutputDescription],
    skin: scorepeek_overlay_ui::Skin,
) -> Vec<crate::config::Canvas> {
    outputs
        .iter()
        .enumerate()
        .map(|(index, output)| {
            let mut id = format!("__scorepeek-editor-stage-{index}");
            while canvases.iter().any(|canvas| canvas.id == id) {
                id.push('_');
            }
            let mut stage = crate::config::empty_canvas(id, crate::runtime::Backend::Wayland);
            stage.skin = skin;
            stage.output.clone_from(&output.name);
            stage.show_on = Some(Vec::new());
            stage.x = 0;
            stage.y = 0;
            if let Some([width, height]) = output.logical_size {
                stage.width = width.max(32);
                stage.height = height.max(32);
            }
            stage
        })
        .collect()
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn run_canvas(
    config: &Config,
    external_stop: Arc<std::sync::atomic::AtomicBool>,
    feed_state: Arc<std::sync::Mutex<OverlayState>>,
    feed_stop: Arc<std::sync::atomic::AtomicBool>,
    published_stages: Arc<std::sync::Mutex<PublishedStages>>,
    wayland_refresh_hz: Arc<std::sync::Mutex<scorepeek_overlay_ui::WaylandRefreshRate>>,
    wakes: &Arc<std::sync::Mutex<std::collections::BTreeMap<String, Ping>>>,
    coordinator: std::sync::mpsc::Sender<CoordinatorCommand>,
    role: SurfaceRole,
    skin_assets: Arc<SkinAssetCache>,
) -> Result<(), String> {
    let startup_started = Instant::now();
    let report = Rc::new(RefCell::new(RunReport::new()));
    let ping = make_ping().map_err(|e| e.to_string())?;
    let wake = ping.0.clone();
    let mut canvas = config
        .canvases
        .first()
        .ok_or("Wayland overlay has no canvas")?
        .clone();
    wakes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(canvas.id.clone(), wake);
    let configured_output = canvas.output.clone();
    let shell_started = Instant::now();
    let shell = Shell::open(
        Some(canvas.output.as_str()),
        canvas.width,
        canvas.height,
        canvas.x,
        canvas.y,
        false,
        ping.1,
    )?;
    crate::diagnostics::emit(
        "native_startup_timing",
        &serde_json::json!({
            "run_id": report.borrow().run_id,
            "canvas_id": canvas.id,
            "phase": "shell_opened",
            "duration_us": duration_us(shell_started.elapsed()),
            "elapsed_us": duration_us(startup_started.elapsed()),
        }),
    );
    let mut pending_resolved_output = None;
    if let Some(selected_output) = shell.output_name.as_deref()
        && canvas.output != selected_output
    {
        pending_resolved_output = Some(selected_output.to_owned());
        if let Some([output_width, output_height]) = shell.output_logical_size {
            canvas.width = canvas.width.min(grid_floor(output_width));
            canvas.height = canvas.height.min(grid_floor(output_height));
            canvas.x = canvas
                .x
                .clamp(0, maximum_grid_position(output_width, canvas.width));
            canvas.y = canvas
                .y
                .clamp(0, maximum_grid_position(output_height, canvas.height));
        }
        crate::diagnostics::emit(
            "native_output_fallback",
            &serde_json::json!({
                "canvas_id": canvas.id,
                "configured_output": configured_output,
                "selected_output": selected_output,
                "status": "draft_required",
            }),
        );
    }
    canvas.x = shell.position[0];
    canvas.y = shell.position[1];
    if pending_resolved_output.is_some()
        && let Some([output_width, output_height]) = shell.output_logical_size
    {
        if canvas.id.starts_with("__scorepeek-editor-") {
            canvas.x = 0;
            canvas.y = 0;
            canvas.width = output_width.max(32);
            canvas.height = output_height.max(32);
        }
        canvas.x = canvas
            .x
            .clamp(0, maximum_grid_position(output_width, canvas.width));
        canvas.y = canvas
            .y
            .clamp(0, maximum_grid_position(output_height, canvas.height));
    }
    let outputs = shell.output_descriptions.clone();
    report
        .borrow_mut()
        .output_name
        .clone_from(&shell.output_name);
    report
        .borrow_mut()
        .operations
        .push(if shell.fractional_scaling {
            "fractional_scale_enabled"
        } else {
            "integer_scale_fallback"
        });
    let renderer_started = Instant::now();
    let renderer = VelloWindowRenderer::with_options(
        VelloRendererOptions::default()
            .base_color(peniko::Color::TRANSPARENT)
            .composite_alpha_mode(CompositeAlphaMode::Transparent),
    );
    let renderer_duration = renderer_started.elapsed();
    crate::diagnostics::emit(
        "native_startup_timing",
        &serde_json::json!({
            "run_id": report.borrow().run_id,
            "canvas_id": canvas.id,
            "phase": "renderer_created",
            "duration_us": duration_us(renderer_duration),
            "elapsed_us": duration_us(startup_started.elapsed()),
        }),
    );
    report
        .borrow_mut()
        .operations
        .push("renderer_context_initialized");
    let appearance = Appearance { skin: canvas.skin };
    let widgets = canvas.widgets.iter().map(widget_layout).collect();
    let app_started = Instant::now();
    let mut app = App::new(
        appearance,
        widgets,
        renderer,
        shell,
        Waker::from(Arc::new(CalloopWaker(ping.0))),
        feed_state,
        feed_stop,
        external_stop,
        canvas,
        skin_assets,
        outputs,
        Rc::clone(&report),
        pending_resolved_output,
        published_stages,
        wayland_refresh_hz,
        startup_started,
        coordinator,
        role,
    )?;
    crate::diagnostics::emit(
        "native_startup_timing",
        &serde_json::json!({
            "run_id": report.borrow().run_id,
            "canvas_id": app.surface_canvas.id,
            "phase": "app_initialized",
            "duration_us": duration_us(app_started.elapsed()),
            "elapsed_us": duration_us(startup_started.elapsed()),
        }),
    );
    let result = app.run();
    let unmap = shutdown_native_surface(
        &mut app,
        |app| {
            app.unmount_skin_resources();
            crate::diagnostics::emit(
                "native_renderer_shutdown",
                &serde_json::json!({
                    "run_id": report.borrow().run_id,
                    "canvas_id": app.surface_canvas.id,
                    "output": app.surface_output,
                    "phase": "app_stopped",
                }),
            );
        },
        |app| {
            crate::diagnostics::emit(
                "native_renderer_shutdown",
                &serde_json::json!({
                    "run_id": report.borrow().run_id,
                    "canvas_id": app.surface_canvas.id,
                    "output": app.surface_output,
                    "phase": "suspend_requested",
                }),
            );
            crate::diagnostics::emit(
                "native_renderer_shutdown",
                &serde_json::json!({
                    "run_id": report.borrow().run_id,
                    "canvas_id": app.surface_canvas.id,
                    "output": app.surface_output,
                    "phase": "suspend_started",
                }),
            );
            app.renderer.suspend();
            crate::diagnostics::emit(
                "native_renderer_shutdown",
                &serde_json::json!({
                    "run_id": report.borrow().run_id,
                    "canvas_id": app.surface_canvas.id,
                    "output": app.surface_output,
                    "phase": "suspend_completed",
                }),
            );
        },
        |app| app.shell.unmap(),
    );
    crate::diagnostics::emit(
        "native_surface_unmap",
        &serde_json::json!({
            "run_id": report.borrow().run_id,
            "canvas_id": app.surface_canvas.id,
            "output": app.surface_output,
            "status": if unmap.is_ok() { "success" } else { "error" },
            "error_type": unmap.as_ref().err().map(|_| "wayland_flush_failed"),
        }),
    );
    let (result, secondary_failure) = native_shutdown_result(result, unmap);
    {
        let mut report = report.borrow_mut();
        report.paint_count = app.paint_count;
        report.render_calls = app.render_calls;
        report.editor_skin_update_requests = app.editor_skin_updates.requests;
        report.editor_skin_render_count = app.editor_skin_updates.renders;
        report.skin_package_open_count = app
            .skin_assets
            .open_count
            .load(std::sync::atomic::Ordering::Relaxed);
        report.skin_package_open_ns = app
            .skin_assets
            .open_ns
            .load(std::sync::atomic::Ordering::Relaxed);
        report.skin_package_clone_count = app
            .skin_assets
            .clone_count
            .load(std::sync::atomic::Ordering::Relaxed);
        report.skin_package_clone_ns = app
            .skin_assets
            .clone_ns
            .load(std::sync::atomic::Ordering::Relaxed);
        report.resource_lookup_count = app
            .skin_assets
            .resource_lookup_count
            .load(std::sync::atomic::Ordering::Relaxed);
        report.resource_lookup_ns = app
            .skin_assets
            .resource_lookup_ns
            .load(std::sync::atomic::Ordering::Relaxed);
        report.skin_runtime_create_count = app.skin_runtime_create_count;
        report.frame_work = app.frame_work.clone();
        report.elapsed_ms = u64::try_from(app.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        report.wayland_refresh_hz = *app
            .wayland_refresh_hz
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let seconds = app.started.elapsed().as_secs_f64();
        report.effective_paint_hz = (seconds > 0.0).then(|| f64::from(app.paint_count) / seconds);
        report.effective_steady_paint_hz =
            (seconds > 0.0).then(|| f64::from(app.steady_paint_count) / seconds);
        report.status = if result.is_ok() { "complete" } else { "failed" };
        report.failure_type = result
            .as_ref()
            .err()
            .map(|error| canvas_worker_error_type(error));
        report.failure = result.as_ref().err().cloned();
        report.secondary_failure_type = secondary_failure.as_deref().map(canvas_worker_error_type);
        report.secondary_failure = secondary_failure;
        report.operations.push("shutdown");
        crate::diagnostics::emit("native_summary", &*report);
    }
    drop(app);
    result
}

struct App {
    // Renderer is dropped before the shell; its own Arc handle also retains ownership.
    renderer: VelloWindowRenderer,
    shell: Shell,
    document: DioxusDocument,
    pointer: PointerInput,
    projection: Reactive<NativeDocumentProjection>,
    published_stages: Arc<std::sync::Mutex<PublishedStages>>,
    waker: Waker,
    started: Instant,
    animating: bool,
    paint_count: u32,
    render_calls: u32,
    steady_paint_count: u32,
    pending_paint: bool,
    full_layout_pending: bool,
    cadence: FrameCadence,
    editor_skin_updates: EditorSkinUpdates,
    report: Rc<RefCell<RunReport>>,
    feed_state: Arc<std::sync::Mutex<OverlayState>>,
    current_state: OverlayState,
    feed_stop: Arc<std::sync::atomic::AtomicBool>,
    external_stop: Arc<std::sync::atomic::AtomicBool>,
    surface_canvas: crate::config::Canvas,
    surface_output: Option<String>,
    canvas: crate::config::Canvas,
    role: SurfaceRole,
    coordinator: std::sync::mpsc::Sender<CoordinatorCommand>,
    output_descriptions: Vec<OutputDescription>,
    pending_resolved_output: Option<String>,
    next_output_persist: Instant,
    surface_logical: [u32; 2],
    wayland_refresh_hz: Arc<std::sync::Mutex<scorepeek_overlay_ui::WaylandRefreshRate>>,
    display_skin: Option<NativeDisplaySkin>,
    editor_skin_previews: std::collections::BTreeMap<String, EditorSkinPreview>,
    next_skin_render: Option<Instant>,
    skin_assets: Arc<SkinAssetCache>,
    skin_runtime_create_count: u64,
    frame_work: FrameWorkProfile,
    pending_frame_start: Option<FrameWorkSample>,
    startup_started: Instant,
    /// Renderer/protocol composition state used only to normalize Wayland key events to DOM.
    text_composition: TextComposition,
}

#[derive(Clone, Default, PartialEq, Eq)]
enum TextComposition {
    #[default]
    Idle,
    Active(String),
}

struct EditorSkinPreview {
    canvas: crate::config::Canvas,
    package: Arc<crate::skin::Package>,
    runtime: crate::skin::Runtime,
    tree: crate::skin::NativeTree,
    next_render: Option<Instant>,
    last_input: serde_json::Value,
    last_state: OverlayState,
}

struct NativeDisplaySkin {
    runtime: crate::skin::Runtime,
    tree: crate::skin::NativeTree,
    release: String,
    manifest: crate::skin::Manifest,
}

fn create_native_display_skin(
    document: &mut DioxusDocument,
    canvas: &crate::config::Canvas,
    package: &Arc<crate::skin::Package>,
    report: &Rc<RefCell<RunReport>>,
    output: Option<&str>,
    state: &OverlayState,
) -> Result<(NativeDisplaySkin, Option<Instant>), String> {
    let mut runtime = new_native_skin_runtime(package, report, &canvas.id, output, &[])?;
    let skin_input = native_skin_input(canvas, state, &package.manifest);
    let started = Instant::now();
    let mut initial = runtime.init(&skin_input).inspect_err(|error| {
        crate::diagnostics::emit(
            "skin_render",
            &serde_json::json!({"skin_id":package.manifest.id,"release":package.manifest.release,"canvas_id":canvas.id,"backend":"native","phase":"init","status":"failed","error_type":skin_error_type(error)}),
        );
    })?;
    namespace_native_skin_output(&package.manifest.id, &mut initial);
    let root = document
        .inner
        .borrow()
        .query_selector("#scorepeek-skin-root")
        .map_err(|error| format!("query native skin root: {error:?}"))?
        .ok_or("native skin root is missing")?;
    let css = namespace_skin_css(
        canvas.skin.name(),
        std::str::from_utf8(
            package
                .resource(crate::skin::STYLE_PATH)
                .ok_or("skin.css missing")?,
        )
        .map_err(|error| format!("skin.css is not UTF-8: {error}"))?,
    );
    let mut tree = crate::skin::NativeTree::new(&mut document.inner.borrow_mut(), root, &css);
    tree.apply(&mut document.inner.borrow_mut(), &initial);
    crate::diagnostics::emit(
        "skin_render",
        &serde_json::json!({"skin_id":package.manifest.id,"release":package.manifest.release,"canvas_id":canvas.id,"backend":"native","phase":"init","status":"success","duration_us":u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),"tree_applied":true}),
    );
    let next = skin_deadline(&initial.schedule, false);
    Ok((
        NativeDisplaySkin {
            runtime,
            tree,
            release: package.manifest.release.clone(),
            manifest: package.manifest.clone(),
        },
        next,
    ))
}

#[derive(Default)]
struct EditorSkinReconciliation {
    retry_owner: bool,
    wasm_calls: u64,
    tree_updates: u64,
    input_generations: u64,
}

fn create_editor_skin_preview(
    document: &mut DioxusDocument,
    presentation: &scorepeek_overlay_ui::CanvasPresentation,
    skin_assets: &Arc<SkinAssetCache>,
    report: &Rc<RefCell<RunReport>>,
    output: Option<&str>,
    state: &OverlayState,
    work: &mut FrameWorkProfile,
) -> Result<EditorSkinPreview, String> {
    let canvas = work.measure("canvas_config", || {
        let mut canvas =
            crate::config::empty_canvas(presentation.id.clone(), crate::runtime::Backend::Wayland);
        canvas.apply_presentation(presentation);
        canvas
    });
    let package = skin_assets.load_profiled(canvas.skin.name(), work)?;
    let mut runtime = work.measure("wasm_runtime_create", || {
        new_native_skin_runtime(&package, report, &canvas.id, output, &[])
    })?;
    let input = work.measure("skin_input", || {
        native_skin_input(&canvas, state, &package.manifest)
    });
    let (mut rendered, timing) = runtime.init_measured(&input)?;
    work.record("wasm_render", timing.wasm);
    work.record("json_tree", timing.json_tree);
    namespace_native_skin_output(&package.manifest.id, &mut rendered);
    let root_id = editor_skin_root_id(&canvas.id);
    let root = document
        .inner
        .borrow()
        .query_selector(&format!("#{root_id}"))
        .map_err(|error| format!("query editor skin root: {error:?}"))?
        .ok_or_else(|| format!("editor skin root is missing for {}", canvas.id))?;
    let css = namespace_skin_css(
        canvas.skin.name(),
        std::str::from_utf8(
            package
                .resource(crate::skin::STYLE_PATH)
                .ok_or("skin.css missing")?,
        )
        .map_err(|error| format!("skin.css is not UTF-8: {error}"))?,
    );
    let mut tree = crate::skin::NativeTree::new(&mut document.inner.borrow_mut(), root, &css);
    work.measure("tree_reconciliation", || {
        tree.apply(&mut document.inner.borrow_mut(), &rendered);
    });
    Ok(EditorSkinPreview {
        canvas,
        package,
        runtime,
        tree,
        next_render: skin_deadline(&rendered.schedule, false),
        last_input: input,
        last_state: state.clone(),
    })
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn reconcile_editor_skin_previews(
    document: &mut DioxusDocument,
    previews: &mut std::collections::BTreeMap<String, EditorSkinPreview>,
    presentations: &[scorepeek_overlay_ui::CanvasPresentation],
    skin_assets: &Arc<SkinAssetCache>,
    report: &Rc<RefCell<RunReport>>,
    output: Option<&str>,
    state: &OverlayState,
    runtime_create_count: &mut u64,
    work: &mut FrameWorkProfile,
) -> Result<EditorSkinReconciliation, String> {
    let reconcile_started = Instant::now();
    let output = output.ok_or("editor skin preview requires an output owner")?;
    let mut reconciliation = EditorSkinReconciliation::default();
    let live = presentations
        .iter()
        .map(|presentation| presentation.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let removed = previews
        .keys()
        .filter(|id| !live.contains(id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    for id in removed {
        if let Some(mut preview) = previews.remove(&id) {
            preview.tree.unmount(&mut document.inner.borrow_mut());
            skin_assets.release_editor_owner(&id, output);
        }
    }

    let mut retry_owner = false;
    for presentation in presentations {
        if !previews.contains_key(&presentation.id) {
            if !skin_assets.acquire_editor_owner(&presentation.id, output) {
                retry_owner = true;
                continue;
            }
            let preview = match create_editor_skin_preview(
                document,
                presentation,
                skin_assets,
                report,
                Some(output),
                state,
                work,
            ) {
                Ok(preview) => preview,
                Err(error) => {
                    skin_assets.release_editor_owner(&presentation.id, output);
                    return Err(error);
                }
            };
            previews.insert(presentation.id.clone(), preview);
            *runtime_create_count = runtime_create_count.saturating_add(1);
            reconciliation.wasm_calls = reconciliation.wasm_calls.saturating_add(1);
            reconciliation.tree_updates = reconciliation.tree_updates.saturating_add(1);
            reconciliation.input_generations = reconciliation.input_generations.saturating_add(1);
            continue;
        }
        let preview = previews
            .get_mut(&presentation.id)
            .expect("checked editor preview");
        let presentation_changed = preview.canvas.presentation() != *presentation;
        if presentation_changed {
            work.measure("canvas_config", || {
                preview.canvas.apply_presentation(presentation);
            });
        }
        let desired_skin = preview.canvas.skin.name();
        if preview.package.manifest.id != desired_skin {
            let next_package = skin_assets.load_profiled(desired_skin, work)?;
            let mut next_runtime = work.measure("wasm_runtime_create", || {
                new_native_skin_runtime(
                    &next_package,
                    report,
                    &preview.canvas.id,
                    Some(output),
                    &[],
                )
            })?;
            let next_input = work.measure("skin_input", || {
                native_skin_input(&preview.canvas, state, &next_package.manifest)
            });
            let (mut rendered, timing) = next_runtime.init_measured(&next_input)?;
            work.record("wasm_render", timing.wasm);
            work.record("json_tree", timing.json_tree);
            namespace_native_skin_output(&next_package.manifest.id, &mut rendered);
            let css = namespace_skin_css(
                desired_skin,
                std::str::from_utf8(
                    next_package
                        .resource(crate::skin::STYLE_PATH)
                        .ok_or("skin.css missing")?,
                )
                .map_err(|error| format!("skin.css is not UTF-8: {error}"))?,
            );
            work.measure("tree_reconciliation", || {
                preview
                    .tree
                    .replace(&mut document.inner.borrow_mut(), &css, &rendered);
            });
            preview.package = next_package;
            preview.runtime = next_runtime;
            preview.next_render = skin_deadline(&rendered.schedule, false);
            preview.last_input = next_input;
            preview.last_state = state.clone();
            *runtime_create_count = runtime_create_count.saturating_add(1);
            reconciliation.wasm_calls = reconciliation.wasm_calls.saturating_add(1);
            reconciliation.tree_updates = reconciliation.tree_updates.saturating_add(1);
            reconciliation.input_generations = reconciliation.input_generations.saturating_add(1);
            continue;
        }
        let due = preview
            .next_render
            .is_some_and(|deadline| Instant::now() >= deadline);
        if !due && !presentation_changed && preview.last_state == *state {
            continue;
        }
        let input = work.measure("skin_input", || {
            native_skin_input(&preview.canvas, state, &preview.package.manifest)
        });
        reconciliation.input_generations = reconciliation.input_generations.saturating_add(1);
        if !due && input == preview.last_input {
            preview.last_state = state.clone();
            continue;
        }
        let (mut rendered, timing) = preview.runtime.render_measured(&input)?;
        work.record("wasm_render", timing.wasm);
        work.record("json_tree", timing.json_tree);
        namespace_native_skin_output(&preview.package.manifest.id, &mut rendered);
        work.measure("tree_reconciliation", || {
            preview
                .tree
                .apply(&mut document.inner.borrow_mut(), &rendered);
        });
        preview.next_render = skin_deadline(&rendered.schedule, false);
        preview.last_input = input;
        preview.last_state = state.clone();
        reconciliation.wasm_calls = reconciliation.wasm_calls.saturating_add(1);
        reconciliation.tree_updates = reconciliation.tree_updates.saturating_add(1);
    }
    reconciliation.retry_owner = retry_owner;
    work.record("skin_reconciliation", reconcile_started.elapsed());
    Ok(reconciliation)
}

impl NativeEventConsumer for App {
    fn configure_event(
        &mut self,
        logical: [u32; 2],
        physical: [u32; 2],
        scale_120: u32,
    ) -> Result<bool, String> {
        let changed = self.configure(logical, physical, scale_120)?;
        if self.editing()
            && let Some(output) = self.surface_output.clone()
        {
            let _ = self.coordinator.send(CoordinatorCommand::EditorInput {
                input: EditorInput::Resize {
                    output,
                    logical_size: logical,
                },
                correlation: None,
            });
        }
        Ok(changed)
    }

    fn pointer_motion_event(&mut self, point: [f64; 2]) {
        self.pointer
            .dispatch(&mut self.document, point, 0x110, None);
    }

    fn pointer_button_event(&mut self, button: u32, pressed: bool, point: [f64; 2]) {
        self.pointer_button(button, pressed, point[0], point[1]);
    }

    fn pointer_scroll_event(&mut self, delta: [f64; 2], point: [f64; 2]) {
        self.pointer.wheel(&mut self.document, point, delta);
    }

    fn text_event(&mut self, command: &scorepeek_overlay_handles::TextCommand) {
        self.input_command(command);
    }

    fn ime_event(&mut self, update: scorepeek_overlay_handles::TextUpdate) {
        self.input_composition(update);
    }

    fn keyboard_focus_event(&mut self, focused: bool) {
        if !focused {
            self.shell.set_text_input(None);
            self.set_text_composing(false);
        }
    }
}

impl App {
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn new(
        _appearance: Appearance,
        _widgets: Vec<WidgetLayout>,
        renderer: VelloWindowRenderer,
        mut shell: Shell,
        waker: Waker,
        feed_state: Arc<std::sync::Mutex<OverlayState>>,
        feed_stop: Arc<std::sync::atomic::AtomicBool>,
        external_stop: Arc<std::sync::atomic::AtomicBool>,
        canvas: crate::config::Canvas,
        skin_assets: Arc<SkinAssetCache>,
        outputs: Vec<OutputDescription>,
        report: Rc<RefCell<RunReport>>,
        pending_resolved_output: Option<String>,
        published_stages: Arc<std::sync::Mutex<PublishedStages>>,
        wayland_refresh_hz: Arc<std::sync::Mutex<scorepeek_overlay_ui::WaylandRefreshRate>>,
        startup_started: Instant,
        coordinator: std::sync::mpsc::Sender<CoordinatorCommand>,
        role: SurfaceRole,
    ) -> Result<Self, String> {
        let initial = match role {
            SurfaceRole::DisplayCanvas => NativeDocumentProjection::Display {
                canvas: canvas.presentation(),
                visible: canvas.show_on.is_none(),
            },
            SurfaceRole::EditorStage => published_stages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .by_output
                .get(
                    shell
                        .output_name
                        .as_deref()
                        .unwrap_or(canvas.output.as_str()),
                )
                .cloned()
                .map(NativeDocumentProjection::Editor)
                .ok_or_else(|| {
                    format!(
                        "editor projection missing for output {}",
                        shell
                            .output_name
                            .as_deref()
                            .unwrap_or(canvas.output.as_str())
                    )
                })?,
        };
        let published = Rc::new(RefCell::new(None));
        let port = NativeEditorPort {
            coordinator: coordinator.clone(),
            source_output: shell.output_name.clone(),
            run_id: report.borrow().run_id.clone(),
            sequence: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let display_package = if role == SurfaceRole::DisplayCanvas {
            Some(skin_assets.load(canvas.skin.name())?)
        } else {
            None
        };
        let vdom = VirtualDom::new_with_props(
            native_overlay,
            NativeOverlayProps {
                initial,
                published: Rc::clone(&published),
                port,
            },
        );
        let (document_config, _) = document_config_inner_with_handle(Arc::clone(&skin_assets));
        let mut document = DioxusDocument::new(vdom, document_config);
        document.initial_build();
        let projection = published
            .borrow()
            .as_ref()
            .copied()
            .expect("native overlay publishes its projection during initial build");
        let current_state = OverlayState::default();
        let (display_skin, display_next_render) = if let Some(package) = display_package {
            let (skin, next) = create_native_display_skin(
                &mut document,
                &canvas,
                &package,
                &report,
                shell.output_name.as_deref(),
                &current_state,
            )?;
            (Some(skin), next)
        } else {
            (None, None)
        };
        let visible = match &*projection.borrow() {
            NativeDocumentProjection::Display { visible, .. } => *visible,
            NativeDocumentProjection::Editor(_) => true,
        };
        let interactive = matches!(&*projection.borrow(), NativeDocumentProjection::Editor(stage) if stage.interactive);
        let editor_presentations = match &*projection.borrow() {
            NativeDocumentProjection::Editor(projection) => projection.canvases.clone(),
            NativeDocumentProjection::Display { .. } => Vec::new(),
        };
        let mut editor_skin_previews =
            std::collections::BTreeMap::<String, EditorSkinPreview>::new();
        let mut editor_skin_updates = EditorSkinUpdates::default();
        let mut frame_work = FrameWorkProfile::default();
        for presentation in &editor_presentations {
            let Some(output) = shell.output_name.as_deref() else {
                return Err("editor skin preview requires an output owner".into());
            };
            if !skin_assets.acquire_editor_owner(&presentation.id, output) {
                editor_skin_updates.request();
                continue;
            }
            let preview = match create_editor_skin_preview(
                &mut document,
                presentation,
                &skin_assets,
                &report,
                shell.output_name.as_deref(),
                &current_state,
                &mut frame_work,
            ) {
                Ok(preview) => preview,
                Err(error) => {
                    skin_assets.release_editor_owner(&presentation.id, output);
                    for (id, mut mounted) in editor_skin_previews {
                        mounted.tree.unmount(&mut document.inner.borrow_mut());
                        skin_assets.release_editor_owner(&id, output);
                    }
                    return Err(error);
                }
            };
            editor_skin_previews.insert(presentation.id.clone(), preview);
        }
        let editor_next_render = editor_skin_previews
            .values()
            .filter_map(|preview| preview.next_render)
            .min();
        let editor_preview_count = u64::try_from(editor_skin_previews.len()).unwrap_or(u64::MAX);
        shell.set_input_enabled(surface_input_enabled(
            role == SurfaceRole::EditorStage,
            interactive,
            visible,
        ));
        shell.set_keyboard_enabled(role == SurfaceRole::EditorStage && interactive);
        let surface_logical = [canvas.width, canvas.height];
        let surface_output = shell.output_name.clone();
        Ok(Self {
            renderer,
            shell,
            document,
            pointer: PointerInput::default(),
            projection,
            published_stages,
            waker,
            started: Instant::now(),
            animating: false,
            paint_count: 0,
            render_calls: 0,
            steady_paint_count: 0,
            pending_paint: false,
            full_layout_pending: false,
            cadence: FrameCadence::default(),
            editor_skin_updates,
            report,
            feed_state,
            current_state,
            feed_stop,
            external_stop,
            surface_canvas: canvas.clone(),
            surface_output,
            canvas,
            role,
            coordinator,
            output_descriptions: outputs,
            pending_resolved_output,
            next_output_persist: Instant::now() + Duration::from_secs(1),
            surface_logical,
            wayland_refresh_hz,
            display_skin,
            editor_skin_previews,
            next_skin_render: if role == SurfaceRole::EditorStage {
                editor_next_render
            } else {
                display_next_render
            },
            skin_assets,
            skin_runtime_create_count: if role == SurfaceRole::DisplayCanvas {
                1
            } else {
                editor_preview_count
            },
            frame_work,
            pending_frame_start: None,
            startup_started,
            text_composition: TextComposition::Idle,
        })
    }

    fn editing(&self) -> bool {
        matches!(
            &*self.projection.borrow(),
            NativeDocumentProjection::Editor(_)
        )
    }

    fn unmount_skin_resources(&mut self) {
        for (id, preview) in &mut self.editor_skin_previews {
            preview.tree.unmount(&mut self.document.inner.borrow_mut());
            if let Some(output) = self.surface_output.as_deref() {
                self.skin_assets.release_editor_owner(id, output);
            }
        }
        self.editor_skin_previews.clear();
        if let Some(mut display) = self.display_skin.take() {
            display.tree.unmount(&mut self.document.inner.borrow_mut());
        }
        self.next_skin_render = None;
        self.animating = false;
        crate::diagnostics::emit(
            "native_skin_resources_unmounted",
            &serde_json::json!({
                "run_id": self.report.borrow().run_id,
                "canvas_id": self.surface_canvas.id,
                "output": self.surface_output,
            }),
        );
    }

    fn visible(&self) -> bool {
        match &*self.projection.borrow() {
            NativeDocumentProjection::Display { visible, .. } => *visible,
            NativeDocumentProjection::Editor(_) => true,
        }
    }

    fn sync_projection(&mut self) -> bool {
        if self.role == SurfaceRole::DisplayCanvas {
            return false;
        }
        let Some(output) = self.surface_output.as_ref() else {
            return false;
        };
        let rebuild_started = Instant::now();
        let candidate = {
            let stages = self
                .published_stages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(candidate) = stages.by_output.get(output) else {
                return false;
            };
            let current = self.projection.borrow();
            let NativeDocumentProjection::Editor(current) = &*current else {
                return false;
            };
            if !accepts_stage_projection(current, candidate) || current == candidate {
                return false;
            }
            candidate.clone()
        };
        self.frame_work
            .record("projection_rebuild", rebuild_started.elapsed());
        let previous = self.canvas.presentation();
        let previews_changed = editor_skin_previews_changed(&self.editor_skin_previews, &candidate);
        if !accept_stage_projection_replica(
            self.projection,
            &mut self.document,
            &mut self.editor_skin_previews,
            &self.skin_assets,
            output,
            &candidate,
        ) {
            return false;
        }
        if let Some(canvas) = candidate.selected_canvas.as_ref() {
            self.canvas =
                crate::config::empty_canvas(canvas.id.clone(), crate::runtime::Backend::Wayland);
            self.canvas.apply_presentation(canvas);
        }
        if previews_changed
            || candidate
                .selected_canvas
                .as_ref()
                .is_some_and(|canvas| editor_skin_presentation_changed(&previous, canvas))
        {
            self.editor_skin_updates.request();
        }
        let interactive = candidate.interactive;
        self.shell.set_input_enabled(interactive);
        self.shell.set_keyboard_enabled(interactive);
        crate::diagnostics::emit(
            "native_editor_projection_received",
            &serde_json::json!({
                "run_id": self.report.borrow().run_id,
                "output": self.surface_output,
                "session_id": match &*self.projection.borrow() { NativeDocumentProjection::Editor(stage) => Some(stage.session_id), NativeDocumentProjection::Display { .. } => None },
                "revision": match &*self.projection.borrow() { NativeDocumentProjection::Editor(stage) => Some(stage.revision), NativeDocumentProjection::Display { .. } => None },
                "canvases": candidate.canvases.iter().map(|canvas| serde_json::json!({
                    "id": canvas.id,
                    "output": canvas.output,
                    "widget_count": canvas.widgets.len(),
                })).collect::<Vec<_>>(),
            }),
        );
        true
    }

    #[allow(clippy::too_many_lines)]
    fn run(&mut self) -> Result<(), String> {
        let _ = self.poll_dioxus();
        while !self.feed_stop.load(std::sync::atomic::Ordering::Acquire)
            && !self
                .external_stop
                .load(std::sync::atomic::Ordering::Acquire)
        {
            let frame_start = self
                .pending_frame_start
                .clone()
                .unwrap_or_else(|| self.frame_work.snapshot());
            let projection_changed = self.sync_projection();
            self.retry_resolved_output();
            let events = match self.shell.dispatch(Duration::from_millis(500)) {
                Ok(events) => events,
                Err(_)
                    if self
                        .external_stop
                        .load(std::sync::atomic::Ordering::Acquire) =>
                {
                    return Ok(());
                }
                Err(error) if error == "output_removed" => return Ok(()),
                Err(error) => return Err(error),
            };
            let mut frame = false;
            let mut configured = false;
            let mut input_damage = false;
            if self.output_descriptions != self.shell.output_descriptions {
                self.output_descriptions
                    .clone_from(&self.shell.output_descriptions);
                let outputs = editor_outputs_from_descriptions(&self.output_descriptions);
                let _ = self.coordinator.send(CoordinatorCommand::EditorInput {
                    input: EditorInput::SetOutputs(outputs),
                    correlation: None,
                });
            }
            for event in events {
                let outcome = dispatch_native_event(self, event)?;
                if outcome.closed {
                    return Ok(());
                }
                configured |= outcome.configured;
                input_damage |= outcome.input_damage;
                frame |= outcome.frame;
            }
            let latest = self
                .feed_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let mut visibility_changed = false;
            if let NativeDocumentProjection::Display { canvas, visible } =
                &*self.projection.borrow()
            {
                let next_visible =
                    scorepeek_overlay_ui::canvas_visible(canvas.show_on.as_deref(), latest.screen);
                if *visible != next_visible {
                    let canvas = canvas.clone();
                    self.projection.set(NativeDocumentProjection::Display {
                        canvas,
                        visible: next_visible,
                    });
                    self.shell.set_input_enabled(next_visible);
                    visibility_changed = true;
                }
            }
            let visible = self.visible();
            if self.current_state != latest {
                self.current_state = latest.clone();
                if visible {
                    if self.editing() {
                        self.editor_skin_updates.request();
                    } else {
                        self.render_skin(&latest)?;
                    }
                }
            } else if visibility_changed && visible && !self.editing() {
                self.render_skin(&latest)?;
            }
            if visible
                && self
                    .next_skin_render
                    .is_some_and(|deadline| Instant::now() >= deadline)
            {
                if self.editing() {
                    self.editor_skin_updates.request();
                } else {
                    self.render_skin(&latest)?;
                }
            }
            let now = self.started.elapsed();
            let refresh = *self
                .wayland_refresh_hz
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.editing() {
                let first_paint = self.paint_count == 0;
                let paint_started = Instant::now();
                let editor_state =
                    if self.current_state.system == scorepeek_overlay_ui::LampState::Inactive {
                        scorepeek_overlay_ui::editor_sample_state()
                    } else {
                        self.current_state.clone()
                    };
                let mut presenter = WindowPresenter {
                    shell: &mut self.shell,
                    renderer: &mut self.renderer,
                };
                let result = run_native_editor_stage_turn(
                    &mut self.document,
                    self.projection,
                    &mut self.editor_skin_previews,
                    &self.skin_assets,
                    &self.report,
                    self.surface_output.as_deref().unwrap_or_default(),
                    &editor_state,
                    &mut self.editor_skin_updates,
                    &mut self.skin_runtime_create_count,
                    &mut self.next_skin_render,
                    &mut self.animating,
                    &mut self.full_layout_pending,
                    &mut self.pending_paint,
                    &mut self.cadence,
                    &mut self.frame_work,
                    &self.waker,
                    &frame_start,
                    NativeEditorStageTurnInput {
                        frame: if frame {
                            NativeFrameBoundary::Frame
                        } else {
                            NativeFrameBoundary::Deferred
                        },
                        surface: if configured {
                            NativeSurfaceReadiness::Configured
                        } else {
                            NativeSurfaceReadiness::Pending
                        },
                        projection_changed,
                        input_damage,
                        now,
                        seconds: now.as_secs_f64(),
                        refresh,
                    },
                    &mut presenter,
                )?;
                if !result.text_input_active {
                    self.set_text_composing(false);
                }
                if result.dioxus_changed {
                    let projection = self.projection.borrow();
                    let (session_id, revision) = match &*projection {
                        NativeDocumentProjection::Editor(stage) => {
                            (Some(stage.session_id), Some(stage.revision))
                        }
                        NativeDocumentProjection::Display { .. } => (None, None),
                    };
                    crate::diagnostics::emit(
                        "native_editor_dioxus_rebuilt",
                        &serde_json::json!({"run_id":self.report.borrow().run_id,"output":self.surface_output,"session_id":session_id,"revision":revision}),
                    );
                }
                if let Some(reason) = result.paint {
                    self.paint_count = self.paint_count.saturating_add(1);
                    self.render_calls = self.render_calls.saturating_add(1);
                    if reason == PaintReason::Steady {
                        self.steady_paint_count = self.steady_paint_count.saturating_add(1);
                    } else if reason.bypasses_cap() {
                        *self
                            .report
                            .borrow_mut()
                            .cap_bypass_paints
                            .entry(reason.name().to_owned())
                            .or_default() += 1;
                    }
                    if first_paint {
                        self.report
                            .borrow_mut()
                            .operations
                            .push("dioxus_blitz_initial_paint");
                        crate::diagnostics::emit(
                            "native_startup_timing",
                            &serde_json::json!({"run_id":self.report.borrow().run_id,"canvas_id":self.surface_canvas.id,"phase":"first_paint","paint_us":duration_us(paint_started.elapsed()),"elapsed_us":duration_us(self.startup_started.elapsed()),"status":"success"}),
                        );
                    }
                    let projection = self.projection.borrow();
                    let (session_id, revision) = match &*projection {
                        NativeDocumentProjection::Editor(stage) => {
                            (stage.session_id, stage.revision)
                        }
                        NativeDocumentProjection::Display { .. } => (0, 0),
                    };
                    crate::diagnostics::emit(
                        "native_editor_painted",
                        &serde_json::json!({"run_id":self.report.borrow().run_id,"output":self.surface_output,"session_id":session_id,"revision":revision,"paint_us":duration_us(paint_started.elapsed())}),
                    );
                }
            } else {
                let first_paint = self.paint_count == 0;
                let paint_started = Instant::now();
                let mut presenter = WindowPresenter {
                    shell: &mut self.shell,
                    renderer: &mut self.renderer,
                };
                let result = run_native_display_turn(
                    &mut self.document,
                    &self.skin_assets,
                    &mut self.full_layout_pending,
                    &mut self.pending_paint,
                    &mut self.animating,
                    &mut self.cadence,
                    &mut self.frame_work,
                    &self.waker,
                    &frame_start,
                    NativeDisplayTurnInput {
                        frame: if frame {
                            NativeFrameBoundary::Frame
                        } else {
                            NativeFrameBoundary::Deferred
                        },
                        surface: if configured {
                            NativeSurfaceReadiness::Configured
                        } else {
                            NativeSurfaceReadiness::Pending
                        },
                        projection_changed,
                        visibility_changed,
                        input_damage,
                        visible,
                        live_widgets: u64::try_from(self.canvas.widgets.len()).unwrap_or(u64::MAX),
                        now,
                        seconds: now.as_secs_f64(),
                        refresh,
                    },
                    &mut presenter,
                )?;
                if result.dioxus_changed {
                    self.set_text_composing(false);
                }
                if let Some(reason) = result.paint {
                    self.paint_count = self.paint_count.saturating_add(1);
                    self.render_calls = self.render_calls.saturating_add(1);
                    if reason == PaintReason::Steady {
                        self.steady_paint_count = self.steady_paint_count.saturating_add(1);
                    } else if reason.bypasses_cap() {
                        *self
                            .report
                            .borrow_mut()
                            .cap_bypass_paints
                            .entry(reason.name().to_owned())
                            .or_default() += 1;
                    }
                    if first_paint {
                        self.report
                            .borrow_mut()
                            .operations
                            .push("dioxus_blitz_initial_paint");
                        crate::diagnostics::emit(
                            "native_startup_timing",
                            &serde_json::json!({"run_id":self.report.borrow().run_id,"canvas_id":self.surface_canvas.id,"phase":"first_paint","paint_us":duration_us(paint_started.elapsed()),"elapsed_us":duration_us(self.startup_started.elapsed()),"status":"success"}),
                        );
                    }
                }
            }
            if frame {
                self.pending_frame_start = None;
            } else if self.pending_paint && self.pending_frame_start.is_none() {
                self.pending_frame_start = Some(frame_start);
            }
        }
        Ok(())
    }

    fn retry_resolved_output(&mut self) {
        if Instant::now() < self.next_output_persist {
            return;
        }
        if self.pending_resolved_output.is_none() {
            return;
        }
        self.next_output_persist = Instant::now() + Duration::from_secs(1);
        let _ = self.coordinator.send(CoordinatorCommand::ResolveOutput {
            output_names: self
                .output_descriptions
                .iter()
                .map(|item| item.name.clone())
                .collect(),
            output: self.surface_output.clone(),
            canvas: self.canvas.id.clone(),
            preview_screen: self.current_state.screen.kind,
        });
    }

    fn pointer_button(&mut self, button: u32, pressed: bool, x: f64, y: f64) {
        if button != 0x110 && button != 0x111 {
            return;
        }
        self.pointer
            .dispatch(&mut self.document, [x, y], button, Some(pressed));
    }

    fn poll_dioxus(&mut self) -> bool {
        let changed = poll_native_document_for_frame(
            &mut self.document,
            &self.waker,
            &mut self.full_layout_pending,
            &mut self.frame_work,
        );
        if changed && self.editing() {
            let (session_id, revision) = match &*self.projection.borrow() {
                NativeDocumentProjection::Editor(stage) => {
                    (Some(stage.session_id), Some(stage.revision))
                }
                NativeDocumentProjection::Display { .. } => (None, None),
            };
            crate::diagnostics::emit(
                "native_editor_dioxus_rebuilt",
                &serde_json::json!({"run_id":self.report.borrow().run_id,"output":self.surface_output,"session_id":session_id,"revision":revision}),
            );
        }
        changed
    }

    fn render_skin(&mut self, state: &OverlayState) -> Result<(), String> {
        let started = Instant::now();
        let display = self
            .display_skin
            .as_mut()
            .ok_or("display skin runtime missing outside display role")?;
        let mut output =
            display
                .runtime
                .render(&native_skin_input(&self.canvas, state, &display.manifest))?;
        namespace_native_skin_output(&display.manifest.id, &mut output);
        display
            .tree
            .apply(&mut self.document.inner.borrow_mut(), &output);
        self.next_skin_render = skin_deadline(&output.schedule, false);
        self.animating = matches!(output.schedule, crate::skin::Schedule::NextFrame);
        crate::diagnostics::emit(
            "skin_render",
            &serde_json::json!({"skin_id":self.canvas.skin.name(),"release":display.release,"canvas_id":self.canvas.id,"backend":"native","phase":"render","status":"success","duration_us":duration_us(started.elapsed()),"next_tick":format!("{:?}",output.schedule),"tree_applied":true}),
        );
        Ok(())
    }

    fn configure(
        &mut self,
        logical: [u32; 2],
        physical: [u32; 2],
        scale_120: u32,
    ) -> Result<bool, String> {
        self.surface_logical = logical;
        let [width, height] = physical;
        let geometry_changed = {
            let mut report = self.report.borrow_mut();
            let changed = report.logical_size != Some(logical)
                || report.physical_size != Some(physical)
                || report.scale_120 != scale_120;
            report.logical_size = Some(logical);
            report.physical_size = Some(physical);
            report.scale_120 = scale_120;
            changed
        };
        self.document.inner.borrow_mut().set_viewport(Viewport::new(
            width,
            height,
            f32::from(u16::try_from(scale_120).map_err(|error| error.to_string())?) / 120.0,
            ColorScheme::Dark,
        ));
        if self.renderer.is_active() {
            if geometry_changed {
                self.renderer.set_size(width, height);
            }
        } else {
            self.renderer
                .resume(self.shell.handles(), width, height, || {});
            if !self.renderer.complete_resume() {
                return Err("gpu_adapter".to_owned());
            }
            let info = self
                .renderer
                .current_device_handle()
                .map(|device| device.adapter.get_info())
                .ok_or_else(|| "gpu_adapter".to_owned())?;
            let mut report = self.report.borrow_mut();
            report.gpu_backend = Some(format!("{:?}", info.backend));
            report.gpu_adapter = Some(info.name);
            if report.gpu_backend.as_deref() != Some("Vulkan") {
                return Err("GPU backend is not Vulkan".into());
            }
            report.operations.push("vulkan_surface_configured");
            report.operations.push("system_fonts_enabled");
        }
        crate::diagnostics::emit("surface_configured", &*self.report.borrow());
        Ok(geometry_changed)
    }
}

const fn surface_input_enabled(editing: bool, interactive: bool, visible: bool) -> bool {
    visible && (!editing || interactive)
}
fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn canvas_worker_error_type(error: &str) -> &'static str {
    if error.starts_with("unmap Wayland surface:") {
        "wayland_surface_unmap_failed"
    } else {
        "canvas_worker_failed"
    }
}

fn native_shutdown_result(
    run: Result<(), String>,
    unmap: Result<(), String>,
) -> (Result<(), String>, Option<String>) {
    match unmap {
        Ok(()) => (run, None),
        Err(error) => (Err(format!("unmap Wayland surface: {error}")), run.err()),
    }
}

fn shutdown_native_surface<T, E>(
    target: &mut T,
    unmount: impl FnOnce(&mut T),
    suspend: impl FnOnce(&mut T),
    unmap: impl FnOnce(&mut T) -> Result<(), E>,
) -> Result<(), E> {
    unmount(target);
    suspend(target);
    unmap(target)
}

#[derive(Serialize)]
struct RunReport {
    run_id: String,
    build_revision: &'static str,
    output_name: Option<String>,
    logical_size: Option<[u32; 2]>,
    physical_size: Option<[u32; 2]>,
    scale_120: u32,
    gpu_backend: Option<String>,
    gpu_adapter: Option<String>,
    paint_count: u32,
    render_calls: u32,
    editor_skin_update_requests: u64,
    editor_skin_render_count: u64,
    skin_package_open_count: u64,
    skin_package_open_ns: u64,
    skin_package_clone_count: u64,
    skin_package_clone_ns: u64,
    resource_lookup_count: u64,
    resource_lookup_ns: u64,
    skin_runtime_create_count: u64,
    frame_work: FrameWorkProfile,
    wayland_refresh_hz: scorepeek_overlay_ui::WaylandRefreshRate,
    elapsed_ms: u64,
    effective_paint_hz: Option<f64>,
    effective_steady_paint_hz: Option<f64>,
    cap_bypass_paints: std::collections::BTreeMap<String, u64>,
    operations: Operations,
    status: &'static str,
    failure_type: Option<&'static str>,
    failure: Option<String>,
    secondary_failure_type: Option<&'static str>,
    secondary_failure: Option<String>,
}

impl RunReport {
    fn new() -> Self {
        static RUN_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        Self {
            run_id: format!(
                "{timestamp}-{}-{}",
                std::process::id(),
                RUN_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ),
            build_revision: option_env!("OVERLAY_BUILD_REVISION").unwrap_or("working-tree"),
            output_name: None,
            logical_size: None,
            physical_size: None,
            scale_120: 120,
            gpu_backend: None,
            gpu_adapter: None,
            paint_count: 0,
            render_calls: 0,
            editor_skin_update_requests: 0,
            editor_skin_render_count: 0,
            skin_package_open_count: 0,
            skin_package_open_ns: 0,
            skin_package_clone_count: 0,
            skin_package_clone_ns: 0,
            resource_lookup_count: 0,
            resource_lookup_ns: 0,
            skin_runtime_create_count: 0,
            frame_work: FrameWorkProfile::default(),
            wayland_refresh_hz: scorepeek_overlay_ui::WaylandRefreshRate::Auto,
            elapsed_ms: 0,
            effective_paint_hz: None,
            effective_steady_paint_hz: None,
            cap_bypass_paints: std::collections::BTreeMap::new(),
            operations: Operations::default(),
            status: "running",
            failure_type: None,
            failure: None,
            secondary_failure_type: None,
            secondary_failure: None,
        }
    }
}

#[derive(Default, Serialize)]
struct Operations(Vec<&'static str>);
impl Operations {
    fn push(&mut self, name: &'static str) {
        if self.0.len() < 128 {
            self.0.push(name);
        }
    }
}

struct SkinAssetCache {
    store: crate::skin::StoreRoot,
    packages: std::sync::Mutex<std::collections::BTreeMap<String, Arc<crate::skin::Package>>>,
    editor_owners: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
    open_count: std::sync::atomic::AtomicU64,
    open_ns: std::sync::atomic::AtomicU64,
    clone_count: std::sync::atomic::AtomicU64,
    clone_ns: std::sync::atomic::AtomicU64,
    resource_lookup_count: std::sync::atomic::AtomicU64,
    resource_lookup_ns: std::sync::atomic::AtomicU64,
}

impl SkinAssetCache {
    fn new(store: crate::skin::StoreRoot) -> Self {
        Self {
            store,
            packages: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            editor_owners: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            open_count: std::sync::atomic::AtomicU64::new(0),
            open_ns: std::sync::atomic::AtomicU64::new(0),
            clone_count: std::sync::atomic::AtomicU64::new(0),
            clone_ns: std::sync::atomic::AtomicU64::new(0),
            resource_lookup_count: std::sync::atomic::AtomicU64::new(0),
            resource_lookup_ns: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn with_package(store: crate::skin::StoreRoot, package: Arc<crate::skin::Package>) -> Self {
        let id = package.manifest.id.clone();
        Self {
            store,
            packages: std::sync::Mutex::new(std::collections::BTreeMap::from([(id, package)])),
            editor_owners: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            open_count: std::sync::atomic::AtomicU64::new(1),
            open_ns: std::sync::atomic::AtomicU64::new(0),
            clone_count: std::sync::atomic::AtomicU64::new(0),
            clone_ns: std::sync::atomic::AtomicU64::new(0),
            resource_lookup_count: std::sync::atomic::AtomicU64::new(0),
            resource_lookup_ns: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn acquire_editor_owner(&self, canvas: &str, output: &str) -> bool {
        let mut owners = self
            .editor_owners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(owner) = owners.get(canvas) {
            owner == output
        } else {
            owners.insert(canvas.to_owned(), output.to_owned());
            true
        }
    }

    fn release_editor_owner(&self, canvas: &str, output: &str) {
        let mut owners = self
            .editor_owners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if owners.get(canvas).is_some_and(|owner| owner == output) {
            owners.remove(canvas);
        }
    }

    fn load(&self, id: &str) -> Result<Arc<crate::skin::Package>, String> {
        let clone_started = Instant::now();
        let mut packages = self
            .packages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(package) = packages.get(id).cloned() {
            self.clone_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.clone_ns.fetch_add(
                duration_ns(clone_started.elapsed()),
                std::sync::atomic::Ordering::Relaxed,
            );
            return Ok(package);
        }
        let open_started = Instant::now();
        let package = Arc::new(self.store.open(id)?);
        self.open_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.open_ns.fetch_add(
            duration_ns(open_started.elapsed()),
            std::sync::atomic::Ordering::Relaxed,
        );
        let clone_started = Instant::now();
        let package = Arc::clone(packages.entry(id.to_owned()).or_insert(package));
        self.clone_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.clone_ns.fetch_add(
            duration_ns(clone_started.elapsed()),
            std::sync::atomic::Ordering::Relaxed,
        );
        Ok(package)
    }

    fn load_profiled(
        &self,
        id: &str,
        work: &mut FrameWorkProfile,
    ) -> Result<Arc<crate::skin::Package>, String> {
        let open_before = WorkStat {
            calls: self.open_count.load(std::sync::atomic::Ordering::Relaxed),
            total_ns: self.open_ns.load(std::sync::atomic::Ordering::Relaxed),
        };
        let clone_before = WorkStat {
            calls: self.clone_count.load(std::sync::atomic::Ordering::Relaxed),
            total_ns: self.clone_ns.load(std::sync::atomic::Ordering::Relaxed),
        };
        let result = self.load(id);
        let open_after = WorkStat {
            calls: self.open_count.load(std::sync::atomic::Ordering::Relaxed),
            total_ns: self.open_ns.load(std::sync::atomic::Ordering::Relaxed),
        };
        let clone_after = WorkStat {
            calls: self.clone_count.load(std::sync::atomic::Ordering::Relaxed),
            total_ns: self.clone_ns.load(std::sync::atomic::Ordering::Relaxed),
        };
        work.record_stat(
            "package_open",
            WorkStat {
                calls: open_after.calls.saturating_sub(open_before.calls),
                total_ns: open_after.total_ns.saturating_sub(open_before.total_ns),
            },
        );
        work.record_stat(
            "package_clone",
            WorkStat {
                calls: clone_after.calls.saturating_sub(clone_before.calls),
                total_ns: clone_after.total_ns.saturating_sub(clone_before.total_ns),
            },
        );
        result
    }

    fn installed_editor_skins(
        &self,
    ) -> Result<Vec<scorepeek_overlay_ui::editor::EditorSkin>, String> {
        let entries = match std::fs::read_dir(self.store.path()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("list skin store: {error}")),
        };
        let mut packages = self
            .packages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in entries {
            let path = entry
                .map_err(|error| format!("list skin store entry: {error}"))?
                .path();
            if path.extension().is_none_or(|extension| extension != "zip") {
                continue;
            }
            let open_started = Instant::now();
            let package = Arc::new(crate::skin::Package::open(&path)?);
            let id = package.manifest.id.clone();
            if packages.insert(id.clone(), package).is_some() {
                return Err(format!("skin store contains duplicate package id {id}"));
            }
            self.open_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.open_ns.fetch_add(
                duration_ns(open_started.elapsed()),
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        packages
            .values()
            .map(|package| {
                let manifest = &package.manifest;
                Ok(scorepeek_overlay_ui::editor::EditorSkin {
                    id: manifest.id.parse()?,
                    name: manifest.name.clone(),
                    release: manifest.release.clone(),
                    preview: format!("/skin/{}/{}", manifest.id, crate::skin::PREVIEW_PATH),
                    preview_video: None,
                    canvas_properties: serde_json::from_value(
                        serde_json::to_value(&manifest.canvas_properties)
                            .map_err(|error| error.to_string())?,
                    )
                    .map_err(|error| error.to_string())?,
                    widget_properties: serde_json::from_value(
                        serde_json::to_value(&manifest.widget_properties)
                            .map_err(|error| error.to_string())?,
                    )
                    .map_err(|error| error.to_string())?,
                })
            })
            .collect()
    }
}

fn namespace_skin_css(id: &str, css: &str) -> String {
    let mut output = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(offset) = rest.find("url(") {
        let (before, value) = rest.split_at(offset + 4);
        output.push_str(before);
        let Some(end) = value.find(')') else {
            output.push_str(value);
            return output;
        };
        let (raw, after) = value.split_at(end);
        let trimmed = raw.trim();
        let quote = trimmed
            .as_bytes()
            .first()
            .copied()
            .filter(|byte| matches!(byte, b'\'' | b'"'));
        let path = quote.map_or(trimmed, |_| &trimmed[1..trimmed.len().saturating_sub(1)]);
        if path.starts_with('/') || path.starts_with('#') || has_uri_scheme(path) {
            output.push_str(raw);
        } else {
            let quote = quote.map_or('"', char::from);
            output.push(quote);
            output.push_str("/skin/");
            output.push_str(id);
            output.push('/');
            output.push_str(path);
            output.push(quote);
        }
        output.push(')');
        rest = &after[1..];
    }
    output.push_str(rest);
    output
}

fn namespace_skin_resource(id: &str, value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.starts_with('#')
        || has_uri_scheme(trimmed)
    {
        value.to_owned()
    } else {
        format!("/skin/{id}/{value}")
    }
}

fn has_uri_scheme(value: &str) -> bool {
    let Some((scheme, _)) = value.split_once(':') else {
        return false;
    };
    let mut characters = scheme.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && characters
            .all(|character| character.is_ascii_alphanumeric() || "+-.".contains(character))
}

fn namespace_native_skin_output(id: &str, output: &mut crate::skin::RenderOutput) {
    fn visit(id: &str, node: &mut crate::skin::Node) {
        let crate::skin::Node::Element {
            attributes,
            children,
            ..
        } = node
        else {
            return;
        };
        if let Some(style) = attributes.get_mut("style") {
            *style = namespace_skin_css(id, style);
        }
        for name in ["src", "poster"] {
            if let Some(value) = attributes.get_mut(name) {
                *value = namespace_skin_resource(id, value);
            }
        }
        for child in children {
            visit(id, child);
        }
    }
    visit(id, &mut output.tree);
}

struct EmbeddedSkinAssets {
    cache: Arc<SkinAssetCache>,
}

impl blitz_traits::net::NetProvider for EmbeddedSkinAssets {
    fn fetch(
        &self,
        _doc_id: usize,
        request: blitz_traits::net::Request,
        handler: Box<dyn blitz_traits::net::NetHandler>,
    ) {
        let started = Instant::now();
        let path = request.url.path().trim_start_matches('/');
        let bytes = if let Some(rest) = path.strip_prefix("skin/")
            && let Some((id, resource)) = rest.split_once('/')
            && crate::skin::validate_id(id).is_ok()
            && let Ok(package) = self.cache.load(id)
            && let Some(bytes) = package.resource(resource)
        {
            blitz_traits::net::Bytes::copy_from_slice(bytes)
        } else if let Some(svg) =
            scorepeek_overlay_ui::composition::aperture_asset(request.url.path())
        {
            blitz_traits::net::Bytes::from(svg)
        } else {
            blitz_traits::net::Bytes::from_static(
                scorepeek_overlay_ui::skin_asset(request.url.path()).unwrap_or_default(),
            )
        };
        self.cache
            .resource_lookup_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.cache.resource_lookup_ns.fetch_add(
            duration_ns(started.elapsed()),
            std::sync::atomic::Ordering::Relaxed,
        );
        handler.bytes(request.url.to_string(), bytes);
    }
}

/// Applies the same presentation tracks used by the browser, without changing domain state.
pub fn apply_motion(document: &mut blitz_dom::BaseDocument, seconds: f64) {
    static TRACKS: std::sync::LazyLock<Vec<scorepeek_overlay_ui::motion::Track>> =
        std::sync::LazyLock::new(|| {
            serde_json::from_str(scorepeek_overlay_ui::motion::SPEC)
                .expect("embedded motion specification")
        });
    for track in TRACKS.iter() {
        if let Ok(nodes) = document.query_selector_all(&track.selector) {
            let value = track.value(seconds);
            for node in nodes {
                document.set_style_property(node, &track.property, &value);
            }
        }
    }
}

fn paint_native_scene(
    scene: &mut impl PaintScene,
    document: &mut blitz_dom::BaseDocument,
    scale: f64,
    width: u32,
    height: u32,
) {
    paint_scene(scene, document, scale, width, height, 0, 0);
    retain_native_image_atlas(scene);
}

fn retain_native_image_atlas(scene: &mut impl PaintScene) {
    // Vello keeps image residency metadata separately from its persistent atlas.
    // A frame without image patches otherwise replaces the atlas with a 1x1 texture
    // without invalidating that metadata, so later raster images are not uploaded.
    static ATLAS_KEEPALIVE: std::sync::LazyLock<peniko::ImageBrush> =
        std::sync::LazyLock::new(|| {
            peniko::ImageBrush::new(peniko::ImageData {
                data: peniko::Blob::new(Arc::new(vec![0_u8; 4])),
                format: peniko::ImageFormat::Rgba8,
                alpha_type: peniko::ImageAlphaType::Alpha,
                width: 1,
                height: 1,
            })
        });
    scene.fill(
        peniko::Fill::NonZero,
        peniko::kurbo::Affine::IDENTITY,
        ATLAS_KEEPALIVE.as_ref(),
        None,
        &peniko::kurbo::Rect::new(0.0, 0.0, 1.0, 1.0),
    );
}

fn resolve_with_loaded_resources(document: &mut blitz_dom::BaseDocument, seconds: f64) {
    document.resolve(seconds);
    // Embedded images complete synchronously during resolve. Ingest their messages, then
    // resolve again so their layers are available to the current paint.
    document.handle_messages();
    document.resolve(seconds);
}

fn editor_text_input_state(
    document: &DioxusDocument,
    interactive: bool,
) -> Option<scorepeek_overlay_handles::TextInputState> {
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
        Some(scorepeek_overlay_handles::TextInputState {
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
enum NativeFrameBoundary {
    Deferred,
    Frame,
}

impl NativeFrameBoundary {
    const fn is_frame(self) -> bool {
        matches!(self, Self::Frame)
    }
}

#[derive(Clone, Copy)]
enum NativeSurfaceReadiness {
    Pending,
    Configured,
}

impl NativeSurfaceReadiness {
    const fn is_configured(self) -> bool {
        matches!(self, Self::Configured)
    }
}

#[derive(Clone, Copy)]
struct NativeEditorStageTurnInput {
    frame: NativeFrameBoundary,
    surface: NativeSurfaceReadiness,
    projection_changed: bool,
    input_damage: bool,
    now: Duration,
    seconds: f64,
    refresh: scorepeek_overlay_ui::WaylandRefreshRate,
}

#[derive(Default)]
#[cfg_attr(not(test), allow(dead_code))]
struct NativeEditorStageTurnResult {
    dioxus_changed: bool,
    reconciled: bool,
    reconciliation: EditorSkinReconciliation,
    paint: Option<PaintReason>,
    text_input_active: bool,
}

#[derive(Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
struct NativeDisplayTurnInput {
    frame: NativeFrameBoundary,
    surface: NativeSurfaceReadiness,
    projection_changed: bool,
    visibility_changed: bool,
    input_damage: bool,
    visible: bool,
    live_widgets: u64,
    now: Duration,
    seconds: f64,
    refresh: scorepeek_overlay_ui::WaylandRefreshRate,
}

#[derive(Default)]
struct NativeDisplayTurnResult {
    dioxus_changed: bool,
    paint: Option<PaintReason>,
}

#[allow(clippy::too_many_arguments)]
fn run_native_display_turn(
    document: &mut DioxusDocument,
    assets: &Arc<SkinAssetCache>,
    full_layout_pending: &mut bool,
    pending_paint: &mut bool,
    animating: &mut bool,
    cadence: &mut FrameCadence,
    work: &mut FrameWorkProfile,
    waker: &Waker,
    frame_start: &FrameWorkSample,
    input: NativeDisplayTurnInput,
    presenter: &mut impl NativeFramePresenter,
) -> Result<NativeDisplayTurnResult, String> {
    let frame = input.frame.is_frame();
    let dioxus_changed =
        frame && poll_native_document_for_frame(document, waker, full_layout_pending, work);
    presenter.set_text_input(None);
    *pending_paint |= dioxus_changed
        || input.projection_changed
        || input.visibility_changed
        || input.input_damage;
    let admission = admit_native_paint(
        presenter.is_active(),
        input.surface.is_configured(),
        input.visibility_changed,
        PaintState {
            editing: false,
            visible: input.visible,
            signal: PaintSignal::from_state(frame, *pending_paint),
            animating: *animating,
        },
        input.now,
        input.refresh,
        cadence,
    );
    let paint = match admission {
        PaintAdmission::Paint(reason) => {
            render_native_frame(
                &mut document.inner.borrow_mut(),
                input.seconds,
                input.visible,
                full_layout_pending,
                presenter,
                assets,
                work,
            )?;
            cadence.record(input.now);
            *pending_paint = false;
            *animating = input.visible;
            Some(reason)
        }
        PaintAdmission::RequestFrameCommit => {
            presenter.request_frame_commit();
            None
        }
        PaintAdmission::None => None,
    };
    if frame {
        work.finish_frame(
            frame_start,
            u64::from(input.visible),
            if input.visible { input.live_widgets } else { 0 },
        );
    }
    Ok(NativeDisplayTurnResult {
        dioxus_changed,
        paint,
    })
}

#[allow(clippy::too_many_arguments)]
fn run_native_editor_stage_turn(
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
    animating: &mut bool,
    full_layout_pending: &mut bool,
    pending_paint: &mut bool,
    cadence: &mut FrameCadence,
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
        if reconciliation.retry_owner {
            updates.request();
        }
        *next_skin_render = previews
            .values()
            .filter_map(|preview| preview.next_render)
            .min();
        *animating = previews
            .values()
            .any(|preview| preview.next_render.is_some());
        skin_changed = true;
    }
    let interactive = matches!(&*projection.borrow(), NativeDocumentProjection::Editor(editor_projection) if editor_projection.interactive);
    let text_input = editor_text_input_state(document, interactive);
    let text_input_active = text_input.is_some();
    presenter.set_text_input(text_input);
    *pending_paint |=
        dioxus_changed || input.projection_changed || input.input_damage || skin_changed;
    let paint_state = PaintState {
        editing: true,
        visible: true,
        signal: PaintSignal::from_state(frame, *pending_paint),
        animating: *animating,
    };
    let admission = admit_native_paint(
        presenter.is_active(),
        input.surface.is_configured(),
        false,
        paint_state,
        input.now,
        input.refresh,
        cadence,
    );
    let paint = match admission {
        PaintAdmission::Paint(reason) => {
            render_native_frame(
                &mut document.inner.borrow_mut(),
                input.seconds,
                true,
                full_layout_pending,
                presenter,
                assets,
                work,
            )?;
            cadence.record(input.now);
            *pending_paint = false;
            *animating = true;
            Some(reason)
        }
        PaintAdmission::RequestFrameCommit => {
            presenter.request_frame_commit();
            None
        }
        PaintAdmission::None => None,
    };
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
        paint,
        text_input_active,
    })
}

fn render_native_frame(
    document: &mut blitz_dom::BaseDocument,
    seconds: f64,
    visible: bool,
    full_layout_pending: &mut bool,
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
    if visible {
        work.measure("motion", || apply_motion(document, seconds));
    }
    let incremental_layout = document.incremental_layout();
    if *full_layout_pending {
        document.set_incremental_layout(false);
    }
    work.measure("blitz_layout", || document.resolve(seconds));
    work.measure("resource_decode", || document.handle_messages());
    work.measure("blitz_layout", || document.resolve(seconds));
    if *full_layout_pending {
        document.set_incremental_layout(incremental_layout);
        *full_layout_pending = false;
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

/// Registers embedded artwork and the Latin font, preserving Japanese system fallbacks.
#[must_use]
pub fn document_config() -> DocumentConfig {
    document_config_inner(None)
}

fn document_config_inner(package: Option<crate::skin::Package>) -> DocumentConfig {
    let store = crate::skin::StoreRoot::discover();
    let cache = match package {
        Some(package) => Arc::new(SkinAssetCache::with_package(store, Arc::new(package))),
        None => Arc::new(SkinAssetCache::new(store)),
    };
    document_config_inner_with_handle(cache).0
}

fn document_config_with_skin_handle(
    package: Arc<crate::skin::Package>,
) -> (DocumentConfig, Arc<SkinAssetCache>) {
    document_config_inner_with_handle(Arc::new(SkinAssetCache::with_package(
        crate::skin::StoreRoot::discover(),
        package,
    )))
}

fn document_config_inner_with_handle(
    cache: Arc<SkinAssetCache>,
) -> (DocumentConfig, Arc<SkinAssetCache>) {
    let mut font_ctx = blitz_dom::FontContext::default();
    font_ctx
        .collection
        .register_fonts(peniko::Blob::new(Arc::new(OXANIUM)), None);
    for (_, bytes) in scorepeek_overlay_ui::FONT_ASSETS {
        font_ctx
            .collection
            .register_fonts(peniko::Blob::new(Arc::new(*bytes)), None);
    }
    let config = DocumentConfig {
        font_ctx: Some(font_ctx),
        base_url: Some("http://scorepeek.invalid/".into()),
        net_provider: Some(Arc::new(EmbeddedSkinAssets {
            cache: Arc::clone(&cache),
        })),
        ..DocumentConfig::default()
    };
    (config, cache)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisualDebugScenario {
    #[serde(default)]
    pub canvases: Option<Vec<scorepeek_overlay_ui::CanvasPresentation>>,
    pub skin: Option<scorepeek_overlay_ui::Skin>,
    #[serde(default = "visual_debug_default_size")]
    pub logical_size: [u32; 2],
    #[serde(default = "visual_debug_default_scale")]
    pub scale: f32,
    pub canvas_id: Option<String>,
    #[serde(default = "visual_debug_default_editing")]
    pub editing: bool,
    #[serde(default)]
    pub selectors: Vec<String>,
    #[serde(default)]
    pub actions: Vec<VisualDebugAction>,
}

const fn visual_debug_default_size() -> [u32; 2] {
    [1920, 1080]
}

const fn visual_debug_default_scale() -> f32 {
    1.0
}

const fn visual_debug_default_editing() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum VisualDebugAction {
    TitleText {
        text: String,
        #[serde(default)]
        composing: bool,
    },
    Motion {
        seconds: f64,
    },
    SetEditing {
        value: bool,
    },
    SetScreen {
        screen: Option<scorepeek_overlay_ui::ScreenKind>,
    },
    Click {
        selector: String,
    },
    Scroll {
        selector: String,
        dx: f64,
        dy: f64,
    },
    Drag {
        from: [f64; 2],
        to: [f64; 2],
        button: VisualDebugButton,
    },
    Capture {
        name: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualDebugButton {
    Left,
    Right,
}

#[derive(Serialize)]
struct VisualDebugManifest {
    schema_version: u32,
    run_id: String,
    resource: VisualDebugResource,
    logical_size: [u32; 2],
    physical_size: Option<[u32; 2]>,
    scale: f32,
    status: &'static str,
    completeness: &'static str,
    operations: Vec<VisualDebugOperation>,
    error: Option<VisualDebugError>,
}

#[derive(Serialize)]
struct VisualDebugResource {
    program: &'static str,
    version: &'static str,
    renderer: &'static str,
}

#[derive(Serialize)]
struct VisualDebugOperation {
    sequence: usize,
    action: String,
    status: &'static str,
    image: String,
    layout: String,
}

#[derive(Serialize)]
struct VisualDebugError {
    operation: String,
    error_type: &'static str,
    message: String,
}

#[derive(Serialize)]
struct VisualDebugLayout {
    schema_version: u32,
    logical_size: [u32; 2],
    physical_size: [u32; 2],
    scale: f32,
    elements: Vec<VisualDebugElement>,
}

#[derive(Serialize)]
struct VisualDebugElement {
    selector: String,
    matches: Vec<[f64; 4]>,
}

struct VisualDebugSession {
    document: DioxusDocument,
    pointer: PointerInput,
    logical_size: [u32; 2],
    physical_size: [u32; 2],
    scale: f32,
    projection: Reactive<NativeDocumentProjection>,
    authority: NativeEditorAuthority,
    commands: std::sync::mpsc::Receiver<CoordinatorCommand>,
    state: OverlayState,
    skins: std::collections::BTreeMap<String, EditorSkinPreview>,
    skin_assets: Arc<SkinAssetCache>,
    report: Rc<RefCell<RunReport>>,
    runtime_create_count: u64,
}

impl VisualDebugSession {
    #[allow(clippy::too_many_lines)]
    fn new(scenario: &VisualDebugScenario, physical_size: [u32; 2]) -> Result<Self, String> {
        let config = crate::config::visual_debug_config();
        let mut canvases = config
            .canvases
            .into_iter()
            .filter(|canvas| canvas.backend == crate::runtime::Backend::Wayland)
            .map(|canvas| canvas.presentation())
            .collect::<Vec<_>>();
        if let Some(replacement) = &scenario.canvases {
            canvases.clone_from(replacement);
        }
        if let Some(skin) = scenario.skin {
            for canvas in &mut canvases {
                canvas.skin = skin;
            }
        }
        let selected = scenario
            .canvas_id
            .as_ref()
            .map_or_else(
                || canvases.first().map(|canvas| canvas.id.clone()),
                |id| {
                    canvases
                        .iter()
                        .find(|canvas| &canvas.id == id)
                        .map(|canvas| canvas.id.clone())
                },
            )
            .ok_or_else(|| "canvas_id does not select a Wayland canvas".to_owned())?;
        let canvas = canvases
            .iter()
            .find(|canvas| canvas.id == selected)
            .cloned()
            .ok_or("the initial Wayland workspace is empty")?;
        let output = EditorOutput {
            name: canvas.output.clone().unwrap_or_else(|| "HEADLESS-1".into()),
            model: "scorepeek visual debugger".into(),
            logical_size: Some(scenario.logical_size),
        };
        let mut model = EditorSession::new(canvases, scenario.logical_size, "visual");
        model.set_session_id(1);
        model.set_skins(embedded_editor_skins());
        model.set_outputs(vec![output.clone()]);
        model.active_output = Some(output.name.clone());
        model.selected_canvas = Some(selected);
        model.editing = scenario.editing;
        model.readonly = false;
        model.advance_revision();
        let published_stages = Arc::new(std::sync::Mutex::new(PublishedStages::default()));
        let authority = NativeEditorAuthority::new(model, published_stages);
        let initial = if scenario.editing {
            NativeDocumentProjection::Editor(authority.session().stage_projection(&output))
        } else {
            NativeDocumentProjection::Display {
                canvas: canvas.clone(),
                visible: true,
            }
        };
        let published = Rc::new(RefCell::new(None));
        let (sender, commands) = std::sync::mpsc::channel();
        let props = NativeOverlayProps {
            initial,
            published: Rc::clone(&published),
            port: NativeEditorPort {
                coordinator: sender,
                source_output: Some(output.name),
                run_id: "visual-debug".into(),
                sequence: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            },
        };
        #[cfg(test)]
        let package: Option<crate::skin::Package> = None;
        #[cfg(not(test))]
        let package = {
            let store = crate::skin::StoreRoot::discover();
            let package_path = store.path().join(format!("{}.zip", canvas.skin.name()));
            package_path
                .exists()
                .then(|| store.open(canvas.skin.name()))
                .transpose()?
        };
        let (document_config, skin_assets) = package.map_or_else(
            || {
                document_config_inner_with_handle(Arc::new(SkinAssetCache::new(
                    crate::skin::StoreRoot::discover(),
                )))
            },
            |package| document_config_with_skin_handle(Arc::new(package)),
        );
        let mut document = DioxusDocument::new(
            VirtualDom::new_with_props(native_overlay, props),
            document_config,
        );
        document.initial_build();
        let projection = published
            .borrow()
            .as_ref()
            .copied()
            .ok_or("native overlay did not publish its projection")?;
        let state = scorepeek_overlay_ui::editor_sample_state();
        let mut skins = std::collections::BTreeMap::new();
        let mounted_canvases = match &*projection.borrow() {
            NativeDocumentProjection::Editor(editor_projection) => {
                editor_projection.canvases.clone()
            }
            NativeDocumentProjection::Display { .. } => vec![canvas.clone()],
        };
        for mounted in mounted_canvases {
            if scenario.editing {
                continue;
            }
            let store = crate::skin::StoreRoot::discover();
            let package_path = store.path().join(format!("{}.zip", mounted.skin.name()));
            if !package_path.exists() {
                continue;
            }
            let package = skin_assets.load(mounted.skin.name())?;
            let root_selector = if scenario.editing {
                format!("#{}", editor_skin_root_id(&mounted.id))
            } else {
                "#scorepeek-skin-root".into()
            };
            let root = document
                .inner
                .borrow()
                .query_selector(&root_selector)
                .map_err(|error| format!("query native skin root: {error:?}"))?
                .ok_or("native skin root is missing")?;
            let css = namespace_skin_css(
                mounted.skin.name(),
                std::str::from_utf8(
                    package
                        .resource(crate::skin::STYLE_PATH)
                        .ok_or("skin.css missing")?,
                )
                .map_err(|error| format!("skin.css is not UTF-8: {error}"))?,
            );
            let mut runtime = crate::skin::Runtime::new(&package)?;
            let input = native_skin_input_presentation(&mounted, &state, &package.manifest);
            let mut initial = runtime.init(&input)?;
            namespace_native_skin_output(&package.manifest.id, &mut initial);
            let mut tree =
                crate::skin::NativeTree::new(&mut document.inner.borrow_mut(), root, &css);
            tree.apply(&mut document.inner.borrow_mut(), &initial);
            skins.insert(
                mounted.id.clone(),
                EditorSkinPreview {
                    canvas: {
                        let mut canvas = crate::config::empty_canvas(
                            mounted.id.clone(),
                            crate::runtime::Backend::Wayland,
                        );
                        canvas.apply_presentation(&mounted);
                        canvas
                    },
                    runtime,
                    tree,
                    package,
                    next_render: skin_deadline(&initial.schedule, false),
                    last_input: input,
                    last_state: state.clone(),
                },
            );
        }
        let report = Rc::new(RefCell::new(RunReport::new()));
        let runtime_create_count = u64::try_from(skins.len()).unwrap_or(u64::MAX);
        let mut session = Self {
            document,
            pointer: PointerInput::default(),
            logical_size: scenario.logical_size,
            physical_size,
            scale: scenario.scale,
            projection,
            authority,
            commands,
            state,
            skins,
            skin_assets,
            report,
            runtime_create_count,
        };
        session.resolve();
        Ok(session)
    }

    fn flush_inputs(&mut self) -> bool {
        let mut changed = false;
        while let Ok(command) = self.commands.try_recv() {
            if let CoordinatorCommand::EditorInput { input, .. } = command {
                let _ = self.authority.dispatch(input);
                changed = true;
            }
        }
        let output = match &*self.projection.borrow() {
            NativeDocumentProjection::Editor(current) => Some(current.output.clone()),
            NativeDocumentProjection::Display { .. } => None,
        };
        if let Some(output) = output {
            let candidate = self.authority.session().stage_projection(&output);
            changed |= accept_stage_projection_replica(
                self.projection,
                &mut self.document,
                &mut self.skins,
                &self.skin_assets,
                &output.name,
                &candidate,
            );
        }
        changed
    }

    fn resolve(&mut self) {
        loop {
            let mut changed = false;
            while poll_native_document(&mut self.document, Waker::noop()) {
                changed = true;
            }
            changed |= self.flush_inputs();
            if !changed {
                break;
            }
        }
        let _ = self.render_skin();
        let mut inner = self.document.inner.borrow_mut();
        inner.set_viewport(Viewport::new(
            self.physical_size[0],
            self.physical_size[1],
            self.scale,
            ColorScheme::Dark,
        ));
        inner.resolve(0.0);
        apply_motion(&mut inner, 1.0);
        resolve_with_loaded_resources(&mut inner, 1.0);
    }

    #[allow(clippy::too_many_lines)]
    fn render_skin(&mut self) -> Result<(), String> {
        let editor = match &*self.projection.borrow() {
            NativeDocumentProjection::Editor(stage) => {
                Some((stage.canvases.clone(), stage.output.name.clone()))
            }
            NativeDocumentProjection::Display { .. } => None,
        };
        if let Some((canvases, output)) = editor {
            let mut work = FrameWorkProfile::default();
            let _ = reconcile_editor_skin_previews(
                &mut self.document,
                &mut self.skins,
                &canvases,
                &self.skin_assets,
                &self.report,
                Some(&output),
                &self.state,
                &mut self.runtime_create_count,
                &mut work,
            )?;
            return Ok(());
        }
        let canvases = match &*self.projection.borrow() {
            NativeDocumentProjection::Display { canvas, .. } => vec![canvas.clone()],
            NativeDocumentProjection::Editor(_) => unreachable!(),
        };
        let live = canvases
            .iter()
            .map(|canvas| canvas.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        self.skins.retain(|id, _| live.contains(id.as_str()));
        for canvas in canvases {
            let desired_skin = canvas.skin.name();
            if !self.skins.contains_key(&canvas.id) {
                let store = crate::skin::StoreRoot::discover();
                let package_path = store.path().join(format!("{desired_skin}.zip"));
                if !package_path.exists() {
                    continue;
                }
                let package = self.skin_assets.load(desired_skin)?;
                let root_selector = if matches!(
                    &*self.projection.borrow(),
                    NativeDocumentProjection::Editor(_)
                ) {
                    format!("#{}", editor_skin_root_id(&canvas.id))
                } else {
                    "#scorepeek-skin-root".into()
                };
                let Some(root) = self
                    .document
                    .inner
                    .borrow()
                    .query_selector(&root_selector)
                    .map_err(|error| format!("query native skin root: {error:?}"))?
                else {
                    continue;
                };
                let css = namespace_skin_css(
                    desired_skin,
                    std::str::from_utf8(
                        package
                            .resource(crate::skin::STYLE_PATH)
                            .ok_or("skin.css missing")?,
                    )
                    .map_err(|error| format!("skin.css is not UTF-8: {error}"))?,
                );
                let mut runtime = crate::skin::Runtime::new(&package)?;
                let initial_input =
                    native_skin_input_presentation(&canvas, &self.state, &package.manifest);
                let mut output = runtime.init(&initial_input)?;
                namespace_native_skin_output(&package.manifest.id, &mut output);
                let mut tree =
                    crate::skin::NativeTree::new(&mut self.document.inner.borrow_mut(), root, &css);
                tree.apply(&mut self.document.inner.borrow_mut(), &output);
                self.skins.insert(
                    canvas.id.clone(),
                    EditorSkinPreview {
                        canvas: {
                            let mut configured = crate::config::empty_canvas(
                                canvas.id.clone(),
                                crate::runtime::Backend::Wayland,
                            );
                            configured.apply_presentation(&canvas);
                            configured
                        },
                        runtime,
                        tree,
                        package,
                        next_render: skin_deadline(&output.schedule, false),
                        last_input: initial_input,
                        last_state: self.state.clone(),
                    },
                );
            }
            let Some(skin) = self.skins.get_mut(&canvas.id) else {
                continue;
            };
            let changed = skin.package.manifest.id != desired_skin;
            if changed {
                skin.package = self.skin_assets.load(desired_skin)?;
                skin.runtime = crate::skin::Runtime::new(&skin.package)?;
            }
            let input =
                native_skin_input_presentation(&canvas, &self.state, &skin.package.manifest);
            let mut output = if changed {
                skin.runtime.init(&input)?
            } else {
                skin.runtime.render(&input)?
            };
            namespace_native_skin_output(&skin.package.manifest.id, &mut output);
            if changed {
                let css = namespace_skin_css(
                    desired_skin,
                    std::str::from_utf8(
                        skin.package
                            .resource(crate::skin::STYLE_PATH)
                            .ok_or("skin.css missing")?,
                    )
                    .map_err(|error| format!("skin.css is not UTF-8: {error}"))?,
                );
                skin.tree
                    .replace(&mut self.document.inner.borrow_mut(), &css, &output);
            } else {
                skin.tree
                    .apply(&mut self.document.inner.borrow_mut(), &output);
            }
        }
        Ok(())
    }

    fn set_screen(&mut self, screen: Option<scorepeek_overlay_ui::ScreenKind>) {
        self.state.screen.kind = screen;
        self.state.screen.suspended_since_unix_ms = None;
        self.state.screen.revision = self.state.screen.revision.saturating_add(1);
        let canvas = match &*self.projection.borrow() {
            NativeDocumentProjection::Display { canvas, .. } => Some(canvas.clone()),
            NativeDocumentProjection::Editor(_) => None,
        };
        if let Some(canvas) = canvas {
            let visible =
                scorepeek_overlay_ui::canvas_visible(canvas.show_on.as_deref(), self.state.screen);
            self.projection
                .set(NativeDocumentProjection::Display { canvas, visible });
        }
        self.resolve();
    }

    fn set_editing(&mut self, editing: bool) {
        let mut model = self.authority.session().clone();
        model.editing = editing;
        model.advance_revision();
        let output = model.outputs.first().cloned();
        self.authority = NativeEditorAuthority::new(
            model,
            Arc::new(std::sync::Mutex::new(PublishedStages::default())),
        );
        // The editor and display projections mount different Dioxus roots. Rebuild the
        // visual harness trees against the new roots just as production surface roles do.
        let previous_output = match &*self.projection.borrow() {
            NativeDocumentProjection::Editor(stage) => Some(stage.output.name.clone()),
            NativeDocumentProjection::Display { .. } => None,
        };
        for (id, mut skin) in std::mem::take(&mut self.skins) {
            skin.tree.unmount(&mut self.document.inner.borrow_mut());
            if let Some(output) = previous_output.as_deref() {
                self.skin_assets.release_editor_owner(&id, output);
            }
        }
        if let Some(output) = output {
            self.projection.set(if editing {
                NativeDocumentProjection::Editor(self.authority.session().stage_projection(&output))
            } else if let Some(canvas) = self.authority.session().current().cloned() {
                NativeDocumentProjection::Display {
                    canvas,
                    visible: true,
                }
            } else {
                return;
            });
        }
        self.resolve();
    }

    fn title_text(&mut self, text: String, composing: bool) -> Result<(), String> {
        if self.authority.session().title.is_none() {
            return Err("title input is not active".into());
        }
        let _ = self
            .authority
            .dispatch(EditorInput::Action(EditorAction::TextComposition {
                field_key: "editor-title-input".into(),
                composing,
            }));
        let _ = self
            .authority
            .dispatch(EditorInput::Action(EditorAction::TitleText(text)));
        self.flush_inputs();
        self.resolve();
        Ok(())
    }

    fn click(&mut self, selector: &str) -> Result<(), String> {
        let point = {
            let inner = self.document.inner.borrow();
            let node = inner
                .query_selector(selector)
                .map_err(|_| "invalid selector")?
                .ok_or_else(|| format!("selector did not match: {selector}"))?;
            let rect = inner
                .get_client_bounding_rect(node)
                .ok_or("selector has no layout")?;
            [rect.x + rect.width / 2.0, rect.y + rect.height / 2.0]
        };
        self.pointer.click(&mut self.document, point);
        self.resolve();
        Ok(())
    }

    fn scroll(&mut self, selector: &str, dx: f64, dy: f64) -> Result<(), String> {
        let point = {
            let inner = self.document.inner.borrow();
            let node = inner
                .query_selector(selector)
                .map_err(|_| "invalid selector".to_owned())?
                .ok_or_else(|| format!("selector did not match: {selector}"))?;
            let rect = inner
                .get_client_bounding_rect(node)
                .ok_or("selector has no layout")?;
            [rect.x + rect.width / 2.0, rect.y + rect.height / 2.0]
        };
        self.pointer.wheel(&mut self.document, point, [dx, dy]);
        self.resolve();
        Ok(())
    }

    #[cfg(test)]
    fn key(&mut self, command: &scorepeek_overlay_handles::TextCommand) {
        self.key_composing(command, false);
    }

    #[cfg(test)]
    fn key_composing(&mut self, command: &scorepeek_overlay_handles::TextCommand, composing: bool) {
        text::dispatch_control_key(&mut self.document, command, composing);
        self.resolve();
    }

    #[cfg(test)]
    fn ime(&mut self, update: scorepeek_overlay_handles::TextUpdate) {
        let field_key = text::focused_field_key(&self.document);
        if let Some(composing) = text::dispatch_control_composition(&mut self.document, update)
            && let Some(field_key) = field_key
        {
            let _ = self
                .authority
                .dispatch(EditorInput::Action(EditorAction::TextComposition {
                    field_key,
                    composing,
                }));
        }
        self.resolve();
    }

    #[cfg(test)]
    fn focus(&mut self, selector: &str) -> Result<(), String> {
        let node = self
            .document
            .inner
            .borrow()
            .query_selector(selector)
            .map_err(|_| "invalid selector".to_owned())?
            .ok_or_else(|| format!("selector did not match: {selector}"))?;
        self.document.inner.borrow_mut().set_focus_to(node);
        self.resolve();
        Ok(())
    }

    fn drag(
        &mut self,
        from: [f64; 2],
        to: [f64; 2],
        button: VisualDebugButton,
    ) -> Result<(), String> {
        if !matches!(
            &*self.projection.borrow(),
            NativeDocumentProjection::Editor(_)
        ) {
            return Err("drag requires editing".into());
        }
        let button = match button {
            VisualDebugButton::Right => 0x111,
            VisualDebugButton::Left => 0x110,
        };
        self.pointer
            .dispatch(&mut self.document, from, button, None);
        self.pointer
            .dispatch(&mut self.document, from, button, Some(true));
        self.resolve();
        if self.authority.session().drag.is_none() {
            return Err("drag did not reach a Dioxus canvas handler".into());
        }
        self.pointer.dispatch(&mut self.document, to, button, None);
        self.resolve();
        self.pointer
            .dispatch(&mut self.document, to, button, Some(false));
        self.resolve();
        Ok(())
    }

    fn render(
        &mut self,
        renderer: &mut anyrender_vello::VelloImageRenderer,
        path: &std::path::Path,
    ) -> Result<(), String> {
        let mut pixels = Vec::new();
        let mut inner = self.document.inner.borrow_mut();
        resolve_with_loaded_resources(&mut inner, 1.0);
        renderer.render_to_vec(
            |scene| {
                paint_native_scene(
                    scene,
                    &mut inner,
                    f64::from(self.scale),
                    self.physical_size[0],
                    self.physical_size[1],
                );
            },
            &mut pixels,
        );
        image::save_buffer_with_format(
            path,
            &pixels,
            self.physical_size[0],
            self.physical_size[1],
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(|error| error.to_string())
    }

    fn layout(&self, selectors: &[String]) -> Result<VisualDebugLayout, String> {
        let inner = self.document.inner.borrow();
        let elements = selectors
            .iter()
            .map(|selector| {
                let matches = inner
                    .query_selector_all(selector)
                    .map_err(|_| format!("invalid selector: {selector}"))?
                    .into_iter()
                    .filter_map(|node| inner.get_client_bounding_rect(node))
                    .map(|rect| [rect.x, rect.y, rect.width, rect.height])
                    .collect();
                Ok(VisualDebugElement {
                    selector: selector.clone(),
                    matches,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(VisualDebugLayout {
            schema_version: 1,
            logical_size: self.logical_size,
            physical_size: self.physical_size,
            scale: self.scale,
            elements,
        })
    }
}

/// Renders the production Dioxus native DOM through Blitz and Vello without a
/// Wayland compositor, recording every requested interaction and layout.
///
/// # Errors
///
/// Returns an error when the scenario is invalid or an artifact cannot be rendered or written.
#[allow(clippy::cast_possible_truncation, clippy::too_many_lines)]
pub fn run_visual_debug(
    scenario: &VisualDebugScenario,
    output: &std::path::Path,
) -> Result<(), String> {
    std::fs::create_dir(output).map_err(|error| format!("create visual output: {error}"))?;
    let physical_size = if scenario.scale.is_finite() && scenario.scale > 0.0 {
        [
            u32::try_from(
                (f64::from(scenario.logical_size[0]) * f64::from(scenario.scale)).round() as i64,
            )
            .map_err(|error| error.to_string())?,
            u32::try_from(
                (f64::from(scenario.logical_size[1]) * f64::from(scenario.scale)).round() as i64,
            )
            .map_err(|error| error.to_string())?,
        ]
    } else {
        return Err("visual scale must be finite and positive".into());
    };
    let selectors = if scenario.selectors.is_empty() {
        [
            ".canvas-content",
            ".overlay-canvas",
            ".widget-slot",
            ".editor-panel-toggle",
            ".editor-panel",
            ".inspector-scroll",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>()
    } else {
        scenario.selectors.clone()
    };
    let mut manifest = VisualDebugManifest {
        schema_version: 1,
        run_id: format!("visual-{}", std::process::id()),
        resource: VisualDebugResource {
            program: "scorepeek-overlay-native-visual",
            version: env!("CARGO_PKG_VERSION"),
            renderer: "dioxus-native-dom+blitz+vello",
        },
        logical_size: scenario.logical_size,
        physical_size: Some(physical_size),
        scale: scenario.scale,
        status: "running",
        completeness: "partial",
        operations: Vec::new(),
        error: None,
    };
    let result = (|| {
        let mut session = VisualDebugSession::new(scenario, physical_size)?;
        let mut renderer =
            anyrender_vello::VelloImageRenderer::new(physical_size[0], physical_size[1]);
        capture_visual_debug(
            &mut session,
            &mut renderer,
            output,
            &selectors,
            &mut manifest,
            0,
            "initial",
        )?;
        for (index, action) in scenario.actions.iter().enumerate() {
            let name = match action {
                VisualDebugAction::TitleText { text, composing } => {
                    session.title_text(text.clone(), *composing)?;
                    if *composing {
                        "title-preedit".into()
                    } else {
                        "title-commit".into()
                    }
                }
                VisualDebugAction::Motion { seconds } => {
                    if !seconds.is_finite() || *seconds < 0.0 {
                        return Err("motion seconds must be finite and nonnegative".into());
                    }
                    session.render_skin()?;
                    let mut inner = session.document.inner.borrow_mut();
                    apply_motion(&mut inner, *seconds);
                    resolve_with_loaded_resources(&mut inner, *seconds);
                    format!("motion-{seconds}")
                }
                VisualDebugAction::SetEditing { value } => {
                    session.set_editing(*value);
                    format!("set-editing-{value}")
                }
                VisualDebugAction::SetScreen { screen } => {
                    session.set_screen(*screen);
                    screen.map_or_else(
                        || "set-screen-none".into(),
                        |screen| format!("set-screen-{screen:?}"),
                    )
                }
                VisualDebugAction::Click { selector } => {
                    session.click(selector)?;
                    format!("click-{}", sanitize_artifact_name(selector))
                }
                VisualDebugAction::Scroll { selector, dx, dy } => {
                    session.scroll(selector, *dx, *dy)?;
                    format!("scroll-{}", sanitize_artifact_name(selector))
                }
                VisualDebugAction::Drag { from, to, button } => {
                    session.drag(*from, *to, *button)?;
                    "drag".into()
                }
                VisualDebugAction::Capture { name } => {
                    format!("capture-{}", sanitize_artifact_name(name))
                }
            };
            capture_visual_debug(
                &mut session,
                &mut renderer,
                output,
                &selectors,
                &mut manifest,
                index + 1,
                &name,
            )?;
        }
        Ok::<(), String>(())
    })();
    match &result {
        Ok(()) => {
            manifest.status = "complete";
            manifest.completeness = "complete";
        }
        Err(error) => {
            manifest.status = "failed";
            manifest.error = Some(VisualDebugError {
                operation: "render".into(),
                error_type: "visual_debug_failed",
                message: error.clone(),
            });
        }
    }
    std::fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    result
}

fn capture_visual_debug(
    session: &mut VisualDebugSession,
    renderer: &mut anyrender_vello::VelloImageRenderer,
    output: &std::path::Path,
    selectors: &[String],
    manifest: &mut VisualDebugManifest,
    sequence: usize,
    name: &str,
) -> Result<(), String> {
    let stem = format!("{sequence:03}-{}", sanitize_artifact_name(name));
    let image_name = format!("{stem}.png");
    let layout_name = format!("{stem}.layout.json");
    session.render(renderer, &output.join(&image_name))?;
    let layout = session.layout(selectors)?;
    std::fs::write(
        output.join(&layout_name),
        serde_json::to_vec_pretty(&layout).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    manifest.operations.push(VisualDebugOperation {
        sequence,
        action: name.to_owned(),
        status: "success",
        image: image_name,
        layout: layout_name,
    });
    Ok(())
}

fn sanitize_artifact_name(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "capture".into()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod skin_tests {
    use super::*;

    #[test]
    fn stage_replica_rejects_stale_revisions_and_accepts_a_new_session() {
        let mut session = EditorSession::new(Vec::new(), [1920, 1080], "test");
        session.set_session_id(7);
        let output = EditorOutput {
            name: "DP-1".into(),
            model: "test".into(),
            logical_size: Some([1920, 1080]),
        };
        session.set_outputs(vec![output.clone()]);
        session.advance_revision();
        let current = session.stage_projection(&output);
        let mut stale = current.clone();
        stale.revision = current.revision.saturating_sub(1);
        assert!(!accepts_stage_projection(&current, &stale));
        let mut next = current.clone();
        next.revision += 1;
        assert!(accepts_stage_projection(&current, &next));
        let mut replacement = stale;
        replacement.session_id += 1;
        assert!(accepts_stage_projection(&current, &replacement));
    }

    #[test]
    fn passive_pointer_motion_does_not_publish_a_native_stage_replica() {
        let mut session = EditorSession::new(Vec::new(), [1920, 1080], "test");
        session.set_outputs(vec![EditorOutput {
            name: "DP-1".into(),
            model: "test".into(),
            logical_size: Some([1920, 1080]),
        }]);
        session.editing = true;
        session.advance_revision();
        let published = Arc::new(std::sync::Mutex::new(PublishedStages::default()));
        let mut authority = NativeEditorAuthority::new(session, Arc::clone(&published));
        let before = published.lock().unwrap().publications;

        let effects = authority.dispatch(EditorInput::Surface(
            scorepeek_overlay_ui::editor_surface::SurfaceAction::Move([320, 180]),
        ));

        assert!(effects.is_empty());
        assert_eq!(published.lock().unwrap().publications, before);
    }

    #[test]
    fn native_run_ids_are_unique_across_parallel_surfaces() {
        let ids = (0..32)
            .map(|_| std::thread::spawn(|| RunReport::new().run_id))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids.len(), 32);
    }

    #[test]
    fn editor_skin_updates_coalesce_while_dragging_and_flush_on_frame_or_release() {
        let mut updates = EditorSkinUpdates::default();
        for _ in 0..100 {
            updates.request();
            assert!(!updates.take_if_ready(false, true));
        }
        assert!(updates.take_if_ready(true, true));
        assert!(!updates.take_if_ready(true, true));

        updates.request();
        assert!(updates.take_if_ready(false, false));
        assert_eq!(updates.requests, 101);
    }

    #[test]
    fn native_animation_work_does_not_copy_skin_archives_per_frame() {
        let manifest: crate::skin::Manifest =
            toml::from_str(include_str!("../../../skins/cyan-system/skin.toml")).unwrap();
        let package = crate::skin::Package::test_with_entries(
            manifest,
            std::collections::BTreeMap::from([("artwork.bin".into(), vec![0; 8 * 1024 * 1024])]),
        );
        let package = Arc::new(package);
        let cache =
            SkinAssetCache::with_package(crate::skin::StoreRoot::discover(), Arc::clone(&package));

        // Four live canvases at 120 Hz model one second of production editor animation.
        for _ in 0..(4 * 120) {
            let resolved = cache.load(&package.manifest.id).unwrap();
            assert!(Arc::ptr_eq(&resolved, &package));
        }

        assert_eq!(cache.packages.lock().unwrap().len(), 1);
        assert_eq!(
            cache.open_count.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }

    #[test]
    fn editor_canvas_owner_prevents_reverse_delivery_from_mounting_two_runtimes() {
        let cache = SkinAssetCache::new(crate::skin::StoreRoot::discover());
        assert!(cache.acquire_editor_owner("canvas-1", "WL-1"));
        assert!(!cache.acquire_editor_owner("canvas-1", "WL-2"));
        cache.release_editor_owner("canvas-1", "WL-2");
        assert!(!cache.acquire_editor_owner("canvas-1", "WL-2"));
        cache.release_editor_owner("canvas-1", "WL-1");
        assert!(cache.acquire_editor_owner("canvas-1", "WL-2"));
        assert_eq!(
            cache.editor_owners.lock().unwrap().get("canvas-1"),
            Some(&"WL-2".to_owned())
        );
    }

    #[test]
    fn native_skin_resources_are_namespaced_by_immutable_skin_identity() {
        let css = namespace_skin_css(
            "dev.atty303.skin",
            "a{src:url('font.ttf')}b{background:url(\"/shared.png\")}c{mask:url(data:image/png;base64,abc)}d{mask:url(https://example.test/shared.svg)}e{mask:url(https:shared.svg)}f{src:url(urn:scorepeek:asset)}",
        );
        assert!(
            css.contains("url('/skin/dev.atty303.skin/font.ttf')"),
            "{css}"
        );
        assert!(css.contains("url(\"/shared.png\")"), "{css}");
        assert!(css.contains("url(data:image/png;base64,abc)"), "{css}");
        assert!(
            css.contains("url(https://example.test/shared.svg)"),
            "{css}"
        );
        assert!(css.contains("url(https:shared.svg)"), "{css}");
        assert!(css.contains("url(urn:scorepeek:asset)"), "{css}");
    }

    #[test]
    fn native_skin_inline_resources_use_the_package_namespace() {
        let mut output = crate::skin::RenderOutput {
            schedule: crate::skin::Schedule::Idle,
            tree: crate::skin::Node::Element {
                key: "background".into(),
                tag: "div".into(),
                attributes: std::collections::BTreeMap::from([
                    (
                        "style".into(),
                        "background-image:url('background.png');mask:url(\"mask.svg\")".into(),
                    ),
                    ("src".into(), "preview.png".into()),
                    ("poster".into(), "file:preview.webm".into()),
                ]),
                children: Vec::new(),
            },
        };

        namespace_native_skin_output("dev.atty303.skin", &mut output);

        let crate::skin::Node::Element { attributes, .. } = output.tree else {
            panic!("probe must remain an element");
        };
        assert_eq!(
            attributes.get("style").map(String::as_str),
            Some(
                "background-image:url('/skin/dev.atty303.skin/background.png');mask:url(\"/skin/dev.atty303.skin/mask.svg\")"
            )
        );
        assert_eq!(
            attributes.get("src").map(String::as_str),
            Some("/skin/dev.atty303.skin/preview.png")
        );
        assert_eq!(
            attributes.get("poster").map(String::as_str),
            Some("file:preview.webm")
        );
    }

    #[test]
    fn native_skin_input_uses_the_canvas_background_authority() {
        let mut canvas = crate::config::empty_canvas(
            "background-probe".into(),
            crate::runtime::Backend::Wayland,
        );
        canvas.background = scorepeek_overlay_ui::Background::Static;
        let manifest: crate::skin::Manifest =
            toml::from_str(include_str!("../../../skins/cyan-system/skin.toml")).unwrap();

        let input = native_skin_input(&canvas, &OverlayState::default(), &manifest);

        assert_eq!(input["canvas"]["properties"]["background"], "static");
    }

    #[test]
    fn unchanged_native_frames_do_not_rebuild_surface_projection() {
        let config = crate::config::visual_debug_config();
        let draft = config
            .canvases
            .iter()
            .filter(|canvas| canvas.backend == crate::runtime::Backend::Wayland)
            .map(crate::config::Canvas::presentation)
            .collect::<Vec<_>>();
        let mut session = EditorSession::new(draft, [1920, 1080], "fake-wayland");
        session.set_session_id(7);
        session.set_outputs(vec![EditorOutput {
            name: "WL-1".into(),
            model: "fake output".into(),
            logical_size: Some([1920, 1080]),
        }]);
        session.editing = true;
        session.advance_revision();
        let mut cache = NativeProjectionCache::default();

        for _ in 0..120 {
            assert_eq!(
                cache
                    .resolve(Some(scorepeek_overlay_ui::Skin::CyanSystem), &session, &[])
                    .unwrap()
                    .len(),
                1
            );
        }

        assert_eq!(cache.rebuilds, 1);
        session.advance_revision();
        let _ = cache
            .resolve(Some(scorepeek_overlay_ui::Skin::CyanSystem), &session, &[])
            .unwrap();
        assert_eq!(cache.rebuilds, 2);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn fake_wayland_axis_scrolls_ancestor_beneath_nested_editor_rows() {
        struct FakeProtocolTarget<'a>(&'a mut VisualDebugSession);
        impl NativeEventConsumer for FakeProtocolTarget<'_> {
            fn configure_event(
                &mut self,
                _logical: [u32; 2],
                physical: [u32; 2],
                scale_120: u32,
            ) -> Result<bool, String> {
                self.0
                    .document
                    .inner
                    .borrow_mut()
                    .set_viewport(Viewport::new(
                        physical[0],
                        physical[1],
                        f32::from(u16::try_from(scale_120).map_err(|error| error.to_string())?)
                            / 120.0,
                        ColorScheme::Dark,
                    ));
                Ok(true)
            }
            fn pointer_motion_event(&mut self, point: [f64; 2]) {
                self.0
                    .pointer
                    .dispatch(&mut self.0.document, point, 0x110, None);
            }
            fn pointer_button_event(&mut self, button: u32, pressed: bool, point: [f64; 2]) {
                self.0
                    .pointer
                    .dispatch(&mut self.0.document, point, button, Some(pressed));
            }
            fn pointer_scroll_event(&mut self, delta: [f64; 2], point: [f64; 2]) {
                self.0.pointer.wheel(&mut self.0.document, point, delta);
            }
            fn text_event(&mut self, command: &scorepeek_overlay_handles::TextCommand) {
                text::dispatch_control_key(&mut self.0.document, command, false);
            }
            fn ime_event(&mut self, update: scorepeek_overlay_handles::TextUpdate) {
                let _ = text::dispatch_control_composition(&mut self.0.document, update);
            }
            fn keyboard_focus_event(&mut self, _focused: bool) {}
        }
        let axis = |session: &mut VisualDebugSession, point: [f64; 2], delta: [f64; 2]| {
            let outcome = dispatch_native_event(
                &mut FakeProtocolTarget(session),
                Event::PointerScroll {
                    dx: delta[0],
                    dy: delta[1],
                    x: point[0],
                    y: point[1],
                },
            )
            .unwrap();
            assert!(outcome.input_damage);
        };
        let canvases = crate::config::visual_debug_config()
            .canvases
            .into_iter()
            .filter(|canvas| canvas.backend == crate::runtime::Backend::Wayland)
            .map(|mut canvas| {
                canvas.output = "WL-1".into();
                canvas.presentation()
            })
            .collect();
        let scenario = VisualDebugScenario {
            canvases: Some(canvases),
            skin: None,
            logical_size: [1280, 1080],
            scale: 1.0,
            canvas_id: Some("wayland-status".into()),
            editing: true,
            selectors: Vec::new(),
            actions: Vec::new(),
        };
        let offset = |session: &VisualDebugSession| {
            let inner = session.document.inner.borrow();
            let node = inner.query_selector(".navigator-scroll").unwrap().unwrap();
            inner.get_node(node).unwrap().scroll_offset().y
        };

        let point_in_navigator = |session: &VisualDebugSession, selector: &str| {
            let inner = session.document.inner.borrow();
            let target = inner
                .get_client_bounding_rect(inner.query_selector(selector).unwrap().unwrap())
                .unwrap();
            let viewport = inner
                .get_client_bounding_rect(
                    inner.query_selector(".navigator-scroll").unwrap().unwrap(),
                )
                .unwrap();
            [
                target.x + target.width / 2.0,
                target
                    .y
                    .max(viewport.y + 2.0)
                    .min(viewport.y + viewport.height - 2.0),
            ]
        };
        for (selector, message, delta) in [
            (
                ".workspace-output-option .navigator-item-select",
                "output row",
                -80.0,
            ),
            (
                ".workspace-output-option .tree-disclosure",
                "disclosure child",
                -80.0,
            ),
        ] {
            let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
            let prior = offset(&session);
            let point = point_in_navigator(&session, selector);
            axis(&mut session, point, [0.0, delta]);
            session.resolve();
            assert!(
                (offset(&session) - prior).abs() > f64::EPSILON,
                "axis over an {message} must reach navigator scroll"
            );
        }
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        let before = offset(&session);
        let canvas_point = {
            let inner = session.document.inner.borrow();
            let node = inner.query_selector(".canvas-select").unwrap().unwrap();
            let target = inner.get_client_bounding_rect(node).unwrap();
            let viewport = inner
                .get_client_bounding_rect(
                    inner.query_selector(".navigator-scroll").unwrap().unwrap(),
                )
                .unwrap();
            [
                target.x + target.width / 2.0,
                target.y.max(viewport.y) + 4.0,
            ]
        };
        session
            .pointer
            .dispatch(&mut session.document, [1000.0, 500.0], 0x110, None);
        let raw_point =
            dioxus::html::geometry::ClientPoint::new(canvas_point[0], canvas_point[1]).to_f32();
        session
            .document
            .handle_ui_event(blitz_traits::events::UiEvent::Wheel(
                blitz_traits::events::BlitzWheelEvent {
                    delta: blitz_traits::events::BlitzWheelDelta::Pixels(0.0, -120.0),
                    coords: blitz_traits::events::PointerCoords {
                        page_x: raw_point.x,
                        page_y: raw_point.y,
                        screen_x: raw_point.x,
                        screen_y: raw_point.y,
                        client_x: raw_point.x,
                        client_y: raw_point.y,
                    },
                    buttons: session.pointer.buttons,
                    mods: dioxus::html::Modifiers::default(),
                    element: blitz_traits::events::Point::default(),
                },
            ));
        session.resolve();
        assert!(
            (offset(&session) - before).abs() <= f64::EPSILON,
            "unadapted Blitz routes wheel through stale hover instead of event coordinates"
        );
        axis(&mut session, canvas_point, [0.0, -120.0]);
        session.resolve();
        let after_canvas = offset(&session);
        assert!(
            (after_canvas - before).abs() > f64::EPSILON,
            "axis over a canvas row must reach navigator scroll"
        );

        let widget_point = {
            let inner = session.document.inner.borrow();
            let viewport = inner
                .get_client_bounding_rect(
                    inner.query_selector(".navigator-scroll").unwrap().unwrap(),
                )
                .unwrap();
            let target = inner
                .get_client_bounding_rect(inner.query_selector(".widget-row").unwrap().unwrap())
                .unwrap();
            [
                target.x + target.width / 2.0,
                target
                    .y
                    .max(viewport.y + 2.0)
                    .min(viewport.y + viewport.height - 2.0),
            ]
        };
        axis(&mut session, widget_point, [0.0, 240.0]);
        session.resolve();
        let after_widget = offset(&session);
        assert!(
            (after_widget - after_canvas).abs() > f64::EPSILON,
            "axis over a widget row must reach navigator scroll"
        );

        let inspector_offset = |session: &VisualDebugSession| {
            let inner = session.document.inner.borrow();
            let node = inner.query_selector(".inspector-scroll").unwrap().unwrap();
            inner.get_node(node).unwrap().scroll_offset().y
        };
        let inspector_point = {
            let inner = session.document.inner.borrow();
            let target = inner
                .get_client_bounding_rect(
                    inner
                        .query_selector(".editor-number-field")
                        .unwrap()
                        .unwrap(),
                )
                .unwrap();
            [
                target.x + target.width / 2.0,
                target.y + target.height / 2.0,
            ]
        };
        let inspector_before = inspector_offset(&session);
        axis(&mut session, inspector_point, [0.0, -240.0]);
        session.resolve();
        assert!(
            (inspector_offset(&session) - inspector_before).abs() > f64::EPSILON,
            "axis over an Inspector field must reach its scroll ancestor"
        );
    }

    #[test]
    fn blitz_adapter_preserves_browser_interaction_identity_across_empty_vdom_diff() {
        let scenario = VisualDebugScenario {
            canvases: None,
            skin: None,
            logical_size: [1280, 720],
            scale: 1.0,
            canvas_id: Some("wayland-status".into()),
            editing: true,
            selectors: Vec::new(),
            actions: Vec::new(),
        };
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        session.click(".editor-number-field").unwrap();
        let focus_before = session
            .document
            .inner
            .borrow()
            .get_focussed_node_id()
            .expect("number field must be focused");
        let point = {
            let inner = session.document.inner.borrow();
            let target = inner
                .get_client_bounding_rect(inner.query_selector(".widget-row").unwrap().unwrap())
                .unwrap();
            [
                target.x + target.width / 2.0,
                target.y + target.height / 2.0,
            ]
        };

        session
            .pointer
            .dispatch(&mut session.document, point, 0x110, None);
        let hover_before = session
            .document
            .inner
            .borrow()
            .get_hover_node_id()
            .expect("pointer move must establish hover");
        session.resolve();
        let inner = session.document.inner.borrow();

        assert_eq!(
            inner.get_hover_node_id(),
            Some(hover_before),
            "browser keeps hover when a Dioxus render produces no semantic DOM replacement"
        );
        assert_eq!(
            inner.get_focussed_node_id(),
            Some(focus_before),
            "browser keeps keyboard focus when pointer motion does not replace the focused DOM node"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn projection_model_describes_wayland_lifecycle_without_resource_churn() {
        #[derive(Default)]
        struct FakeWayland {
            projection_cache: NativeProjectionCache,
            surfaces: std::collections::BTreeMap<String, String>,
            trees: std::collections::BTreeSet<(String, String)>,
            runtimes: std::collections::BTreeMap<(String, String), scorepeek_overlay_ui::Skin>,
            revisions: std::collections::BTreeMap<String, (u64, u64)>,
            creates: u64,
            unmaps: u64,
            runtime_creates: u64,
            runtime_drops: u64,
        }
        impl FakeWayland {
            #[allow(clippy::too_many_lines)]
            fn apply(&mut self, session: &EditorSession) {
                let display = session
                    .draft
                    .iter()
                    .map(|presentation| {
                        let mut canvas = crate::config::empty_canvas(
                            presentation.id.clone(),
                            crate::runtime::Backend::Wayland,
                        );
                        canvas.apply_presentation(presentation);
                        canvas
                    })
                    .collect::<Vec<_>>();
                let projected = self
                    .projection_cache
                    .resolve(
                        Some(scorepeek_overlay_ui::Skin::CyanSystem),
                        session,
                        &display,
                    )
                    .unwrap();
                let lifecycle = reconcile_worker_lifecycle(
                    self.surfaces
                        .iter()
                        .map(|(id, output)| (id.as_str(), Some(output.as_str()), false)),
                    projected,
                );
                for id in lifecycle.stop_join {
                    let output = self.surfaces.remove(&id).unwrap();
                    self.unmaps += 1;
                    self.revisions.remove(&output);
                    let dropped = self
                        .runtimes
                        .keys()
                        .filter(|(owner, _)| owner == &output)
                        .cloned()
                        .collect::<Vec<_>>();
                    self.runtime_drops += u64::try_from(dropped.len()).unwrap();
                    for key in dropped {
                        self.runtimes.remove(&key);
                        self.trees.remove(&key);
                    }
                }
                for id in lifecycle.start {
                    let canvas = projected
                        .iter()
                        .find(|canvas| canvas.id == id)
                        .expect("fake adapter receives production lifecycle IDs only");
                    self.surfaces.insert(id, canvas.output.clone());
                    self.creates += 1;
                }
                let outputs = if session.editing {
                    session.outputs.clone()
                } else {
                    session
                        .draft
                        .iter()
                        .filter_map(|canvas| canvas.output.as_ref())
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter()
                        .map(|name| EditorOutput {
                            name: name.clone(),
                            model: "fake".into(),
                            logical_size: None,
                        })
                        .collect()
                };
                for output in outputs {
                    let revision = (session.session_id, session.revision);
                    if self
                        .revisions
                        .get(&output.name)
                        .is_some_and(|old| *old >= revision)
                    {
                        continue;
                    }
                    self.revisions.insert(output.name.clone(), revision);
                    let live = session
                        .stage_projection(&output)
                        .canvases
                        .into_iter()
                        .map(|canvas| ((output.name.clone(), canvas.id), canvas.skin))
                        .collect::<std::collections::BTreeMap<_, _>>();
                    let removed = self
                        .runtimes
                        .keys()
                        .filter(|key| key.0 == output.name && !live.contains_key(*key))
                        .cloned()
                        .collect::<Vec<_>>();
                    for key in removed {
                        self.runtimes.remove(&key);
                        self.trees.remove(&key);
                        self.runtime_drops += 1;
                    }
                    for (key, skin) in live {
                        match self.runtimes.insert(key.clone(), skin) {
                            None => self.runtime_creates += 1,
                            Some(previous) if previous != skin => {
                                self.runtime_drops += 1;
                                self.runtime_creates += 1;
                            }
                            Some(_) => {}
                        }
                        self.trees.insert(key);
                    }
                }
            }
        }

        let mut canvases = crate::config::visual_debug_config()
            .canvases
            .into_iter()
            .filter(|canvas| canvas.backend == crate::runtime::Backend::Wayland)
            .take(2)
            .map(|canvas| canvas.presentation())
            .collect::<Vec<_>>();
        canvases[0].output = Some("WL-1".into());
        canvases[1].output = Some("WL-2".into());
        let mut session = EditorSession::new(canvases, [1920, 1080], "fake-wayland");
        session.set_session_id(11);
        session.set_outputs(vec![
            EditorOutput {
                name: "WL-1".into(),
                model: "fake one".into(),
                logical_size: Some([1920, 1080]),
            },
            EditorOutput {
                name: "WL-2".into(),
                model: "fake two".into(),
                logical_size: Some([1280, 720]),
            },
        ]);
        session.editing = true;
        session.readonly = false;
        session.selected_canvas = Some(session.draft[0].id.clone());
        session.advance_revision();
        let mut fake = FakeWayland::default();
        fake.apply(&session);
        assert_eq!(
            (fake.surfaces.len(), fake.runtimes.len(), fake.trees.len()),
            (2, 2, 2)
        );
        let steady = (
            fake.projection_cache.rebuilds,
            fake.creates,
            fake.unmaps,
            fake.runtime_creates,
            fake.runtime_drops,
        );
        for _ in 0..120 {
            fake.apply(&session);
        }
        assert_eq!(
            (
                fake.projection_cache.rebuilds,
                fake.creates,
                fake.unmaps,
                fake.runtime_creates,
                fake.runtime_drops,
            ),
            steady,
            "120 steady frames must not rebuild projections, surfaces, or heavyweight runtimes"
        );

        session.draft[1].show_on = Some(vec![scorepeek_overlay_ui::ScreenKind::Result]);
        session.advance_revision();
        fake.apply(&session);
        assert_eq!(fake.runtimes.len(), 1, "hidden canvases must be unmounted");
        session.reduce(EditorInput::Action(EditorAction::PreviewScreen(
            scorepeek_overlay_ui::ScreenKind::Result,
        )));
        fake.apply(&session);
        assert_eq!(fake.runtimes.len(), 2, "visible canvases must be remounted");

        session.reduce(EditorInput::Action(EditorAction::Output("WL-2".into())));
        fake.apply(&session);
        assert_eq!(
            fake.surfaces.len(),
            2,
            "moving a canvas must not duplicate editor stages"
        );
        assert!(fake.runtimes.keys().all(|(output, _)| output == "WL-2"));

        let creates = fake.runtime_creates;
        let drops = fake.runtime_drops;
        session.draft[0].skin = scorepeek_overlay_ui::Skin::ResultAurora;
        session.advance_revision();
        fake.apply(&session);
        assert_eq!(fake.runtime_creates, creates + 1);
        assert_eq!(fake.runtime_drops, drops + 1);
        assert_eq!(
            fake.runtimes.len(),
            2,
            "a skin switch replaces only its owner"
        );

        session.reduce(EditorInput::Action(EditorAction::DeleteCanvas));
        fake.apply(&session);
        assert_eq!(fake.runtimes.len(), 1);
        assert_eq!(fake.trees, fake.runtimes.keys().cloned().collect(),);

        session.reduce(EditorInput::SetOutputs(vec![EditorOutput {
            name: "WL-2".into(),
            model: "fake two".into(),
            logical_size: Some([1280, 720]),
        }]));
        fake.apply(&session);
        assert_eq!(
            fake.surfaces.values().collect::<Vec<_>>(),
            vec![&"WL-2".to_owned()]
        );
        assert!(fake.unmaps >= 1);

        session.editing = false;
        session.advance_revision();
        fake.apply(&session);
        assert!(
            fake.surfaces
                .keys()
                .all(|id| !id.starts_with("__scorepeek-editor-stage"))
        );
        assert_eq!(fake.trees, fake.runtimes.keys().cloned().collect());
        assert_eq!(
            fake.runtime_creates - fake.runtime_drops,
            fake.runtimes.len() as u64
        );

        let display_creates = fake.creates;
        session.editing = true;
        session.advance_revision();
        fake.apply(&session);
        assert_eq!(
            fake.surfaces.len(),
            session.outputs.len(),
            "reopening creates exactly one editor stage for each current output"
        );
        assert!(fake.creates > display_creates);
        assert_eq!(fake.trees, fake.runtimes.keys().cloned().collect());
    }

    #[test]
    #[allow(clippy::items_after_statements, clippy::too_many_lines)]
    fn fake_wayland_adapter_drives_production_stage_and_skin_lifecycle() {
        struct TestSkinStore(std::path::PathBuf);
        impl Drop for TestSkinStore {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        static NEXT_STORE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

        #[allow(clippy::struct_excessive_bools)]
        struct FakeStage {
            output: String,
            projection: Reactive<NativeDocumentProjection>,
            document: DioxusDocument,
            coordinator: std::sync::mpsc::Sender<CoordinatorCommand>,
            commands: std::sync::mpsc::Receiver<CoordinatorCommand>,
            previews: std::collections::BTreeMap<String, EditorSkinPreview>,
            assets: Arc<SkinAssetCache>,
            report: Rc<RefCell<RunReport>>,
            skin_updates: EditorSkinUpdates,
            runtime_creates: u64,
            projection_accepts: u64,
            dioxus_polls: u64,
            reconciliations: u64,
            input_generations: u64,
            wasm_calls: u64,
            tree_updates: u64,
            work: FrameWorkProfile,
            layouts: u64,
            resource_resolves: u64,
            scenes: u64,
            presents: u64,
            commits: u64,
            motion_seconds: f64,
            elapsed: Duration,
            full_layout_pending: bool,
            pending_paint: bool,
            animating: bool,
            cadence: FrameCadence,
            paint_count: u64,
            physical_size: [u32; 2],
            scale: f32,
            pointer: PointerInput,
            text_composing: bool,
            renderer: anyrender_vello::VelloImageRenderer,
            pending_frame_start: Option<FrameWorkSample>,
            next_skin_render: Option<Instant>,
        }
        impl FakeStage {
            fn new(stage: StageProjection, assets: Arc<SkinAssetCache>) -> Result<Self, String> {
                let output = stage.output.name.clone();
                let initial = NativeDocumentProjection::Editor(stage);
                let published = Rc::new(RefCell::new(None));
                let (sender, commands) = std::sync::mpsc::channel();
                let props = NativeOverlayProps {
                    initial,
                    published: Rc::clone(&published),
                    port: NativeEditorPort {
                        coordinator: sender.clone(),
                        source_output: Some(output.clone()),
                        run_id: "fake-wayland-stage".into(),
                        sequence: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                    },
                };
                let mut document = DioxusDocument::new(
                    VirtualDom::new_with_props(native_overlay, props),
                    document_config_inner_with_handle(Arc::clone(&assets)).0,
                );
                document.initial_build();
                let projection = published
                    .borrow()
                    .as_ref()
                    .copied()
                    .ok_or("fake stage did not publish its projection")?;
                let mut skin_updates = EditorSkinUpdates::default();
                skin_updates.request();
                let stage = Self {
                    output,
                    projection,
                    document,
                    coordinator: sender,
                    commands,
                    previews: std::collections::BTreeMap::new(),
                    assets,
                    report: Rc::new(RefCell::new(RunReport::new())),
                    skin_updates,
                    runtime_creates: 0,
                    projection_accepts: 0,
                    dioxus_polls: 0,
                    reconciliations: 0,
                    input_generations: 0,
                    wasm_calls: 0,
                    tree_updates: 0,
                    work: FrameWorkProfile::default(),
                    layouts: 0,
                    resource_resolves: 0,
                    scenes: 0,
                    presents: 0,
                    commits: 0,
                    motion_seconds: 0.0,
                    elapsed: Duration::ZERO,
                    full_layout_pending: true,
                    pending_paint: true,
                    animating: false,
                    cadence: FrameCadence::default(),
                    paint_count: 0,
                    physical_size: [1, 1],
                    scale: 1.0,
                    pointer: PointerInput::default(),
                    text_composing: false,
                    renderer: anyrender_vello::VelloImageRenderer::new(1920, 1080),
                    pending_frame_start: None,
                    next_skin_render: None,
                };
                Ok(stage)
            }

            fn accept(&mut self, next: &StageProjection) {
                let frame_start = self.work.snapshot();
                let rebuild_started = Instant::now();
                let previews_changed = editor_skin_previews_changed(&self.previews, next);
                let accepted = accept_stage_projection_replica(
                    self.projection,
                    &mut self.document,
                    &mut self.previews,
                    &self.assets,
                    &self.output,
                    next,
                );
                if accepted {
                    if self.pending_frame_start.is_none() {
                        self.pending_frame_start = Some(frame_start);
                    }
                    self.work
                        .record("projection_rebuild", rebuild_started.elapsed());
                    if previews_changed {
                        self.skin_updates.request();
                    }
                    self.projection_accepts += 1;
                    self.pending_paint = true;
                }
            }

            fn frame(&mut self) -> Result<(), String> {
                self.turn(
                    NativeEventOutcome {
                        frame: true,
                        ..NativeEventOutcome::default()
                    },
                    120,
                )
            }

            fn turn(&mut self, outcome: NativeEventOutcome, hz: u32) -> Result<(), String> {
                let seconds = 1.0 / f64::from(hz);
                self.elapsed += Duration::from_secs_f64(seconds);
                if outcome.frame {
                    self.dioxus_polls += 1;
                    self.motion_seconds += seconds;
                }
                struct FakePresenter<'a> {
                    scenes: &'a mut u64,
                    presents: &'a mut u64,
                    commits: &'a mut u64,
                    text_input_active: &'a mut bool,
                }
                impl NativeFramePresenter for FakePresenter<'_> {
                    fn is_active(&self) -> bool {
                        true
                    }

                    fn set_text_input(
                        &mut self,
                        input: Option<scorepeek_overlay_handles::TextInputState>,
                    ) {
                        *self.text_input_active = input.is_some();
                    }

                    fn request_frame_commit(&mut self) {
                        *self.commits = self.commits.saturating_add(1);
                    }

                    fn present(
                        &mut self,
                        document: &mut blitz_dom::BaseDocument,
                        scale: f64,
                        width: u32,
                        height: u32,
                        work: &mut FrameWorkProfile,
                    ) -> Result<(), String> {
                        let mut scene = anyrender::Scene::new();
                        work.measure("scene", || {
                            paint_native_scene(&mut scene, document, scale, width, height);
                        });
                        work.measure("gpu_present", || {
                            *self.presents = self.presents.saturating_add(1);
                        });
                        work.measure("surface_commit", || {
                            *self.commits = self.commits.saturating_add(1);
                        });
                        *self.scenes = self.scenes.saturating_add(1);
                        Ok(())
                    }
                }
                let refresh = scorepeek_overlay_ui::WaylandRefreshRate::capped(
                    u16::try_from(hz).map_err(|error| error.to_string())?,
                )?;
                let frame_start = if outcome.frame {
                    self.pending_frame_start
                        .take()
                        .unwrap_or_else(|| self.work.snapshot())
                } else {
                    self.work.snapshot()
                };
                let mut text_input_active = false;
                let mut presenter = FakePresenter {
                    scenes: &mut self.scenes,
                    presents: &mut self.presents,
                    commits: &mut self.commits,
                    text_input_active: &mut text_input_active,
                };
                let result = run_native_editor_stage_turn(
                    &mut self.document,
                    self.projection,
                    &mut self.previews,
                    &self.assets,
                    &self.report,
                    &self.output,
                    &scorepeek_overlay_ui::editor_sample_state(),
                    &mut self.skin_updates,
                    &mut self.runtime_creates,
                    &mut self.next_skin_render,
                    &mut self.animating,
                    &mut self.full_layout_pending,
                    &mut self.pending_paint,
                    &mut self.cadence,
                    &mut self.work,
                    Waker::noop(),
                    &frame_start,
                    NativeEditorStageTurnInput {
                        frame: if outcome.frame {
                            NativeFrameBoundary::Frame
                        } else {
                            NativeFrameBoundary::Deferred
                        },
                        surface: if outcome.configured {
                            NativeSurfaceReadiness::Configured
                        } else {
                            NativeSurfaceReadiness::Pending
                        },
                        projection_changed: false,
                        input_damage: outcome.input_damage,
                        now: self.elapsed,
                        seconds: self.motion_seconds,
                        refresh,
                    },
                    &mut presenter,
                )?;
                if result.reconciled {
                    self.reconciliations = self.reconciliations.saturating_add(1);
                }
                self.input_generations = self
                    .input_generations
                    .saturating_add(result.reconciliation.input_generations);
                self.wasm_calls = self
                    .wasm_calls
                    .saturating_add(result.reconciliation.wasm_calls);
                self.tree_updates = self
                    .tree_updates
                    .saturating_add(result.reconciliation.tree_updates);
                if result.paint.is_some() {
                    self.paint_count = self.paint_count.saturating_add(1);
                    self.layouts = self.layouts.saturating_add(1);
                    self.resource_resolves = self.resource_resolves.saturating_add(1);
                }
                if !outcome.frame && self.pending_paint && self.pending_frame_start.is_none() {
                    self.pending_frame_start = Some(frame_start);
                }
                Ok(())
            }

            fn drain_commands(&mut self) -> Vec<CoordinatorCommand> {
                std::iter::from_fn(|| self.commands.try_recv().ok()).collect()
            }

            fn render_pixels(&mut self) -> Vec<u8> {
                let mut pixels = Vec::new();
                let mut inner = self.document.inner.borrow_mut();
                resolve_with_loaded_resources(&mut inner, self.motion_seconds);
                self.renderer.render_to_vec(
                    |scene| paint_native_scene(scene, &mut inner, 1.0, 1920, 1080),
                    &mut pixels,
                );
                pixels
            }

            fn shutdown(mut self, operations: &mut Vec<String>) {
                let phases = RefCell::new(Vec::new());
                shutdown_native_surface(
                    &mut self,
                    |stage| {
                        for (id, mut preview) in std::mem::take(&mut stage.previews) {
                            preview.tree.unmount(&mut stage.document.inner.borrow_mut());
                            stage.assets.release_editor_owner(&id, &stage.output);
                            phases
                                .borrow_mut()
                                .push(format!("runtime-drop:{id}:{}", stage.output));
                        }
                    },
                    |stage| {
                        phases
                            .borrow_mut()
                            .push(format!("suspend:{}", stage.output));
                    },
                    |stage| {
                        phases.borrow_mut().push(format!("unmap:{}", stage.output));
                        Ok::<(), ()>(())
                    },
                )
                .unwrap();
                operations.extend(phases.into_inner());
                operations.push(format!("join:{}", self.output));
            }
        }

        impl NativeEventConsumer for FakeStage {
            fn configure_event(
                &mut self,
                logical: [u32; 2],
                physical: [u32; 2],
                scale_120: u32,
            ) -> Result<bool, String> {
                let scale =
                    f32::from(u16::try_from(scale_120).map_err(|error| error.to_string())?) / 120.0;
                self.document.inner.borrow_mut().set_viewport(Viewport::new(
                    physical[0],
                    physical[1],
                    scale,
                    ColorScheme::Dark,
                ));
                self.physical_size = physical;
                self.scale = scale;
                self.pending_paint = true;
                let _ = self.coordinator.send(CoordinatorCommand::EditorInput {
                    input: EditorInput::Resize {
                        output: self.output.clone(),
                        logical_size: logical,
                    },
                    correlation: None,
                });
                Ok(logical != [0, 0])
            }

            fn pointer_motion_event(&mut self, point: [f64; 2]) {
                self.pointer
                    .dispatch(&mut self.document, point, 0x110, None);
            }

            fn pointer_button_event(&mut self, button: u32, pressed: bool, point: [f64; 2]) {
                self.pointer
                    .dispatch(&mut self.document, point, button, Some(pressed));
            }

            fn pointer_scroll_event(&mut self, delta: [f64; 2], point: [f64; 2]) {
                self.pointer.wheel(&mut self.document, point, delta);
            }

            fn text_event(&mut self, command: &scorepeek_overlay_handles::TextCommand) {
                text::dispatch_control_key(&mut self.document, command, self.text_composing);
            }

            fn ime_event(&mut self, update: scorepeek_overlay_handles::TextUpdate) {
                if let Some(composing) =
                    text::dispatch_control_composition(&mut self.document, update)
                {
                    self.text_composing = composing;
                }
            }

            fn keyboard_focus_event(&mut self, focused: bool) {
                if !focused {
                    self.text_composing = false;
                }
            }
        }

        #[allow(clippy::struct_excessive_bools)]
        struct FakeDisplay {
            canvas_id: String,
            live_widgets: u64,
            document: DioxusDocument,
            commands: std::sync::mpsc::Receiver<CoordinatorCommand>,
            pointer: PointerInput,
            text_composing: bool,
            skin: NativeDisplaySkin,
            assets: Arc<SkinAssetCache>,
            work: FrameWorkProfile,
            full_layout_pending: bool,
            pending_paint: bool,
            animating: bool,
            cadence: FrameCadence,
            elapsed: Duration,
            motion_seconds: f64,
            renderer: anyrender_vello::VelloImageRenderer,
            presents: u64,
            commits: u64,
            paint_count: u64,
        }

        impl FakeDisplay {
            fn new(
                canvas: &crate::config::Canvas,
                assets: Arc<SkinAssetCache>,
            ) -> Result<Self, String> {
                let published = Rc::new(RefCell::new(None));
                let (sender, commands) = std::sync::mpsc::channel();
                let props = NativeOverlayProps {
                    initial: NativeDocumentProjection::Display {
                        canvas: canvas.presentation(),
                        visible: true,
                    },
                    published,
                    port: NativeEditorPort {
                        coordinator: sender,
                        source_output: Some(canvas.output.clone()),
                        run_id: "fake-wayland-display".into(),
                        sequence: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                    },
                };
                let mut document = DioxusDocument::new(
                    VirtualDom::new_with_props(native_overlay, props),
                    document_config_inner_with_handle(Arc::clone(&assets)).0,
                );
                document.initial_build();
                while poll_native_document(&mut document, Waker::noop()) {}
                let package = assets.load(canvas.skin.name())?;
                let report = Rc::new(RefCell::new(RunReport::new()));
                let (skin, _) = create_native_display_skin(
                    &mut document,
                    canvas,
                    &package,
                    &report,
                    Some(&canvas.output),
                    &scorepeek_overlay_ui::editor_sample_state(),
                )?;
                Ok(Self {
                    canvas_id: canvas.id.clone(),
                    live_widgets: u64::try_from(canvas.widgets.len()).unwrap_or(u64::MAX),
                    document,
                    commands,
                    pointer: PointerInput::default(),
                    text_composing: false,
                    skin,
                    assets,
                    work: FrameWorkProfile::default(),
                    full_layout_pending: true,
                    pending_paint: true,
                    animating: false,
                    cadence: FrameCadence::default(),
                    elapsed: Duration::ZERO,
                    motion_seconds: 0.0,
                    renderer: anyrender_vello::VelloImageRenderer::new(canvas.width, canvas.height),
                    presents: 0,
                    commits: 0,
                    paint_count: 0,
                })
            }

            fn turn(&mut self, outcome: NativeEventOutcome, hz: u32) -> Result<(), String> {
                let seconds = 1.0 / f64::from(hz);
                self.elapsed += Duration::from_secs_f64(seconds);
                if outcome.frame {
                    self.motion_seconds += seconds;
                }
                struct FakeDisplayPresenter<'a> {
                    renderer: &'a mut anyrender_vello::VelloImageRenderer,
                    presents: &'a mut u64,
                    commits: &'a mut u64,
                }
                impl NativeFramePresenter for FakeDisplayPresenter<'_> {
                    fn is_active(&self) -> bool {
                        true
                    }

                    fn set_text_input(
                        &mut self,
                        input: Option<scorepeek_overlay_handles::TextInputState>,
                    ) {
                        assert!(
                            input.is_none(),
                            "display surfaces never activate text input"
                        );
                    }

                    fn request_frame_commit(&mut self) {
                        *self.commits = self.commits.saturating_add(1);
                    }

                    fn present(
                        &mut self,
                        document: &mut blitz_dom::BaseDocument,
                        scale: f64,
                        width: u32,
                        height: u32,
                        work: &mut FrameWorkProfile,
                    ) -> Result<(), String> {
                        let mut pixels = Vec::new();
                        let present_started = Instant::now();
                        let mut scene_elapsed = Duration::ZERO;
                        self.renderer.render_to_vec(
                            |scene| {
                                let scene_started = Instant::now();
                                paint_native_scene(scene, document, scale, width, height);
                                scene_elapsed += scene_started.elapsed();
                            },
                            &mut pixels,
                        );
                        work.record("scene", scene_elapsed);
                        work.record(
                            "gpu_present",
                            present_started.elapsed().saturating_sub(scene_elapsed),
                        );
                        if pixels.is_empty() {
                            return Err("fake display paint produced no pixels".into());
                        }
                        *self.presents = self.presents.saturating_add(1);
                        work.measure("surface_commit", || {
                            *self.commits = self.commits.saturating_add(1);
                        });
                        Ok(())
                    }
                }
                let refresh = scorepeek_overlay_ui::WaylandRefreshRate::capped(
                    u16::try_from(hz).map_err(|error| error.to_string())?,
                )?;
                let frame_start = self.work.snapshot();
                let mut presenter = FakeDisplayPresenter {
                    renderer: &mut self.renderer,
                    presents: &mut self.presents,
                    commits: &mut self.commits,
                };
                let result = run_native_display_turn(
                    &mut self.document,
                    &self.assets,
                    &mut self.full_layout_pending,
                    &mut self.pending_paint,
                    &mut self.animating,
                    &mut self.cadence,
                    &mut self.work,
                    Waker::noop(),
                    &frame_start,
                    NativeDisplayTurnInput {
                        frame: if outcome.frame {
                            NativeFrameBoundary::Frame
                        } else {
                            NativeFrameBoundary::Deferred
                        },
                        surface: if outcome.configured {
                            NativeSurfaceReadiness::Configured
                        } else {
                            NativeSurfaceReadiness::Pending
                        },
                        projection_changed: false,
                        visibility_changed: false,
                        input_damage: outcome.input_damage,
                        visible: true,
                        live_widgets: self.live_widgets,
                        now: self.elapsed,
                        seconds: self.motion_seconds,
                        refresh,
                    },
                    &mut presenter,
                )?;
                if result.paint.is_some() {
                    self.paint_count = self.paint_count.saturating_add(1);
                }
                Ok(())
            }

            fn drain_commands(&mut self) -> Vec<CoordinatorCommand> {
                std::iter::from_fn(|| self.commands.try_recv().ok()).collect()
            }

            fn shutdown(mut self, operations: &mut Vec<String>) {
                self.skin
                    .tree
                    .unmount(&mut self.document.inner.borrow_mut());
                operations.push(format!("runtime-drop:{}", self.canvas_id));
                operations.push(format!("unmap:{}", self.canvas_id));
                operations.push(format!("join:{}", self.canvas_id));
            }
        }

        impl NativeEventConsumer for FakeDisplay {
            fn configure_event(
                &mut self,
                _logical: [u32; 2],
                physical: [u32; 2],
                scale_120: u32,
            ) -> Result<bool, String> {
                let scale =
                    f32::from(u16::try_from(scale_120).map_err(|error| error.to_string())?) / 120.0;
                self.document.inner.borrow_mut().set_viewport(Viewport::new(
                    physical[0],
                    physical[1],
                    scale,
                    ColorScheme::Dark,
                ));
                Ok(true)
            }

            fn pointer_motion_event(&mut self, point: [f64; 2]) {
                self.pointer
                    .dispatch(&mut self.document, point, 0x110, None);
            }

            fn pointer_button_event(&mut self, button: u32, pressed: bool, point: [f64; 2]) {
                self.pointer
                    .dispatch(&mut self.document, point, button, Some(pressed));
            }

            fn pointer_scroll_event(&mut self, delta: [f64; 2], point: [f64; 2]) {
                self.pointer.wheel(&mut self.document, point, delta);
            }

            fn text_event(&mut self, command: &scorepeek_overlay_handles::TextCommand) {
                text::dispatch_control_key(&mut self.document, command, self.text_composing);
            }

            fn ime_event(&mut self, update: scorepeek_overlay_handles::TextUpdate) {
                if let Some(composing) =
                    text::dispatch_control_composition(&mut self.document, update)
                {
                    self.text_composing = composing;
                }
            }

            fn keyboard_focus_event(&mut self, focused: bool) {
                if !focused {
                    self.text_composing = false;
                }
            }
        }

        struct FakeAdapter {
            projection_cache: NativeProjectionCache,
            published: Arc<std::sync::Mutex<PublishedStages>>,
            surfaces: std::collections::BTreeMap<String, String>,
            stages: std::collections::BTreeMap<String, FakeStage>,
            displays: std::collections::BTreeMap<String, FakeDisplay>,
            assets: Arc<SkinAssetCache>,
            operations: Vec<String>,
            config_conversions: u64,
            work: FrameWorkProfile,
            transported_effects: Vec<EditorEffectKind>,
            transported_inputs: Vec<String>,
        }
        impl FakeAdapter {
            fn new(
                published: Arc<std::sync::Mutex<PublishedStages>>,
                assets: Arc<SkinAssetCache>,
            ) -> Self {
                Self {
                    projection_cache: NativeProjectionCache::default(),
                    published,
                    surfaces: std::collections::BTreeMap::new(),
                    stages: std::collections::BTreeMap::new(),
                    displays: std::collections::BTreeMap::new(),
                    assets,
                    operations: Vec::new(),
                    config_conversions: 0,
                    work: FrameWorkProfile::default(),
                    transported_effects: Vec::new(),
                    transported_inputs: Vec::new(),
                }
            }

            fn drain_stage_transport(&mut self, authority: &mut NativeEditorAuthority) {
                let commands = self
                    .stages
                    .values_mut()
                    .flat_map(FakeStage::drain_commands)
                    .chain(
                        self.displays
                            .values_mut()
                            .flat_map(FakeDisplay::drain_commands),
                    )
                    .collect::<Vec<_>>();
                for command in commands {
                    let effects = match command {
                        CoordinatorCommand::EditorInput { input, .. } => {
                            self.transported_inputs.push(format!("{input:?}"));
                            authority.dispatch(input)
                        }
                        CoordinatorCommand::Open {
                            output,
                            canvas,
                            preview_screen,
                        } => authority.dispatch(EditorInput::Open {
                            output,
                            canvas: Some(canvas),
                            preview: preview_screen
                                .unwrap_or(scorepeek_overlay_ui::ScreenKind::MusicSelect),
                        }),
                        CoordinatorCommand::ResolveOutput { .. } => Vec::new(),
                    };
                    for effect in effects {
                        let kind = effect.kind();
                        self.transported_effects.push(kind);
                        let requested_draft = effect.requested_draft().map(<[_]>::to_vec);
                        let canvases = requested_draft
                            .clone()
                            .unwrap_or_else(|| authority.session().draft.clone());
                        let followups = authority.dispatch(EditorInput::BackendCompleted {
                            effect: kind,
                            requested_draft,
                            reply: EditorBackendReply {
                                ok: true,
                                readonly: false,
                                error: None,
                                canvases,
                                dirty: kind == EditorEffectKind::Update,
                            },
                        });
                        for followup in followups {
                            self.transported_effects.push(followup.kind());
                        }
                    }
                }
            }

            fn apply_at(
                &mut self,
                authority: &mut NativeEditorAuthority,
                refresh_hz: u32,
            ) -> Result<(), String> {
                self.drain_stage_transport(authority);
                authority.poll();
                let session = authority.session();
                let display = if session.editing {
                    Vec::new()
                } else {
                    let started = Instant::now();
                    let canvases = session
                        .draft
                        .iter()
                        .map(|presentation| {
                            let mut canvas = crate::config::empty_canvas(
                                presentation.id.clone(),
                                crate::runtime::Backend::Wayland,
                            );
                            canvas.apply_presentation(presentation);
                            canvas
                        })
                        .collect::<Vec<_>>();
                    self.work.record("canvas_config", started.elapsed());
                    canvases
                };
                let started = Instant::now();
                let projected = self.projection_cache.resolve(
                    Some(scorepeek_overlay_ui::Skin::CyanSystem),
                    &session,
                    &display,
                )?;
                self.work.record("projection", started.elapsed());
                let lifecycle = reconcile_worker_lifecycle(
                    self.surfaces
                        .iter()
                        .map(|(id, output)| (id.as_str(), Some(output.as_str()), false)),
                    projected,
                );
                for id in lifecycle.stop_join {
                    if let Some(output) = self.surfaces.remove(&id) {
                        self.operations.push(format!("stop-object:{id}"));
                        self.operations.push(format!("stop:{output}"));
                        if let Some(mut stage) = self.stages.remove(&output) {
                            let wake = dispatch_native_event(&mut stage, Event::Wake)?;
                            assert!(!wake.closed);
                            self.operations.push(format!("wake:{output}"));
                            let closed = dispatch_native_event(&mut stage, Event::Closed)?;
                            assert!(closed.closed);
                            self.operations.push(format!("close:{output}"));
                            stage.shutdown(&mut self.operations);
                        } else if let Some(display) = self.displays.remove(&id) {
                            display.shutdown(&mut self.operations);
                        }
                    }
                }
                for id in lifecycle.start {
                    let canvas = projected
                        .iter()
                        .find(|canvas| canvas.id == id)
                        .expect("production lifecycle returned an unknown canvas");
                    self.config_conversions += 1;
                    self.operations.push(format!("configure:{}", canvas.output));
                    self.surfaces.insert(id, canvas.output.clone());
                    if !session.editing {
                        let mut display = FakeDisplay::new(canvas, Arc::clone(&self.assets))?;
                        let configured = dispatch_native_event(
                            &mut display,
                            Event::Configure {
                                logical: [canvas.width, canvas.height],
                                physical: [canvas.width, canvas.height],
                                scale_120: 120,
                            },
                        )?;
                        assert!(configured.configured);
                        display.turn(configured, refresh_hz)?;
                        let frame = dispatch_native_event(&mut display, Event::Frame)?;
                        display.turn(frame, refresh_hz)?;
                        self.displays.insert(canvas.id.clone(), display);
                    }
                }
                if session.editing {
                    let published = self
                        .published
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .by_output
                        .clone();
                    for projection in published.values().rev().cloned() {
                        if let Some(stage) = self.stages.get_mut(&projection.output.name) {
                            stage.accept(&projection);
                        } else {
                            let output = projection.output.name.clone();
                            let logical = projection.output.logical_size.unwrap_or([1920, 1080]);
                            let mut stage = FakeStage::new(projection, Arc::clone(&self.assets))?;
                            let configured = dispatch_native_event(
                                &mut stage,
                                Event::Configure {
                                    logical,
                                    physical: logical,
                                    scale_120: 120,
                                },
                            )?;
                            assert!(configured.configured);
                            stage.turn(configured, refresh_hz)?;
                            self.operations.push(format!("surface-create:{output}"));
                            self.operations.push(format!("surface-configure:{output}"));
                            self.stages.insert(output, stage);
                        }
                    }
                    for projection in published.into_values() {
                        let stage = self
                            .stages
                            .get_mut(&projection.output.name)
                            .expect("stage created for every output");
                        stage.accept(&projection);
                        if stage.skin_updates.pending || stage.pending_paint || stage.animating {
                            let outcome = dispatch_native_event(stage, Event::Frame)?;
                            stage.turn(outcome, refresh_hz)?;
                        }
                    }
                }
                Ok(())
            }

            fn apply(&mut self, authority: &mut NativeEditorAuthority) -> Result<(), String> {
                self.apply_at(authority, 120)
            }

            fn runtime_creates(&self) -> u64 {
                self.stages
                    .values()
                    .map(|stage| stage.runtime_creates)
                    .sum()
            }

            fn output_event(
                &mut self,
                authority: &mut NativeEditorAuthority,
                outputs: &[OutputDescription],
            ) -> Result<(), String> {
                authority.dispatch(EditorInput::SetOutputs(editor_outputs_from_descriptions(
                    outputs,
                )));
                self.operations.push("output-discovery".into());
                self.apply(authority)
            }

            fn event(&mut self, output: &str, event: Event) -> Result<NativeEventOutcome, String> {
                let stage = self
                    .stages
                    .get_mut(output)
                    .ok_or_else(|| format!("fake event targets missing output {output}"))?;
                dispatch_native_event(stage, event)
            }

            fn display_event(
                &mut self,
                canvas: &str,
                event: Event,
            ) -> Result<NativeEventOutcome, String> {
                let display = self
                    .displays
                    .get_mut(canvas)
                    .ok_or_else(|| format!("fake event targets missing canvas {canvas}"))?;
                dispatch_native_event(display, event)
            }

            #[allow(clippy::cast_possible_truncation)]
            fn stage_point(&self, output: &str, selector: &str) -> Result<[f64; 2], String> {
                let stage = self
                    .stages
                    .get(output)
                    .ok_or_else(|| format!("missing stage {output}"))?;
                let inner = stage.document.inner.borrow();
                let node = inner
                    .query_selector(selector)
                    .map_err(|error| format!("invalid selector {selector}: {error:?}"))?
                    .ok_or_else(|| format!("selector did not match: {selector}"))?;
                let rect = inner
                    .get_client_bounding_rect(node)
                    .ok_or_else(|| format!("selector has no layout: {selector}"))?;
                let point = [rect.x + rect.width / 2.0, rect.y + rect.height / 2.0];
                let mut hit = inner.element_from_point(point[0] as f32, point[1] as f32);
                let mut reaches_target = false;
                while let Some(hit_node) = hit {
                    if hit_node == node {
                        reaches_target = true;
                        break;
                    }
                    hit = inner.get_node(hit_node).and_then(|item| item.parent);
                }
                if !reaches_target {
                    let obscurer = inner
                        .element_from_point(point[0] as f32, point[1] as f32)
                        .and_then(|id| {
                            let rect = inner.get_client_bounding_rect(id);
                            inner
                                .get_node(id)
                                .and_then(|item| item.element_data())
                                .map(|element| (element, rect))
                        })
                        .map_or_else(
                            || "non-element".into(),
                            |(element, rect)| {
                                let class = element
                                    .attrs
                                    .iter()
                                    .find(|attribute| attribute.name.local.as_ref() == "class")
                                    .map_or("", |attribute| attribute.value.as_str());
                                let section = element
                                    .attrs
                                    .iter()
                                    .find(|attribute| {
                                        attribute.name.local.as_ref() == "data-section"
                                    })
                                    .map_or("", |attribute| attribute.value.as_str());
                                format!("{}.{class}[{section}] {rect:?}", element.name.local)
                            },
                        );
                    return Err(format!(
                        "selector center is obscured: {selector} at {},{} by {obscurer}",
                        point[0], point[1],
                    ));
                }
                Ok(point)
            }

            #[allow(clippy::cast_possible_truncation)]
            fn stage_descendant_point(
                &self,
                output: &str,
                selector: &str,
            ) -> Result<[f64; 2], String> {
                let stage = self
                    .stages
                    .get(output)
                    .ok_or_else(|| format!("missing stage {output}"))?;
                let inner = stage.document.inner.borrow();
                let node = inner
                    .query_selector(selector)
                    .map_err(|error| format!("invalid selector {selector}: {error:?}"))?
                    .ok_or_else(|| format!("selector did not match: {selector}"))?;
                let [width, height] = stage.physical_size;
                for y_step in 1..20 {
                    for x_step in 1..20 {
                        let point = [
                            f64::from(width * x_step / 20),
                            f64::from(height * y_step / 20),
                        ];
                        let mut hit = inner.element_from_point(point[0] as f32, point[1] as f32);
                        while let Some(hit_node) = hit {
                            if hit_node == node {
                                return Ok(point);
                            }
                            hit = inner.get_node(hit_node).and_then(|item| item.parent);
                        }
                    }
                }
                Err(format!("selector has no visible descendant: {selector}"))
            }

            fn click_stage(
                &mut self,
                authority: &mut NativeEditorAuthority,
                output: &str,
                selector: &str,
            ) -> Result<(), String> {
                let [x, y] = self
                    .stage_point(output, selector)
                    .or_else(|_| self.stage_descendant_point(output, selector))?;
                for event in [
                    Event::PointerMotion { x, y },
                    Event::PointerButton {
                        button: 0x110,
                        pressed: true,
                        x,
                        y,
                    },
                    Event::PointerButton {
                        button: 0x110,
                        pressed: false,
                        x,
                        y,
                    },
                ] {
                    self.event(output, event)?;
                    self.apply(authority)?;
                }
                self.frame(authority, 120)
            }

            fn drag_stage(
                &mut self,
                authority: &mut NativeEditorAuthority,
                output: &str,
                selector: &str,
                delta: [f64; 2],
            ) -> Result<(), String> {
                let [x, y] = self.stage_point(output, selector)?;
                self.event(output, Event::PointerMotion { x, y })?;
                self.event(
                    output,
                    Event::PointerButton {
                        button: 0x110,
                        pressed: true,
                        x,
                        y,
                    },
                )?;
                self.apply(authority)?;
                for step in 1..=4 {
                    let fraction = f64::from(step) / 4.0;
                    self.event(
                        output,
                        Event::PointerMotion {
                            x: x + delta[0] * fraction,
                            y: y + delta[1] * fraction,
                        },
                    )?;
                    self.apply(authority)?;
                    self.frame(authority, 120)?;
                }
                self.event(
                    output,
                    Event::PointerButton {
                        button: 0x110,
                        pressed: false,
                        x: x + delta[0],
                        y: y + delta[1],
                    },
                )?;
                self.apply(authority)?;
                self.frame(authority, 120)
            }

            fn scroll_stage(
                &mut self,
                authority: &mut NativeEditorAuthority,
                output: &str,
                selector: &str,
                dy: f64,
            ) -> Result<(), String> {
                let [x, y] = self.stage_descendant_point(output, selector)?;
                self.event(output, Event::PointerScroll { dx: 0.0, dy, x, y })?;
                self.apply(authority)?;
                self.frame(authority, 120)
            }

            fn frame(
                &mut self,
                authority: &mut NativeEditorAuthority,
                hz: u32,
            ) -> Result<(), String> {
                for stage in self.stages.values_mut() {
                    if stage.pending_frame_start.is_none() {
                        stage.pending_frame_start = Some(stage.work.snapshot());
                    }
                }
                self.apply_at(authority, hz)
            }
        }

        fn assert_converged(fake: &FakeAdapter, authority: &NativeEditorAuthority) {
            let session = authority.session();
            let expected_surfaces = fake
                .projection_cache
                .canvases
                .iter()
                .map(|canvas| (canvas.id.clone(), canvas.output.clone()))
                .collect::<std::collections::BTreeMap<_, _>>();
            assert_eq!(
                fake.surfaces, expected_surfaces,
                "fake surfaces must exactly equal the production lifecycle projection"
            );
            let mut paint_targets = fake
                .displays
                .iter()
                .filter(|(_, display)| display.presents > 0)
                .map(|(id, _)| id.clone())
                .collect::<std::collections::BTreeSet<_>>();
            for (output, stage) in &fake.stages {
                if stage.presents > 0
                    && let Some((id, _)) = fake
                        .surfaces
                        .iter()
                        .find(|(_, surface_output)| *surface_output == output)
                {
                    paint_targets.insert(id.clone());
                }
            }
            assert_eq!(
                paint_targets,
                fake.surfaces.keys().cloned().collect(),
                "production paint admission and fake presentation must exactly cover live surfaces"
            );

            let owners = fake
                .assets
                .editor_owners
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if !session.editing {
                assert!(
                    fake.stages.is_empty(),
                    "closed editor must have no stage replica"
                );
                assert!(
                    owners.is_empty(),
                    "closed editor must retain no canvas owner"
                );
                assert_eq!(
                    fake.displays
                        .keys()
                        .cloned()
                        .collect::<std::collections::BTreeSet<_>>(),
                    fake.surfaces.keys().cloned().collect(),
                    "display skin runtime/tree set must exactly equal live display surfaces"
                );
                for (id, display) in &fake.displays {
                    assert_eq!(&display.canvas_id, id);
                    assert!(
                        display.paint_count > 0,
                        "every live display must be painted"
                    );
                    assert!(display.presents > 0, "every live display must be presented");
                    assert!(display.commits > 0, "every live display must be committed");
                    assert!(
                        display
                            .document
                            .inner
                            .borrow()
                            .query_selector("#scorepeek-skin-root > *")
                            .unwrap()
                            .is_some(),
                        "every live display must retain a mounted production skin tree"
                    );
                    assert!(
                        !display.skin.release.is_empty(),
                        "every live display must retain its production skin runtime metadata"
                    );
                }
                return;
            }
            assert!(
                fake.displays.is_empty(),
                "editing mode must not retain display skin runtimes"
            );

            let expected_stages = session
                .outputs
                .iter()
                .map(|output| (output.name.clone(), session.stage_projection(output)))
                .collect::<std::collections::BTreeMap<_, _>>();
            let published = fake
                .published
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .by_output
                .clone();
            assert!(
                published == expected_stages,
                "published projections must exactly equal authority-derived projections"
            );
            assert_eq!(
                fake.stages
                    .keys()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>(),
                expected_stages
                    .keys()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>(),
                "one replica stage must exist for every current output and no other output"
            );

            let mut expected_owners = std::collections::BTreeMap::new();
            for (output, expected) in expected_stages {
                let stage = fake.stages.get(&output).expect("expected stage exists");
                match &*stage.projection.borrow() {
                    NativeDocumentProjection::Editor(actual) => assert!(
                        actual == &expected,
                        "stage replica must atomically accept the complete published projection"
                    ),
                    NativeDocumentProjection::Display { .. } => {
                        panic!("editor stage cannot retain a display projection")
                    }
                }
                let expected_canvases = expected
                    .canvases
                    .iter()
                    .map(|canvas| canvas.id.clone())
                    .collect::<std::collections::BTreeSet<_>>();
                assert_eq!(
                    stage
                        .previews
                        .keys()
                        .cloned()
                        .collect::<std::collections::BTreeSet<_>>(),
                    expected_canvases,
                    "mounted skin runtimes must exactly equal visible projected canvases"
                );
                let inner = stage.document.inner.borrow();
                assert_eq!(
                    inner
                        .query_selector_all(".scorepeek-skin-scope .overlay-canvas")
                        .unwrap()
                        .len(),
                    expected.canvases.len(),
                    "mounted skin DOM roots must exactly equal visible projected canvases"
                );
                assert_eq!(
                    inner
                        .query_selector_all(".editor-widget-hit")
                        .unwrap()
                        .len(),
                    if expected.interactive {
                        expected
                            .canvases
                            .iter()
                            .map(|canvas| canvas.widgets.len())
                            .sum::<usize>()
                    } else {
                        0
                    },
                    "hit regions must exactly equal projected widgets"
                );
                for canvas in &expected.canvases {
                    assert!(
                        inner
                            .query_selector(&format!(
                                ".scorepeek-skin-scope .overlay-canvas[data-canvas-id='{}']",
                                canvas.id
                            ))
                            .unwrap()
                            .is_some(),
                        "every projected canvas must own one skin DOM root"
                    );
                    assert!(
                        inner
                            .query_selector(&format!(".editor-canvas[data-canvas='{}']", canvas.id))
                            .unwrap()
                            .is_some(),
                        "every projected canvas must own one editor hit root"
                    );
                    expected_owners.insert(canvas.id.clone(), output.clone());
                    if expected.interactive {
                        for widget in &canvas.widgets {
                            assert!(
                                inner
                                    .query_selector(&format!(
                                        ".editor-canvas[data-canvas='{}'] .editor-widget-hit[data-widget='{}']",
                                        canvas.id, widget.id
                                    ))
                                    .unwrap()
                                    .is_some(),
                                "every interactive projected widget must own one matching hit region"
                            );
                        }
                    }
                }
            }
            assert_eq!(
                owners, expected_owners,
                "canvas leases must exactly equal mounted stage/runtime ownership"
            );
        }

        let mut canvases = crate::config::visual_debug_config()
            .canvases
            .into_iter()
            .filter(|canvas| canvas.backend == crate::runtime::Backend::Wayland)
            .take(2)
            .map(|canvas| canvas.presentation())
            .collect::<Vec<_>>();
        canvases[0].output = Some("WL-1".into());
        canvases[1].output = Some("WL-2".into());
        canvases[0].background = scorepeek_overlay_ui::Background::Static;
        canvases[0].x = 0;
        canvases[0].y = 0;
        canvases[0].width = canvases[0].width.min(1_200);
        canvases[0].height = canvases[0].height.min(700);
        let mut animated_widget = canvases[1].widgets[0].clone();
        animated_widget.x = 600;
        animated_widget.y = 160;
        animated_widget.width = animated_widget.width.min(480);
        animated_widget.height = animated_widget.height.min(200);
        let mut unrelated_widget = animated_widget.clone();
        unrelated_widget.id = "selection-unrelated".into();
        unrelated_widget.y = 420;
        canvases[1].widgets = vec![animated_widget, unrelated_widget];
        canvases[1].background = scorepeek_overlay_ui::Background::Animated;
        canvases[1].width = 1_200;
        canvases[1].height = 700;
        canvases[1].x = 0;
        canvases[1].y = 0;
        let mut session = EditorSession::new(canvases, [1920, 1080], "fake-wayland");
        session.set_session_id(41);
        session.set_skins(embedded_editor_skins());
        let initial_outputs = vec![
            OutputDescription {
                name: "WL-1".into(),
                model: "fake one".into(),
                logical_size: Some([1920, 1080]),
            },
            OutputDescription {
                name: "WL-2".into(),
                model: "fake two".into(),
                logical_size: Some([1280, 720]),
            },
        ];
        session.set_outputs(editor_outputs_from_descriptions(&initial_outputs));
        session.readonly = false;
        session.reduce(EditorInput::Open {
            output: Some("WL-1".into()),
            canvas: Some(session.draft[0].id.clone()),
            preview: scorepeek_overlay_ui::ScreenKind::MusicSelect,
        });
        let published = Arc::new(std::sync::Mutex::new(PublishedStages::default()));
        let mut authority = NativeEditorAuthority::new(session, Arc::clone(&published));
        let acquired = authority.session().draft.clone();
        authority.dispatch(EditorInput::BackendCompleted {
            effect: EditorEffectKind::Acquire,
            requested_draft: None,
            reply: EditorBackendReply {
                ok: true,
                readonly: false,
                error: None,
                canvases: acquired,
                dirty: false,
            },
        });
        let store_path = std::env::temp_dir().join(format!(
            "scorepeek-fake-wayland-skins-{}-{}",
            std::process::id(),
            NEXT_STORE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let store_guard = TestSkinStore(store_path.clone());
        let store = crate::skin::StoreRoot::new(store_path);
        let package_root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/skins");
        for package in ["cyan-system.zip", "result-aurora.zip", "dj-blackbox.zip"] {
            store
                .install(&package_root.join(package))
                .unwrap_or_else(|error| panic!("install test skin {package}: {error}"));
        }
        let assets = Arc::new(SkinAssetCache::new(store));
        let mut cold_load_work = FrameWorkProfile::default();
        let cold_start = cold_load_work.snapshot();
        assets
            .load_profiled(
                scorepeek_overlay_ui::Skin::CyanSystem.name(),
                &mut cold_load_work,
            )
            .unwrap();
        cold_load_work.finish_frame(&cold_start, 0, 0);
        let cold_load = cold_load_work.frames.back().unwrap();
        assert_eq!(cold_load.phases["package_open"].calls, 1);
        assert_eq!(cold_load.phases["package_clone"].calls, 1);
        assert_eq!(
            cold_load.phases["package_open"].total_ns,
            assets.open_ns.load(std::sync::atomic::Ordering::Relaxed)
        );
        assert_eq!(
            cold_load.phases["package_clone"].total_ns,
            assets.clone_ns.load(std::sync::atomic::Ordering::Relaxed)
        );
        let mut fake = FakeAdapter::new(published, assets);
        fake.apply(&mut authority).unwrap();
        let configured = fake
            .event(
                "WL-1",
                Event::Configure {
                    logical: [2_000, 1_200],
                    physical: [2_000, 1_200],
                    scale_120: 120,
                },
            )
            .unwrap();
        assert!(configured.configured);
        fake.apply(&mut authority).unwrap();
        assert_eq!(
            authority
                .session()
                .outputs
                .iter()
                .find(|output| output.name == "WL-1")
                .and_then(|output| output.logical_size),
            Some([2_000, 1_200]),
            "fake configure must traverse the production resize transport into Dioxus authority"
        );
        let configured = fake
            .event(
                "WL-1",
                Event::Configure {
                    logical: [1_920, 1_080],
                    physical: [1_920, 1_080],
                    scale_120: 120,
                },
            )
            .unwrap();
        assert!(configured.configured);
        fake.apply(&mut authority).unwrap();
        for event in [
            Event::PointerMotion { x: 8.0, y: 8.0 },
            Event::PointerButton {
                button: 0x110,
                pressed: true,
                x: 8.0,
                y: 8.0,
            },
            Event::PointerButton {
                button: 0x110,
                pressed: false,
                x: 8.0,
                y: 8.0,
            },
            Event::PointerScroll {
                dx: 0.0,
                dy: -20.0,
                x: 8.0,
                y: 8.0,
            },
            Event::Text(scorepeek_overlay_handles::TextCommand::Cancel),
            Event::Ime(scorepeek_overlay_handles::TextUpdate::default()),
            Event::KeyboardFocus(false),
            Event::Wake,
        ] {
            let outcome = fake.event("WL-1", event).unwrap();
            assert!(!outcome.closed);
        }
        fake.scroll_stage(&mut authority, "WL-1", ".navigator-scroll", -800.0)
            .unwrap();
        fake.click_stage(
            &mut authority,
            "WL-1",
            ".workspace-output-option[data-output='WL-2'] > .navigator-item-line > .navigator-item-select",
        )
        .unwrap();
        assert_eq!(authority.session().active_output.as_deref(), Some("WL-2"));
        fake.scroll_stage(&mut authority, "WL-2", ".navigator-scroll", -800.0)
            .unwrap();
        fake.click_stage(
            &mut authority,
            "WL-2",
            ".canvas-select[data-canvas-id='wayland-selection'] > .navigator-item-line > .navigator-item-select",
        )
        .unwrap();
        let (drag_output, dragged_canvas, dragged_widget, before_drag) = {
            let session = authority.session();
            let canvas = session.current().expect("opened canvas remains selected");
            let widget = canvas
                .widgets
                .iter()
                .find(|widget| {
                    let horizontal_center = canvas
                        .x
                        .saturating_add(widget.x)
                        .saturating_add(i32::try_from(widget.width / 2).unwrap_or(i32::MAX));
                    let center = canvas
                        .y
                        .saturating_add(widget.y)
                        .saturating_add(i32::try_from(widget.height / 2).unwrap_or(i32::MAX));
                    horizontal_center > 500 && (100..680).contains(&center)
                })
                .expect("fixture has a draggable widget inside the logical viewport");
            (
                session.active_output.clone().unwrap(),
                canvas.id.clone(),
                widget.id.clone(),
                [widget.x, widget.y],
            )
        };
        let revision_before_drag = authority.session().revision;
        fake.drag_stage(
            &mut authority,
            &drag_output,
            &format!(
                ".editor-canvas[data-canvas='{dragged_canvas}'] .editor-widget-hit[data-widget='{dragged_widget}']"
            ),
            [64.0, 40.0],
        )
        .unwrap();
        assert!(
            authority.session().revision >= revision_before_drag + 4,
            "transported inputs: {:?}",
            fake.transported_inputs
        );
        let after_drag = {
            let session = authority.session();
            let widget = session
                .draft
                .iter()
                .find(|canvas| canvas.id == dragged_canvas)
                .unwrap()
                .widgets
                .iter()
                .find(|widget| widget.id == dragged_widget)
                .unwrap();
            [widget.x, widget.y]
        };
        assert_eq!(after_drag, [before_drag[0] + 64, before_drag[1] + 40]);
        assert!(
            fake.transported_effects.contains(&EditorEffectKind::Update),
            "protocol pointer events must traverse Dioxus and coordinator transport to authority; inputs={:?}",
            fake.transported_inputs
        );
        authority.dispatch(EditorInput::Action(EditorAction::SelectOutput(
            "WL-1".into(),
        )));
        authority.dispatch(EditorInput::Action(EditorAction::SelectCanvas(
            "wayland-status".into(),
        )));
        fake.apply(&mut authority).unwrap();
        assert_eq!(authority.session().active_output.as_deref(), Some("WL-1"));
        assert_converged(&fake, &authority);
        assert_eq!((fake.surfaces.len(), fake.stages.len()), (2, 2));
        let package_clones_before = fake
            .assets
            .clone_count
            .load(std::sync::atomic::Ordering::Relaxed);
        let wl1 = fake.stages.get_mut("WL-1").unwrap();
        wl1.frame().unwrap();
        let measured_package_clones = wl1
            .work
            .frames
            .back()
            .expect("a frame callback records one workload sample")
            .phases["package_clone"]
            .calls;
        let package_clones_after = fake
            .assets
            .clone_count
            .load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            measured_package_clones,
            package_clones_after.saturating_sub(package_clones_before),
            "NetProvider package clones must be attributed to the frame that fetched the resource"
        );
        let pixels = wl1.render_pixels();
        let background_offset = (40 * 1920 + 400) * 4;
        let background_pixel = pixels[background_offset..background_offset + 4].to_vec();
        assert_eq!(
            background_pixel[3], 255,
            "the production native tree/resource/layout/scene path must paint the skin background"
        );
        assert_ne!(
            &background_pixel[..3],
            &[14, 25, 37],
            "the native pixel oracle must see package artwork, not only its fallback color"
        );
        assert_eq!(
            fake.stages
                .values()
                .map(|stage| stage.previews.len())
                .sum::<usize>(),
            2
        );
        let heavy = (
            fake.projection_cache.rebuilds,
            fake.config_conversions,
            fake.runtime_creates(),
            fake.assets
                .open_count
                .load(std::sync::atomic::Ordering::Relaxed),
            fake.assets
                .resource_lookup_count
                .load(std::sync::atomic::Ordering::Relaxed),
            fake.stages
                .values()
                .map(|stage| stage.reconciliations)
                .sum::<u64>(),
            fake.stages
                .values()
                .map(|stage| stage.input_generations)
                .sum::<u64>(),
        );
        let work_calls = |fake: &FakeAdapter, phase: &'static str| {
            fake.work.calls(phase)
                + fake
                    .stages
                    .values()
                    .map(|stage| stage.work.calls(phase))
                    .sum::<u64>()
        };
        let retained_work = [
            "package_clone",
            "wasm_runtime_create",
            "skin_input",
            "wasm_render",
            "json_tree",
            "tree_reconciliation",
            "skin_reconciliation",
        ]
        .map(|phase| work_calls(&fake, phase));
        let frame_counts = |fake: &FakeAdapter| {
            fake.stages
                .values()
                .map(|stage| {
                    (
                        stage.dioxus_polls,
                        stage.wasm_calls,
                        stage.tree_updates,
                        stage.layouts,
                        stage.resource_resolves,
                        stage.scenes,
                        stage.presents,
                        stage.commits,
                    )
                })
                .fold((0, 0, 0, 0, 0, 0, 0, 0), |a, b| {
                    (
                        a.0 + b.0,
                        a.1 + b.1,
                        a.2 + b.2,
                        a.3 + b.3,
                        a.4 + b.4,
                        a.5 + b.5,
                        a.6 + b.6,
                        a.7 + b.7,
                    )
                })
        };
        for hz in [60_u32, 120] {
            fake.apply(&mut authority).unwrap();
            let sample_counts = fake
                .stages
                .iter()
                .map(|(output, stage)| (output.clone(), stage.work.frames.len()))
                .collect::<std::collections::BTreeMap<_, _>>();
            let frame_before = frame_counts(&fake);
            let projection_before = work_calls(&fake, "projection");
            let config_before = work_calls(&fake, "canvas_config");
            let work_before = [
                "dioxus_poll",
                "blitz_layout",
                "resource_decode",
                "scene",
                "gpu_present",
                "surface_commit",
            ]
            .map(|phase| work_calls(&fake, phase));
            for _ in 0..hz {
                fake.frame(&mut authority, hz).unwrap();
            }
            assert_eq!(
                (
                    fake.projection_cache.rebuilds,
                    fake.config_conversions,
                    fake.runtime_creates(),
                    fake.assets
                        .open_count
                        .load(std::sync::atomic::Ordering::Relaxed),
                    fake.assets
                        .resource_lookup_count
                        .load(std::sync::atomic::Ordering::Relaxed),
                    fake.stages
                        .values()
                        .map(|stage| stage.reconciliations)
                        .sum::<u64>(),
                    fake.stages
                        .values()
                        .map(|stage| stage.input_generations)
                        .sum::<u64>(),
                ),
                heavy,
                "steady frames must poll and paint without rebuilding projection/config/runtime"
            );
            assert_eq!(
                [
                    "package_clone",
                    "wasm_runtime_create",
                    "skin_input",
                    "wasm_render",
                    "json_tree",
                    "tree_reconciliation",
                    "skin_reconciliation",
                ]
                .map(|phase| work_calls(&fake, phase)),
                retained_work,
                "steady production turns must not reach retained skin reconstruction phases"
            );
            let frame_after = frame_counts(&fake);
            let expected = u64::from(hz) * u64::try_from(fake.stages.len()).unwrap();
            assert_eq!(
                work_calls(&fake, "projection") - projection_before,
                u64::from(hz)
            );
            assert_eq!(work_calls(&fake, "canvas_config"), config_before);
            let work_after = [
                "dioxus_poll",
                "blitz_layout",
                "resource_decode",
                "scene",
                "gpu_present",
                "surface_commit",
            ]
            .map(|phase| work_calls(&fake, phase));
            assert!(
                (expected..=expected + u64::try_from(fake.stages.len()).unwrap())
                    .contains(&(work_after[0] - work_before[0]))
            );
            assert_eq!(work_after[1] - work_before[1], expected * 2);
            for index in 2..work_after.len() {
                assert_eq!(work_after[index] - work_before[index], expected);
            }
            assert!(
                (expected..=expected + u64::try_from(fake.stages.len()).unwrap())
                    .contains(&(frame_after.0 - frame_before.0))
            );
            assert_eq!(
                (
                    frame_after.1 - frame_before.1,
                    frame_after.2 - frame_before.2
                ),
                (0, 0),
                "CSS animation frames must not rerun idle Wasm or rebuild its JSON tree"
            );
            assert_eq!(frame_after.3 - frame_before.3, expected);
            assert_eq!(frame_after.4 - frame_before.4, expected);
            assert_eq!(frame_after.5 - frame_before.5, expected);
            assert_eq!(frame_after.6 - frame_before.6, expected);
            assert!(
                (expected..=expected + u64::try_from(fake.stages.len()).unwrap())
                    .contains(&(frame_after.7 - frame_before.7))
            );
            for (output, stage) in &fake.stages {
                let before = sample_counts[output];
                let samples = stage.work.frames.iter().skip(before).collect::<Vec<_>>();
                assert_eq!(samples.len(), usize::try_from(hz).unwrap());
                let expected_live_canvases =
                    u64::try_from(stage.previews.len()).unwrap_or(u64::MAX);
                let expected_live_widgets =
                    stage.previews.values().fold(0_u64, |count, preview| {
                        count.saturating_add(
                            u64::try_from(preview.canvas.widgets.len()).unwrap_or(u64::MAX),
                        )
                    });
                for sample in samples {
                    assert!(
                        FrameWorkProfile::REQUIRED_PHASES
                            .iter()
                            .all(|phase| sample.phases.contains_key(phase)),
                        "every frame must represent every required phase as measured work, zero work, or unmeasured"
                    );
                    for phase in [
                        "projection_rebuild",
                        "canvas_config",
                        "package_open",
                        "package_clone",
                        "wasm_runtime_create",
                        "skin_input",
                        "wasm_render",
                        "json_tree",
                        "tree_reconciliation",
                    ] {
                        assert_eq!(sample.phases[phase].calls, 0, "unexpected {phase} work");
                    }
                    assert_eq!(sample.phases["dioxus_poll"].calls, 1);
                    assert_eq!(sample.phases["blitz_layout"].calls, 2);
                    assert_eq!(sample.phases["scene"].calls, 1);
                    assert_eq!(sample.phases["gpu_present"].calls, 1);
                    assert_eq!(sample.phases["surface_commit"].calls, 1);
                    assert_eq!(sample.live_canvases, expected_live_canvases);
                    assert_eq!(sample.live_widgets, expected_live_widgets);
                }
            }
        }

        authority.dispatch(EditorInput::Action(EditorAction::CanvasVisibleNone));
        fake.apply(&mut authority).unwrap();
        assert!(
            fake.stages.values().any(|stage| {
                stage
                    .work
                    .frames
                    .back()
                    .is_some_and(|sample| sample.phases["projection_rebuild"].calls > 0)
            }),
            "a projection accepted on a deferred turn must remain attributed to the frame that presents it"
        );
        assert_converged(&fake, &authority);
        let pixels_after_unmount = fake.stages.get_mut("WL-1").unwrap().render_pixels();
        assert_ne!(
            pixels_after_unmount[background_offset..background_offset + 4],
            background_pixel,
            "the retained renderer must not preserve pixels from an unmounted canvas"
        );
        assert_eq!(
            fake.stages
                .values()
                .map(|stage| stage.previews.len())
                .sum::<usize>(),
            1,
            "visibility removal must unmount the selected canvas preview"
        );
        authority.dispatch(EditorInput::Action(EditorAction::CanvasVisibleAll));
        fake.apply(&mut authority).unwrap();
        assert_converged(&fake, &authority);
        assert_eq!(
            fake.stages
                .values()
                .map(|stage| stage.previews.len())
                .sum::<usize>(),
            2
        );

        let runtime_creates = fake.runtime_creates();
        let replacement_skin = if authority.session().current().unwrap().skin
            == scorepeek_overlay_ui::Skin::ResultAurora
        {
            scorepeek_overlay_ui::Skin::CyanSystem
        } else {
            scorepeek_overlay_ui::Skin::ResultAurora
        };
        authority.dispatch(EditorInput::Action(EditorAction::Skin(replacement_skin)));
        fake.apply(&mut authority).unwrap();
        assert_converged(&fake, &authority);
        assert_eq!(
            fake.runtime_creates(),
            runtime_creates + 1,
            "skin replacement must create exactly one replacement runtime"
        );

        let _deleted_canvas = authority.session().selected_canvas.clone().unwrap();
        for _ in 0..8 {
            fake.scroll_stage(&mut authority, "WL-1", ".inspector-scroll", -2_000.0)
                .unwrap();
        }
        fake.click_stage(&mut authority, "WL-1", ".delete-canvas")
            .unwrap();
        assert_converged(&fake, &authority);
        assert_eq!(
            fake.stages
                .values()
                .map(|stage| stage.previews.len())
                .sum::<usize>(),
            1
        );
        assert_eq!(
            fake.assets
                .editor_owners
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1,
            "canvas deletion must release its runtime owner"
        );
        for _ in 0..8 {
            authority.dispatch(EditorInput::Action(EditorAction::AddCanvas));
            fake.apply(&mut authority).unwrap();
            authority.dispatch(EditorInput::Action(EditorAction::DeleteCanvas));
            fake.apply(&mut authority).unwrap();
        }
        assert_converged(&fake, &authority);
        let deleted_history_sample_sequences = fake
            .stages
            .iter()
            .map(|(output, stage)| {
                (
                    output.clone(),
                    stage.work.frames.back().map_or(0, |sample| sample.sequence),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let work_before_delete_frame = fake
            .stages
            .values()
            .map(|stage| {
                (
                    stage.input_generations,
                    stage.wasm_calls,
                    stage.tree_updates,
                )
            })
            .fold((0, 0, 0), |sum, count| {
                (sum.0 + count.0, sum.1 + count.1, sum.2 + count.2)
            });
        for _ in 0..60 {
            fake.frame(&mut authority, 60).unwrap();
        }
        let work_after_delete_frame = fake
            .stages
            .values()
            .map(|stage| {
                (
                    stage.input_generations,
                    stage.wasm_calls,
                    stage.tree_updates,
                )
            })
            .fold((0, 0, 0), |sum, count| {
                (sum.0 + count.0, sum.1 + count.1, sum.2 + count.2)
            });
        assert_eq!(
            (
                work_after_delete_frame.0 - work_before_delete_frame.0,
                work_after_delete_frame.1 - work_before_delete_frame.1,
                work_after_delete_frame.2 - work_before_delete_frame.2,
            ),
            (0, 0, 0),
            "CSS animation after deletion must not revisit deleted or unchanged live skin runtimes"
        );
        for (output, stage) in &fake.stages {
            let expected_live_canvases = u64::try_from(stage.previews.len()).unwrap_or(u64::MAX);
            let expected_live_widgets = stage.previews.values().fold(0_u64, |count, preview| {
                count
                    .saturating_add(u64::try_from(preview.canvas.widgets.len()).unwrap_or(u64::MAX))
            });
            let samples = stage
                .work
                .frames
                .iter()
                .filter(|sample| sample.sequence > deleted_history_sample_sequences[output])
                .collect::<Vec<_>>();
            assert_eq!(samples.len(), 60);
            for sample in samples {
                assert_eq!(sample.live_canvases, expected_live_canvases);
                assert_eq!(sample.live_widgets, expected_live_widgets);
                assert_eq!(sample.phases["skin_input"].calls, 0);
                assert_eq!(sample.phases["wasm_render"].calls, 0);
                assert_eq!(sample.phases["json_tree"].calls, 0);
                assert_eq!(sample.phases["tree_reconciliation"].calls, 0);
            }
        }
        fake.click_stage(&mut authority, "WL-1", ".add-canvas")
            .unwrap();
        assert_converged(&fake, &authority);
        assert_eq!(
            fake.stages
                .values()
                .map(|stage| stage.previews.len())
                .sum::<usize>(),
            2
        );

        let stale_wl2 = fake
            .published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .by_output
            .get("WL-2")
            .cloned()
            .unwrap();
        let _moved_canvas = authority.session().selected_canvas.clone().unwrap();
        for _ in 0..8 {
            fake.scroll_stage(&mut authority, "WL-1", ".inspector-scroll", -2_000.0)
                .unwrap();
        }
        fake.click_stage(&mut authority, "WL-1", ".output-option[data-output='WL-2']")
            .unwrap();
        assert_converged(&fake, &authority);
        assert_eq!(
            fake.stages
                .values()
                .map(|stage| stage.previews.len())
                .sum::<usize>(),
            2
        );
        assert_eq!(
            fake.assets
                .editor_owners
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            2,
            "reverse delivery still leaves one owner per canvas"
        );
        assert_eq!(fake.stages["WL-2"].previews.len(), 2);
        assert!(
            fake.stages["WL-2"].previews["wayland-selection"]
                .canvas
                .widgets
                .len()
                > 1,
            "the animated workload must include multiple widgets"
        );
        let wl2 = fake.stages.get_mut("WL-2").unwrap();
        let accepted = wl2.projection_accepts;
        let current_revision = match &*wl2.projection.borrow() {
            NativeDocumentProjection::Editor(stage) => stage.revision,
            NativeDocumentProjection::Display { .. } => unreachable!(),
        };
        wl2.accept(&stale_wl2);
        assert_eq!(wl2.projection_accepts, accepted);
        assert_eq!(
            match &*wl2.projection.borrow() {
                NativeDocumentProjection::Editor(stage) => stage.revision,
                NativeDocumentProjection::Display { .. } => unreachable!(),
            },
            current_revision,
            "an out-of-order transport replica must not replace the current stage"
        );

        fake.output_event(
            &mut authority,
            &[OutputDescription {
                name: "WL-2".into(),
                model: "fake two".into(),
                logical_size: Some([1280, 720]),
            }],
        )
        .unwrap();
        assert_converged(&fake, &authority);
        assert_eq!(fake.surfaces.len(), 1);
        let stop = fake
            .operations
            .iter()
            .position(|op| op == "stop:WL-1")
            .unwrap();
        let suspend = fake
            .operations
            .iter()
            .position(|op| op == "suspend:WL-1")
            .unwrap();
        let unmap = fake
            .operations
            .iter()
            .position(|op| op == "unmap:WL-1")
            .unwrap();
        let join = fake
            .operations
            .iter()
            .position(|op| op == "join:WL-1")
            .unwrap();
        assert!(stop < suspend && suspend < unmap && unmap < join);
        assert!(
            fake.operations
                .iter()
                .enumerate()
                .filter(|(_, op)| op.starts_with("runtime-drop:") && op.ends_with(":WL-1"))
                .all(|(index, _)| index < suspend),
            "all retained skin resources must drop before renderer suspension"
        );

        fake.click_stage(&mut authority, "WL-2", ".discard-action")
            .unwrap();
        assert!(
            fake.transported_effects
                .contains(&EditorEffectKind::Discard)
        );
        assert_converged(&fake, &authority);
        assert!(fake.stages.is_empty());
        let closed_display_ids = fake
            .displays
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert!(!closed_display_ids.is_empty());
        for display in fake.displays.values() {
            let sample = display
                .work
                .frames
                .back()
                .expect("production display turn must retain its frame sample");
            assert_eq!(sample.live_canvases, 1);
            assert_eq!(sample.live_widgets, display.live_widgets);
            assert!(sample.phases["scene"].calls > 0);
            assert!(sample.phases["gpu_present"].calls > 0);
            assert!(sample.phases["surface_commit"].calls > 0);
            assert!(
                FrameWorkProfile::REQUIRED_PHASES
                    .iter()
                    .all(|phase| sample.phases.contains_key(phase))
            );
        }
        let reopen_canvas = authority
            .session()
            .draft
            .first()
            .map(|canvas| canvas.id.clone())
            .unwrap();
        for event in [
            Event::PointerMotion { x: 100.0, y: 100.0 },
            Event::PointerButton {
                button: 0x111,
                pressed: true,
                x: 100.0,
                y: 100.0,
            },
            Event::PointerButton {
                button: 0x111,
                pressed: false,
                x: 100.0,
                y: 100.0,
            },
        ] {
            fake.display_event(&reopen_canvas, event).unwrap();
            fake.apply(&mut authority).unwrap();
        }
        fake.apply(&mut authority).unwrap();
        assert_converged(&fake, &authority);
        assert_eq!(fake.stages.len(), 1);
        let operation_ids = |prefix: &str| {
            fake.operations
                .iter()
                .filter_map(|operation| operation.strip_prefix(prefix).map(str::to_owned))
                .filter(|id| closed_display_ids.contains(id))
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert_eq!(operation_ids("stop-object:"), closed_display_ids);
        assert_eq!(operation_ids("runtime-drop:"), closed_display_ids);
        assert_eq!(operation_ids("unmap:"), closed_display_ids);
        assert_eq!(operation_ids("join:"), closed_display_ids);
        drop(fake);
        drop(store_guard);
    }

    #[test]
    fn canvas_position_does_not_invalidate_skin_but_content_geometry_does() {
        let mut before = crate::config::visual_debug_config()
            .canvases
            .into_iter()
            .find(|canvas| canvas.backend == crate::runtime::Backend::Wayland)
            .expect("visual debug config must contain a Wayland canvas")
            .presentation();
        let mut after = before.clone();
        after.x += 40;
        after.y += 24;
        assert!(!editor_skin_presentation_changed(&before, &after));

        after.width += 4;
        assert!(editor_skin_presentation_changed(&before, &after));

        after = before.clone();
        before
            .widgets
            .first_mut()
            .expect("visual debug canvas must contain a widget")
            .x += 4;
        assert!(editor_skin_presentation_changed(&after, &before));
    }

    #[test]
    fn retained_skin_tree_can_restore_live_css_after_preview() {
        let scenario: VisualDebugScenario =
            serde_json::from_str(include_str!("../tests/fixtures/visual-composition.json"))
                .unwrap();
        let session = VisualDebugSession::new(&scenario, [1920, 1080]).unwrap();
        let root = session
            .document
            .inner
            .borrow()
            .query_selector("#scorepeek-skin-root")
            .unwrap()
            .unwrap();
        let output = crate::skin::RenderOutput {
            schedule: crate::skin::Schedule::Idle,
            tree: crate::skin::Node::Element {
                key: "probe".into(),
                tag: "div".into(),
                attributes: std::collections::BTreeMap::from([(
                    "id".into(),
                    "native-css-probe".into(),
                )]),
                children: Vec::new(),
            },
        };
        let mut tree = crate::skin::NativeTree::new(
            &mut session.document.inner.borrow_mut(),
            root,
            "#native-css-probe { display: block; width: 80px; height: 20px; }",
        );
        tree.apply(&mut session.document.inner.borrow_mut(), &output);
        session.document.inner.borrow_mut().resolve(0.0);
        let width = |session: &VisualDebugSession| {
            let inner = session.document.inner.borrow();
            let probe = inner.query_selector("#native-css-probe").unwrap().unwrap();
            inner.get_client_bounding_rect(probe).unwrap().width
        };
        assert!((width(&session) - 80.0).abs() < f64::EPSILON);

        tree.set_css(
            &mut session.document.inner.borrow_mut(),
            "#native-css-probe { display: block; width: 160px; height: 20px; }",
        );
        session.document.inner.borrow_mut().resolve(0.0);
        assert!((width(&session) - 160.0).abs() < f64::EPSILON);
    }

    fn resize_widget(
        widget: &mut WidgetLayout,
        original: &WidgetLayout,
        start: [f64; 2],
        corner: ResizeCorner,
        x: f64,
        y: f64,
        canvas: [u32; 2],
    ) {
        let rect = scorepeek_overlay_ui::editor_model::resize(
            [
                original.x,
                original.y,
                i32::try_from(original.width).unwrap_or(i32::MAX),
                i32::try_from(original.height).unwrap_or(i32::MAX),
            ],
            [snap_i32(x - start[0]), snap_i32(y - start[1])],
            Some(corner.name()),
            canvas,
            [16, 16],
            if original.kind == scorepeek_overlay_ui::WidgetKind::Empty {
                original.settings.aspect_ratio
            } else {
                scorepeek_overlay_ui::AspectRatio::Free
            },
        );
        widget.x = rect[0];
        widget.y = rect[1];
        widget.width = rect[2].unsigned_abs();
        widget.height = rect[3].unsigned_abs();
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ResizeCorner {
        NorthWest,
        NorthEast,
        SouthWest,
        SouthEast,
    }
    impl ResizeCorner {
        fn name(self) -> &'static str {
            match self {
                Self::NorthWest => "nw",
                Self::NorthEast => "ne",
                Self::SouthWest => "sw",
                Self::SouthEast => "se",
            }
        }
    }

    #[test]
    fn pointer_motion_delivers_actual_button_state() {
        use dioxus::html::input_data::MouseButton;
        type Moves = Rc<RefCell<Vec<bool>>>;
        fn probe(moves: Moves) -> Element {
            rsx! { div { style: "position:absolute;inset:0", onpointermove: move |event| moves.borrow_mut().push(event.held_buttons().contains(MouseButton::Primary)), "selectable text" } }
        }
        let moves = Moves::default();
        let mut document = DioxusDocument::new(
            VirtualDom::new_with_props(probe, moves.clone()),
            document_config(),
        );
        document.initial_build();
        document.inner.borrow_mut().resolve(1.0);
        let mut pointer = PointerInput::default();
        pointer.dispatch(&mut document, [10.0, 10.0], 0x110, None);
        pointer.dispatch(&mut document, [10.0, 10.0], 0x110, Some(true));
        pointer.dispatch(&mut document, [11.0, 10.0], 0x110, None);
        pointer.dispatch(&mut document, [11.0, 10.0], 0x110, Some(false));
        pointer.dispatch(&mut document, [12.0, 10.0], 0x110, None);
        assert_eq!(*moves.borrow(), [false, true, false]);
    }

    #[test]
    fn stage_shutdown_is_broadcast_before_any_worker_is_reaped() {
        struct Worker(Arc<std::sync::atomic::AtomicBool>);
        impl WorkerControl for Worker {
            fn request_stop(&self) {
                self.0.store(true, std::sync::atomic::Ordering::Release);
            }
        }
        let workers = (0..3)
            .map(|index| {
                (
                    index.to_string(),
                    Worker(Arc::new(std::sync::atomic::AtomicBool::new(false))),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let wakes = std::sync::Mutex::new(std::collections::BTreeMap::new());
        let ids = workers.keys().cloned().collect::<Vec<_>>();

        stop_workers(ids.iter(), &workers, &wakes);

        assert!(
            workers
                .values()
                .all(|worker| worker.0.load(std::sync::atomic::Ordering::Acquire))
        );
    }

    #[test]
    fn editor_stage_is_removed_when_display_projection_replaces_it() {
        let desired = std::collections::BTreeSet::from(["wayland-canvas".to_owned()]);
        let projected = vec![crate::config::empty_canvas(
            "wayland-canvas".into(),
            crate::runtime::Backend::Wayland,
        )];

        assert!(worker_needs_replacement(
            "__scorepeek-editor-stage-0",
            Some("WL-1"),
            &desired,
            &projected,
            false,
        ));
    }

    #[test]
    fn surface_unmap_failure_has_a_stable_worker_error_type() {
        assert_eq!(
            canvas_worker_error_type("unmap Wayland surface: connection closed"),
            "wayland_surface_unmap_failed"
        );
        assert_eq!(
            canvas_worker_error_type("gpu_adapter"),
            "canvas_worker_failed"
        );
    }

    #[test]
    fn unmap_failure_is_primary_when_the_app_loop_also_failed() {
        let (result, secondary) = native_shutdown_result(
            Err("dispatch Wayland events: connection closed".into()),
            Err("connection closed".into()),
        );
        let primary = result.unwrap_err();

        assert_eq!(
            canvas_worker_error_type(&primary),
            "wayland_surface_unmap_failed"
        );
        assert_eq!(
            secondary.as_deref(),
            Some("dispatch Wayland events: connection closed")
        );
        assert_eq!(
            secondary.as_deref().map(canvas_worker_error_type),
            Some("canvas_worker_failed")
        );
    }

    #[test]
    fn only_active_editor_stage_accepts_input() {
        assert!(surface_input_enabled(true, true, true));
        assert!(!surface_input_enabled(true, false, true));
        assert!(!surface_input_enabled(true, true, false));
        assert!(surface_input_enabled(false, false, true));
    }

    #[test]
    fn frame_cadence_coalesces_steady_paints_and_bypasses_lifecycle_work() {
        let capped = scorepeek_overlay_ui::WaylandRefreshRate::capped(30).unwrap();
        let period = Duration::from_secs_f64(1.0 / 30.0);
        let mut cadence = FrameCadence::default();

        assert!(cadence.permits(Duration::ZERO, capped, PaintReason::Steady));
        cadence.record(Duration::ZERO);
        assert!(!cadence.permits(
            period.checked_sub(Duration::from_nanos(1)).unwrap(),
            capped,
            PaintReason::Steady
        ));
        assert!(cadence.permits(period, capped, PaintReason::Steady));
        assert!(cadence.permits(Duration::from_millis(1), capped, PaintReason::Reconfigure));
        cadence.record(Duration::from_millis(1));
        assert!(!cadence.permits(Duration::from_millis(2), capped, PaintReason::Steady));
        assert!(cadence.permits(
            Duration::from_millis(2),
            scorepeek_overlay_ui::WaylandRefreshRate::Auto,
            PaintReason::Steady,
        ));
        assert!(cadence.permits(
            Duration::from_millis(1),
            scorepeek_overlay_ui::WaylandRefreshRate::Auto,
            PaintReason::Editor,
        ));
        assert!(skin_deadline(&crate::skin::Schedule::NextFrame, false).is_some());
        assert!(skin_deadline(&crate::skin::Schedule::NextFrame, true).is_none());
    }

    #[test]
    fn editor_damage_waits_for_a_frame_and_visible_preview_motion_keeps_painting() {
        let idle_damage = PaintState {
            editing: true,
            visible: true,
            signal: PaintSignal::Damage,
            animating: false,
        };
        assert!(idle_damage.editor_frame_needed());
        assert_eq!(idle_damage.ordinary_reason(), None);
        assert_eq!(
            PaintState {
                signal: PaintSignal::DamageAndFrame,
                ..idle_damage
            }
            .ordinary_reason(),
            Some(PaintReason::Editor)
        );
        assert_eq!(
            PaintState {
                editing: false,
                ..idle_damage
            }
            .ordinary_reason(),
            Some(PaintReason::Steady)
        );
        assert_eq!(
            PaintState {
                signal: PaintSignal::Frame,
                animating: true,
                ..idle_damage
            }
            .ordinary_reason(),
            Some(PaintReason::Steady)
        );
        for state in [
            PaintState {
                signal: PaintSignal::DamageAndFrame,
                ..idle_damage
            },
            PaintState {
                signal: PaintSignal::None,
                ..idle_damage
            },
            PaintState {
                editing: false,
                ..idle_damage
            },
            PaintState {
                visible: false,
                ..idle_damage
            },
        ] {
            assert!(!state.editor_frame_needed());
        }
    }

    #[test]
    fn editor_stages_are_output_owned_when_canvas_assignment_changes() {
        let mut canvases = crate::config::visual_debug_config()
            .canvases
            .into_iter()
            .filter(|canvas| canvas.backend == crate::runtime::Backend::Wayland)
            .collect::<Vec<_>>();
        let outputs = vec![
            OutputDescription {
                name: "WL-1".into(),
                model: "Nested output 1".into(),
                logical_size: Some([1280, 720]),
            },
            OutputDescription {
                name: "WL-2".into(),
                model: "Nested output 2".into(),
                logical_size: Some([1920, 1080]),
            },
        ];
        let before = editor_stage_projections(&canvases, &outputs, canvases[0].skin);
        canvases[0].output = "WL-2".into();
        let after = editor_stage_projections(&canvases, &outputs, canvases[0].skin);

        assert_eq!(before, after);
        assert!(before.iter().all(|stage| stage.x == 0 && stage.y == 0));
        assert_eq!(
            before
                .iter()
                .map(|stage| (stage.id.as_str(), stage.output.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("__scorepeek-editor-stage-0", "WL-1"),
                ("__scorepeek-editor-stage-1", "WL-2"),
            ]
        );
    }

    #[test]
    fn early_compositor_callbacks_reach_later_permitted_motion_paints() {
        let capped = scorepeek_overlay_ui::WaylandRefreshRate::capped(30).unwrap();
        let mut cadence = FrameCadence::default();
        let mut paints = Vec::new();
        for now in [0, 16, 32, 48, 64, 80, 96].map(Duration::from_millis) {
            if cadence.permits(now, capped, PaintReason::Steady) {
                cadence.record(now);
                paints.push(now);
            }
            // A denied production callback is published with request_frame_and_commit(), so the
            // next compositor callback in this sequence remains reachable without a state event.
        }
        assert_eq!(
            paints,
            [
                Duration::ZERO,
                Duration::from_millis(48),
                Duration::from_millis(96)
            ]
        );
    }

    #[test]
    fn refresh_rate_parser_rejects_incomplete_and_out_of_range_drafts() {
        assert_eq!(
            parse_refresh_rate("30").unwrap(),
            scorepeek_overlay_ui::WaylandRefreshRate::capped(30).unwrap()
        );
        for invalid in ["", "0", "1001", "auto", "29.97"] {
            assert!(parse_refresh_rate(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn empty_geometry_uses_viewport_coordinates_for_every_visible_aperture() {
        let mut scenario: VisualDebugScenario =
            serde_json::from_str(include_str!("../tests/fixtures/visual-composition.json"))
                .unwrap();
        scenario.editing = true;
        scenario.actions.clear();
        let session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        let inner = session.document.inner.borrow();
        let geometries = inner.query_selector_all(".empty-geometry").unwrap();
        assert_eq!(geometries.len(), 4);
        let first = inner.get_client_bounding_rect(geometries[0]).unwrap();
        assert_eq!(
            (first.x, first.y, first.width, first.height),
            (40.0, 40.0, 1200.0, 680.0)
        );
    }

    #[test]
    fn visual_debug_surface_contains_every_visible_canvas_in_one_stage_projection() {
        let scenario = VisualDebugScenario {
            canvases: None,
            skin: None,
            logical_size: [1920, 1080],
            scale: 1.0,
            canvas_id: Some("wayland-status".into()),
            editing: true,
            selectors: Vec::new(),
            actions: Vec::new(),
        };
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        session
            .click(".screen-picker .list-picker-trigger")
            .unwrap();
        session
            .focus(".screen-picker .list-picker-trigger")
            .unwrap();
        session
            .click(".screen-picker .list-picker-option[data-index='4']")
            .unwrap();
        session.scroll(".navigator-scroll", 0.0, -2000.0).unwrap();
        session
            .click(".canvas-select[data-canvas-id='wayland-result']")
            .unwrap();

        let (expected_canvases, expected_widgets) = match &*session.projection.borrow() {
            NativeDocumentProjection::Editor(stage) => (
                stage.canvases.len(),
                stage
                    .canvases
                    .iter()
                    .map(|canvas| canvas.widgets.len())
                    .sum::<usize>(),
            ),
            NativeDocumentProjection::Display { .. } => panic!("expected editor projection"),
        };
        let inner = session.document.inner.borrow();
        let canvas_rects = inner
            .query_selector_all(".editor-canvas")
            .unwrap()
            .into_iter()
            .filter_map(|id| inner.get_client_bounding_rect(id))
            .filter(|rect| rect.width > 0.0 && rect.height > 0.0)
            .count();
        let widget_rects = inner
            .query_selector_all(".editor-widget-hit")
            .unwrap()
            .into_iter()
            .filter_map(|id| inner.get_client_bounding_rect(id))
            .filter(|rect| rect.width > 0.0 && rect.height > 0.0)
            .count();
        let rendered_skin_roots = inner
            .query_selector_all(".scorepeek-skin-scope .overlay-canvas")
            .unwrap()
            .len();
        assert_eq!(canvas_rects, expected_canvases);
        assert_eq!(widget_rects, expected_widgets);
        assert_eq!(rendered_skin_roots, expected_canvases);
    }

    #[test]
    fn visual_debug_canvas_delete_drops_runtime_tree_and_dom_together() {
        let scenario = VisualDebugScenario {
            canvases: None,
            skin: None,
            logical_size: [1920, 1080],
            scale: 1.0,
            canvas_id: Some("wayland-status".into()),
            editing: true,
            selectors: Vec::new(),
            actions: Vec::new(),
        };
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        session
            .authority
            .dispatch(EditorInput::Action(EditorAction::DeleteCanvas));
        let output = match &*session.projection.borrow() {
            NativeDocumentProjection::Editor(stage) => stage.output.clone(),
            NativeDocumentProjection::Display { .. } => panic!("expected editor stage"),
        };
        session.projection.set(NativeDocumentProjection::Editor(
            session.authority.session().stage_projection(&output),
        ));
        session.resolve();

        let expected = match &*session.projection.borrow() {
            NativeDocumentProjection::Editor(stage) => stage.canvases.len(),
            NativeDocumentProjection::Display { .. } => unreachable!(),
        };
        assert_eq!(session.skins.len(), expected);
        assert_eq!(
            session
                .document
                .inner
                .borrow()
                .query_selector_all(".scorepeek-skin-scope .overlay-canvas")
                .unwrap()
                .len(),
            expected
        );
    }

    #[test]
    fn visual_debug_new_canvas_mounts_its_skin_on_the_same_reactive_turn() {
        let scenario = VisualDebugScenario {
            canvases: None,
            skin: None,
            logical_size: [1920, 1080],
            scale: 1.0,
            canvas_id: Some("wayland-status".into()),
            editing: true,
            selectors: Vec::new(),
            actions: Vec::new(),
        };
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        loop {
            let id = {
                session
                    .authority
                    .session()
                    .draft
                    .first()
                    .map(|canvas| canvas.id.clone())
            };
            let Some(id) = id else { break };
            session
                .authority
                .dispatch(EditorInput::Action(EditorAction::SelectCanvas(id)));
            session
                .authority
                .dispatch(EditorInput::Action(EditorAction::DeleteCanvas));
        }
        session
            .authority
            .dispatch(EditorInput::Action(EditorAction::AddCanvas));
        let output = session.authority.session().outputs[0].clone();
        session.projection.set(NativeDocumentProjection::Editor(
            session.authority.session().stage_projection(&output),
        ));

        session.resolve();

        assert_eq!(session.skins.len(), 1);
        assert_eq!(
            session
                .document
                .inner
                .borrow()
                .query_selector_all(".scorepeek-skin-scope .overlay-canvas")
                .unwrap()
                .len(),
            1,
            "projection, Dioxus root creation, and skin mount must complete without another input"
        );
    }

    #[test]
    fn visual_debug_role_transition_remounts_skin_content_in_the_display_root() {
        let scenario = VisualDebugScenario {
            canvases: None,
            skin: None,
            logical_size: [1920, 1080],
            scale: 1.0,
            canvas_id: Some("wayland-status".into()),
            editing: true,
            selectors: Vec::new(),
            actions: Vec::new(),
        };
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();

        session.set_editing(false);

        assert_eq!(
            session
                .document
                .inner
                .borrow()
                .query_selector_all("#scorepeek-skin-root .overlay-canvas")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn display_context_menu_enters_through_the_shared_dioxus_surface_action() {
        let scenario = VisualDebugScenario {
            canvases: None,
            skin: None,
            logical_size: [1920, 1080],
            scale: 1.0,
            canvas_id: Some("wayland-status".into()),
            editing: true,
            selectors: Vec::new(),
            actions: Vec::new(),
        };
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        session.set_editing(false);
        while session.commands.try_recv().is_ok() {}
        session
            .pointer
            .dispatch(&mut session.document, [960.0, 540.0], 0x111, None);
        session
            .pointer
            .dispatch(&mut session.document, [960.0, 540.0], 0x111, Some(true));
        session
            .pointer
            .dispatch(&mut session.document, [960.0, 540.0], 0x111, Some(false));
        while poll_native_document(&mut session.document, Waker::noop()) {}

        assert!(
            std::iter::from_fn(|| session.commands.try_recv().ok())
                .any(|command| matches!(command, CoordinatorCommand::Open { .. }))
        );
    }

    #[test]
    fn native_keyboard_edits_the_focused_dioxus_number_field() {
        let scenario = VisualDebugScenario {
            canvases: None,
            skin: None,
            logical_size: [1920, 1080],
            scale: 1.0,
            canvas_id: Some("wayland-status".into()),
            editing: true,
            selectors: Vec::new(),
            actions: Vec::new(),
        };
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        session.click(".editor-number-field").unwrap();
        session.key(&scorepeek_overlay_handles::TextCommand::SelectAll);
        session.key(&scorepeek_overlay_handles::TextCommand::Insert("16".into()));
        session.key(&scorepeek_overlay_handles::TextCommand::Accept);

        assert_eq!(session.authority.session().current().unwrap().x, 16);
        assert!(session.authority.session().chrome.field_drafts.is_empty());
    }

    #[test]
    fn native_keyboard_uses_the_shared_title_input_contract() {
        let scenario = VisualDebugScenario {
            canvases: None,
            skin: None,
            logical_size: [1920, 1080],
            scale: 1.0,
            canvas_id: Some("wayland-status".into()),
            editing: true,
            selectors: Vec::new(),
            actions: Vec::new(),
        };
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        session
            .click(".widget-picker .list-picker-trigger")
            .unwrap();
        session
            .click(".widget-picker .list-picker-option[data-index='5']")
            .unwrap();
        session.click(".empty-title-input").unwrap();
        session.click("#editor-title-input").unwrap();
        session.key(&scorepeek_overlay_handles::TextCommand::Insert(
            "手元".into(),
        ));
        session.key(&scorepeek_overlay_handles::TextCommand::Accept);

        let model = session.authority.session();
        let widget = model
            .current()
            .unwrap()
            .widgets
            .iter()
            .find(|widget| widget.kind == scorepeek_overlay_ui::WidgetKind::Empty)
            .unwrap();
        assert_eq!(widget.settings.title, "手元");
        assert!(model.title.is_none());
    }

    #[test]
    fn native_ime_batch_uses_browser_order_and_shared_composition_state() {
        let scenario = VisualDebugScenario {
            canvases: None,
            skin: None,
            logical_size: [1920, 1080],
            scale: 1.0,
            canvas_id: Some("wayland-status".into()),
            editing: true,
            selectors: Vec::new(),
            actions: Vec::new(),
        };
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        session
            .click(".widget-picker .list-picker-trigger")
            .unwrap();
        session
            .click(".widget-picker .list-picker-option[data-index='5']")
            .unwrap();
        session.click(".empty-title-input").unwrap();
        session.focus("#editor-title-input").unwrap();

        session.ime(scorepeek_overlay_handles::TextUpdate {
            commit: Some("日本".into()),
            ..Default::default()
        });
        assert_eq!(
            session.authority.session().title.as_ref().unwrap().text,
            "日本"
        );

        session.ime(scorepeek_overlay_handles::TextUpdate {
            preedit: Some("ほん".into()),
            preedit_cursor: [6, 6],
            ..Default::default()
        });
        assert!(
            session
                .authority
                .session()
                .title
                .as_ref()
                .unwrap()
                .composing
        );

        session.ime(scorepeek_overlay_handles::TextUpdate {
            commit: Some("語".into()),
            preedit: Some(String::new()),
            delete_before: 3,
            ..Default::default()
        });
        let model = session.authority.session();
        let title = model.title.as_ref().unwrap();
        assert_eq!(title.text, "日語");
        assert!(!title.composing);
    }

    #[test]
    fn native_ime_targets_the_focused_shared_text_control() {
        let scenario = VisualDebugScenario {
            canvases: None,
            skin: None,
            logical_size: [1920, 1080],
            scale: 1.0,
            canvas_id: Some("wayland-status".into()),
            editing: true,
            selectors: Vec::new(),
            actions: Vec::new(),
        };
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        session.click(".editor-text-field").unwrap();
        session.key(&scorepeek_overlay_handles::TextCommand::SelectAll);
        {
            let document = session.document.inner.borrow();
            let node = document.get_focussed_node_id().unwrap();
            let input = document
                .get_node(node)
                .unwrap()
                .element_data()
                .unwrap()
                .text_input_data()
                .unwrap();
            assert_eq!(input.editor.raw_selection().text_range(), 0..6);
        }
        session.ime(scorepeek_overlay_handles::TextUpdate {
            preedit: Some("はいしん".into()),
            preedit_cursor: [12, 12],
            ..Default::default()
        });
        {
            let document = session.document.inner.borrow();
            let node = document.get_focussed_node_id().unwrap();
            let input = document
                .get_node(node)
                .unwrap()
                .element_data()
                .unwrap()
                .text_input_data()
                .unwrap();
            assert_eq!(input.editor.raw_text(), "はいしん");
        }
        assert!(
            session
                .authority
                .session()
                .chrome
                .field_drafts
                .values()
                .any(|draft| draft.composing)
        );
        session.ime(scorepeek_overlay_handles::TextUpdate {
            commit: Some("配信画面".into()),
            preedit: Some(String::new()),
            ..Default::default()
        });

        assert_eq!(
            session.authority.session().current().unwrap().name,
            "配信画面"
        );
        assert!(
            session
                .authority
                .session()
                .chrome
                .field_drafts
                .values()
                .all(|draft| !draft.composing)
        );
    }

    #[test]
    fn native_skin_property_draft_survives_rebuild_and_commits_through_shared_state() {
        let scenario = VisualDebugScenario {
            canvases: None,
            skin: None,
            logical_size: [1920, 1080],
            scale: 1.0,
            canvas_id: Some("wayland-status".into()),
            editing: true,
            selectors: Vec::new(),
            actions: Vec::new(),
        };
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        session
            .scroll(
                ".workspace-output-option .navigator-item-select",
                0.0,
                -120.0,
            )
            .unwrap();
        session
            .click(".widget-row[data-widget-id='status']")
            .unwrap();
        assert_eq!(
            session.authority.session().selected_widget.as_deref(),
            Some("status"),
            "the native navigator click must select the status widget"
        );
        session.scroll(".inspector-scroll", 0.0, 1200.0).unwrap();
        session.focus(".property-value-input").unwrap();
        session.key(&scorepeek_overlay_handles::TextCommand::SelectAll);
        session.key(&scorepeek_overlay_handles::TextCommand::Insert(
            "999".into(),
        ));
        assert!(
            session
                .authority
                .session()
                .chrome
                .field_drafts
                .values()
                .any(|draft| draft.text == "999" && !draft.valid)
        );
        session.click(".editor-panel-toggle").unwrap();
        session.click(".editor-panel-toggle").unwrap();

        assert!(
            session
                .authority
                .session()
                .chrome
                .field_drafts
                .values()
                .any(|draft| draft.text == "999" && !draft.valid)
        );

        session.focus(".property-value-input").unwrap();
        session.key(&scorepeek_overlay_handles::TextCommand::SelectAll);
        session.key(&scorepeek_overlay_handles::TextCommand::Insert("25".into()));
        session.key_composing(&scorepeek_overlay_handles::TextCommand::Accept, true);
        assert!(
            session
                .authority
                .session()
                .chrome
                .field_drafts
                .values()
                .any(|draft| draft.text == "25")
        );
        session.key(&scorepeek_overlay_handles::TextCommand::Accept);

        let model = session.authority.session();
        assert!(model.chrome.field_drafts.is_empty());
        let widget = model
            .current()
            .unwrap()
            .widgets
            .iter()
            .find(|widget| widget.id == "status")
            .unwrap();
        assert_eq!(
            widget
                .skin_properties
                .get("fill-opacity-percent")
                .and_then(serde_json::Value::as_i64),
            Some(25)
        );
    }

    #[test]
    fn native_keyboard_drives_the_shared_list_picker_contract() {
        let scenario = VisualDebugScenario {
            canvases: None,
            skin: None,
            logical_size: [1920, 1080],
            scale: 1.0,
            canvas_id: Some("wayland-status".into()),
            editing: true,
            selectors: Vec::new(),
            actions: Vec::new(),
        };
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        let point = {
            let inner = session.document.inner.borrow();
            let trigger = inner
                .query_selector(".screen-picker .list-picker-trigger")
                .unwrap()
                .unwrap();
            let rect = inner.get_client_bounding_rect(trigger).unwrap();
            [rect.x + rect.width / 2.0, rect.y + rect.height / 2.0]
        };
        session
            .pointer
            .dispatch_blitz(&mut session.document, point, 0x110, None);
        session
            .pointer
            .dispatch_blitz(&mut session.document, point, 0x110, Some(true));
        session
            .pointer
            .dispatch_blitz(&mut session.document, point, 0x110, Some(false));
        session.resolve();
        let raw_trigger = session
            .document
            .inner
            .borrow()
            .query_selector(".screen-picker .list-picker-trigger")
            .unwrap()
            .unwrap();
        assert_ne!(
            session.document.inner.borrow().get_focussed_node_id(),
            Some(raw_trigger),
            "raw Blitz does not implement the browser button-click focus default"
        );

        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        session
            .click(".screen-picker .list-picker-trigger")
            .unwrap();
        let trigger = session
            .document
            .inner
            .borrow()
            .query_selector(".screen-picker .list-picker-trigger")
            .unwrap()
            .unwrap();
        assert_eq!(
            session.document.inner.borrow().get_focussed_node_id(),
            Some(trigger)
        );
        session.key(&scorepeek_overlay_handles::TextCommand::Down);
        let trigger = session
            .document
            .inner
            .borrow()
            .query_selector(".screen-picker .list-picker-trigger")
            .unwrap()
            .unwrap();
        assert_eq!(
            session.document.inner.borrow().get_focussed_node_id(),
            Some(trigger)
        );
        session.key(&scorepeek_overlay_handles::TextCommand::Accept);

        assert_eq!(
            session.authority.session().preview,
            scorepeek_overlay_ui::ScreenKind::ModeSelect
        );
        assert!(!session.authority.session().chrome.screen_picker_open);
    }

    #[test]
    fn embedded_artwork_decodes_with_the_native_png_feature() {
        for (path, bytes) in scorepeek_overlay_ui::SKIN_ASSETS {
            let image = image::load_from_memory(bytes).expect(path);
            assert!(image.width() > 0 && image.height() > 0, "{path}");
        }
    }

    #[test]
    fn every_native_scene_retains_an_image_atlas_generation() {
        let mut scene = anyrender::Scene::new();
        retain_native_image_atlas(&mut scene);
        assert!(matches!(
            scene.commands.as_slice(),
            [anyrender::recording::RenderCommand::Fill(command)]
                if matches!(command.brush, anyrender::Paint::Image(_))
        ));
    }

    #[test]
    fn compact_canvas_editor_expands_inside_the_output() {
        assert_eq!(
            editor_geometry([1700, 1000], [560, 72], Some([1920, 1080])),
            ([0, 0], [1920, 1080])
        );
        assert_eq!(
            editor_geometry([20, 20], [800, 640], Some([1920, 1080])),
            ([0, 0], [1920, 1080])
        );
        assert_eq!(editor_panel_width(Some(1920)), 384);
        assert_eq!(editor_panel_width(Some(5120)), 480);
        assert_eq!(editor_panel_width(Some(1280)), 360);
        assert_eq!(editor_panel_width(None), 400);
        assert_eq!(grid_floor(1366), 1364);
        assert_eq!(maximum_grid_position(1366, 560), 804);
    }

    #[test]
    fn aggregate_canvas_visibility_preserves_explicit_screen_membership() {
        use scorepeek_overlay_ui::editor_model::SCREENS;
        let mut canvas = crate::config::visual_debug_config().canvases[0].presentation();
        canvas.show_on = None;
        let mut model = EditorSession::new(vec![canvas.clone()], [1920, 1080], "wayland");
        model.readonly = false;
        for screen in SCREENS {
            model.action(&EditorAction::CanvasVisible(screen, false));
        }
        assert_eq!(model.draft[0].show_on, Some(Vec::new()));
        for screen in SCREENS {
            model.action(&EditorAction::CanvasVisible(screen, true));
        }
        assert_eq!(model.draft[0].show_on, Some(SCREENS.to_vec()));
    }

    #[test]
    fn migrated_minimum_widget_resizes_from_every_corner() {
        for (x, y) in [(8, 8), (-24, -24), (40, 40)] {
            let original = WidgetLayout {
                id: "minimum".into(),
                kind: scorepeek_overlay_ui::WidgetKind::Empty,
                x,
                y,
                width: 16,
                height: 16,
                settings: scorepeek_overlay_ui::WidgetSettings::default(),
                skin_properties: std::collections::BTreeMap::new(),
            };
            for corner in [
                ResizeCorner::NorthWest,
                ResizeCorner::NorthEast,
                ResizeCorner::SouthWest,
                ResizeCorner::SouthEast,
            ] {
                let mut widget = original.clone();
                resize_widget(
                    &mut widget,
                    &original,
                    [0.0, 0.0],
                    corner,
                    4.0,
                    4.0,
                    [32, 32],
                );
                assert!(widget.width >= 16 && widget.height >= 16);
                assert!(widget.width <= 64 && widget.height <= 64);
            }
        }
    }
    #[test]
    fn locked_empty_resize_keeps_ratio_at_minimum_and_canvas_bounds() {
        for (mode, ratio) in [
            (scorepeek_overlay_ui::AspectRatio::Wide, 16.0 / 9.0),
            (scorepeek_overlay_ui::AspectRatio::Standard, 4.0 / 3.0),
            (scorepeek_overlay_ui::AspectRatio::Current([1, 8]), 0.125),
            (scorepeek_overlay_ui::AspectRatio::Current([8, 1]), 8.0),
        ] {
            let mut original = scorepeek_overlay_ui::default_widgets().remove(0);
            original.kind = scorepeek_overlay_ui::WidgetKind::Empty;
            original.x = 40;
            original.y = 40;
            original.width = 640;
            original.height = 360;
            original.settings.aspect_ratio = mode;
            for corner in [
                ResizeCorner::NorthWest,
                ResizeCorner::NorthEast,
                ResizeCorner::SouthWest,
                ResizeCorner::SouthEast,
            ] {
                for delta in [-4000.0, 4000.0] {
                    let mut widget = original.clone();
                    resize_widget(
                        &mut widget,
                        &original,
                        [0.0, 0.0],
                        corner,
                        delta,
                        delta,
                        [1920, 1080],
                    );
                    assert!(widget.width >= 16 && widget.height >= 16);
                    assert!(widget.x >= 0 && widget.y >= 0);
                    assert!(i64::from(widget.x) + i64::from(widget.width) <= 1920);
                    assert!(i64::from(widget.y) + i64::from(widget.height) <= 1080);
                    let error = if ratio >= 1.0 {
                        (f64::from(widget.width) / ratio - f64::from(widget.height)).abs()
                    } else {
                        (f64::from(widget.height) * ratio - f64::from(widget.width)).abs()
                    };
                    assert!(error <= 4.0, "{mode:?}: {}x{}", widget.width, widget.height);
                }
            }
        }
    }
}
