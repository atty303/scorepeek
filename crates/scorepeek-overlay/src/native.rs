mod text;
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

    fn dispatch(
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
        if pressed == Some(false) && button == MouseEventButton::Main {
            Self::focus_clicked_button(document, [x, y]);
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
        self.dispatch(document, point, 0x110, None);
        let point = dioxus::html::geometry::ClientPoint::new(point[0], point[1]).to_f32();
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

/// The browser retains focus when a keyed DOM element is patched in place. Blitz can replace the
/// native node during the equivalent Dioxus rebuild, so carry only the standard HTML `id` identity
/// across that renderer boundary. No editor action or control meaning is interpreted here.
fn poll_native_document(document: &mut DioxusDocument, waker: &Waker) -> bool {
    let focus_id = {
        let inner = document.inner.borrow();
        inner
            .get_focussed_node_id()
            .and_then(|node| inner.get_node(node))
            .and_then(|node| node.element_data())
            .and_then(|element| element.id.as_ref())
            .map(ToString::to_string)
    };
    let changed = document.poll(Some(TaskContext::from_waker(waker)));
    let replacement = if changed {
        focus_id.and_then(|focus_id| document.inner.borrow().get_element_by_id(&focus_id))
    } else {
        None
    };
    if let Some(node) = replacement {
        document.inner.borrow_mut().set_focus_to(node);
    }
    changed
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

#[cfg(test)]
fn passive_pointer_move(action: &SurfaceAction, dragging: bool) -> Option<[i32; 2]> {
    if dragging {
        return None;
    }
    match action {
        SurfaceAction::Move(point) => Some(*point),
        _ => None,
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

fn accepts_stage_projection(current: &StageProjection, candidate: &StageProjection) -> bool {
    current.session_id != candidate.session_id || candidate.revision > current.revision
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
enum SurfaceRole {
    DisplayCanvas,
    EditorStage,
}

#[derive(Clone, Default)]
struct PublishedStages {
    by_output: std::collections::BTreeMap<String, StageProjection>,
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
        let before = self.runtime.session.read().revision;
        crate::diagnostics::emit(
            "native_editor_action_received",
            &serde_json::json!({"session_id":self.runtime.session.read().session_id,"revision":before,"input":input.diagnostic_name()}),
        );
        let effects = self.runtime.dispatch.call(input);
        self.poll();
        let after = self.runtime.session.read().revision;
        crate::diagnostics::emit(
            "native_editor_action_reduced",
            &serde_json::json!({"session_id":self.runtime.session.read().session_id,"before_revision":before,"revision":after,"effect_count":effects.len()}),
        );
        self.publish();
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
        self.published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .by_output = stages
            .into_iter()
            .map(|stage| (stage.output.name.clone(), stage))
            .collect();
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
    let canvas_properties = manifest.effective_canvas_properties(&canvas.skin_properties);
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
    let canvas_properties = manifest.effective_canvas_properties(&canvas.skin_properties);
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

fn installed_editor_skins() -> Vec<scorepeek_overlay_ui::editor::EditorSkin> {
    static INSTALLED: std::sync::OnceLock<Vec<scorepeek_overlay_ui::editor::EditorSkin>> =
        std::sync::OnceLock::new();
    INSTALLED.get_or_init(load_installed_editor_skins).clone()
}

fn load_installed_editor_skins() -> Vec<scorepeek_overlay_ui::editor::EditorSkin> {
    #[cfg(test)]
    return embedded_editor_skins();

    #[cfg(not(test))]
    {
        let store = crate::skin::StoreRoot::discover();
        store
            .list()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|skin| {
                let package = store.open(&skin.id).ok()?;
                Some(scorepeek_overlay_ui::editor::EditorSkin {
                    id: skin.id.parse().ok()?,
                    name: skin.name,
                    release: skin.release,
                    preview: format!("/skin/{}/{}", skin.id, crate::skin::PREVIEW_PATH),
                    preview_video: None,
                    canvas_properties: serde_json::from_value(
                        serde_json::to_value(package.manifest.canvas_properties).ok()?,
                    )
                    .ok()?,
                    widget_properties: serde_json::from_value(
                        serde_json::to_value(package.manifest.widget_properties).ok()?,
                    )
                    .ok()?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
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
            reply,
        }));
    }
}

/// Runs until the parent's lifetime lease closes.
/// # Errors
/// Returns Wayland, GPU or event-loop failures.
#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
pub fn run(config: Config, input: impl std::io::Read + Send + 'static) -> Result<(), String> {
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
    let editor_id = format!("wayland-{}", std::process::id());
    let wayland_refresh_hz = Arc::new(std::sync::Mutex::new(config.wayland_refresh_hz));
    let start_editing = config.edit_on_start || desired.is_empty();
    let probe = desired
        .first()
        .cloned()
        .map_or_else(|| editor_bootstrap(&config), Ok)?;
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
    session.set_skins(installed_editor_skins());
    session.set_outputs(
        outputs
            .iter()
            .map(|output| EditorOutput {
                name: output.name.clone(),
                model: output.model.clone(),
                logical_size: output.logical_size,
            })
            .collect(),
    );
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
    let mut next_keepalive = Instant::now() + Duration::from_secs(5);
    while !stop.load(std::sync::atomic::Ordering::Acquire) {
        while let Ok(command) = coordinator_rx.try_recv() {
            match command {
                CoordinatorCommand::EditorInput { input, correlation } => {
                    if let Some(correlation) = correlation {
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
        let projected = if editing {
            let session = authority.session();
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
            editor_stage_canvases(&config, &draft, &output_descriptions)?
        } else {
            desired.clone()
        };
        let desired_ids = projected
            .iter()
            .map(|canvas| canvas.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let remove = workers
            .iter()
            .filter(|(id, worker)| {
                worker_needs_replacement(
                    id,
                    worker.output.as_deref(),
                    &desired_ids,
                    &projected,
                    worker.join.is_finished(),
                )
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        stop_workers(remove.iter(), &workers, &canvas_wakes);
        for id in remove {
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
        for canvas in &projected {
            if workers.contains_key(&canvas.id)
                || failed
                    .get(&canvas.id)
                    .is_some_and(|failure| recent_same_failure(failure, canvas))
            {
                continue;
            }
            let mut canvas_config = config.clone();
            canvas_config.canvases = vec![canvas.clone()];
            let canvas_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stopping = Arc::clone(&canvas_stop);
            let state = Arc::clone(&feed_state);
            let stopped = Arc::clone(&feed_stop);
            let stages = Arc::clone(&published_stages);
            let refresh = Arc::clone(&wayland_refresh_hz);
            let wakes = Arc::clone(&canvas_wakes);
            let coordinator = coordinator_tx.clone();
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

fn editor_bootstrap(config: &Config) -> Result<crate::config::Canvas, String> {
    let skin = crate::skin::StoreRoot::new(config.skin_store.clone())
        .list()?
        .first()
        .ok_or("Wayland editor requires at least one installed skin")?
        .id
        .parse::<scorepeek_overlay_ui::Skin>()?;
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
    Ok(bootstrap)
}

fn editor_stage_canvases(
    config: &Config,
    canvases: &[crate::config::Canvas],
    outputs: &[OutputDescription],
) -> Result<Vec<crate::config::Canvas>, String> {
    if outputs.is_empty() {
        return Ok(vec![editor_bootstrap(config)?]);
    }
    let skin = if let Some(canvas) = canvases.first() {
        canvas.skin
    } else {
        editor_bootstrap(config)?.skin
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
        config.skin_store.clone(),
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
    crate::diagnostics::emit(
        "native_renderer_shutdown",
        &serde_json::json!({
            "run_id": report.borrow().run_id,
            "canvas_id": app.surface_canvas.id,
            "output": app.surface_output,
            "phase": "app_stopped",
        }),
    );
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
    let unmap = app.shell.unmap();
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
        report.skin_package_open_count = app.skin_package_open_count;
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
    skin_runtime: crate::skin::Runtime,
    editor_skin_previews: std::collections::BTreeMap<String, EditorSkinPreview>,
    skin_tree: crate::skin::NativeTree,
    next_skin_render: Option<Instant>,
    skin_release: String,
    skin_manifest: crate::skin::Manifest,
    skin_store: std::path::PathBuf,
    skin_assets: Arc<std::sync::Mutex<Option<crate::skin::Package>>>,
    skin_package_open_count: u64,
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
    package: crate::skin::Package,
    runtime: crate::skin::Runtime,
    tree: crate::skin::NativeTree,
    next_render: Option<Instant>,
}

fn create_editor_skin_preview(
    document: &mut DioxusDocument,
    presentation: &scorepeek_overlay_ui::CanvasPresentation,
    skin_store: &std::path::Path,
    skin_assets: &Arc<std::sync::Mutex<Option<crate::skin::Package>>>,
    report: &Rc<RefCell<RunReport>>,
    output: Option<&str>,
    state: &OverlayState,
) -> Result<EditorSkinPreview, String> {
    let mut canvas =
        crate::config::empty_canvas(presentation.id.clone(), crate::runtime::Backend::Wayland);
    canvas.apply_presentation(presentation);
    let package = crate::skin::StoreRoot::new(skin_store.to_path_buf()).open(canvas.skin.name())?;
    *skin_assets
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(package.clone());
    let mut runtime = new_native_skin_runtime(&package, report, &canvas.id, output, &[])?;
    let rendered = runtime.init(&native_skin_input(&canvas, state, &package.manifest))?;
    let root_id = editor_skin_root_id(&canvas.id);
    let root = document
        .inner
        .borrow()
        .query_selector(&format!("#{root_id}"))
        .map_err(|error| format!("query editor skin root: {error:?}"))?
        .ok_or_else(|| format!("editor skin root is missing for {}", canvas.id))?;
    let css = std::str::from_utf8(
        package
            .resource(crate::skin::STYLE_PATH)
            .ok_or("skin.css missing")?,
    )
    .map_err(|error| format!("skin.css is not UTF-8: {error}"))?;
    let mut tree = crate::skin::NativeTree::new(&mut document.inner.borrow_mut(), root, css);
    tree.apply(&mut document.inner.borrow_mut(), &rendered);
    Ok(EditorSkinPreview {
        canvas,
        package,
        runtime,
        tree,
        next_render: skin_deadline(&rendered.schedule, false),
    })
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
        skin_store: std::path::PathBuf,
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
        let package = crate::skin::StoreRoot::new(skin_store.clone()).open(canvas.skin.name())?;
        let vdom = VirtualDom::new_with_props(
            native_overlay,
            NativeOverlayProps {
                initial,
                published: Rc::clone(&published),
                port,
            },
        );
        let (document_config, skin_assets) = document_config_with_skin_handle(package.clone());
        let mut document = DioxusDocument::new(vdom, document_config);
        document.initial_build();
        let projection = published
            .borrow()
            .as_ref()
            .copied()
            .expect("native overlay publishes its projection during initial build");
        let mut skin_runtime = new_native_skin_runtime(
            &package,
            &report,
            &canvas.id,
            shell.output_name.as_deref(),
            &[],
        )?;
        let current_state = OverlayState::default();
        let skin_input = native_skin_input(&canvas, &current_state, &package.manifest);
        let started = Instant::now();
        let initial_skin = skin_runtime.init(&skin_input).inspect_err(|error| {
            crate::diagnostics::emit(
                "skin_render",
                &serde_json::json!({"skin_id":package.manifest.id,"release":package.manifest.release,"canvas_id":canvas.id,"backend":"native","phase":"init","status":"failed","error_type":skin_error_type(error)}),
            );
        })?;
        let root = document
            .inner
            .borrow()
            .query_selector("#scorepeek-skin-root")
            .map_err(|error| format!("query native skin root: {error:?}"))?
            .ok_or("native skin root is missing")?;
        let css = std::str::from_utf8(
            package
                .resource(crate::skin::STYLE_PATH)
                .ok_or("skin.css missing")?,
        )
        .map_err(|error| format!("skin.css is not UTF-8: {error}"))?
        .to_owned();
        let mut skin_tree =
            crate::skin::NativeTree::new(&mut document.inner.borrow_mut(), root, &css);
        skin_tree.apply(&mut document.inner.borrow_mut(), &initial_skin);
        crate::diagnostics::emit(
            "skin_render",
            &serde_json::json!({"skin_id":package.manifest.id,"release":package.manifest.release,"canvas_id":canvas.id,"backend":"native","phase":"init","status":"success","duration_us":u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),"tree_applied":true}),
        );
        let visible = match &*projection.borrow() {
            NativeDocumentProjection::Display { visible, .. } => *visible,
            NativeDocumentProjection::Editor(_) => true,
        };
        let interactive = matches!(&*projection.borrow(), NativeDocumentProjection::Editor(stage) if stage.interactive);
        let editor_presentations = match &*projection.borrow() {
            NativeDocumentProjection::Editor(projection) => projection.canvases.clone(),
            NativeDocumentProjection::Display { .. } => Vec::new(),
        };
        let mut editor_skin_previews = std::collections::BTreeMap::new();
        for presentation in &editor_presentations {
            let preview = create_editor_skin_preview(
                &mut document,
                presentation,
                &skin_store,
                &skin_assets,
                &report,
                shell.output_name.as_deref(),
                &current_state,
            )?;
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
            editor_skin_updates: EditorSkinUpdates::default(),
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
            skin_runtime,
            editor_skin_previews,
            skin_tree,
            next_skin_render: if role == SurfaceRole::EditorStage {
                editor_next_render
            } else {
                skin_deadline(&initial_skin.schedule, false)
            },
            skin_release: package.manifest.release.clone(),
            skin_manifest: package.manifest.clone(),
            skin_store,
            skin_assets,
            skin_package_open_count: 1_u64.saturating_add(editor_preview_count),
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

    fn visible(&self) -> bool {
        match &*self.projection.borrow() {
            NativeDocumentProjection::Display { visible, .. } => *visible,
            NativeDocumentProjection::Editor(_) => true,
        }
    }

    fn dragging(&self) -> bool {
        matches!(&*self.projection.borrow(), NativeDocumentProjection::Editor(stage) if stage.drag.is_some())
    }

    fn sync_projection(&mut self) -> bool {
        let next = match self.role {
            SurfaceRole::DisplayCanvas => return false,
            SurfaceRole::EditorStage => self
                .surface_output
                .as_ref()
                .and_then(|output| {
                    self.published_stages
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .by_output
                        .get(output)
                        .cloned()
                })
                .map(NativeDocumentProjection::Editor),
        };
        let Some(next) = next else { return false };
        if let (
            NativeDocumentProjection::Editor(current),
            NativeDocumentProjection::Editor(candidate),
        ) = (&*self.projection.borrow(), &next)
            && !accepts_stage_projection(current, candidate)
        {
            return false;
        }
        let changed = *self.projection.borrow() != next;
        if !changed {
            return false;
        }
        let previous = self.canvas.presentation();
        if let NativeDocumentProjection::Editor(stage) = &next {
            let previews_changed = stage.canvases.len() != self.editor_skin_previews.len()
                || stage.canvases.iter().any(|canvas| {
                    self.editor_skin_previews
                        .get(&canvas.id)
                        .is_none_or(|preview| {
                            editor_skin_presentation_changed(&preview.canvas.presentation(), canvas)
                        })
                });
            if let Some(canvas) = stage.selected_canvas.as_ref() {
                self.canvas = crate::config::empty_canvas(
                    canvas.id.clone(),
                    crate::runtime::Backend::Wayland,
                );
                self.canvas.apply_presentation(canvas);
            }
            if previews_changed
                || stage
                    .selected_canvas
                    .as_ref()
                    .is_some_and(|canvas| editor_skin_presentation_changed(&previous, canvas))
            {
                self.editor_skin_updates.request();
            }
        }
        let interactive =
            matches!(&next, NativeDocumentProjection::Editor(stage) if stage.interactive);
        self.projection.set(next);
        self.shell.set_input_enabled(interactive);
        self.shell.set_keyboard_enabled(interactive);
        crate::diagnostics::emit(
            "native_editor_projection_received",
            &serde_json::json!({
                "run_id": self.report.borrow().run_id,
                "output": self.surface_output,
                "session_id": match &*self.projection.borrow() { NativeDocumentProjection::Editor(stage) => Some(stage.session_id), NativeDocumentProjection::Display { .. } => None },
                "revision": match &*self.projection.borrow() { NativeDocumentProjection::Editor(stage) => Some(stage.revision), NativeDocumentProjection::Display { .. } => None },
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
                Err(error) => return Err(error),
            };
            let mut frame = false;
            let mut configured = false;
            let mut input_damage = false;
            if self.output_descriptions != self.shell.output_descriptions {
                self.output_descriptions
                    .clone_from(&self.shell.output_descriptions);
                let outputs = self
                    .output_descriptions
                    .iter()
                    .map(|output| EditorOutput {
                        name: output.name.clone(),
                        model: output.model.clone(),
                        logical_size: output.logical_size,
                    })
                    .collect();
                let _ = self.coordinator.send(CoordinatorCommand::EditorInput {
                    input: EditorInput::SetOutputs(outputs),
                    correlation: None,
                });
            }
            for event in events {
                match event {
                    Event::Configure {
                        logical,
                        physical,
                        scale_120,
                    } => {
                        configured |= self.configure(logical, physical, scale_120)?;
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
                    }
                    Event::Wake => {}
                    Event::PointerMotion { x, y } => {
                        self.pointer
                            .dispatch(&mut self.document, [x, y], 0x110, None);
                        input_damage = true;
                    }
                    Event::PointerButton {
                        button,
                        pressed,
                        x,
                        y,
                    } => {
                        self.pointer_button(button, pressed, x, y);
                        input_damage = true;
                    }
                    Event::PointerScroll { dx, dy, x, y } => {
                        self.pointer.wheel(&mut self.document, [x, y], [dx, dy]);
                        input_damage = true;
                    }
                    Event::Text(command) => self.input_command(&command),
                    Event::Ime(update) => self.input_composition(update),
                    Event::KeyboardFocus(focused) => {
                        if !focused {
                            self.shell.set_text_input(None);
                            self.set_text_composing(false);
                        }
                    }
                    Event::Frame => frame = true,
                    Event::Closed => return Ok(()),
                }
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
            let mut skin_changed = false;
            if self.editing()
                && self
                    .editor_skin_updates
                    .take_if_ready(frame, self.dragging())
            {
                self.update_editor_skin();
                skin_changed = true;
            }
            // A Wayland frame is a browser-like Dioxus turn. Poll unconditionally; idle VDOM
            // polls are cheap and prevent omitted manual wake predicates from freezing the UI.
            let changed = self.poll_dioxus();
            self.sync_text_input();
            self.pending_paint |=
                changed || projection_changed || visibility_changed || input_damage || skin_changed;
            let paint_state = PaintState {
                editing: self.editing(),
                visible,
                signal: PaintSignal::from_state(frame, self.pending_paint),
                animating: self.animating,
            };
            let reason = if configured {
                Some(if self.paint_count == 0 {
                    PaintReason::InitialConfigure
                } else {
                    PaintReason::Reconfigure
                })
            } else if visibility_changed && !visible {
                Some(PaintReason::VisibilityClear)
            } else {
                paint_state.ordinary_reason()
            };
            if self.renderer.is_active()
                && let Some(reason) = reason
            {
                let now = self.started.elapsed();
                let refresh = *self
                    .wayland_refresh_hz
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if self.cadence.permits(now, refresh, reason) {
                    self.paint(reason, now)?;
                } else if visible {
                    self.shell.request_frame_and_commit();
                }
            } else if self.renderer.is_active() && paint_state.editor_frame_needed() {
                self.shell.request_frame_and_commit();
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

    fn update_editor_skin(&mut self) {
        let started = Instant::now();
        self.editor_skin_updates.pending = false;
        self.editor_skin_updates.renders = self.editor_skin_updates.renders.saturating_add(1);
        let result = self.render_editor_skin();
        if let Err(error) = &result {
            self.next_skin_render = None;
            self.animating = false;
            crate::diagnostics::emit(
                "skin_render",
                &serde_json::json!({"skin_id":self.canvas.skin.name(),"canvas_id":self.canvas.id,"backend":"native","phase":"editor-preview","status":"failed","error_type":skin_error_type(error)}),
            );
        }
        let duration = duration_us(started.elapsed());
        crate::diagnostics::emit(
            "native_editor_skin_timing",
            &serde_json::json!({"run_id":self.report.borrow().run_id,"canvas_id":self.canvas.id,"output":self.surface_output,"duration_us":duration,"startup_elapsed_us":duration_us(self.startup_started.elapsed()),"status":if result.is_ok(){"success"}else{"error"}}),
        );
    }

    fn render_editor_skin(&mut self) -> Result<(), String> {
        let state = if self.current_state.system == scorepeek_overlay_ui::LampState::Inactive {
            scorepeek_overlay_ui::editor_sample_state()
        } else {
            self.current_state.clone()
        };
        let presentations = match &*self.projection.borrow() {
            NativeDocumentProjection::Editor(projection) => projection.canvases.clone(),
            NativeDocumentProjection::Display { .. } => return Ok(()),
        };
        let ids = presentations
            .iter()
            .map(|canvas| canvas.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        self.editor_skin_previews
            .retain(|canvas_id, _| ids.contains(canvas_id));
        for presentation in &presentations {
            if !self.editor_skin_previews.contains_key(&presentation.id) {
                let preview = create_editor_skin_preview(
                    &mut self.document,
                    presentation,
                    &self.skin_store,
                    &self.skin_assets,
                    &self.report,
                    self.surface_output.as_deref(),
                    &state,
                )?;
                self.editor_skin_previews
                    .insert(presentation.id.clone(), preview);
                self.skin_package_open_count = self.skin_package_open_count.saturating_add(1);
                continue;
            }
            let preview = self
                .editor_skin_previews
                .get_mut(&presentation.id)
                .expect("checked editor preview");
            preview.canvas.apply_presentation(presentation);
            let desired_skin = preview.canvas.skin.name();
            let changed_package = preview.package.manifest.id != desired_skin;
            if changed_package {
                preview.package =
                    crate::skin::StoreRoot::new(self.skin_store.clone()).open(desired_skin)?;
                preview.runtime = new_native_skin_runtime(
                    &preview.package,
                    &self.report,
                    &preview.canvas.id,
                    self.surface_output.as_deref(),
                    &[],
                )?;
                self.skin_package_open_count = self.skin_package_open_count.saturating_add(1);
            }
            *self
                .skin_assets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(preview.package.clone());
            let input = native_skin_input(&preview.canvas, &state, &preview.package.manifest);
            let output = if changed_package {
                preview.runtime.init(&input)?
            } else {
                preview.runtime.render(&input)?
            };
            if changed_package {
                let css = std::str::from_utf8(
                    preview
                        .package
                        .resource(crate::skin::STYLE_PATH)
                        .ok_or("skin.css missing")?,
                )
                .map_err(|error| format!("skin.css is not UTF-8: {error}"))?;
                preview
                    .tree
                    .replace(&mut self.document.inner.borrow_mut(), css, &output);
            } else {
                preview
                    .tree
                    .apply(&mut self.document.inner.borrow_mut(), &output);
            }
            preview.next_render = skin_deadline(&output.schedule, false);
        }
        self.next_skin_render = self
            .editor_skin_previews
            .values()
            .filter_map(|preview| preview.next_render)
            .min();
        self.animating = self
            .editor_skin_previews
            .values()
            .any(|preview| preview.next_render.is_some());
        Ok(())
    }

    fn poll_dioxus(&mut self) -> bool {
        let mut changed = false;
        while poll_native_document(&mut self.document, &self.waker) {
            changed = true;
        }
        self.full_layout_pending |= changed;
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

    fn sync_text_input(&mut self) {
        if !matches!(&*self.projection.borrow(), NativeDocumentProjection::Editor(stage) if stage.interactive)
        {
            self.shell.set_text_input(None);
            self.set_text_composing(false);
            return;
        }
        let input = {
            let document = self.document.inner.borrow();
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
        };
        if input.is_none() {
            self.set_text_composing(false);
        }
        self.shell.set_text_input(input);
    }

    fn render_skin(&mut self, state: &OverlayState) -> Result<(), String> {
        let started = Instant::now();
        let output = self.skin_runtime.render(&native_skin_input(
            &self.canvas,
            state,
            &self.skin_manifest,
        ))?;
        self.skin_tree
            .apply(&mut self.document.inner.borrow_mut(), &output);
        self.next_skin_render = skin_deadline(&output.schedule, false);
        self.animating = matches!(output.schedule, crate::skin::Schedule::NextFrame);
        crate::diagnostics::emit(
            "skin_render",
            &serde_json::json!({"skin_id":self.canvas.skin.name(),"release":self.skin_release,"canvas_id":self.canvas.id,"backend":"native","phase":"render","status":"success","duration_us":duration_us(started.elapsed()),"next_tick":format!("{:?}",output.schedule),"tree_applied":true}),
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

    fn paint(&mut self, reason: PaintReason, now: Duration) -> Result<(), String> {
        let first_paint = self.paint_count == 0;
        let started = Instant::now();
        self.paint_exclusive()?;
        self.cadence.record(now);
        self.pending_paint = false;
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
            crate::diagnostics::emit(
                "native_startup_timing",
                &serde_json::json!({"run_id":self.report.borrow().run_id,"canvas_id":self.surface_canvas.id,"phase":"first_paint","paint_us":duration_us(started.elapsed()),"elapsed_us":duration_us(self.startup_started.elapsed()),"status":"success"}),
            );
        }
        if self.editing() {
            let projection = self.projection.borrow();
            let (session_id, revision) = match &*projection {
                NativeDocumentProjection::Editor(stage) => (stage.session_id, stage.revision),
                NativeDocumentProjection::Display { .. } => (0, 0),
            };
            crate::diagnostics::emit(
                "native_editor_painted",
                &serde_json::json!({"run_id":self.report.borrow().run_id,"output":self.surface_output,"session_id":session_id,"revision":revision,"paint_us":duration_us(started.elapsed())}),
            );
        }
        Ok(())
    }

    fn paint_exclusive(&mut self) -> Result<(), String> {
        if !self.renderer.is_active() {
            return Err("native renderer inactive".into());
        }
        let visible = self.visible();
        let mut inner = self.document.inner.borrow_mut();
        let seconds = self.started.elapsed().as_secs_f64();
        if visible {
            apply_motion(&mut inner, seconds);
        }
        let incremental_layout = inner.incremental_layout();
        if self.full_layout_pending {
            inner.set_incremental_layout(false);
        }
        resolve_with_loaded_resources(&mut inner, seconds);
        if self.full_layout_pending {
            inner.set_incremental_layout(incremental_layout);
            self.full_layout_pending = false;
        }
        self.animating = visible;
        self.shell.request_frame();
        let (width, height) = inner.viewport().window_size;
        let scale = inner.viewport().scale_f64();
        self.renderer
            .render(|scene| paint_native_scene(scene, &mut inner, scale, width, height));
        self.paint_count = self.paint_count.saturating_add(1);
        self.render_calls = self.render_calls.saturating_add(1);
        if self.paint_count == 1 {
            self.report
                .borrow_mut()
                .operations
                .push("dioxus_blitz_initial_paint");
        }
        Ok(())
    }
}

const fn surface_input_enabled(editing: bool, interactive: bool, visible: bool) -> bool {
    visible && (!editing || interactive)
}
fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
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

struct EmbeddedSkinAssets {
    active: Arc<std::sync::Mutex<Option<crate::skin::Package>>>,
    store: crate::skin::StoreRoot,
}

impl blitz_traits::net::NetProvider for EmbeddedSkinAssets {
    fn fetch(
        &self,
        _doc_id: usize,
        request: blitz_traits::net::Request,
        handler: Box<dyn blitz_traits::net::NetHandler>,
    ) {
        let path = request.url.path().trim_start_matches('/');
        if let Some(rest) = path.strip_prefix("skin/")
            && let Some((id, resource)) = rest.split_once('/')
            && crate::skin::validate_id(id).is_ok()
            && let Ok(package) = self.store.open(id)
            && let Some(bytes) = package.resource(resource)
        {
            handler.bytes(
                request.url.to_string(),
                blitz_traits::net::Bytes::copy_from_slice(bytes),
            );
            return;
        }
        if let Some(svg) = scorepeek_overlay_ui::composition::aperture_asset(request.url.path()) {
            handler.bytes(request.url.to_string(), blitz_traits::net::Bytes::from(svg));
            return;
        }
        let active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(package) = active.as_ref() {
            let bytes = package.resource(path).unwrap_or_default();
            handler.bytes(
                request.url.to_string(),
                blitz_traits::net::Bytes::copy_from_slice(bytes),
            );
            return;
        }
        let bytes = scorepeek_overlay_ui::skin_asset(request.url.path()).unwrap_or_default();
        handler.bytes(
            request.url.to_string(),
            blitz_traits::net::Bytes::from_static(bytes),
        );
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

/// Registers embedded artwork and the Latin font, preserving Japanese system fallbacks.
#[must_use]
pub fn document_config() -> DocumentConfig {
    document_config_inner(None)
}

fn document_config_inner(package: Option<crate::skin::Package>) -> DocumentConfig {
    document_config_inner_with_handle(Arc::new(std::sync::Mutex::new(package))).0
}

fn document_config_with_skin_handle(
    package: crate::skin::Package,
) -> (
    DocumentConfig,
    Arc<std::sync::Mutex<Option<crate::skin::Package>>>,
) {
    document_config_inner_with_handle(Arc::new(std::sync::Mutex::new(Some(package))))
}

fn document_config_inner_with_handle(
    active: Arc<std::sync::Mutex<Option<crate::skin::Package>>>,
) -> (
    DocumentConfig,
    Arc<std::sync::Mutex<Option<crate::skin::Package>>>,
) {
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
            active: Arc::clone(&active),
            store: crate::skin::StoreRoot::discover(),
        })),
        ..DocumentConfig::default()
    };
    (config, active)
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
    skins: std::collections::BTreeMap<String, VisualSkin>,
    skin_assets: Arc<std::sync::Mutex<Option<crate::skin::Package>>>,
}

struct VisualSkin {
    runtime: crate::skin::Runtime,
    tree: crate::skin::NativeTree,
    package: crate::skin::Package,
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
        model.set_skins(installed_editor_skins());
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
        let (document_config, skin_assets) = package.as_ref().map_or_else(
            || document_config_inner_with_handle(Arc::new(std::sync::Mutex::new(None))),
            |package| document_config_with_skin_handle(package.clone()),
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
            let store = crate::skin::StoreRoot::discover();
            let package_path = store.path().join(format!("{}.zip", mounted.skin.name()));
            if !package_path.exists() {
                continue;
            }
            let package = store.open(mounted.skin.name())?;
            *skin_assets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(package.clone());
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
            let css = std::str::from_utf8(
                package
                    .resource(crate::skin::STYLE_PATH)
                    .ok_or("skin.css missing")?,
            )
            .map_err(|error| format!("skin.css is not UTF-8: {error}"))?;
            let mut runtime = crate::skin::Runtime::new(&package)?;
            let initial = runtime.init(&native_skin_input_presentation(
                &mounted,
                &state,
                &package.manifest,
            ))?;
            let mut tree =
                crate::skin::NativeTree::new(&mut document.inner.borrow_mut(), root, css);
            tree.apply(&mut document.inner.borrow_mut(), &initial);
            skins.insert(
                mounted.id.clone(),
                VisualSkin {
                    runtime,
                    tree,
                    package,
                },
            );
        }
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
            let next = NativeDocumentProjection::Editor(
                self.authority.session().stage_projection(&output),
            );
            let changed = { *self.projection.borrow() != next };
            if changed {
                self.projection.set(next);
            }
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

    fn render_skin(&mut self) -> Result<(), String> {
        let canvases = match &*self.projection.borrow() {
            NativeDocumentProjection::Display { canvas, .. } => vec![canvas.clone()],
            NativeDocumentProjection::Editor(stage) => stage.canvases.clone(),
        };
        for canvas in canvases {
            let desired_skin = canvas.skin.name();
            if !self.skins.contains_key(&canvas.id) {
                let store = crate::skin::StoreRoot::discover();
                let package_path = store.path().join(format!("{desired_skin}.zip"));
                if !package_path.exists() {
                    continue;
                }
                let package = store.open(desired_skin)?;
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
                let css = std::str::from_utf8(
                    package
                        .resource(crate::skin::STYLE_PATH)
                        .ok_or("skin.css missing")?,
                )
                .map_err(|error| format!("skin.css is not UTF-8: {error}"))?;
                let mut runtime = crate::skin::Runtime::new(&package)?;
                let output = runtime.init(&native_skin_input_presentation(
                    &canvas,
                    &self.state,
                    &package.manifest,
                ))?;
                let mut tree =
                    crate::skin::NativeTree::new(&mut self.document.inner.borrow_mut(), root, css);
                tree.apply(&mut self.document.inner.borrow_mut(), &output);
                self.skins.insert(
                    canvas.id.clone(),
                    VisualSkin {
                        runtime,
                        tree,
                        package,
                    },
                );
            }
            let Some(skin) = self.skins.get_mut(&canvas.id) else {
                continue;
            };
            let changed = skin.package.manifest.id != desired_skin;
            if changed {
                skin.package = crate::skin::StoreRoot::discover().open(desired_skin)?;
                skin.runtime = crate::skin::Runtime::new(&skin.package)?;
            }
            *self
                .skin_assets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(skin.package.clone());
            let input =
                native_skin_input_presentation(&canvas, &self.state, &skin.package.manifest);
            let output = if changed {
                skin.runtime.init(&input)?
            } else {
                skin.runtime.render(&input)?
            };
            if changed {
                let css = std::str::from_utf8(
                    skin.package
                        .resource(crate::skin::STYLE_PATH)
                        .ok_or("skin.css missing")?,
                )
                .map_err(|error| format!("skin.css is not UTF-8: {error}"))?;
                skin.tree
                    .replace(&mut self.document.inner.borrow_mut(), css, &output);
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
        self.skins.clear();
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

    #[test]
    fn passive_pointer_motion_does_not_enter_the_editor_model_path() {
        assert_eq!(
            passive_pointer_move(&SurfaceAction::Move([320, 180]), false),
            Some([320, 180])
        );
        assert_eq!(
            passive_pointer_move(&SurfaceAction::Move([320, 180]), true),
            None
        );
        assert_eq!(passive_pointer_move(&SurfaceAction::End, false), None);
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
        session.ime(scorepeek_overlay_handles::TextUpdate {
            preedit: Some("はいしん".into()),
            preedit_cursor: [12, 12],
            ..Default::default()
        });
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
            .click(".widget-row[data-widget-id='status']")
            .unwrap();
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
