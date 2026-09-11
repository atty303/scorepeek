mod text;
use scorepeek_overlay_ui::editor_model::{Drag, Model as EditorModel};
use scorepeek_overlay_ui::editor_surface::{
    EditorCanvas, EditorSelectionMetrics, EditorSurface, PlacementPreview, SurfaceAction,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
    task::{Context as TaskContext, Wake, Waker},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use text::TitleEdit;

use crate::runtime::{Config, Feed};
use anyrender::{CompositeAlphaMode, ImageRenderer, PaintScene, WindowRenderer};
use anyrender_vello::{VelloRendererOptions, VelloWindowRenderer};
use blitz_dom::{Document, DocumentConfig};
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};
use dioxus::prelude::*;
use dioxus_core::VirtualDom;
use dioxus_native_dom::DioxusDocument;
use scorepeek_overlay_handles::{CursorStyle, Event, OutputDescription, Shell};
use scorepeek_overlay_ui::editor::{
    EditorAccess, EditorAction, EditorChrome, EditorOutput, EditorPanel, EditorTitleState,
    EditorView,
};
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum EditorPointerObservation {
    #[default]
    AwaitingPress,
    AwaitingRelease,
    Observed,
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

fn editor_panel_width(output_width: Option<u32>) -> u32 {
    match output_width {
        Some(width) => (width / 5).clamp(360, 480),
        None => 400,
    }
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

fn active_editor_canvas_on_surface(
    canvas_id: &str,
    show_on: Option<&[scorepeek_overlay_ui::ScreenKind]>,
    selected_canvas_id: Option<&str>,
    surface_canvas_ids: &std::collections::BTreeSet<String>,
    screen: scorepeek_overlay_ui::ScreenKind,
) -> bool {
    selected_canvas_id == Some(canvas_id)
        && surface_canvas_ids.contains(canvas_id)
        && show_on.is_none_or(|screens| screens.contains(&screen))
}

fn surface_canvas_ids_for_draft(
    draft: &[scorepeek_overlay_ui::CanvasPresentation],
    surface_output: Option<&str>,
) -> std::collections::BTreeSet<String> {
    draft
        .iter()
        .filter(|canvas| {
            canvas
                .output
                .as_deref()
                .is_none_or(|output| Some(output) == surface_output)
        })
        .map(|canvas| canvas.id.clone())
        .collect()
}

#[derive(Clone)]
struct NativeCanvasSettings {
    id: String,
    has_selection: bool,
    output: Option<String>,
    active_output: Option<String>,
    show_on: Option<Vec<scorepeek_overlay_ui::ScreenKind>>,
    background: scorepeek_overlay_ui::Background,
    opacity_percent: u8,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    preview_screen: scorepeek_overlay_ui::ScreenKind,
    panel_width: u32,
    new_canvas_skin: scorepeek_overlay_ui::Skin,
}

impl NativeCanvasSettings {
    fn apply_presentation(&mut self, presentation: &scorepeek_overlay_ui::CanvasPresentation) {
        self.id.clone_from(&presentation.id);
        self.output.clone_from(&presentation.output);
        self.show_on.clone_from(&presentation.show_on);
        self.background = presentation.background;
        self.opacity_percent = presentation.opacity_percent;
        self.x = presentation.x;
        self.y = presentation.y;
        self.width = presentation.width;
        self.height = presentation.height;
    }
}

#[derive(Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
struct EditorWorkspaceUi {
    panel_open: bool,
    preview_screen: scorepeek_overlay_ui::ScreenKind,
    widget_add_open: bool,
}

impl Default for EditorWorkspaceUi {
    fn default() -> Self {
        Self {
            panel_open: true,
            preview_screen: scorepeek_overlay_ui::ScreenKind::MusicSelect,
            widget_add_open: false,
        }
    }
}

#[derive(Clone)]
struct DraftUndo {
    canvases: Vec<scorepeek_overlay_ui::CanvasPresentation>,
    wayland_refresh_hz: scorepeek_overlay_ui::WaylandRefreshRate,
}

fn remember_draft_change(
    undo: &mut Option<DraftUndo>,
    before: DraftUndo,
    after: &[scorepeek_overlay_ui::CanvasPresentation],
    after_refresh: scorepeek_overlay_ui::WaylandRefreshRate,
) -> bool {
    if before.canvases == after && before.wayland_refresh_hz == after_refresh {
        return false;
    }
    *undo = Some(before);
    true
}

fn coordinator_draft_update(
    canvases: Vec<scorepeek_overlay_ui::CanvasPresentation>,
    outputs: &[EditorOutput],
    wayland_refresh_hz: scorepeek_overlay_ui::WaylandRefreshRate,
    correlation: Option<InteractionCorrelation>,
) -> Option<CoordinatorCommand> {
    scorepeek_overlay_ui::editor::document_valid(&canvases, outputs).then_some(
        CoordinatorCommand::UpdateDraft {
            canvases,
            wayland_refresh_hz,
            correlation,
        },
    )
}

fn replace_canvas_selection(
    selected_canvas: &mut Option<String>,
    selected_widget: &mut Option<String>,
    next: Option<String>,
) {
    *selected_canvas = next;
    selected_widget.take();
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

#[derive(Default)]
struct NativeEditorSession {
    ui: EditorWorkspaceUi,
    undo: Option<DraftUndo>,
    fallback: std::collections::BTreeSet<String>,
    draft: Vec<scorepeek_overlay_ui::CanvasPresentation>,
    dirty: bool,
    readonly: bool,
    selected_widget: Option<String>,
    pending_widget: Option<scorepeek_overlay_ui::WidgetKind>,
    interaction: Option<Drag>,
    outputs: std::collections::BTreeMap<String, OutputDescription>,
    selected_canvas: Option<String>,
    active_output: Option<String>,
    output_selection_epoch: u64,
    wayland_refresh_hz: scorepeek_overlay_ui::WaylandRefreshRate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SurfaceRole {
    DisplayCanvas,
    EditorStage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditorPhase {
    Display,
    Editing,
}

#[derive(Clone, Debug, PartialEq)]
struct InteractionCorrelation {
    run_id: String,
    interaction_id: u64,
    action: &'static str,
}

#[derive(Debug)]
enum CoordinatorCommand {
    Open {
        output: Option<String>,
        canvas: String,
        preview_screen: Option<scorepeek_overlay_ui::ScreenKind>,
        resolved_canvas: Option<scorepeek_overlay_ui::CanvasPresentation>,
    },
    Close {
        reason: &'static str,
        correlation: Option<InteractionCorrelation>,
    },
    UpdateDraft {
        canvases: Vec<scorepeek_overlay_ui::CanvasPresentation>,
        wayland_refresh_hz: scorepeek_overlay_ui::WaylandRefreshRate,
        correlation: Option<InteractionCorrelation>,
    },
    Save {
        canvases: Vec<scorepeek_overlay_ui::CanvasPresentation>,
        wayland_refresh_hz: scorepeek_overlay_ui::WaylandRefreshRate,
        correlation: Option<InteractionCorrelation>,
    },
    ResolveOutput {
        output_names: Vec<String>,
        output: Option<String>,
        canvas: String,
        preview_screen: Option<scorepeek_overlay_ui::ScreenKind>,
        resolved_canvas: scorepeek_overlay_ui::CanvasPresentation,
    },
}

#[derive(Clone, Debug, PartialEq)]
enum CoordinatorTransition {
    Opened(Option<scorepeek_overlay_ui::CanvasPresentation>),
    Closed {
        reason: &'static str,
        correlation: Option<InteractionCorrelation>,
    },
}

fn apply_coordinator_command(
    phase: &mut EditorPhase,
    workspace: &mut NativeEditorSession,
    suppressed: &mut std::collections::BTreeSet<String>,
    command: CoordinatorCommand,
) -> Option<CoordinatorTransition> {
    match command {
        CoordinatorCommand::Open {
            output,
            canvas,
            preview_screen,
            resolved_canvas,
        } if *phase == EditorPhase::Display => {
            workspace.active_output = output;
            workspace.selected_canvas = Some(canvas);
            if let Some(screen) = preview_screen {
                workspace.ui.preview_screen = screen;
            }
            *phase = EditorPhase::Editing;
            Some(CoordinatorTransition::Opened(resolved_canvas))
        }
        CoordinatorCommand::Close {
            reason,
            correlation,
        } if *phase == EditorPhase::Editing => {
            if reason == "discard" {
                suppressed.extend(std::mem::take(&mut workspace.fallback));
            }
            workspace.undo = None;
            workspace.selected_canvas = None;
            workspace.selected_widget = None;
            workspace.pending_widget = None;
            workspace.interaction = None;
            *phase = EditorPhase::Display;
            Some(CoordinatorTransition::Closed {
                reason,
                correlation,
            })
        }
        _ => None,
    }
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
    role: SurfaceRole,
    stop: Arc<std::sync::atomic::AtomicBool>,
    join: std::thread::JoinHandle<Result<(), String>>,
}

impl WorkerControl for NativeWorker {
    fn request_stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
    }
}

fn replace_workspace_outputs(
    workspace: &mut NativeEditorSession,
    outputs: std::collections::BTreeMap<String, OutputDescription>,
) -> bool {
    let next_active = workspace
        .active_output
        .as_ref()
        .filter(|active| outputs.contains_key(*active))
        .cloned()
        .or_else(|| outputs.keys().next().cloned());
    let active_changed = workspace.active_output != next_active;
    workspace.outputs = outputs;
    if active_changed {
        workspace.active_output = next_active;
        workspace.selected_canvas.take();
        workspace.selected_widget.take();
        workspace.pending_widget.take();
        workspace.output_selection_epoch = workspace.output_selection_epoch.saturating_add(1);
    }
    active_changed
}

struct PendingEditorInteraction {
    id: u64,
    action: &'static str,
    source_output: Option<String>,
    started: Instant,
    action_us: u64,
    skin_us: u64,
    dioxus_us: u64,
}

const PENDING_EDITOR_INTERACTION_CAPACITY: usize = 64;

fn enqueue_pending_interaction(
    interactions: &mut std::collections::VecDeque<PendingEditorInteraction>,
    interaction: PendingEditorInteraction,
) -> Option<PendingEditorInteraction> {
    let dropped = (interactions.len() == PENDING_EDITOR_INTERACTION_CAPACITY)
        .then(|| interactions.pop_front())
        .flatten();
    interactions.push_back(interaction);
    dropped
}

fn attribute_skin_duration(
    interactions: &mut std::collections::VecDeque<PendingEditorInteraction>,
    duration_us: u64,
) -> Vec<u64> {
    interactions
        .iter_mut()
        .map(|interaction| {
            interaction.skin_us = interaction.skin_us.saturating_add(duration_us);
            interaction.id
        })
        .collect()
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
    appearance: Rc<Cell<Appearance>>,
    widgets: Rc<RefCell<Vec<WidgetLayout>>>,
    editing: Rc<Cell<bool>>,
    interactive: Rc<Cell<bool>>,
    panel_open: Rc<Cell<bool>>,
    widget_add_open: Rc<Cell<bool>>,
    dirty: Rc<Cell<bool>>,
    undo_available: Rc<Cell<bool>>,
    selected: Rc<RefCell<Option<String>>>,
    pending_widget: Rc<Cell<Option<scorepeek_overlay_ui::WidgetKind>>>,
    pending_point: Rc<Cell<[f64; 2]>>,
    managed: Rc<RefCell<Vec<scorepeek_overlay_ui::CanvasPresentation>>>,
    outputs: Rc<RefCell<Vec<OutputDescription>>>,
    state: Rc<RefCell<OverlayState>>,
    visible: Rc<Cell<bool>>,
    settings: Rc<RefCell<NativeCanvasSettings>>,
    surface_canvas_ids: Rc<RefCell<std::collections::BTreeSet<String>>>,
    reactive: Rc<RefCell<Option<NativeReactiveState>>>,
    actions: Rc<RefCell<Vec<EditorAction>>>,
    surface_actions: Rc<RefCell<Vec<SurfaceAction>>>,
    refresh_rate: Rc<Cell<scorepeek_overlay_ui::WaylandRefreshRate>>,
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

    fn borrow_mut(&self) -> dioxus::signals::WritableRef<'static, Signal<T>> {
        self.0.write_unchecked()
    }

    fn set(&self, value: T) {
        let mut signal = self.0;
        signal.set(value);
    }
}

impl<T: PartialEq + 'static> Reactive<T> {
    fn set_if_changed(&self, value: T) {
        if *self.borrow() != value {
            self.set(value);
        }
    }
}

impl<T: Copy + 'static> Reactive<T> {
    fn get(&self) -> T {
        *self.borrow()
    }
}

impl<T: 'static> Reactive<Option<T>> {
    fn take(&self) -> Option<T> {
        self.borrow_mut().take()
    }
}

#[derive(Clone)]
struct NativeReactiveState {
    appearance: Reactive<Appearance>,
    widgets: Reactive<Vec<WidgetLayout>>,
    editing: Reactive<bool>,
    interactive: Reactive<bool>,
    panel_open: Reactive<bool>,
    widget_add_open: Reactive<bool>,
    dirty: Reactive<bool>,
    undo_available: Reactive<bool>,
    selected: Reactive<Option<String>>,
    pending_widget: Reactive<Option<scorepeek_overlay_ui::WidgetKind>>,
    pending_point: Reactive<[f64; 2]>,
    managed: Reactive<Vec<scorepeek_overlay_ui::CanvasPresentation>>,
    outputs: Reactive<Vec<OutputDescription>>,
    state: Reactive<OverlayState>,
    visible: Reactive<bool>,
    settings: Reactive<NativeCanvasSettings>,
    surface_canvas_ids: Reactive<std::collections::BTreeSet<String>>,
    title_edit: Reactive<Option<TitleEdit>>,
    refresh_edit: Reactive<Option<TitleEdit>>,
    refresh_rate: Reactive<scorepeek_overlay_ui::WaylandRefreshRate>,
    readonly: Reactive<bool>,
}

fn use_native_reactive_state(
    NativeOverlayProps {
        state,
        visible,
        settings,
        surface_canvas_ids,
        reactive,
        appearance,
        widgets,
        editing,
        interactive,
        panel_open,
        widget_add_open,
        dirty,
        undo_available,
        selected,
        pending_widget,
        pending_point,
        managed,
        outputs,
        refresh_rate,
        ..
    }: NativeOverlayProps,
) -> NativeReactiveState {
    let reactive_state = NativeReactiveState {
        readonly: Reactive(use_signal(|| false)),
        title_edit: Reactive(use_signal(|| None)),
        refresh_edit: Reactive(use_signal(|| None)),
        refresh_rate: Reactive(use_signal(move || refresh_rate.get())),
        appearance: Reactive(use_signal(move || appearance.get())),
        widgets: Reactive(use_signal(move || widgets.borrow().clone())),
        editing: Reactive(use_signal(move || editing.get())),
        interactive: Reactive(use_signal(move || interactive.get())),
        panel_open: Reactive(use_signal(move || panel_open.get())),
        widget_add_open: Reactive(use_signal(move || widget_add_open.get())),
        dirty: Reactive(use_signal(move || dirty.get())),
        undo_available: Reactive(use_signal(move || undo_available.get())),
        selected: Reactive(use_signal(move || selected.borrow().clone())),
        pending_widget: Reactive(use_signal(move || pending_widget.get())),
        pending_point: Reactive(use_signal(move || pending_point.get())),
        managed: Reactive(use_signal(move || managed.borrow().clone())),
        outputs: Reactive(use_signal(move || outputs.borrow().clone())),
        state: Reactive(use_signal(move || state.borrow().clone())),
        visible: Reactive(use_signal(move || visible.get())),
        settings: Reactive(use_signal(move || settings.borrow().clone())),
        surface_canvas_ids: Reactive(use_signal(move || surface_canvas_ids.borrow().clone())),
    };
    *reactive.borrow_mut() = Some(reactive_state.clone());
    reactive_state
}

#[allow(clippy::cast_precision_loss)]
fn native_overlay(props: NativeOverlayProps) -> Element {
    let actions = props.actions.clone();
    let surface_actions = props.surface_actions.clone();
    let onsurface = Callback::new(move |action| surface_actions.borrow_mut().push(action));
    let reactive = use_native_reactive_state(props);
    let NativeReactiveState {
        appearance: _,
        widgets: _,
        editing,
        interactive,
        selected,
        managed,
        settings,
        ..
    } = reactive.clone();
    let current = reactive.state.borrow().clone();
    let sample = editing.get() && current.system == scorepeek_overlay_ui::LampState::Inactive;
    let current_settings = settings.borrow().clone();
    let selected_canvas_id = current_settings
        .has_selection
        .then_some(current_settings.id.as_str());
    let surface_canvas_ids = reactive.surface_canvas_ids.borrow().clone();
    let selected_visible = active_editor_canvas_on_surface(
        &current_settings.id,
        current_settings.show_on.as_deref(),
        selected_canvas_id,
        &surface_canvas_ids,
        current_settings.preview_screen,
    );
    rsx! {
      EditorSurface { onaction:onsurface,
        div { class:"canvas-content",style:if editing.get(){format!("display:{};opacity:{};position:absolute;left:{}px;top:{}px;width:{}px;height:{}px",if reactive.visible.get()&&selected_visible{"block"}else{"none"},f32::from(current_settings.opacity_percent)/100.0,current_settings.x,current_settings.y,current_settings.width,current_settings.height)}else{format!("display:{};opacity:{}",if reactive.visible.get(){"block"}else{"none"},f32::from(current_settings.opacity_percent)/100.0)},
            div { id:"scorepeek-skin-root", class:"scorepeek-skin-scope", "data-backend":"native", style:"position:absolute;inset:0" }
        }
        if editing.get() {
            for canvas in managed.borrow().iter().filter(|canvas| active_editor_canvas_on_surface(&canvas.id,canvas.show_on.as_deref(),selected_canvas_id,&surface_canvas_ids,current_settings.preview_screen)) {
                Fragment { key:"{canvas.id}",
                    EditorCanvas {canvas:canvas.clone(),editing:interactive.get(),selected:interactive.get(),selected_widget:selected.borrow().clone(),onaction:onsurface,
                        div {}
                    }
                    if interactive.get() {
                        EditorSelectionMetrics { canvas: canvas.clone(), selected_widget: selected.borrow().clone() }
                    }
                }
            }
        }
        if interactive.get() {
            EditorPanel {
                    view: EditorView {
                        backend_label:"EDITOR".into(),
                        canvases: managed.borrow().clone(),
                        selected_canvas: current_settings.has_selection.then(||current_settings.id.clone()),
                        selected_widget:selected.borrow().clone(),
                        preview_screen:current_settings.preview_screen,
                        outputs:reactive.outputs.borrow().iter().map(|output|EditorOutput {name:output.name.clone(),model:output.model.clone(),logical_size:output.logical_size}).collect(),
                        active_output:current_settings.active_output.clone(),
                        panel_width:current_settings.panel_width,
                        chrome:EditorChrome {panel_open:reactive.panel_open.get(),
                        widget_add_open:reactive.widget_add_open.get(),sample},
                        access:EditorAccess {dirty:reactive.dirty.get(),readonly:reactive.readonly.get(),
                        undo_available:reactive.undo_available.get(),save_validity:{let outputs=reactive.outputs.borrow().iter().map(|output|EditorOutput {name:output.name.clone(),model:output.model.clone(),logical_size:output.logical_size}).collect::<Vec<_>>();if scorepeek_overlay_ui::editor::document_valid(&managed.borrow(),&outputs){scorepeek_overlay_ui::editor::SaveValidity::Valid}else{scorepeek_overlay_ui::editor::SaveValidity::Invalid}}},
                        title:reactive.title_edit.borrow().as_ref().map_or(EditorTitleState::Closed,|edit|if edit.preedit.is_empty(){EditorTitleState::Editing}else{EditorTitleState::Composing}),
                        skins:installed_editor_skins(),
                        new_canvas_skin:current_settings.new_canvas_skin,
                    },
                    title_input:rsx! { if let Some(edit)=reactive.title_edit.borrow().as_ref() {
                        div { class:"empty-title-edit", role:"textbox", "aria-label":"Widget title", "aria-multiline":"false", {title_input_content(edit)} }
                    } },
                    onaction: move |action| actions.borrow_mut().push(action),
            }
            if selected_visible { if let Some(kind) = reactive.pending_widget.get() { PlacementPreview {kind,point:reactive.pending_point.get()} } }
        }
    }
    }
}

fn title_input_content(edit: &TitleEdit) -> Element {
    rsx! {
        span { {edit.text[..edit.range().start].to_owned()} }
        if edit.preedit.is_empty() {
            if edit.cursor == edit.range().start { span { "│" } }
            span { style:"background:#355a83", {edit.text[edit.range()].to_owned()} }
            if edit.cursor != edit.range().start { span { "│" } }
        } else if let Some(cursor) = &edit.preedit_cursor {
            span { style:"text-decoration:underline", {edit.preedit[..cursor.start].to_owned()} }
            span { style:"background:#355a83", {edit.preedit[cursor.clone()].to_owned()} }
            if cursor.is_empty() { span { "│" } }
            span { style:"text-decoration:underline", {edit.preedit[cursor.end..].to_owned()} }
        } else { span { style:"text-decoration:underline", "{edit.preedit}" } }
        span { {edit.text[edit.range().end..].to_owned()} }
    }
}

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

fn release_editor_backend(
    control_socket: &std::path::Path,
    editor_id: &str,
    correlation: Option<&InteractionCorrelation>,
) -> Result<(), String> {
    let started = Instant::now();
    let result = crate::control::request(
        control_socket,
        &crate::control::Request::ReleaseBackend {
            backend: crate::runtime::Backend::Wayland,
            editor_id: editor_id.to_owned(),
        },
    );
    let success = result.as_ref().is_ok_and(|response| response.ok);
    crate::diagnostics::emit(
        "native_editor_control_timing",
        &serde_json::json!({
            "owner": "coordinator",
            "run_id": correlation.map(|item| item.run_id.as_str()),
            "interaction_id": correlation.map(|item| item.interaction_id),
            "action": correlation.map(|item| item.action),
            "request": "release_backend",
            "duration_us": duration_us(started.elapsed()),
            "status": if success { "success" } else { "error" },
            "error_type": (!success).then_some("control_release_failed"),
        }),
    );
    match result {
        Ok(response) if response.ok => Ok(()),
        Ok(response) => Err(response
            .error
            .unwrap_or_else(|| "release Wayland editor backend rejected".into())),
        Err(error) => Err(format!("release Wayland editor backend: {error}")),
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
    let workspace_ui = Arc::new(std::sync::Mutex::new(NativeEditorSession::default()));
    let (coordinator_tx, coordinator_rx) = std::sync::mpsc::channel();
    let editor_id = format!("wayland-{}", std::process::id());
    let mut editor_phase = if config.edit_on_start || desired.is_empty() {
        EditorPhase::Editing
    } else {
        EditorPhase::Display
    };
    let mut workspace_transition_reason =
        (editor_phase == EditorPhase::Editing).then_some("startup");
    let mut next_keepalive = Instant::now() + Duration::from_secs(5);
    workspace_ui
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .wayland_refresh_hz = config.wayland_refresh_hz;
    let wayland_refresh_hz = Arc::new(std::sync::Mutex::new(config.wayland_refresh_hz));
    let mut workspace_was_open = false;
    let suppressed = Arc::new(std::sync::Mutex::new(
        std::collections::BTreeSet::<String>::new(),
    ));
    if editor_phase == EditorPhase::Editing {
        workspace_ui
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .selected_canvas = desired.first().map(|canvas| canvas.id.clone());
    }
    if editor_phase == EditorPhase::Editing {
        let probe = desired
            .first()
            .cloned()
            .map_or_else(|| editor_bootstrap(&config), Ok)?;
        let started = Instant::now();
        let outputs = discover_editor_outputs(&probe)?;
        crate::diagnostics::emit(
            "native_startup_timing",
            &serde_json::json!({
                "phase": "outputs_discovered",
                "output_count": outputs.len(),
                "duration_us": duration_us(started.elapsed()),
                "status": "success",
            }),
        );
        let mut workspace = workspace_ui
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        workspace.outputs = outputs
            .into_iter()
            .map(|output| (output.name.clone(), output))
            .collect();
        workspace.active_output = workspace.outputs.keys().next().cloned();
    }
    if editor_phase == EditorPhase::Editing
        && let Ok(response) = crate::control::request(
            &config.control_socket,
            &crate::control::Request::AcquireBackend {
                backend: crate::runtime::Backend::Wayland,
                editor_id: editor_id.clone(),
            },
        )
    {
        let mut workspace = workspace_ui
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        workspace.draft = response.canvases;
        workspace.dirty = response.dirty;
        workspace.readonly = response.readonly;
        if let Some(refresh) = response.wayland_refresh_hz {
            workspace.wayland_refresh_hz = refresh;
            *wayland_refresh_hz
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = refresh;
        }
    }
    if desired.is_empty()
        && workspace_ui
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .outputs
            .is_empty()
    {
        desired.push(editor_bootstrap(&config)?);
        crate::diagnostics::emit(
            "native_editor_stage",
            &serde_json::json!({"status":"bootstrap","canvas_count":0}),
        );
    }
    while !stop.load(std::sync::atomic::Ordering::Acquire) {
        let mut pending_transition = None;
        let mut pending_transition_started = None;
        while let Ok(command) = coordinator_rx.try_recv() {
            let transition = match command {
                CoordinatorCommand::UpdateDraft {
                    canvases,
                    wayland_refresh_hz: refresh,
                    correlation,
                } => {
                    {
                        let mut workspace = workspace_ui
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        workspace.draft.clone_from(&canvases);
                        workspace.dirty = true;
                        workspace.wayland_refresh_hz = refresh;
                    }
                    let started = Instant::now();
                    let result = crate::control::request(
                        &config.control_socket,
                        &crate::control::Request::UpdateBackendDraft {
                            backend: crate::runtime::Backend::Wayland,
                            editor_id: editor_id.clone(),
                            canvases,
                            wayland_refresh_hz: Some(refresh),
                        },
                    );
                    crate::diagnostics::emit(
                        "native_editor_control_timing",
                        &serde_json::json!({
                            "owner": "coordinator",
                            "run_id": correlation.as_ref().map(|item| item.run_id.as_str()),
                            "interaction_id": correlation.as_ref().map(|item| item.interaction_id),
                            "action": correlation.as_ref().map(|item| item.action),
                            "request": "update_backend_draft",
                            "duration_us": duration_us(started.elapsed()),
                            "status": if result.as_ref().is_ok_and(|response| response.ok) { "success" } else { "error" },
                            "error_type": result.as_ref().err().map(|_| "control_request_failed"),
                        }),
                    );
                    if let Ok(response) = result {
                        let mut workspace = workspace_ui
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        workspace.readonly = response.readonly;
                    }
                    for wake in canvas_wakes
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .values()
                    {
                        wake.ping();
                    }
                    None
                }
                CoordinatorCommand::Save {
                    canvases,
                    wayland_refresh_hz: refresh,
                    correlation,
                } => {
                    let keep_editor_open = canvases.is_empty();
                    let started = Instant::now();
                    let result = crate::control::request(
                        &config.control_socket,
                        &crate::control::Request::CommitBackend {
                            backend: crate::runtime::Backend::Wayland,
                            editor_id: editor_id.clone(),
                            canvases,
                            wayland_refresh_hz: Some(refresh),
                        },
                    );
                    crate::diagnostics::emit(
                        "native_editor_control_timing",
                        &serde_json::json!({
                            "owner": "coordinator",
                            "run_id": correlation.as_ref().map(|item| item.run_id.as_str()),
                            "interaction_id": correlation.as_ref().map(|item| item.interaction_id),
                            "action": correlation.as_ref().map(|item| item.action),
                            "request": "commit_backend",
                            "duration_us": duration_us(started.elapsed()),
                            "status": if result.as_ref().is_ok_and(|response| response.ok) { "success" } else { "error" },
                            "error_type": result.as_ref().err().map(|_| "control_request_failed"),
                        }),
                    );
                    if let Ok(response) = result
                        && response.ok
                    {
                        {
                            let mut workspace = workspace_ui
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            workspace.dirty = response.dirty;
                            workspace.readonly = response.readonly;
                            workspace.fallback.clear();
                            workspace.undo = None;
                            if let Some(refresh) = response.wayland_refresh_hz {
                                workspace.wayland_refresh_hz = refresh;
                                *wayland_refresh_hz
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner) = refresh;
                            }
                        }
                        if keep_editor_open {
                            for wake in canvas_wakes
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .values()
                            {
                                wake.ping();
                            }
                            None
                        } else {
                            apply_coordinator_command(
                                &mut editor_phase,
                                &mut workspace_ui
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                                &mut suppressed
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                                CoordinatorCommand::Close {
                                    reason: "save",
                                    correlation,
                                },
                            )
                        }
                    } else {
                        None
                    }
                }
                CoordinatorCommand::ResolveOutput {
                    output_names,
                    output,
                    canvas,
                    preview_screen,
                    resolved_canvas,
                } => {
                    let started = Instant::now();
                    let result = crate::control::request(
                        &config.control_socket,
                        &crate::control::Request::ResolveWaylandOutputs {
                            outputs: output_names,
                        },
                    );
                    crate::diagnostics::emit(
                        "native_editor_control_timing",
                        &serde_json::json!({
                            "owner": "coordinator",
                            "request": "resolve_wayland_outputs",
                            "duration_us": duration_us(started.elapsed()),
                            "status": if result.as_ref().is_ok_and(|response| response.ok) { "success" } else { "error" },
                            "error_type": result.as_ref().err().map(|_| "control_request_failed"),
                        }),
                    );
                    if result.is_ok_and(|response| response.ok) {
                        apply_coordinator_command(
                            &mut editor_phase,
                            &mut workspace_ui
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner),
                            &mut suppressed
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner),
                            CoordinatorCommand::Open {
                                output,
                                canvas,
                                preview_screen,
                                resolved_canvas: Some(resolved_canvas),
                            },
                        )
                    } else {
                        None
                    }
                }
                command => apply_coordinator_command(
                    &mut editor_phase,
                    &mut workspace_ui
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                    &mut suppressed
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                    command,
                ),
            };
            match &transition {
                Some(CoordinatorTransition::Opened(_)) => {
                    workspace_transition_reason = Some("open");
                }
                Some(CoordinatorTransition::Closed { reason, .. }) => {
                    workspace_transition_reason = Some(*reason);
                }
                None => {}
            }
            if transition.is_some() {
                pending_transition = transition;
                pending_transition_started = Some(Instant::now());
            }
        }
        if editor_phase == EditorPhase::Editing && Instant::now() >= next_keepalive {
            let _ = crate::control::request(
                &config.control_socket,
                &crate::control::Request::KeepAliveBackend {
                    backend: crate::runtime::Backend::Wayland,
                    editor_id: editor_id.clone(),
                },
            );
            next_keepalive = Instant::now() + Duration::from_secs(5);
        }
        let workspace_is_open = editor_phase == EditorPhase::Editing;
        if workspace_was_open != workspace_is_open {
            crate::diagnostics::emit(
                "native_editor_workspace_transition",
                &serde_json::json!({
                    "from": if workspace_was_open { "open" } else { "closed" },
                    "to": if workspace_is_open { "open" } else { "closed" },
                    "reason": workspace_transition_reason.take().unwrap_or("projection"),
                    "phase": "accepted",
                    "worker_count": workers.len(),
                    "status": "accepted",
                }),
            );
        }
        if workspace_was_open
            && !workspace_is_open
            && let Ok(response) = crate::control::request(
                &config.control_socket,
                &crate::control::Request::GetBackend {
                    backend: crate::runtime::Backend::Wayland,
                },
            )
        {
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
            if desired.is_empty() {
                editor_phase = EditorPhase::Editing;
                pending_transition = None;
                pending_transition_started = None;
                workspace_transition_reason = Some("empty_workspace");
                crate::diagnostics::emit(
                    "native_editor_workspace_transition",
                    &serde_json::json!({
                        "from": "closed",
                        "to": "open",
                        "reason": "empty_workspace",
                        "phase": "recovered",
                        "worker_count": workers.len(),
                        "status": "success",
                    }),
                );
            }
        }
        workspace_was_open = editor_phase == EditorPhase::Editing;
        let editing = editor_phase == EditorPhase::Editing;
        let projected = if editing {
            let outputs = workspace_ui
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .outputs
                .values()
                .cloned()
                .collect::<Vec<_>>();
            editor_stage_canvases(&config, &desired, &outputs)?
        } else {
            desired.clone()
        };
        let desired_ids: std::collections::BTreeSet<_> = projected
            .iter()
            .filter(|canvas| {
                editing
                    || !suppressed
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .contains(&canvas.id)
            })
            .map(|canvas| canvas.id.clone())
            .collect();
        let remove: Vec<_> = workers
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
            .collect();
        let stop_started = Instant::now();
        for id in &remove {
            if let Some(worker) = workers.get(id) {
                crate::diagnostics::emit(
                    "native_editor_stage_shutdown",
                    &serde_json::json!({
                        "canvas_id": id,
                        "output": worker.output,
                        "role": match worker.role { SurfaceRole::DisplayCanvas => "display", SurfaceRole::EditorStage => "editor_stage" },
                        "phase": "stop_requested",
                        "reason": "projection_changed",
                    }),
                );
            }
        }
        stop_workers(remove.iter(), &workers, &canvas_wakes);
        for id in remove {
            if let Some(worker) = workers.remove(&id) {
                let output = worker.output.clone();
                match worker.join.join() {
                    Ok(Ok(())) => {
                        crate::diagnostics::emit(
                            "native_editor_stage_shutdown",
                            &serde_json::json!({
                                "canvas_id": id,
                                "output": output,
                                "role": match worker.role { SurfaceRole::DisplayCanvas => "display", SurfaceRole::EditorStage => "editor_stage" },
                                "phase": "stopped",
                                "duration_us": duration_us(stop_started.elapsed()),
                                "status": "success",
                            }),
                        );
                    }
                    Ok(Err(error)) => {
                        if let Some(canvas) = projected.iter().find(|canvas| canvas.id == id) {
                            failed
                                .insert(id.clone(), (Some(canvas.output.clone()), Instant::now()));
                        }
                        crate::diagnostics::emit(
                            "native_canvas_failed",
                            &serde_json::json!({"canvas_id":id,"error":error}),
                        );
                        crate::diagnostics::emit(
                            "native_editor_stage_shutdown",
                            &serde_json::json!({
                                "canvas_id": id,
                                "output": output,
                                "role": match worker.role { SurfaceRole::DisplayCanvas => "display", SurfaceRole::EditorStage => "editor_stage" },
                                "phase": "stopped",
                                "duration_us": duration_us(stop_started.elapsed()),
                                "status": "error",
                                "error_type": canvas_worker_error_type(&error),
                            }),
                        );
                    }
                    Err(_) => {
                        crate::diagnostics::emit(
                            "native_canvas_failed",
                            &serde_json::json!({"canvas_id":id,"error":"panicked"}),
                        );
                        crate::diagnostics::emit(
                            "native_editor_stage_shutdown",
                            &serde_json::json!({
                                "canvas_id": id,
                                "output": output,
                                "role": match worker.role { SurfaceRole::DisplayCanvas => "display", SurfaceRole::EditorStage => "editor_stage" },
                                "phase": "stopped",
                                "duration_us": duration_us(stop_started.elapsed()),
                                "status": "error",
                                "error_type": "canvas_worker_panicked",
                            }),
                        );
                    }
                }
            }
            canvas_wakes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&id);
        }
        let completed_transition = pending_transition.clone();
        match pending_transition {
            Some(CoordinatorTransition::Opened(resolved_canvas)) => {
                let had_resolved_canvas = resolved_canvas.is_some();
                let response = crate::control::request(
                    &config.control_socket,
                    &crate::control::Request::AcquireBackend {
                        backend: crate::runtime::Backend::Wayland,
                        editor_id: editor_id.clone(),
                    },
                );
                let (draft, refresh) = {
                    let mut workspace = workspace_ui
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Ok(response) = response {
                        workspace.draft = response.canvases;
                        workspace.dirty = response.dirty;
                        if let Some(refresh) = response.wayland_refresh_hz {
                            workspace.wayland_refresh_hz = refresh;
                            *wayland_refresh_hz
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner) = refresh;
                        }
                    }
                    if let Some(resolved_canvas) = resolved_canvas
                        && let Some(existing) = workspace
                            .draft
                            .iter_mut()
                            .find(|canvas| canvas.id == resolved_canvas.id)
                    {
                        *existing = resolved_canvas;
                    }
                    (workspace.draft.clone(), workspace.wayland_refresh_hz)
                };
                if had_resolved_canvas {
                    let _ = crate::control::request(
                        &config.control_socket,
                        &crate::control::Request::UpdateBackendDraft {
                            backend: crate::runtime::Backend::Wayland,
                            editor_id: editor_id.clone(),
                            canvases: draft,
                            wayland_refresh_hz: Some(refresh),
                        },
                    );
                }
                next_keepalive = Instant::now() + Duration::from_secs(5);
            }
            Some(CoordinatorTransition::Closed { correlation, .. }) => {
                release_editor_backend(&config.control_socket, &editor_id, correlation.as_ref())?;
            }
            None => {}
        }
        if let Some(transition) = completed_transition {
            crate::diagnostics::emit(
                "native_editor_workspace_transition",
                &serde_json::json!({
                    "from": if matches!(&transition, CoordinatorTransition::Opened(_)) { "closed" } else { "open" },
                    "to": if matches!(&transition, CoordinatorTransition::Opened(_)) { "open" } else { "closed" },
                    "reason": match &transition { CoordinatorTransition::Opened(_) => "open", CoordinatorTransition::Closed { reason, .. } => reason },
                    "run_id": match &transition { CoordinatorTransition::Opened(_) => None, CoordinatorTransition::Closed { correlation, .. } => correlation.as_ref().map(|item| item.run_id.as_str()) },
                    "interaction_id": match &transition { CoordinatorTransition::Opened(_) => None, CoordinatorTransition::Closed { correlation, .. } => correlation.as_ref().map(|item| item.interaction_id) },
                    "action": match &transition { CoordinatorTransition::Opened(_) => None, CoordinatorTransition::Closed { correlation, .. } => correlation.as_ref().map(|item| item.action) },
                    "phase": "previous_surface_set_removed",
                    "worker_count": workers.len(),
                    "duration_us": pending_transition_started.map(|started| duration_us(started.elapsed())),
                    "status": "success",
                }),
            );
        }
        for canvas in &projected {
            if !editing
                && suppressed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contains(&canvas.id)
            {
                continue;
            }
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
            let workspace_ui = Arc::clone(&workspace_ui);
            let refresh_rate = Arc::clone(&wayland_refresh_hz);
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
                        workspace_ui,
                        refresh_rate,
                        wakes,
                        coordinator,
                        role,
                    )
                })
                .map_err(|error| error.to_string())?;
            workers.insert(
                canvas.id.clone(),
                NativeWorker {
                    output: Some(canvas.output.clone()),
                    role,
                    stop: canvas_stop,
                    join,
                },
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let shutdown_started = Instant::now();
    for (id, worker) in &workers {
        crate::diagnostics::emit(
            "native_editor_stage_shutdown",
            &serde_json::json!({
                "canvas_id": id,
                "output": worker.output,
                "role": match worker.role { SurfaceRole::DisplayCanvas => "display", SurfaceRole::EditorStage => "editor_stage" },
                "phase": "stop_requested",
                "reason": "parent_lease_closed",
            }),
        );
    }
    let shutdown_ids = workers.keys().cloned().collect::<Vec<_>>();
    stop_workers(shutdown_ids.iter(), &workers, &canvas_wakes);
    for (id, worker) in workers {
        let output = worker.output.clone();
        let result = worker.join.join();
        let error_type = match &result {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(canvas_worker_error_type(error)),
            Err(_) => Some("canvas_worker_panicked"),
        };
        crate::diagnostics::emit(
            "native_editor_stage_shutdown",
            &serde_json::json!({
                "canvas_id": id,
                "output": output,
                "role": match worker.role { SurfaceRole::DisplayCanvas => "display", SurfaceRole::EditorStage => "editor_stage" },
                "phase": "stopped",
                "duration_us": duration_us(shutdown_started.elapsed()),
                "status": if result.as_ref().is_ok_and(std::result::Result::is_ok) { "success" } else { "error" },
                "error_type": error_type,
            }),
        );
    }
    if editor_phase == EditorPhase::Editing {
        release_editor_backend(&config.control_socket, &editor_id, None)?;
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
    workspace_ui: Arc<std::sync::Mutex<NativeEditorSession>>,
    wayland_refresh_hz: Arc<std::sync::Mutex<scorepeek_overlay_ui::WaylandRefreshRate>>,
    wakes: Arc<std::sync::Mutex<std::collections::BTreeMap<String, Ping>>>,
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
        workspace_ui,
        wakes,
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
    app.emit_unpainted_interaction_terminals();
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
    actions: Rc<RefCell<Vec<EditorAction>>>,
    surface_actions: Rc<RefCell<Vec<SurfaceAction>>>,
    shared_state: Reactive<OverlayState>,
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
    feed_stop: Arc<std::sync::atomic::AtomicBool>,
    external_stop: Arc<std::sync::atomic::AtomicBool>,
    surface_canvas: crate::config::Canvas,
    surface_output: Option<String>,
    canvas: crate::config::Canvas,
    appearance: Reactive<Appearance>,
    role: SurfaceRole,
    coordinator: std::sync::mpsc::Sender<CoordinatorCommand>,
    editing: Reactive<bool>,
    interactive: Reactive<bool>,
    panel_open: Reactive<bool>,
    widget_add_open: Reactive<bool>,
    dirty: Reactive<bool>,
    undo_available: Reactive<bool>,
    readonly: Reactive<bool>,
    editor_pointer_observation: EditorPointerObservation,
    input_started: Option<Instant>,
    interaction_sequence: u64,
    pending_interactions: std::collections::VecDeque<PendingEditorInteraction>,
    selected: Reactive<Option<String>>,
    pending_widget: Reactive<Option<scorepeek_overlay_ui::WidgetKind>>,
    pending_point: Reactive<[f64; 2]>,
    shared_widgets: Reactive<Vec<WidgetLayout>>,
    interaction: Option<Drag>,
    output_selection_epoch: u64,
    managed: Reactive<Vec<scorepeek_overlay_ui::CanvasPresentation>>,
    outputs: Reactive<Vec<OutputDescription>>,
    visible: Reactive<bool>,
    pending_resolved_output: Option<String>,
    next_output_persist: Instant,
    workspace_ui: Arc<std::sync::Mutex<NativeEditorSession>>,
    workspace_wakes: Arc<std::sync::Mutex<std::collections::BTreeMap<String, Ping>>>,
    surface_canvas_ids: Reactive<std::collections::BTreeSet<String>>,
    settings: Reactive<NativeCanvasSettings>,
    surface_logical: [u32; 2],
    title_edit: Reactive<Option<TitleEdit>>,
    refresh_edit: Reactive<Option<TitleEdit>>,
    refresh_rate: Reactive<scorepeek_overlay_ui::WaylandRefreshRate>,
    wayland_refresh_hz: Arc<std::sync::Mutex<scorepeek_overlay_ui::WaylandRefreshRate>>,
    skin_runtime: crate::skin::Runtime,
    preview_skin_runtime: Option<(String, String, crate::skin::Runtime)>,
    skin_tree: crate::skin::NativeTree,
    next_skin_render: Option<Instant>,
    skin_release: String,
    skin_manifest: crate::skin::Manifest,
    skin_package: crate::skin::Package,
    skin_store: std::path::PathBuf,
    skin_assets: Arc<std::sync::Mutex<Option<crate::skin::Package>>>,
    skin_package_open_count: u64,
    startup_started: Instant,
}

impl App {
    #[allow(
        clippy::cast_precision_loss,
        clippy::too_many_arguments,
        clippy::too_many_lines
    )]
    fn new(
        appearance: Appearance,
        widgets: Vec<WidgetLayout>,
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
        workspace_ui: Arc<std::sync::Mutex<NativeEditorSession>>,
        workspace_wakes: Arc<std::sync::Mutex<std::collections::BTreeMap<String, Ping>>>,
        wayland_refresh_hz: Arc<std::sync::Mutex<scorepeek_overlay_ui::WaylandRefreshRate>>,
        startup_started: Instant,
        coordinator: std::sync::mpsc::Sender<CoordinatorCommand>,
        role: SurfaceRole,
    ) -> Result<Self, String> {
        let shared_state = Rc::new(RefCell::new(OverlayState::default()));
        let reactive = Rc::new(RefCell::new(None));
        let shared_widgets = Rc::new(RefCell::new(widgets));
        let editing = Rc::new(Cell::new(role == SurfaceRole::EditorStage));
        let (ui, output_selection_epoch, initial_draft, initial_readonly) = {
            let workspace = workspace_ui
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                workspace.ui,
                workspace.output_selection_epoch,
                workspace.draft.clone(),
                workspace.readonly,
            )
        };
        let interactive = Rc::new(Cell::new(
            role == SurfaceRole::EditorStage
                && workspace_ui
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .active_output
                    .as_deref()
                    == shell.output_name.as_deref(),
        ));
        let panel_open = Rc::new(Cell::new(ui.panel_open));
        let widget_add_open = Rc::new(Cell::new(ui.widget_add_open));
        let dirty = Rc::new(Cell::new(
            workspace_ui
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .dirty,
        ));
        let undo_available = Rc::new(Cell::new(false));
        let selected = Rc::new(RefCell::new(None));
        let pending_widget = Rc::new(Cell::new(None));
        let pending_point = Rc::new(Cell::new([340.0, 24.0]));
        let managed = Rc::new(RefCell::new(initial_draft));
        let outputs = Rc::new(RefCell::new(outputs));
        let appearance = Rc::new(Cell::new(appearance));
        let refresh_rate = Rc::new(Cell::new(
            *wayland_refresh_hz
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        ));
        let panel_width = editor_panel_width(shell.output_logical_size.map(|[width, _]| width));
        let settings = Rc::new(RefCell::new(NativeCanvasSettings {
            id: canvas.id.clone(),
            has_selection: true,
            output: Some(canvas.output.clone()),
            active_output: Some(canvas.output.clone()),
            show_on: canvas.show_on.clone(),
            background: canvas.background,
            opacity_percent: canvas.opacity_percent,
            x: canvas.x,
            y: canvas.y,
            width: canvas.width,
            height: canvas.height,
            preview_screen: ui.preview_screen,
            panel_width,
            new_canvas_skin: canvas.skin,
        }));
        let initially_visible = role == SurfaceRole::EditorStage || canvas.show_on.is_none();
        shell.set_input_enabled(role == SurfaceRole::DisplayCanvas || interactive.get());
        let visible = Rc::new(Cell::new(initially_visible));
        let surface_canvas_ids = Rc::new(RefCell::new(std::collections::BTreeSet::from([canvas
            .id
            .clone()])));
        let actions = Rc::new(RefCell::new(Vec::new()));
        let surface_actions = Rc::new(RefCell::new(Vec::new()));
        let package = crate::skin::StoreRoot::new(skin_store.clone()).open(canvas.skin.name())?;
        let vdom = VirtualDom::new_with_props(
            native_overlay,
            NativeOverlayProps {
                appearance: Rc::clone(&appearance),
                widgets: Rc::clone(&shared_widgets),
                editing: Rc::clone(&editing),
                interactive: Rc::clone(&interactive),
                panel_open: Rc::clone(&panel_open),
                widget_add_open: Rc::clone(&widget_add_open),
                dirty: Rc::clone(&dirty),
                undo_available: Rc::clone(&undo_available),
                selected: Rc::clone(&selected),
                pending_widget: Rc::clone(&pending_widget),
                pending_point: Rc::clone(&pending_point),
                managed: Rc::clone(&managed),
                outputs: Rc::clone(&outputs),
                state: Rc::clone(&shared_state),
                visible: Rc::clone(&visible),
                settings: Rc::clone(&settings),
                surface_canvas_ids: Rc::clone(&surface_canvas_ids),
                reactive: Rc::clone(&reactive),
                actions: actions.clone(),
                surface_actions: surface_actions.clone(),
                refresh_rate: Rc::clone(&refresh_rate),
            },
        );
        let (document_config, skin_assets) = document_config_with_skin_handle(package.clone());
        let mut document = DioxusDocument::new(vdom, document_config);
        document.initial_build();
        let mut skin_runtime = new_native_skin_runtime(
            &package,
            &report,
            &canvas.id,
            shell.output_name.as_deref(),
            &[],
        )?;
        let skin_input = native_skin_input(&canvas, &OverlayState::default(), &package.manifest);
        let started = Instant::now();
        let initial = skin_runtime.init(&skin_input).inspect_err(|error| {
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
        skin_tree.apply(&mut document.inner.borrow_mut(), &initial);
        crate::diagnostics::emit(
            "skin_render",
            &serde_json::json!({"skin_id":package.manifest.id,"release":package.manifest.release,"canvas_id":canvas.id,"backend":"native","phase":"init","status":"success","duration_us":u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),"tree_applied":true}),
        );
        let next_skin_render = skin_deadline(&initial.schedule, false);
        let reactive = reactive
            .borrow()
            .clone()
            .expect("native overlay must publish its reactive state during initial build");
        reactive.readonly.set_if_changed(initial_readonly);
        let surface_logical = [canvas.width, canvas.height];

        if pending_resolved_output.is_some() {
            workspace_ui
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .fallback
                .insert(canvas.id.clone());
        }
        let surface_output = shell.output_name.clone();
        {
            let mut workspace = workspace_ui
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            workspace.outputs = shell
                .output_descriptions
                .iter()
                .map(|output| (output.name.clone(), output.clone()))
                .collect();
            if workspace.active_output.is_none() {
                workspace.active_output = workspace.outputs.keys().next().cloned();
            }
        }
        Ok(Self {
            renderer,
            shell,
            document,
            pointer: PointerInput::default(),
            actions,
            surface_actions,
            shared_state: reactive.state,
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
            feed_stop,
            external_stop,
            surface_canvas: canvas.clone(),
            surface_output,
            canvas,
            appearance: reactive.appearance,
            role,
            coordinator,
            editing: reactive.editing,
            interactive: reactive.interactive,
            panel_open: reactive.panel_open,
            widget_add_open: reactive.widget_add_open,
            dirty: reactive.dirty,
            undo_available: reactive.undo_available,
            readonly: reactive.readonly,
            editor_pointer_observation: EditorPointerObservation::default(),
            input_started: None,
            interaction_sequence: 0,
            pending_interactions: std::collections::VecDeque::new(),
            selected: reactive.selected,
            pending_widget: reactive.pending_widget,
            pending_point: reactive.pending_point,
            shared_widgets: reactive.widgets,
            interaction: None,
            output_selection_epoch,

            managed: reactive.managed,
            outputs: reactive.outputs,
            pending_resolved_output,
            next_output_persist: Instant::now() + Duration::from_secs(1),
            visible: reactive.visible,
            workspace_ui,
            workspace_wakes,
            surface_canvas_ids: reactive.surface_canvas_ids,
            settings: reactive.settings,
            surface_logical,
            title_edit: reactive.title_edit,
            refresh_edit: reactive.refresh_edit,
            refresh_rate: reactive.refresh_rate,
            wayland_refresh_hz,
            skin_runtime,
            preview_skin_runtime: None,
            skin_tree,
            next_skin_render,
            skin_release: package.manifest.release.clone(),
            skin_manifest: package.manifest.clone(),
            skin_package: package,
            skin_store,
            skin_assets,
            skin_package_open_count: 1,
            startup_started,
        })
    }
    #[allow(clippy::too_many_lines)]
    fn run(&mut self) -> Result<(), String> {
        let _ = self.poll_dioxus();
        while !self.feed_stop.load(std::sync::atomic::Ordering::Acquire)
            && !self
                .external_stop
                .load(std::sync::atomic::Ordering::Acquire)
        {
            self.sync_workspace_projection();
            let should_edit = self.role == SurfaceRole::EditorStage;
            let should_interact = should_edit
                && self
                    .workspace_ui
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .active_output
                    .as_deref()
                    == self.surface_output.as_deref();
            if self.interactive.get() != should_interact {
                self.interactive.set(should_interact);
                self.shell.set_input_enabled(surface_input_enabled(
                    should_edit,
                    should_interact,
                    self.visible.get(),
                ));
            }
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
            let mut wake = false;
            let mut frame = false;
            let mut configured = false;
            let mut document_changed = false;
            if *self.outputs.borrow() != self.shell.output_descriptions {
                self.outputs
                    .borrow_mut()
                    .clone_from(&self.shell.output_descriptions);
                let outputs = self
                    .shell
                    .output_descriptions
                    .iter()
                    .map(|output| (output.name.clone(), output.clone()))
                    .collect();
                let active_changed = replace_workspace_outputs(
                    &mut self
                        .workspace_ui
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                    outputs,
                );
                if active_changed {
                    self.wake_workspace();
                }
                wake = true;
            }
            for event in events {
                match event {
                    Event::Configure {
                        logical,
                        physical,
                        scale_120,
                    } => {
                        let changed = self.configure(logical, physical, scale_120)?;
                        configured |= changed;
                        wake |= changed;
                    }
                    Event::Wake => wake = true,
                    Event::PointerMotion { x, y } => {
                        self.pointer_motion(x, y);
                        wake = true;
                    }
                    Event::PointerButton {
                        button,
                        pressed,
                        x,
                        y,
                    } => {
                        self.pointer_button(button, pressed, x, y);
                        wake = true;
                    }
                    Event::PointerScroll { dx, dy, x, y } => {
                        if self.editing.get() && self.panel_open.get() {
                            self.pointer.wheel(&mut self.document, [x, y], [dx, dy]);
                            document_changed = true;
                            wake = true;
                        }
                    }
                    Event::Text(command) => {
                        self.input_command(&command);
                        wake = true;
                    }
                    Event::Ime(update) => {
                        if let Some(edit) = self.refresh_edit.borrow_mut().as_mut() {
                            edit.ime(update);
                            self.update_refresh_input(true);
                        } else if let Some(edit) = self.title_edit.borrow_mut().as_mut() {
                            edit.ime(update);
                            self.update_title_input(true);
                        }
                        wake = true;
                    }
                    Event::KeyboardFocus(focused) => {
                        crate::diagnostics::emit(
                            "title_input_focus",
                            &serde_json::json!({"focused":focused}),
                        );
                        if !focused {
                            self.finish_title_edit(false);
                            self.finish_refresh_edit(false);
                            wake = true;
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
            let visible = self.editing.get()
                || scorepeek_overlay_ui::canvas_visible(
                    self.surface_canvas.show_on.as_deref(),
                    latest.screen,
                );
            let visibility_changed = self.visible.get() != visible;
            if visibility_changed {
                self.visible.set(visible);
                let accepts_input = visible && (!self.editing.get() || self.interactive.get());
                self.shell.set_input_enabled(accepts_input);
                crate::diagnostics::emit(
                    "native_canvas_visibility",
                    &serde_json::json!({
                        "canvas_id": self.canvas.id,
                        "visible": visible,
                        "screen_revision": latest.screen.revision,
                        "reason": if self.editing.get() { "editor_preview" } else { "screen_state" },
                    }),
                );
                wake = true;
            }
            if *self.shared_state.borrow() != latest {
                *self.shared_state.borrow_mut() = latest.clone();
                if visible {
                    if self.editing.get() {
                        self.editor_skin_updates.request();
                    } else {
                        self.render_skin(&latest)?;
                    }
                }
                wake = true;
            } else if visibility_changed && visible && !self.editing.get() {
                self.render_skin(&latest)?;
            }
            if visible
                && self
                    .next_skin_render
                    .is_some_and(|deadline| Instant::now() >= deadline)
            {
                if self.editing.get() {
                    self.editor_skin_updates.request();
                } else {
                    self.render_skin(&latest)?;
                }
                wake = true;
            }
            if self.editing.get()
                && self
                    .editor_skin_updates
                    .take_if_ready(frame, self.interaction.is_some())
            {
                self.update_editor_skin();
                wake = true;
            }
            let changed = wake && self.poll_dioxus();
            self.pending_paint |= changed || visibility_changed || document_changed;
            let paint_state = PaintState {
                editing: self.editing.get(),
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
                    if changed {
                        crate::diagnostics::emit(
                            "native_state_paint",
                            &serde_json::json!({"paint_count":self.paint_count,"render_calls":self.render_calls}),
                        );
                    }
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
        let Some(output) = self.pending_resolved_output.clone() else {
            return;
        };
        let output_names = self
            .outputs
            .borrow()
            .iter()
            .map(|output| output.name.clone())
            .collect();
        let mut fallback = self.canvas.clone();
        fallback.output.clone_from(&output);
        self.next_output_persist = Instant::now() + Duration::from_secs(1);
        let _ = self.coordinator.send(CoordinatorCommand::ResolveOutput {
            output_names,
            output: self.surface_output.clone(),
            canvas: self.canvas.id.clone(),
            preview_screen: self.shared_state.borrow().screen.kind,
            resolved_canvas: fallback.presentation(),
        });
        crate::diagnostics::emit(
            "native_output_fallback",
            &serde_json::json!({
                "canvas_id": self.canvas.id,
                "selected_output": output,
                "status": "resolve_requested",
            }),
        );
    }
    fn apply_selected_presentation(
        &mut self,
        presentation: &scorepeek_overlay_ui::CanvasPresentation,
    ) {
        self.canvas =
            crate::config::empty_canvas(presentation.id.clone(), crate::runtime::Backend::Wayland);
        self.canvas.apply_presentation(presentation);
        self.appearance.set(Appearance {
            skin: presentation.skin,
        });
        self.shared_widgets
            .borrow_mut()
            .clone_from(&presentation.widgets);
        self.settings.borrow_mut().apply_presentation(presentation);
    }

    #[allow(clippy::too_many_lines)]
    fn sync_workspace_projection(&mut self) {
        let (
            ui,
            draft,
            dirty,
            readonly,
            undo_available,
            selected_widget,
            pending_widget,
            interaction,
            wayland_refresh_hz,
            active_output,
            output_selection_epoch,
        ) = {
            let workspace = self
                .workspace_ui
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                workspace.ui,
                workspace.draft.clone(),
                workspace.dirty,
                workspace.readonly,
                workspace.undo.is_some(),
                workspace.selected_widget.clone(),
                workspace.pending_widget,
                workspace.interaction.clone(),
                workspace.wayland_refresh_hz,
                workspace.active_output.clone(),
                workspace.output_selection_epoch,
            )
        };
        if self.output_selection_epoch != output_selection_epoch {
            self.output_selection_epoch = output_selection_epoch;
            self.workspace_ui
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .selected_canvas
                .take();
            self.selected.set(None);
            self.pending_widget.set(None);
            self.interaction = None;
            self.finish_title_edit(false);
            self.finish_refresh_edit(false);
        }
        let surface_canvas_ids =
            surface_canvas_ids_for_draft(&draft, self.surface_output.as_deref());
        self.refresh_rate.set_if_changed(wayland_refresh_hz);
        if self.settings.borrow().active_output != active_output {
            self.settings.borrow_mut().active_output = active_output;
        }
        if *self.surface_canvas_ids.borrow() != surface_canvas_ids {
            self.surface_canvas_ids
                .borrow_mut()
                .clone_from(&surface_canvas_ids);
        }
        self.panel_open.set_if_changed(ui.panel_open);
        self.widget_add_open.set_if_changed(ui.widget_add_open);
        let has_selection = self
            .workspace_ui
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .selected_canvas
            .is_some();
        let settings_changed = {
            let settings = self.settings.borrow();
            settings.preview_screen != ui.preview_screen || settings.has_selection != has_selection
        };
        if settings_changed {
            let mut settings = self.settings.borrow_mut();
            settings.preview_screen = ui.preview_screen;
            settings.has_selection = has_selection;
        }
        self.dirty.set_if_changed(dirty);
        self.readonly.set_if_changed(readonly);
        self.undo_available.set_if_changed(undo_available);
        if *self.selected.borrow() != selected_widget {
            self.finish_title_edit(false);
            *self.selected.borrow_mut() = selected_widget;
        }
        self.pending_widget.set_if_changed(pending_widget);
        if self.interactive.get() && self.interaction.is_none() && interaction.is_some() {
            self.interaction = interaction;
        }
        if self.interaction.is_none() && *self.managed.borrow() != draft {
            self.managed.borrow_mut().clone_from(&draft);
        }
        if self.editing.get() && self.interaction.is_none() {
            let selected = self
                .workspace_ui
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .selected_canvas
                .clone();
            let presentation = selected.as_deref().and_then(|id| {
                self.managed
                    .borrow()
                    .iter()
                    .find(|canvas| canvas.id == id)
                    .cloned()
            });
            if let Some(presentation) = presentation
                && surface_canvas_ids.contains(&presentation.id)
                && self.canvas.presentation() != presentation
            {
                if self.canvas.id != presentation.id {
                    self.finish_title_edit(false);
                }
                self.apply_selected_presentation(&presentation);
                self.update_editor_skin();
            }
        }
    }
    fn set_editor_geometry(&mut self, editing: bool) {
        if !editing {
            self.shell.set_geometry(
                self.surface_canvas.x,
                self.surface_canvas.y,
                self.surface_canvas.width,
                self.surface_canvas.height,
            );
            return;
        }
        let ([x, y], [width, height]) = editor_geometry(
            [self.canvas.x, self.canvas.y],
            [self.canvas.width, self.canvas.height],
            self.shell.output_logical_size,
        );
        self.shell.set_geometry(x, y, width, height);
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::too_many_lines
    )]
    fn pointer_button(&mut self, button: u32, pressed: bool, x: f64, y: f64) {
        if button != 0x110 && button != 0x111 {
            return;
        }
        if !self.editing.get() {
            if button == 0x111 && pressed {
                let _ = self.coordinator.send(CoordinatorCommand::Open {
                    output: self.surface_output.clone(),
                    canvas: self.canvas.id.clone(),
                    preview_screen: self.shared_state.borrow().screen.kind,
                    resolved_canvas: None,
                });
            }
            return;
        }
        if pressed && self.editor_pointer_observation == EditorPointerObservation::AwaitingPress {
            self.editor_pointer_observation = EditorPointerObservation::AwaitingRelease;
        }
        if !pressed {
            self.input_started = Some(Instant::now());
        }
        self.pointer
            .dispatch(&mut self.document, [x, y], button, Some(pressed));
        if !pressed && self.editor_pointer_observation == EditorPointerObservation::AwaitingRelease
        {
            crate::diagnostics::emit(
                "native_editor_pointer",
                &serde_json::json!({
                    "status": "dispatched",
                    "button": if button == 0x111 { "secondary" } else { "primary" },
                    "readonly": self.readonly.get(),
                    "editor_actions": self.actions.borrow().len(),
                    "surface_actions": self.surface_actions.borrow().len(),
                }),
            );
            self.editor_pointer_observation = EditorPointerObservation::Observed;
        }
        self.drain_editor_events();
        if !pressed {
            self.input_started = None;
        }
    }
    fn drain_editor_events(&mut self) {
        let actions = std::mem::take(&mut *self.actions.borrow_mut());
        for action in actions {
            self.editor_action(&action);
        }
        let actions = std::mem::take(&mut *self.surface_actions.borrow_mut());
        for action in actions {
            self.surface_action(action);
        }
    }
    fn editor_model(&self) -> EditorModel {
        let mut model = EditorModel::new(self.draft_snapshot(), self.surface_logical, "wayland");
        model.set_skins(installed_editor_skins());
        model.set_outputs(
            self.outputs
                .borrow()
                .iter()
                .map(|output| EditorOutput {
                    name: output.name.clone(),
                    model: output.model.clone(),
                    logical_size: output.logical_size,
                })
                .collect(),
        );
        model.activate_output(self.surface_output.as_deref());
        model.readonly = self.readonly.get();
        model.editing = self.editing.get();
        model.preview = self.settings.borrow().preview_screen;
        model.selected_canvas = self
            .settings
            .borrow()
            .has_selection
            .then(|| self.canvas.id.clone());
        model.selected_widget.clone_from(&self.selected.borrow());
        model.chrome.panel_open = self.panel_open.get();
        model.chrome.widget_add_open = self.widget_add_open.get();
        model.new_canvas_skin = self.settings.borrow().new_canvas_skin;
        model.placing = self.pending_widget.get();
        model.drag.clone_from(&self.interaction);
        model
    }
    fn apply_editor_model(&mut self, model: EditorModel) {
        let previous = self.canvas.presentation();
        if let Some(canvas) = model.current() {
            self.apply_selected_presentation(canvas);
        }
        self.managed.set(model.draft);
        self.settings.borrow_mut().preview_screen = model.preview;
        self.settings.borrow_mut().new_canvas_skin = model.new_canvas_skin;
        self.panel_open.set(model.chrome.panel_open);
        self.widget_add_open.set(model.chrome.widget_add_open);
        self.pending_widget.set(model.placing);
        self.select_canvas(model.selected_canvas);
        self.selected.set(model.selected_widget);
        self.interaction = model.drag;
        if editor_skin_presentation_changed(&previous, &self.canvas.presentation()) {
            self.editor_skin_updates.request();
        }
        self.sync_workspace_ui();
    }

    fn update_editor_skin(&mut self) {
        let started = Instant::now();
        self.editor_skin_updates.pending = false;
        self.editor_skin_updates.renders = self.editor_skin_updates.renders.saturating_add(1);
        let result = self.render_editor_skin();
        if let Err(error) = &result {
            self.next_skin_render = None;
            self.animating = false;
            self.skin_tree.apply(
                &mut self.document.inner.borrow_mut(),
                &crate::skin::RenderOutput {
                    schedule: crate::skin::Schedule::Idle,
                    tree: crate::skin::Node::Element {
                        key: "skin-preview-error".into(),
                        tag: "div".into(),
                        attributes: std::collections::BTreeMap::from([
                            ("class".into(), "skin-preview-error".into()),
                            (
                                "style".into(),
                                "box-sizing:border-box;width:100%;height:100%;padding:16px;background:#24080a;color:#ffb4b8;font:16px sans-serif".into(),
                            ),
                        ]),
                        children: vec![crate::skin::Node::Text {
                            key: "skin-preview-error-text".into(),
                            text: "SKIN PREVIEW UNAVAILABLE".into(),
                        }],
                    },
                },
            );
            crate::diagnostics::emit(
                "skin_render",
                &serde_json::json!({"skin_id":self.canvas.skin.name(),"canvas_id":self.canvas.id,"backend":"native","phase":if self.editing.get() { "editor-preview" } else { "render" },"status":"failed","error_type":skin_error_type(error),"tree_applied":true}),
            );
        }
        let duration = started.elapsed();
        let interaction_ids =
            attribute_skin_duration(&mut self.pending_interactions, duration_us(duration));
        crate::diagnostics::emit(
            "native_editor_skin_timing",
            &serde_json::json!({
                "run_id": self.report.borrow().run_id,
                "interaction_ids": interaction_ids,
                "canvas_id": self.canvas.id,
                "output": self.surface_output,
                "duration_us": duration_us(duration),
                "startup_elapsed_us": duration_us(self.startup_started.elapsed()),
                "status": if result.is_ok() { "success" } else { "error" },
                "error_type": result.as_ref().err().map(|error| skin_error_type(error)),
            }),
        );
    }

    fn create_skin_runtime(&self) -> Result<crate::skin::Runtime, String> {
        let interaction_ids = self
            .pending_interactions
            .iter()
            .map(|interaction| interaction.id)
            .collect::<Vec<_>>();
        new_native_skin_runtime(
            &self.skin_package,
            &self.report,
            &self.canvas.id,
            self.surface_output.as_deref(),
            &interaction_ids,
        )
    }

    fn render_editor_skin(&mut self) -> Result<(), String> {
        let state = if self.editing.get()
            && self.shared_state.borrow().system == scorepeek_overlay_ui::LampState::Inactive
        {
            scorepeek_overlay_ui::editor_sample_state()
        } else {
            self.shared_state.borrow().clone()
        };
        let desired_skin = self.canvas.skin.name();
        if self.skin_package.manifest.id != desired_skin {
            let package =
                crate::skin::StoreRoot::new(self.skin_store.clone()).open(desired_skin)?;
            *self
                .skin_assets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(package.clone());
            self.skin_package = package;
            self.skin_package_open_count = self.skin_package_open_count.saturating_add(1);
        }
        if self.editing.get() {
            let same_preview =
                self.preview_skin_runtime
                    .as_ref()
                    .is_some_and(|(id, release, _)| {
                        id == &self.skin_package.manifest.id
                            && release == &self.skin_package.manifest.release
                    });
            let input = native_skin_input(&self.canvas, &state, &self.skin_package.manifest);
            let output = if same_preview {
                self.preview_skin_runtime
                    .as_mut()
                    .expect("matching preview runtime must exist")
                    .2
                    .render(&input)?
            } else {
                let mut runtime = self.create_skin_runtime()?;
                let output = runtime.init(&input)?;
                self.preview_skin_runtime = Some((
                    self.skin_package.manifest.id.clone(),
                    self.skin_package.manifest.release.clone(),
                    runtime,
                ));
                output
            };
            if same_preview {
                self.skin_tree
                    .apply(&mut self.document.inner.borrow_mut(), &output);
            } else {
                let css = std::str::from_utf8(
                    self.skin_package
                        .resource(crate::skin::STYLE_PATH)
                        .ok_or("skin.css missing")?,
                )
                .map_err(|error| format!("skin.css is not UTF-8: {error}"))?;
                self.skin_tree
                    .replace(&mut self.document.inner.borrow_mut(), css, &output);
            }
            self.next_skin_render = skin_deadline(&output.schedule, self.editing.get());
            self.animating = false;
        } else if self.skin_manifest.id == self.skin_package.manifest.id
            && self.skin_release == self.skin_package.manifest.release
        {
            self.render_skin(&state)?;
            let css = std::str::from_utf8(
                self.skin_package
                    .resource(crate::skin::STYLE_PATH)
                    .ok_or("skin.css missing")?,
            )
            .map_err(|error| format!("skin.css is not UTF-8: {error}"))?;
            self.skin_tree
                .set_css(&mut self.document.inner.borrow_mut(), css);
        } else {
            let mut runtime = self.create_skin_runtime()?;
            let output = runtime.init(&native_skin_input(
                &self.canvas,
                &state,
                &self.skin_package.manifest,
            ))?;
            let css = std::str::from_utf8(
                self.skin_package
                    .resource(crate::skin::STYLE_PATH)
                    .ok_or("skin.css missing")?,
            )
            .map_err(|error| format!("skin.css is not UTF-8: {error}"))?;
            self.skin_tree
                .replace(&mut self.document.inner.borrow_mut(), css, &output);
            self.next_skin_render = skin_deadline(&output.schedule, self.editing.get());
            self.animating = matches!(output.schedule, crate::skin::Schedule::NextFrame);
            self.skin_release
                .clone_from(&self.skin_package.manifest.release);
            self.skin_manifest = self.skin_package.manifest.clone();
            self.skin_runtime = runtime;
        }
        Ok(())
    }
    fn surface_action(&mut self, action: SurfaceAction) {
        if !self.editing.get() {
            return;
        }
        if matches!(action, SurfaceAction::Enter(_)) {
            return;
        }
        if let Some(point) = passive_pointer_move(&action, self.interaction.is_some()) {
            self.pending_point
                .set([f64::from(point[0]), f64::from(point[1])]);
            return;
        }
        if matches!(
            action,
            SurfaceAction::Start { .. } | SurfaceAction::Select(_)
        ) {
            self.finish_title_edit(false);
        }
        let mut model = self.editor_model();
        let changed = model.surface(action);
        let undo = model.undo.take();
        self.pending_point
            .set([f64::from(model.point[0]), f64::from(model.point[1])]);
        self.apply_editor_model(model);
        if changed && let Some(before) = undo {
            self.finish_draft_change(DraftUndo {
                canvases: before,
                wayland_refresh_hz: self.refresh_rate.get(),
            });
        }
    }
    #[allow(clippy::too_many_lines)]
    fn editor_action(&mut self, action: &EditorAction) {
        let started = self.input_started.take().unwrap_or_else(Instant::now);
        self.interaction_sequence = self.interaction_sequence.saturating_add(1);
        let interaction_id = self.interaction_sequence;
        let action_name = editor_action_name(action);
        let dropped = enqueue_pending_interaction(
            &mut self.pending_interactions,
            PendingEditorInteraction {
                id: interaction_id,
                action: action_name,
                source_output: self.surface_output.clone(),
                started,
                action_us: 0,
                skin_us: 0,
                dioxus_us: 0,
            },
        );
        if let Some(dropped) = dropped {
            crate::diagnostics::emit(
                "native_editor_interaction",
                &serde_json::json!({
                    "run_id": self.report.borrow().run_id,
                    "interaction_id": dropped.id,
                    "action": dropped.action,
                    "source_output": dropped.source_output,
                    "phase": "dropped",
                    "duration_us": duration_us(dropped.started.elapsed()),
                    "status": "dropped",
                    "error_type": "interaction_queue_full",
                }),
            );
        }
        self.editor_action_inner(action);
        if let Some(interaction) = self
            .pending_interactions
            .back_mut()
            .filter(|interaction| interaction.id == interaction_id)
        {
            interaction.action_us = duration_us(started.elapsed());
            crate::diagnostics::emit(
                "native_editor_interaction",
                &serde_json::json!({
                    "run_id": self.report.borrow().run_id,
                    "interaction_id": interaction.id,
                    "action": interaction.action,
                    "source_output": interaction.source_output,
                    "phase": "state_applied",
                    "duration_us": interaction.action_us,
                    "status": "success",
                }),
            );
        }
    }

    #[allow(clippy::too_many_lines)]
    fn editor_action_inner(&mut self, action: &EditorAction) {
        if let EditorAction::SelectOutput(output) = action {
            let mut workspace = self
                .workspace_ui
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            workspace.active_output = Some(output.clone());
            workspace.output_selection_epoch = workspace.output_selection_epoch.saturating_add(1);
            workspace.interaction = None;
            drop(workspace);
            self.interaction = None;
            self.select_canvas(None);
            return;
        }
        if !matches!(action, EditorAction::AcceptTitle | EditorAction::EditTitle) {
            self.finish_title_edit(false);
        }
        self.finish_refresh_edit(false);
        match action {
            EditorAction::Save => {
                self.save_and_close();
                return;
            }
            EditorAction::Close | EditorAction::Discard => {
                let _ = self.coordinator.send(CoordinatorCommand::Close {
                    reason: if *action == EditorAction::Discard {
                        "discard"
                    } else {
                        "close"
                    },
                    correlation: self.current_interaction_correlation(),
                });
                return;
            }
            EditorAction::Undo => {
                self.finish_refresh_edit(false);
                self.undo_last_change();
                return;
            }
            _ => {}
        }
        if *action == EditorAction::AcceptTitle {
            self.finish_title_edit(true);
            return;
        }
        if *action == EditorAction::CancelTitle {
            self.finish_title_edit(false);
            return;
        }
        let before = self.undo_snapshot();
        let mut model = self.editor_model();
        model.action(action);
        if *action == EditorAction::AddCanvas
            && let Some(canvas) = model.draft.last_mut()
        {
            canvas.output.clone_from(&self.surface_output);
        }
        let title = model.title.clone();
        self.apply_editor_model(model);
        if let Some(title) = title {
            crate::diagnostics::emit(
                "title_edit_started",
                &serde_json::json!({"canvas_id":self.canvas.id}),
            );
            self.title_edit
                .set(Some(TitleEdit::new(title.widget, title.text)));
            self.update_title_input(false);
        }
        self.finish_draft_change(before);
    }

    fn pointer_motion(&mut self, x: f64, y: f64) {
        self.pointer
            .dispatch(&mut self.document, [x, y], 0x110, None);
        self.drain_editor_events();
        self.shell.set_cursor(
            if self
                .interaction
                .as_ref()
                .is_some_and(|drag| drag.corner.is_some())
            {
                CursorStyle::Resize
            } else if self.interaction.is_some() {
                CursorStyle::Grabbing
            } else {
                CursorStyle::Default
            },
        );
    }
    fn persist_canvas(&mut self) {
        self.sync_canvas_to_managed();
        self.update_draft();
    }
    fn sync_canvas_to_managed(&mut self) {
        {
            let settings = self.settings.borrow();
            self.canvas.show_on.clone_from(&settings.show_on);
            self.canvas.background = settings.background;
            self.canvas.opacity_percent = settings.opacity_percent;
        }
        let mut presentation = self.canvas.presentation();
        presentation.skin = self.appearance.get().skin;
        presentation
            .widgets
            .clone_from(&self.shared_widgets.borrow());
        if let Some(existing) = self
            .managed
            .borrow_mut()
            .iter_mut()
            .find(|canvas| canvas.id == presentation.id)
        {
            *existing = presentation;
        }
    }

    fn draft_snapshot(&self) -> Vec<scorepeek_overlay_ui::CanvasPresentation> {
        self.managed.borrow().clone()
    }

    fn undo_snapshot(&self) -> DraftUndo {
        DraftUndo {
            canvases: self.draft_snapshot(),
            wayland_refresh_hz: self.refresh_rate.get(),
        }
    }

    fn finish_draft_change(&mut self, before: DraftUndo) {
        let changed = {
            let after = self.managed.borrow();
            let mut workspace = self
                .workspace_ui
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            remember_draft_change(&mut workspace.undo, before, &after, self.refresh_rate.get())
        };
        if !changed {
            return;
        }
        self.undo_available.set(true);
        self.dirty.set_if_changed(true);
        self.update_draft();
    }

    fn undo_last_change(&mut self) {
        let Some(undo) = self
            .workspace_ui
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .undo
            .take()
        else {
            return;
        };
        let DraftUndo {
            canvases: restored,
            wayland_refresh_hz,
        } = undo;
        let mut model = self.editor_model();
        model.undo = Some(restored);
        model.action(&EditorAction::Undo);
        self.apply_editor_model(model);
        self.set_refresh_rate_draft(wayland_refresh_hz);
        self.undo_available.set(false);
        self.set_editor_geometry(true);
        self.update_draft();
    }

    fn sync_workspace_ui(&self) {
        self.workspace_ui
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .ui = EditorWorkspaceUi {
            panel_open: self.panel_open.get(),
            preview_screen: self.settings.borrow().preview_screen,
            widget_add_open: self.widget_add_open.get(),
        };
        let mut workspace = self
            .workspace_ui
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        workspace
            .selected_widget
            .clone_from(&self.selected.borrow());
        workspace.pending_widget = self.pending_widget.get();
        workspace.interaction.clone_from(&self.interaction);
        drop(workspace);
        self.wake_workspace();
    }

    fn wake_workspace(&self) {
        for wake in self
            .workspace_wakes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
        {
            wake.ping();
        }
    }

    fn current_interaction_correlation(&self) -> Option<InteractionCorrelation> {
        self.pending_interactions
            .back()
            .map(|interaction| InteractionCorrelation {
                run_id: self.report.borrow().run_id.clone(),
                interaction_id: interaction.id,
                action: interaction.action,
            })
    }

    fn select_canvas(&self, next: Option<String>) {
        let mut workspace = self
            .workspace_ui
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        replace_canvas_selection(
            &mut workspace.selected_canvas,
            &mut self.selected.borrow_mut(),
            next,
        );
        drop(workspace);
        self.sync_workspace_ui();
    }
    fn update_draft(&mut self) {
        let canvases = self.managed.borrow().clone();
        self.dirty.set_if_changed(true);
        let outputs = self
            .outputs
            .borrow()
            .iter()
            .map(|output| EditorOutput {
                name: output.name.clone(),
                model: output.model.clone(),
                logical_size: output.logical_size,
            })
            .collect::<Vec<_>>();
        if let Some(command) = coordinator_draft_update(
            canvases,
            &outputs,
            self.refresh_rate.get(),
            self.current_interaction_correlation(),
        ) {
            let _ = self.coordinator.send(command);
        }
    }
    fn save_and_close(&mut self) {
        if self.refresh_edit.borrow().is_some() && !self.finish_refresh_edit(true) {
            return;
        }
        let mut model = self.editor_model();
        model.normalize_for_save();
        if !model.document_valid() {
            return;
        }
        self.apply_editor_model(model);
        self.persist_canvas();
        let canvases = self.managed.borrow().clone();
        let _ = self.coordinator.send(CoordinatorCommand::Save {
            canvases,
            wayland_refresh_hz: self.refresh_rate.get(),
            correlation: self.current_interaction_correlation(),
        });
    }

    fn set_refresh_rate_draft(&self, refresh: scorepeek_overlay_ui::WaylandRefreshRate) {
        self.refresh_rate.set_if_changed(refresh);
        *self
            .wayland_refresh_hz
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = refresh;
        self.workspace_ui
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .wayland_refresh_hz = refresh;
    }

    fn poll_dioxus(&mut self) -> bool {
        let started = Instant::now();
        let mut changed = false;
        while self
            .document
            .poll(Some(TaskContext::from_waker(&self.waker)))
        {
            changed = true;
        }
        // Blitz incremental damage may retain layout-child IDs removed by a Dioxus mutation.
        // Rebuild layout once before resuming incremental animation paints.
        self.full_layout_pending |= changed;
        for interaction in &mut self.pending_interactions {
            interaction.dioxus_us = interaction
                .dioxus_us
                .saturating_add(duration_us(started.elapsed()));
        }
        changed
    }
    fn render_skin(&mut self, state: &OverlayState) -> Result<(), String> {
        let started = Instant::now();
        let output = self
            .skin_runtime
            .render(&native_skin_input(&self.canvas, state, &self.skin_manifest))
            .inspect_err(|error| {
                crate::diagnostics::emit(
                    "skin_render",
                    &serde_json::json!({"skin_id":self.canvas.skin.name(),"release":self.skin_release,"canvas_id":self.canvas.id,"backend":"native","phase":"render","status":"failed","error_type":skin_error_type(error),"duration_us":u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)}),
                );
            })?;
        self.skin_tree
            .apply(&mut self.document.inner.borrow_mut(), &output);
        self.next_skin_render = skin_deadline(&output.schedule, self.editing.get());
        self.animating = matches!(output.schedule, crate::skin::Schedule::NextFrame);
        crate::diagnostics::emit(
            "skin_render",
            &serde_json::json!({"skin_id":self.canvas.skin.name(),"release":self.skin_release,"canvas_id":self.canvas.id,"backend":"native","phase":"render","status":"success","duration_us":u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),"next_tick":format!("{:?}",output.schedule),"tree_applied":true}),
        );
        Ok(())
    }
    fn configure(
        &mut self,
        logical: [u32; 2],
        physical: [u32; 2],
        scale_120: u32,
    ) -> Result<bool, String> {
        let [width, height] = physical;
        self.surface_logical = logical;
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
            f32::from(u16::try_from(scale_120).map_err(|e| e.to_string())?) / 120.0,
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
    fn emit_unpainted_interaction_terminals(&mut self) {
        for interaction in self.pending_interactions.drain(..) {
            crate::diagnostics::emit(
                "native_editor_interaction",
                &serde_json::json!({
                    "run_id": self.report.borrow().run_id,
                    "interaction_id": interaction.id,
                    "action": interaction.action,
                    "source_output": interaction.source_output,
                    "paint_output": self.surface_output,
                    "phase": "closed_before_paint",
                    "action_us": interaction.action_us,
                    "skin_us": interaction.skin_us,
                    "dioxus_us": interaction.dioxus_us,
                    "duration_us": duration_us(interaction.started.elapsed()),
                    "status": "canceled",
                }),
            );
        }
    }

    fn paint(&mut self, reason: PaintReason, now: Duration) -> Result<(), String> {
        let first_paint = self.paint_count == 0;
        let paint_started = Instant::now();
        let result = self.paint_exclusive();
        let paint_duration = paint_started.elapsed();
        if first_paint {
            crate::diagnostics::emit(
                "native_startup_timing",
                &serde_json::json!({
                    "run_id": self.report.borrow().run_id,
                    "canvas_id": self.surface_canvas.id,
                    "output": self.surface_output,
                    "phase": "first_paint",
                    "paint_us": duration_us(paint_duration),
                    "elapsed_us": duration_us(self.startup_started.elapsed()),
                    "status": if result.is_ok() { "success" } else { "error" },
                    "error_type": result.as_ref().err().map(|_| "renderer_paint_failed"),
                }),
            );
        }
        if let Err(error) = result {
            for interaction in self.pending_interactions.drain(..) {
                crate::diagnostics::emit(
                    "native_editor_interaction",
                    &serde_json::json!({
                        "run_id": self.report.borrow().run_id,
                        "interaction_id": interaction.id,
                        "action": interaction.action,
                        "source_output": interaction.source_output,
                        "paint_output": self.surface_output,
                        "phase": "paint_failed",
                        "action_us": interaction.action_us,
                        "skin_us": interaction.skin_us,
                        "dioxus_us": interaction.dioxus_us,
                        "paint_us": duration_us(paint_duration),
                        "duration_us": duration_us(interaction.started.elapsed()),
                        "status": "error",
                        "error_type": "renderer_paint_failed",
                    }),
                );
            }
            return Err(error);
        }
        self.cadence.record(now);
        self.pending_paint = false;
        if reason == PaintReason::Steady {
            self.steady_paint_count = self.steady_paint_count.saturating_add(1);
        } else if reason.bypasses_cap() {
            let mut report = self.report.borrow_mut();
            *report
                .cap_bypass_paints
                .entry(reason.name().to_owned())
                .or_default() += 1;
        }
        for interaction in self.pending_interactions.drain(..) {
            crate::diagnostics::emit(
                "native_editor_interaction",
                &serde_json::json!({
                    "run_id": self.report.borrow().run_id,
                    "interaction_id": interaction.id,
                    "action": interaction.action,
                    "source_output": interaction.source_output,
                    "paint_output": self.surface_output,
                    "phase": "painted",
                    "action_us": interaction.action_us,
                    "skin_us": interaction.skin_us,
                    "dioxus_us": interaction.dioxus_us,
                    "paint_us": duration_us(paint_duration),
                    "duration_us": duration_us(interaction.started.elapsed()),
                    "status": "success",
                }),
            );
        }
        Ok(())
    }

    fn paint_exclusive(&mut self) -> Result<(), String> {
        if !self.renderer.is_active() {
            return Err("native renderer inactive".into());
        }
        let mut inner = self.document.inner.borrow_mut();
        let seconds = self.started.elapsed().as_secs_f64();
        if self.visible.get() {
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
        self.animating = self.visible.get();
        // Every present publishes the next compositor callback. Visible editor previews share the
        // same presentation motion as ordinary canvases; lifecycle paints also need the callback
        // to release frame-paced surface resources before another state change is rendered.
        self.shell.request_frame();
        let (width, height) = inner.viewport().window_size;
        let scale = inner.viewport().scale_f64();
        self.renderer.render(|scene| {
            paint_native_scene(scene, &mut inner, scale, width, height);
        });
        self.paint_count += 1;
        self.render_calls += 1;
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
const fn editor_action_name(action: &EditorAction) -> &'static str {
    match action {
        EditorAction::TogglePanel => "toggle_panel",
        EditorAction::PreviewScreen(_) => "preview_screen",
        EditorAction::SelectOutput(_) => "select_output",
        EditorAction::SelectCanvas(_) => "select_canvas",
        EditorAction::CanvasName(_) => "canvas_name",
        EditorAction::CanvasVisible(_, _) => "canvas_visibility",
        EditorAction::CanvasVisibleAll => "canvas_visibility_all",
        EditorAction::CanvasVisibleNone => "canvas_visibility_none",
        EditorAction::CanvasGeometry(_, _) => "canvas_geometry",
        EditorAction::WidgetGeometry(_, _) => "widget_geometry",
        EditorAction::AddCanvas => "add_canvas",
        EditorAction::DeleteCanvas => "delete_canvas",
        EditorAction::NewCanvasSkin(_) => "new_canvas_skin",
        EditorAction::Skin(_) => "skin",
        EditorAction::CanvasSkinProperty(_, _) => "canvas_skin_property",
        EditorAction::WidgetSkinProperty(_, _) => "widget_skin_property",
        EditorAction::Background(_) => "background",
        EditorAction::Opacity(_) => "opacity",
        EditorAction::Output(_) => "output",
        EditorAction::FitToOutput => "fit_to_output",
        EditorAction::SelectWidget { .. } => "select_widget",
        EditorAction::ToggleWidgetAdd => "toggle_widget_add",
        EditorAction::AddWidget(_) => "add_widget",
        EditorAction::Undo => "undo",
        EditorAction::Discard => "discard",
        EditorAction::Save => "save",
        EditorAction::Close => "close",
        EditorAction::FrameWidth(_) => "frame_width",
        EditorAction::EditTitle => "edit_title",
        EditorAction::AcceptTitle => "accept_title",
        EditorAction::CancelTitle => "cancel_title",
        EditorAction::FillDelta(_) => "fill_delta",
        EditorAction::FillOpacity(_) => "fill_opacity",
        EditorAction::AspectRatio(_) => "aspect_ratio",
        EditorAction::HistoryCount(_) => "history_count",
        EditorAction::GraphMonths(_) => "graph_months",
        EditorAction::DeleteWidget => "delete_widget",
    }
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
    actions: Rc<RefCell<Vec<EditorAction>>>,
    surface_actions: Rc<RefCell<Vec<SurfaceAction>>>,
    logical_size: [u32; 2],
    physical_size: [u32; 2],
    scale: f32,
    appearance: Reactive<Appearance>,
    widgets: Reactive<Vec<WidgetLayout>>,
    editing: Reactive<bool>,
    interactive: Reactive<bool>,
    panel_open: Reactive<bool>,
    widget_add_open: Reactive<bool>,
    selected: Reactive<Option<String>>,
    managed: Reactive<Vec<scorepeek_overlay_ui::CanvasPresentation>>,
    state: Reactive<OverlayState>,
    visible: Reactive<bool>,
    settings: Reactive<NativeCanvasSettings>,
    #[cfg(test)]
    surface_canvas_ids: Reactive<std::collections::BTreeSet<String>>,
    title_edit: Reactive<Option<TitleEdit>>,
    skin: Option<(
        crate::skin::Runtime,
        crate::skin::NativeTree,
        crate::skin::Manifest,
    )>,
    skin_assets: Arc<std::sync::Mutex<Option<crate::skin::Package>>>,
}

impl VisualDebugSession {
    #[allow(clippy::too_many_lines)]
    fn new(scenario: &VisualDebugScenario, physical_size: [u32; 2]) -> Result<Self, String> {
        let config = crate::config::visual_debug_config();
        let mut managed = config
            .canvases
            .into_iter()
            .filter(|canvas| canvas.backend == crate::runtime::Backend::Wayland)
            .map(|canvas| canvas.presentation())
            .collect::<Vec<_>>();
        if let Some(canvases) = &scenario.canvases {
            managed.clone_from(canvases);
        }
        if let Some(skin) = scenario.skin {
            for canvas in &mut managed {
                canvas.skin = skin;
            }
        }
        let selected_index = scenario
            .canvas_id
            .as_ref()
            .map_or(Some(0), |id| {
                managed.iter().position(|canvas| &canvas.id == id)
            })
            .ok_or_else(|| "canvas_id does not select a Wayland canvas".to_owned())?;
        let canvas = managed
            .get(selected_index)
            .cloned()
            .ok_or_else(|| "the initial Wayland workspace is empty".to_owned())?;
        let appearance = Rc::new(Cell::new(Appearance { skin: canvas.skin }));
        let widgets = Rc::new(RefCell::new(canvas.widgets.clone()));
        let editing = Rc::new(Cell::new(scenario.editing));
        let interactive = Rc::new(Cell::new(scenario.editing));
        let panel_open = Rc::new(Cell::new(true));
        let widget_add_open = Rc::new(Cell::new(false));
        let selected = Rc::new(RefCell::new(None));
        let surface_canvas_ids = managed
            .iter()
            .map(|canvas| canvas.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let managed = Rc::new(RefCell::new(managed));
        let settings = Rc::new(RefCell::new(NativeCanvasSettings {
            id: canvas.id.clone(),
            has_selection: true,
            output: canvas.output.clone(),
            active_output: canvas.output.clone(),
            show_on: canvas.show_on.clone(),
            background: canvas.background,
            opacity_percent: canvas.opacity_percent,
            x: canvas.x,
            y: canvas.y,
            width: canvas.width,
            height: canvas.height,
            preview_screen: scorepeek_overlay_ui::ScreenKind::MusicSelect,
            panel_width: editor_panel_width(Some(scenario.logical_size[0])),
            new_canvas_skin: canvas.skin,
        }));
        let reactive = Rc::new(RefCell::new(None));
        let actions = Rc::new(RefCell::new(Vec::new()));
        let surface_actions = Rc::new(RefCell::new(Vec::new()));
        let props = NativeOverlayProps {
            appearance: Rc::clone(&appearance),
            widgets: Rc::clone(&widgets),
            editing: Rc::clone(&editing),
            interactive: Rc::clone(&interactive),
            panel_open: Rc::clone(&panel_open),
            widget_add_open: Rc::clone(&widget_add_open),
            dirty: Rc::new(Cell::new(false)),
            undo_available: Rc::new(Cell::new(false)),
            selected: Rc::clone(&selected),
            pending_widget: Rc::new(Cell::new(None)),
            pending_point: Rc::new(Cell::new([0.0, 0.0])),
            managed: Rc::clone(&managed),
            outputs: Rc::new(RefCell::new(vec![OutputDescription {
                name: "HEADLESS-1".into(),
                model: "scorepeek visual debugger".into(),
                logical_size: Some(scenario.logical_size),
            }])),
            state: Rc::new(RefCell::new(scorepeek_overlay_ui::editor_sample_state())),
            visible: Rc::new(Cell::new(true)),
            settings: Rc::clone(&settings),
            surface_canvas_ids: Rc::new(RefCell::new(surface_canvas_ids)),
            reactive: Rc::clone(&reactive),
            actions: actions.clone(),
            surface_actions: surface_actions.clone(),
            refresh_rate: Rc::new(Cell::new(scorepeek_overlay_ui::WaylandRefreshRate::Auto)),
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
        let reactive = reactive
            .borrow()
            .clone()
            .ok_or_else(|| "native overlay did not publish its reactive state".to_owned())?;
        let skin = if let Some(package) = package {
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
            .map_err(|error| format!("skin.css is not UTF-8: {error}"))?;
            let mut runtime = crate::skin::Runtime::new(&package)?;
            let initial = runtime.init(&native_skin_input_presentation(
                &canvas,
                &scorepeek_overlay_ui::editor_sample_state(),
                &package.manifest,
            ))?;
            let mut tree =
                crate::skin::NativeTree::new(&mut document.inner.borrow_mut(), root, css);
            tree.apply(&mut document.inner.borrow_mut(), &initial);
            Some((runtime, tree, package.manifest))
        } else {
            None
        };
        let mut session = Self {
            document,
            pointer: PointerInput::default(),
            actions,
            surface_actions,
            logical_size: scenario.logical_size,
            physical_size,
            scale: scenario.scale,
            appearance: reactive.appearance,
            widgets: reactive.widgets,
            editing: reactive.editing,
            interactive: reactive.interactive,
            panel_open: reactive.panel_open,
            widget_add_open: reactive.widget_add_open,
            selected: reactive.selected,
            managed: reactive.managed,
            state: reactive.state,
            visible: reactive.visible,
            settings: reactive.settings,
            #[cfg(test)]
            surface_canvas_ids: reactive.surface_canvas_ids,
            title_edit: reactive.title_edit,
            skin,
            skin_assets,
        };
        session.resolve();
        Ok(session)
    }

    fn resolve(&mut self) {
        while self
            .document
            .poll(Some(TaskContext::from_waker(Waker::noop())))
        {}
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
        let canvas_id = self.settings.borrow().id.clone();
        let Some(canvas) = self
            .managed
            .borrow()
            .iter()
            .find(|canvas| canvas.id == canvas_id)
            .cloned()
        else {
            return Ok(());
        };
        let state = self.state.borrow().clone();
        let desired_skin = canvas.skin.name();
        if self
            .skin
            .as_ref()
            .is_some_and(|(_, _, manifest)| manifest.id != desired_skin)
        {
            let package = crate::skin::StoreRoot::discover().open(desired_skin)?;
            let css = std::str::from_utf8(
                package
                    .resource(crate::skin::STYLE_PATH)
                    .ok_or("skin.css missing")?,
            )
            .map_err(|error| format!("skin.css is not UTF-8: {error}"))?;
            let mut runtime = crate::skin::Runtime::new(&package)?;
            let output = runtime.init(&native_skin_input_presentation(
                &canvas,
                &state,
                &package.manifest,
            ))?;
            *self
                .skin_assets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(package.clone());
            let slot = self.skin.as_mut().expect("checked visual skin runtime");
            slot.1
                .replace(&mut self.document.inner.borrow_mut(), css, &output);
            slot.0 = runtime;
            slot.2 = package.manifest;
            return Ok(());
        }
        if let Some((runtime, tree, manifest)) = &mut self.skin {
            let output =
                runtime.render(&native_skin_input_presentation(&canvas, &state, manifest))?;
            tree.apply(&mut self.document.inner.borrow_mut(), &output);
        }
        Ok(())
    }

    fn set_screen(&mut self, screen: Option<scorepeek_overlay_ui::ScreenKind>) {
        {
            let mut state = self.state.borrow_mut();
            state.screen.kind = screen;
            state.screen.suspended_since_unix_ms = None;
            state.screen.revision = state.screen.revision.saturating_add(1);
            self.visible.set(
                self.editing.get()
                    || scorepeek_overlay_ui::canvas_visible(
                        self.settings.borrow().show_on.as_deref(),
                        state.screen,
                    ),
            );
        }
        self.resolve();
    }

    fn load_canvas(&self, canvas: scorepeek_overlay_ui::CanvasPresentation) {
        self.appearance.set(Appearance { skin: canvas.skin });
        self.widgets.borrow_mut().clone_from(&canvas.widgets);
        let preview_screen = self.settings.borrow().preview_screen;
        let active_output = self.settings.borrow().active_output.clone();
        *self.settings.borrow_mut() = NativeCanvasSettings {
            id: canvas.id,
            has_selection: true,
            output: canvas.output,
            active_output,
            show_on: canvas.show_on,
            background: canvas.background,
            opacity_percent: canvas.opacity_percent,
            x: canvas.x,
            y: canvas.y,
            width: canvas.width,
            height: canvas.height,
            preview_screen,
            panel_width: editor_panel_width(Some(self.logical_size[0])),
            new_canvas_skin: canvas.skin,
        };
    }

    fn editor_model(&self) -> EditorModel {
        let mut model =
            EditorModel::new(self.managed.borrow().clone(), self.logical_size, "wayland");
        model.set_skins(installed_editor_skins());
        model.readonly = false;
        model.editing = self.editing.get();
        model.preview = self.settings.borrow().preview_screen;
        model.selected_canvas = self
            .settings
            .borrow()
            .has_selection
            .then(|| self.settings.borrow().id.clone());
        model.selected_widget.clone_from(&self.selected.borrow());
        model.chrome.panel_open = self.panel_open.get();
        model.chrome.widget_add_open = self.widget_add_open.get();
        model.new_canvas_skin = self.settings.borrow().new_canvas_skin;
        model
    }
    fn apply_editor_model(&self, model: &EditorModel) {
        self.managed.set(model.draft.clone());
        self.settings.borrow_mut().preview_screen = model.preview;
        self.settings.borrow_mut().new_canvas_skin = model.new_canvas_skin;
        if let Some(canvas) = model.current() {
            self.load_canvas(canvas.clone());
        }
        self.settings.borrow_mut().has_selection = model.selected_canvas.is_some();
        self.selected.set(model.selected_widget.clone());
        self.panel_open.set(model.chrome.panel_open);
        self.widget_add_open.set(model.chrome.widget_add_open);
    }
    fn drain_editor_events(&mut self, model: &mut EditorModel) {
        let actions = std::mem::take(&mut *self.actions.borrow_mut());
        for action in actions {
            if let Some(edit) = self.title_edit.borrow().as_ref() {
                model.title = Some(scorepeek_overlay_ui::editor_model::TitleDraft {
                    canvas: self.settings.borrow().id.clone(),
                    widget: edit.widget.clone(),
                    text: edit.text.clone(),
                    composing: !edit.preedit.is_empty(),
                });
            }
            model.action(&action);
            self.title_edit.set(
                model
                    .title
                    .as_ref()
                    .map(|title| TitleEdit::new(title.widget.clone(), title.text.clone())),
            );
        }
        let actions = std::mem::take(&mut *self.surface_actions.borrow_mut());
        for action in actions {
            model.surface(action);
        }
        self.apply_editor_model(model);
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
        let mut model = self.editor_model();
        self.pointer.click(&mut self.document, point);
        self.drain_editor_events(&mut model);
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

    fn drag(
        &mut self,
        from: [f64; 2],
        to: [f64; 2],
        button: VisualDebugButton,
    ) -> Result<(), String> {
        if !self.editing.get() {
            return Err("drag requires editing".into());
        }
        let button = match button {
            VisualDebugButton::Right => 0x111,
            VisualDebugButton::Left => 0x110,
        };
        let mut model = self.editor_model();
        self.pointer
            .dispatch(&mut self.document, from, button, None);
        self.pointer
            .dispatch(&mut self.document, from, button, Some(true));
        self.drain_editor_events(&mut model);
        if model.drag.is_none() {
            return Err("drag did not reach a Dioxus canvas handler".into());
        }
        self.pointer.dispatch(&mut self.document, to, button, None);
        self.drain_editor_events(&mut model);
        self.pointer
            .dispatch(&mut self.document, to, button, Some(false));
        self.drain_editor_events(&mut model);
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
        let mut elements = Vec::with_capacity(selectors.len());
        for selector in selectors {
            let nodes = inner
                .query_selector_all(selector)
                .map_err(|_| format!("invalid selector: {selector}"))?;
            let matches = nodes
                .iter()
                .filter_map(|node| inner.get_client_bounding_rect(*node))
                .map(|rect| [rect.x, rect.y, rect.width, rect.height])
                .collect();
            elements.push(VisualDebugElement {
                selector: selector.clone(),
                matches,
            });
        }
        Ok(VisualDebugLayout {
            schema_version: 1,
            logical_size: self.logical_size,
            physical_size: self.physical_size,
            scale: self.scale,
            elements,
        })
    }
}

fn visual_debug_physical_size(logical_size: [u32; 2], scale: f32) -> Result<[u32; 2], String> {
    fn scaled(logical: u32, scale: f32) -> Result<u32, String> {
        let physical = (f64::from(logical) * f64::from(scale)).ceil();
        if !physical.is_finite() || physical <= 0.0 || physical > f64::from(u32::MAX) {
            Err("scaled physical size is out of range".to_owned())
        } else {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Ok(physical as u32)
        }
    }

    if logical_size.contains(&0) || !scale.is_finite() || scale <= 0.0 {
        return Err("logical size and scale must be positive".into());
    }

    Ok([
        scaled(logical_size[0], scale)?,
        scaled(logical_size[1], scale)?,
    ])
}

fn validate_visual_debug_scenario(scenario: &VisualDebugScenario) -> Result<[u32; 2], String> {
    let physical_size = visual_debug_physical_size(scenario.logical_size, scenario.scale)?;
    if let Some(canvas_id) = scenario.canvas_id.as_ref()
        && !crate::config::visual_debug_config()
            .canvases
            .iter()
            .any(|canvas| {
                canvas.backend == crate::runtime::Backend::Wayland && canvas.id == *canvas_id
            })
    {
        return Err("canvas_id does not select a Wayland canvas".into());
    }
    Ok(physical_size)
}

/// Runs the deterministic native visual debugger without connecting to Wayland.
///
/// The output directory must not exist. Every operation produces a PNG and a selector-layout JSON;
/// `manifest.json` correlates them and records a typed failure when the scenario stops early.
///
/// # Errors
/// Returns a scenario, interaction, rendering, or artifact-write error after recording it in the
/// run manifest whenever the output directory was created successfully.
#[allow(clippy::too_many_lines)]
pub fn run_visual_debug(
    scenario: &VisualDebugScenario,
    output: &std::path::Path,
) -> Result<(), String> {
    std::fs::create_dir(output).map_err(|error| format!("create output directory: {error}"))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let physical_size = validate_visual_debug_scenario(scenario);
    let scenario_invalid = physical_size.is_err();
    let mut manifest = VisualDebugManifest {
        schema_version: 1,
        run_id: format!("{timestamp}-{}", std::process::id()),
        resource: VisualDebugResource {
            program: "scorepeek-overlay-native-visual-debug",
            version: env!("CARGO_PKG_VERSION"),
            renderer: "dioxus-native-dom/blitz/vello",
        },
        logical_size: scenario.logical_size,
        physical_size: physical_size.as_ref().ok().copied(),
        scale: scenario.scale,
        status: "running",
        completeness: "partial",
        operations: Vec::new(),
        error: None,
    };
    let result: Result<(), String> = (|| {
        let validated_physical_size = physical_size.as_ref().map_err(Clone::clone)?;
        let mut session = VisualDebugSession::new(scenario, *validated_physical_size)?;
        let mut renderer = anyrender_vello::VelloImageRenderer::new(
            validated_physical_size[0],
            validated_physical_size[1],
        );
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
                    {
                        let mut editing = session.title_edit.borrow_mut();
                        let edit = editing.as_mut().ok_or("title input is not active")?;
                        edit.ime(if *composing {
                            scorepeek_overlay_handles::TextUpdate {
                                preedit: text.clone(),
                                ..Default::default()
                            }
                        } else {
                            scorepeek_overlay_handles::TextUpdate {
                                commit: Some(text.clone()),
                                ..Default::default()
                            }
                        });
                    }
                    session.resolve();
                    if *composing {
                        "title-preedit"
                    } else {
                        "title-commit"
                    }
                    .into()
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
                    session.editing.set(*value);
                    session.interactive.set(*value);
                    session.resolve();
                    format!("set-editing-{value}")
                }
                VisualDebugAction::SetScreen { screen } => {
                    session.set_screen(*screen);
                    screen.map_or_else(
                        || "set-screen-none".to_owned(),
                        |screen| format!("set-screen-{screen:?}"),
                    )
                }
                VisualDebugAction::Click { selector } => {
                    session.click(selector)?;
                    "click".into()
                }
                VisualDebugAction::Scroll { selector, dx, dy } => {
                    session.scroll(selector, *dx, *dy)?;
                    "scroll".into()
                }
                VisualDebugAction::Drag { from, to, button } => {
                    session.drag(*from, *to, *button)?;
                    "drag".into()
                }
                VisualDebugAction::Capture { name } => {
                    session.resolve();
                    sanitize_artifact_name(name)
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
        Ok(())
    })();
    match &result {
        Ok(()) => {
            manifest.status = "success";
            manifest.completeness = "complete";
        }
        Err(message) => {
            manifest.status = "error";
            manifest.error = Some(VisualDebugError {
                operation: format!("operation-{}", manifest.operations.len()),
                error_type: if scenario_invalid {
                    "scenario_invalid"
                } else if message.contains("selector") || message.contains("drag") {
                    "interaction_target_invalid"
                } else {
                    "artifact_write_failed"
                },
                message: message.clone(),
            });
        }
    }
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
    std::fs::write(output.join("manifest.json"), bytes)
        .map_err(|error| format!("write manifest: {error}"))?;
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
    fn pending_editor_interactions_retain_every_action_until_paint() {
        let pending = |id| PendingEditorInteraction {
            id,
            action: "select_canvas",
            source_output: Some("output".into()),
            started: Instant::now(),
            action_us: 0,
            skin_us: 0,
            dioxus_us: 0,
        };
        let mut interactions = std::collections::VecDeque::new();
        assert!(enqueue_pending_interaction(&mut interactions, pending(1)).is_none());
        assert!(enqueue_pending_interaction(&mut interactions, pending(2)).is_none());
        assert_eq!(attribute_skin_duration(&mut interactions, 37), vec![1, 2]);
        assert!(
            interactions
                .iter()
                .all(|interaction| interaction.skin_us == 37)
        );
        assert_eq!(
            interactions
                .drain(..)
                .map(|interaction| interaction.id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
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
    fn widget_body_click_selects_over_rendered_content() {
        let scenario: VisualDebugScenario =
            serde_json::from_str(include_str!("../tests/fixtures/visual-debug.json")).unwrap();
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        session
            .click(".screen-picker .list-picker-trigger")
            .unwrap();
        session
            .click(".screen-picker .list-picker-option[data-index='4']")
            .unwrap();
        session.scroll(".navigator-scroll", 0.0, -2000.0).unwrap();
        session
            .click(".canvas-select[data-canvas-id='wayland-result']")
            .unwrap();
        session.panel_open.set(false);
        session.resolve();
        for id in ["selection", "score", "history-list", "history-graph"] {
            session
                .click(&format!(".editor-widget-hit[data-widget='{id}']"))
                .unwrap();
            assert_eq!(
                session.selected.borrow().as_deref(),
                Some(id),
                "body click should select {id}"
            );
        }
    }

    #[test]
    fn editor_styles_survive_zero_visible_canvases_and_last_canvas_off() {
        let scenario: VisualDebugScenario =
            serde_json::from_str(include_str!("../tests/fixtures/visual-empty-editor.json"))
                .unwrap();
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        for expected in [0, 1, 0] {
            if expected != 0
                || session
                    .document
                    .inner
                    .borrow()
                    .query_selector_all(".editor-canvas")
                    .unwrap()
                    .len()
                    == 1
            {
                let mut model = session.editor_model();
                let visible = expected == 1;
                assert!(model.action(&EditorAction::CanvasVisible(
                    scorepeek_overlay_ui::ScreenKind::MusicSelect,
                    visible,
                )));
                session.apply_editor_model(&model);
                session.resolve();
            }
            let inner = session.document.inner.borrow();
            assert_eq!(
                inner.query_selector_all(".editor-canvas").unwrap().len(),
                expected
            );
            let panel = inner.query_selector(".editor-panel").unwrap().unwrap();
            let panel = inner.get_client_bounding_rect(panel).unwrap();
            assert_eq!((panel.width, panel.height), (384.0, 1080.0));
            let footer = inner.query_selector("footer").unwrap().unwrap();
            let footer = inner.get_client_bounding_rect(footer).unwrap();
            assert!(footer.width > 250.0 && footer.y > 900.0 && footer.y + footer.height <= 1080.0);
            let body = inner.query_selector(".inspector-scroll").unwrap().unwrap();
            assert!(inner.get_client_bounding_rect(body).unwrap().height > 100.0);
        }
    }
    #[test]
    fn unselected_canvas_has_no_native_hit_regions() {
        let mut scenario: VisualDebugScenario =
            serde_json::from_str(include_str!("../tests/fixtures/visual-composition.json"))
                .unwrap();
        let mut other = scenario.canvases.as_ref().unwrap()[0].clone();
        other.id = "other".into();
        other.x = 800;
        other.y = 400;
        scenario.canvases.as_mut().unwrap().push(other);
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        session.editing.set(true);
        session.panel_open.set(false);
        session.resolve();

        let inner = session.document.inner.borrow();
        assert_eq!(inner.query_selector_all(".editor-canvas").unwrap().len(), 1);
        assert!(
            inner
                .query_selector(".editor-canvas[data-canvas='other']")
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn shared_dioxus_handles_resize_from_all_four_corners() {
        let mut scenario: VisualDebugScenario =
            serde_json::from_str(include_str!("../tests/fixtures/visual-composition.json"))
                .unwrap();
        let canvas = &mut scenario.canvases.as_mut().unwrap()[0];
        let cam = canvas
            .widgets
            .iter_mut()
            .find(|widget| widget.id == "cam")
            .unwrap();
        cam.x = 0;
        cam.y = 0;
        cam.width = canvas.width;
        cam.height = canvas.height;
        for corner in ["nw", "ne", "sw", "se"] {
            let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
            session.editing.set(true);
            session.interactive.set(true);
            session.panel_open.set(false);
            session.selected.set(Some("cam".into()));
            session.resolve();
            let original = session
                .widgets
                .borrow()
                .iter()
                .find(|w| w.id == "cam")
                .unwrap()
                .clone();
            let point = {
                let inner = session.document.inner.borrow();
                let node = inner
                    .query_selector(&format!(
                        ".editor-widget-hit[data-widget='cam'] .resize-handle.{corner}"
                    ))
                    .unwrap()
                    .unwrap();
                let rect = inner.get_client_bounding_rect(node).unwrap();
                assert!(rect.width >= 10.0 && rect.height >= 10.0);
                [rect.x + rect.width / 2.0, rect.y + rect.height / 2.0]
            };
            let dx = if corner.contains('w') { 20.0 } else { -20.0 };
            let dy = if corner.contains('n') { 20.0 } else { -20.0 };
            session
                .drag(
                    point,
                    [point[0] + dx, point[1] + dy],
                    VisualDebugButton::Left,
                )
                .unwrap();
            let resized = session
                .widgets
                .borrow()
                .iter()
                .find(|w| w.id == "cam")
                .unwrap()
                .clone();
            assert_eq!(resized.width, original.width - 20, "{corner}");
            assert_eq!(resized.height, original.height - 20, "{corner}");
        }
    }

    #[test]
    fn small_selection_metrics_escape_object_opacity_and_clipping() {
        let mut scenario: VisualDebugScenario =
            serde_json::from_str(include_str!("../tests/fixtures/visual-composition.json"))
                .unwrap();
        let canvas = &mut scenario.canvases.as_mut().unwrap()[0];
        canvas.opacity_percent = 1;
        let cam = canvas
            .widgets
            .iter_mut()
            .find(|widget| widget.id == "cam")
            .unwrap();
        cam.x = 0;
        cam.y = 0;
        cam.width = 16;
        cam.height = 16;
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        session.editing.set(true);
        session.interactive.set(true);
        session.panel_open.set(false);
        session.selected.set(Some("cam".into()));
        session.resolve();

        let inner = session.document.inner.borrow();
        let canvas = inner.query_selector(".editor-canvas").unwrap().unwrap();
        let canvas_html = inner.get_node(canvas).unwrap().outer_html();
        let canvas_start = canvas_html.split_once('>').unwrap().0;
        assert!(!canvas_start.contains("opacity:"));
        let content = inner
            .query_selector(".editor-canvas-content")
            .unwrap()
            .unwrap();
        assert!(
            inner
                .get_node(content)
                .unwrap()
                .outer_html()
                .contains("opacity:0.01")
        );
        let metrics = inner.query_selector(".selection-metrics").unwrap().unwrap();
        let metrics_html = inner.get_node(metrics).unwrap().outer_html();
        assert!(metrics_html.contains("selection-label"));
        assert!(metrics_html.contains("selection-geometry"));
        assert!(metrics_html.contains("EMPTY 2"));
        assert!(metrics_html.contains("0,0 · 16×16"));
        let metrics = inner.get_client_bounding_rect(metrics).unwrap();
        assert!(metrics.width > 16.0);
        assert!(metrics.x >= 0.0 && metrics.y >= 0.0, "{metrics:?}");
        assert!(metrics.x + metrics.width <= f64::from(scenario.logical_size[0]));
        assert!(metrics.y + metrics.height <= f64::from(scenario.logical_size[1]));
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
    fn active_output_disconnect_selects_a_remaining_output_and_clears_selection() {
        let mut workspace = NativeEditorSession {
            active_output: Some("WL-2".into()),
            selected_widget: Some("score".into()),
            pending_widget: Some(scorepeek_overlay_ui::WidgetKind::Score),
            ..NativeEditorSession::default()
        };
        workspace.selected_canvas = Some("canvas-on-wl-2".into());
        let outputs = [OutputDescription {
            name: "WL-1".into(),
            model: "Nested output 1".into(),
            logical_size: Some([1280, 720]),
        }]
        .into_iter()
        .map(|output| (output.name.clone(), output))
        .collect();

        assert!(replace_workspace_outputs(&mut workspace, outputs));
        assert_eq!(workspace.active_output.as_deref(), Some("WL-1"));
        assert_eq!(workspace.output_selection_epoch, 1);
        assert!(workspace.selected_widget.is_none());
        assert!(workspace.pending_widget.is_none());
        assert!(workspace.selected_canvas.is_none());
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
    fn one_close_command_ends_the_editor_session_after_canvas_selection_changes() {
        let mut phase = EditorPhase::Editing;
        let mut workspace = NativeEditorSession {
            selected_canvas: Some("canvas-a".into()),
            selected_widget: Some("score".into()),
            ..NativeEditorSession::default()
        };
        let mut suppressed = std::collections::BTreeSet::new();
        replace_canvas_selection(
            &mut workspace.selected_canvas,
            &mut workspace.selected_widget,
            Some("canvas-b".into()),
        );

        assert_eq!(workspace.selected_canvas.as_deref(), Some("canvas-b"));

        assert_eq!(
            apply_coordinator_command(
                &mut phase,
                &mut workspace,
                &mut suppressed,
                CoordinatorCommand::Close {
                    reason: "close",
                    correlation: None,
                },
            ),
            Some(CoordinatorTransition::Closed {
                reason: "close",
                correlation: None,
            })
        );
        assert_eq!(phase, EditorPhase::Display);
        assert!(workspace.selected_canvas.is_none());
        assert!(workspace.selected_widget.is_none());
        assert_eq!(
            apply_coordinator_command(
                &mut phase,
                &mut workspace,
                &mut suppressed,
                CoordinatorCommand::Close {
                    reason: "close",
                    correlation: None,
                },
            ),
            None
        );
    }

    #[test]
    fn one_draft_snapshot_undoes_every_field_and_is_not_replaced_by_a_no_op() {
        let before = crate::config::visual_debug_config()
            .canvases
            .into_iter()
            .filter(|canvas| canvas.backend == crate::runtime::Backend::Wayland)
            .map(|canvas| canvas.presentation())
            .collect::<Vec<_>>();
        let mut after = before.clone();
        after[0].skin = Skin::DjBlackbox;
        after[0].opacity_percent = 25;
        after[0].show_on = Some(Vec::new());
        after[0].output = Some("DP-2".into());
        after[0].x += 20;
        after[0].widgets.clear();
        after.pop();

        let mut undo = None;
        assert!(remember_draft_change(
            &mut undo,
            DraftUndo {
                canvases: before.clone(),
                wayland_refresh_hz: scorepeek_overlay_ui::WaylandRefreshRate::Auto,
            },
            &after,
            scorepeek_overlay_ui::WaylandRefreshRate::Capped(30),
        ));
        assert!(!remember_draft_change(
            &mut undo,
            DraftUndo {
                canvases: after.clone(),
                wayland_refresh_hz: scorepeek_overlay_ui::WaylandRefreshRate::Capped(30),
            },
            &after,
            scorepeek_overlay_ui::WaylandRefreshRate::Capped(30),
        ));
        let DraftUndo {
            canvases: restored,
            wayland_refresh_hz,
        } = undo.take().unwrap();
        assert_eq!(restored, before);
        assert_eq!(
            wayland_refresh_hz,
            scorepeek_overlay_ui::WaylandRefreshRate::Auto
        );
        assert!(undo.is_none());
    }

    #[test]
    fn undo_between_invalid_names_never_builds_a_native_draft_command() {
        let outputs = vec![EditorOutput {
            name: "DP-1".into(),
            model: "test".into(),
            logical_size: Some([800, 600]),
        }];
        let mut model =
            scorepeek_overlay_ui::editor_model::Model::new(Vec::new(), [800, 600], "ignored");
        model.set_outputs(outputs.clone());
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        assert!(model.action(&EditorAction::CanvasName(" ".into())));
        assert!(model.action(&EditorAction::CanvasName("  ".into())));
        assert!(model.action(&EditorAction::Undo));
        assert_eq!(model.current().unwrap().name, " ");

        assert!(
            coordinator_draft_update(
                model.draft,
                &outputs,
                scorepeek_overlay_ui::WaylandRefreshRate::Auto,
                None,
            )
            .is_none()
        );
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
    fn visual_debug_surface_contains_only_the_selected_visible_canvas() {
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
            .click(".screen-picker .list-picker-option[data-index='4']")
            .unwrap();
        session.scroll(".navigator-scroll", 0.0, -2000.0).unwrap();
        session
            .click(".canvas-select[data-canvas-id='wayland-result']")
            .unwrap();

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
        assert_eq!(canvas_rects, 1);
        assert_eq!(widget_rects, 4);
    }

    #[test]
    fn editor_canvas_requires_selection_surface_ownership_and_preview_visibility() {
        use scorepeek_overlay_ui::ScreenKind::{Play, Result};

        let surface_canvas_ids = std::collections::BTreeSet::from(["selected".to_owned()]);
        assert!(active_editor_canvas_on_surface(
            "selected",
            Some(&[Result]),
            Some("selected"),
            &surface_canvas_ids,
            Result,
        ));
        assert!(!active_editor_canvas_on_surface(
            "other",
            Some(&[Result]),
            Some("selected"),
            &surface_canvas_ids,
            Result,
        ));
        assert!(!active_editor_canvas_on_surface(
            "selected",
            Some(&[Result]),
            Some("other"),
            &surface_canvas_ids,
            Result,
        ));
        assert!(!active_editor_canvas_on_surface(
            "selected",
            Some(&[Result]),
            Some("selected"),
            &std::collections::BTreeSet::new(),
            Result,
        ));
        assert!(!active_editor_canvas_on_surface(
            "selected",
            Some(&[Play]),
            Some("selected"),
            &surface_canvas_ids,
            Result,
        ));
    }

    #[test]
    fn selected_canvas_is_not_previewed_on_another_output_surface() {
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
        let mut draft = session.managed.borrow().clone();
        draft
            .iter_mut()
            .find(|canvas| canvas.id == "wayland-status")
            .unwrap()
            .output = Some("OUTPUT-B".to_owned());
        session
            .surface_canvas_ids
            .set(surface_canvas_ids_for_draft(&draft, Some("OUTPUT-A")));
        session.resolve();

        let inner = session.document.inner.borrow();
        assert!(
            inner
                .query_selector_all(".editor-canvas")
                .unwrap()
                .is_empty()
        );
        let content = inner.query_selector(".canvas-content").unwrap().unwrap();
        let content = inner.get_client_bounding_rect(content).unwrap();
        assert_eq!((content.width, content.height), (0.0, 0.0));
    }

    #[test]
    fn visual_debug_north_west_canvas_resize_updates_position_and_size_together() {
        let scenario = VisualDebugScenario {
            canvases: None,
            skin: None,
            logical_size: [1920, 1080],
            scale: 1.0,
            canvas_id: Some("wayland-result".into()),
            editing: true,
            selectors: Vec::new(),
            actions: Vec::new(),
        };
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        session
            .click(".screen-picker .list-picker-trigger")
            .unwrap();
        session
            .click(".screen-picker .list-picker-option[data-index='5']")
            .unwrap();
        session.click(".editor-panel-toggle").unwrap();

        session
            .drag([25.0, 105.0], [9.0, 89.0], VisualDebugButton::Left)
            .unwrap();

        let settings = session.settings.borrow();
        assert_eq!(
            (settings.x, settings.y, settings.width, settings.height),
            (4, 84, 576, 976)
        );
        drop(settings);
        let inner = session.document.inner.borrow();
        let canvas = inner
            .query_selector(".editor-canvas.selected")
            .unwrap()
            .unwrap();
        let rect = inner.get_client_bounding_rect(canvas).unwrap();
        assert_eq!(
            (rect.x, rect.y, rect.width, rect.height),
            (4.0, 84.0, 576.0, 976.0)
        );
    }

    #[test]
    fn reactive_widget_selection_rebuilds_the_native_dom() {
        let scenario = VisualDebugScenario {
            canvases: None,
            skin: None,
            logical_size: [1920, 1080],
            scale: 1.0,
            canvas_id: Some("wayland-result".into()),
            editing: true,
            selectors: Vec::new(),
            actions: Vec::new(),
        };
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();

        session
            .click(".screen-picker .list-picker-trigger")
            .unwrap();
        session
            .click(".screen-picker .list-picker-option[data-index='5']")
            .unwrap();
        *session.selected.borrow_mut() = Some("selection".into());
        session.resolve();
        assert!(
            session
                .document
                .inner
                .borrow()
                .query_selector(".editor-widget-hit.selected[data-widget='selection']")
                .unwrap()
                .is_some()
        );

        *session.selected.borrow_mut() = Some("history-graph".into());
        session.resolve();
        let inner = session.document.inner.borrow();
        assert!(
            inner
                .query_selector(".editor-widget-hit.selected[data-widget='selection']")
                .unwrap()
                .is_none()
        );
        assert!(
            inner
                .query_selector(".editor-widget-hit.selected[data-widget='history-graph']")
                .unwrap()
                .is_some()
        );
        drop(inner);

        session
            .click(".canvas-select[data-canvas-id='wayland-result']")
            .unwrap();
        assert!(
            session
                .document
                .inner
                .borrow()
                .query_selector(".widget-slot.selected")
                .unwrap()
                .is_none()
        );
    }

    use scorepeek_overlay_ui::Skin;

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
    fn editor_canvas_keeps_the_skin_root_aligned_with_canvas_content() {
        for skin in [Skin::CyanSystem, Skin::ResultAurora, Skin::DjBlackbox] {
            let mut document = DioxusDocument::new(
                VirtualDom::new_with_props(
                    native_overlay,
                    NativeOverlayProps {
                        actions: Rc::default(),
                        surface_actions: Rc::default(),
                        refresh_rate: Rc::new(Cell::new(
                            scorepeek_overlay_ui::WaylandRefreshRate::Auto,
                        )),
                        appearance: Rc::new(Cell::new(Appearance { skin })),
                        widgets: Rc::new(RefCell::new(scorepeek_overlay_ui::default_widgets())),
                        editing: Rc::new(Cell::new(true)),
                        interactive: Rc::new(Cell::new(true)),
                        panel_open: Rc::new(Cell::new(true)),
                        widget_add_open: Rc::new(Cell::new(false)),
                        dirty: Rc::new(Cell::new(false)),
                        undo_available: Rc::new(Cell::new(false)),
                        selected: Rc::new(RefCell::new(None)),
                        pending_widget: Rc::new(Cell::new(None)),
                        pending_point: Rc::new(Cell::new([0.0, 0.0])),
                        managed: Rc::new(RefCell::new(Vec::new())),
                        outputs: Rc::new(RefCell::new(Vec::new())),
                        state: Rc::new(RefCell::new(
                            serde_json::from_value(
                                serde_json::from_str::<serde_json::Value>(include_str!(
                                    "../tests/fixtures/skin-preview.json"
                                ))
                                .unwrap()["state"]
                                    .clone(),
                            )
                            .unwrap(),
                        )),
                        visible: Rc::new(Cell::new(true)),
                        settings: Rc::new(RefCell::new(NativeCanvasSettings {
                            id: "test".into(),
                            has_selection: true,
                            output: None,
                            active_output: None,
                            show_on: None,
                            background: scorepeek_overlay_ui::Background::None,
                            opacity_percent: 100,
                            x: 0,
                            y: 0,
                            width: 560,
                            height: 1040,
                            preview_screen: scorepeek_overlay_ui::ScreenKind::MusicSelect,
                            panel_width: 400,
                            new_canvas_skin: scorepeek_overlay_ui::Skin::CyanSystem,
                        })),
                        surface_canvas_ids: Rc::new(RefCell::new(
                            std::collections::BTreeSet::from(["test".into()]),
                        )),
                        reactive: Rc::new(RefCell::new(None)),
                    },
                ),
                document_config(),
            );
            document.initial_build();
            let mut inner = document.inner.borrow_mut();
            inner.set_viewport(Viewport::new(560, 1040, 1.25, ColorScheme::Dark));
            inner.resolve(0.0);
            inner.resolve(1.0);
            let root = inner
                .query_selector("#scorepeek-skin-root")
                .unwrap()
                .unwrap();
            let rect = inner.get_client_bounding_rect(root).unwrap();
            let canvas = inner.query_selector(".canvas-content").unwrap().unwrap();
            assert_eq!(
                rect,
                inner.get_client_bounding_rect(canvas).unwrap(),
                "{skin:?}"
            );
        }
    }

    #[test]
    fn compact_canvas_content_fills_the_viewport_and_does_not_clip_widgets() {
        let mut document = DioxusDocument::new(
            VirtualDom::new_with_props(
                native_overlay,
                NativeOverlayProps {
                    actions: Rc::default(),
                    surface_actions: Rc::default(),
                    refresh_rate: Rc::new(Cell::new(
                        scorepeek_overlay_ui::WaylandRefreshRate::Auto,
                    )),
                    appearance: Rc::new(Cell::new(Appearance {
                        skin: Skin::CyanSystem,
                    })),
                    widgets: Rc::new(RefCell::new(vec![WidgetLayout {
                        id: "status".into(),
                        kind: scorepeek_overlay_ui::WidgetKind::Status,
                        x: 0,
                        y: 0,
                        width: 560,
                        height: 72,
                        settings: scorepeek_overlay_ui::WidgetSettings::default(),
                        skin_properties: std::collections::BTreeMap::new(),
                    }])),
                    editing: Rc::new(Cell::new(false)),
                    interactive: Rc::new(Cell::new(false)),
                    panel_open: Rc::new(Cell::new(true)),
                    widget_add_open: Rc::new(Cell::new(false)),
                    dirty: Rc::new(Cell::new(false)),
                    undo_available: Rc::new(Cell::new(true)),
                    selected: Rc::new(RefCell::new(None)),
                    pending_widget: Rc::new(Cell::new(None)),
                    pending_point: Rc::new(Cell::new([0.0, 0.0])),
                    managed: Rc::new(RefCell::new(Vec::new())),
                    outputs: Rc::new(RefCell::new(Vec::new())),
                    state: Rc::new(RefCell::new(OverlayState::default())),
                    visible: Rc::new(Cell::new(true)),
                    settings: Rc::new(RefCell::new(NativeCanvasSettings {
                        id: "test".into(),
                        has_selection: true,
                        output: None,
                        active_output: None,
                        show_on: None,
                        background: scorepeek_overlay_ui::Background::None,
                        opacity_percent: 100,
                        x: 0,
                        y: 0,
                        width: 560,
                        height: 72,
                        preview_screen: scorepeek_overlay_ui::ScreenKind::MusicSelect,
                        panel_width: 400,
                        new_canvas_skin: scorepeek_overlay_ui::Skin::CyanSystem,
                    })),
                    surface_canvas_ids: Rc::new(RefCell::new(std::collections::BTreeSet::from([
                        "test".into(),
                    ]))),
                    reactive: Rc::new(RefCell::new(None)),
                },
            ),
            document_config(),
        );
        document.initial_build();
        let mut inner = document.inner.borrow_mut();
        inner.set_viewport(Viewport::new(560, 72, 1.0, ColorScheme::Dark));
        inner.resolve(0.0);
        apply_motion(&mut inner, 1.0);
        resolve_with_loaded_resources(&mut inner, 1.0);

        for selector in [".canvas-content", "#scorepeek-skin-root"] {
            let id = inner.query_selector(selector).unwrap().unwrap();
            let rect = inner.get_client_bounding_rect(id).unwrap();
            assert!(
                rect.width >= 559.0 && rect.height >= 71.0,
                "{selector}: {rect:?}"
            );
        }
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
    #[allow(clippy::too_many_lines)]
    fn output_picker_stays_inside_the_overlay_panel_at_actual_canvas_coordinates() {
        let canvas = scorepeek_overlay_ui::CanvasPresentation {
            id: "wayland-selection".into(),
            name: "Selection".into(),
            skin: Skin::CyanSystem,
            skin_properties: std::collections::BTreeMap::new(),
            show_on: None,
            background: scorepeek_overlay_ui::Background::None,
            opacity_percent: 100,
            output: Some("DP-1".into()),
            x: 120,
            y: 80,
            width: 560,
            height: 120,
            widgets: Vec::new(),
        };
        let managed = (0..6)
            .map(|index| {
                let mut item = canvas.clone();
                if index > 0 {
                    item.id = format!("wayland-extra-{index}");
                }
                item
            })
            .collect::<Vec<_>>();
        let mut document = DioxusDocument::new(
            VirtualDom::new_with_props(
                native_overlay,
                NativeOverlayProps {
                    actions: Rc::default(),
                    surface_actions: Rc::default(),
                    refresh_rate: Rc::new(Cell::new(
                        scorepeek_overlay_ui::WaylandRefreshRate::Auto,
                    )),
                    appearance: Rc::new(Cell::new(Appearance {
                        skin: Skin::CyanSystem,
                    })),
                    widgets: Rc::new(RefCell::new(Vec::new())),
                    editing: Rc::new(Cell::new(true)),
                    interactive: Rc::new(Cell::new(true)),
                    panel_open: Rc::new(Cell::new(true)),
                    widget_add_open: Rc::new(Cell::new(false)),
                    dirty: Rc::new(Cell::new(false)),
                    undo_available: Rc::new(Cell::new(true)),
                    selected: Rc::new(RefCell::new(None)),
                    pending_widget: Rc::new(Cell::new(None)),
                    pending_point: Rc::new(Cell::new([0.0, 0.0])),
                    managed: Rc::new(RefCell::new(managed)),
                    outputs: Rc::new(RefCell::new(vec![OutputDescription {
                        name: "DP-1".into(),
                        model: "Odyssey G9".into(),
                        logical_size: Some([1920, 1080]),
                    }])),
                    state: Rc::new(RefCell::new(OverlayState::default())),
                    visible: Rc::new(Cell::new(true)),
                    settings: Rc::new(RefCell::new(NativeCanvasSettings {
                        id: "wayland-selection".into(),
                        has_selection: true,
                        output: Some("DP-1".into()),
                        active_output: Some("DP-1".into()),
                        show_on: None,
                        background: scorepeek_overlay_ui::Background::None,
                        opacity_percent: 100,
                        x: 120,
                        y: 80,
                        width: 560,
                        height: 120,
                        preview_screen: scorepeek_overlay_ui::ScreenKind::MusicSelect,
                        panel_width: 384,
                        new_canvas_skin: scorepeek_overlay_ui::Skin::CyanSystem,
                    })),
                    surface_canvas_ids: Rc::new(RefCell::new(std::collections::BTreeSet::from([
                        "wayland-selection".into(),
                    ]))),
                    reactive: Rc::new(RefCell::new(None)),
                },
            ),
            document_config(),
        );
        document.initial_build();
        let mut inner = document.inner.borrow_mut();
        inner.set_viewport(Viewport::new(1920, 1080, 1.0, ColorScheme::Dark));
        inner.resolve(0.0);
        apply_motion(&mut inner, 1.0);
        resolve_with_loaded_resources(&mut inner, 1.0);

        let rect = |selector| {
            inner
                .get_client_bounding_rect(inner.query_selector(selector).unwrap().unwrap())
                .unwrap()
        };
        let panel = rect(".editor-panel");
        let navigator = rect(".object-navigator");
        let inspector = rect(".object-inspector");
        let action_bar = rect(".editor-action-bar");
        let undo = rect(".undo-action");
        let delete = rect(".delete-canvas");
        let preview = rect(".editor-canvas.selected");
        let nested_canvas = rect(".canvas-select[data-canvas-id='wayland-selection']");
        let cyan = rect(".skin-option[data-index='0']");
        let aurora = rect(".skin-option[data-index='1']");
        let blackbox = rect(".skin-option[data-index='2']");
        let last_before = rect(".canvas-select[data-canvas-id='wayland-extra-5']");
        assert!((panel.width - 384.0).abs() < 1.0, "{panel:?}");
        assert!(navigator.y < inspector.y, "{navigator:?} {inspector:?}");
        assert!(action_bar.y >= inspector.y, "{action_bar:?} {inspector:?}");
        assert!(
            (action_bar.width - panel.width).abs() <= 1.5,
            "{action_bar:?} {panel:?}"
        );
        assert!(undo.x >= panel.x && undo.y >= action_bar.y, "{undo:?}");
        assert!(delete.width > 0.0, "{delete:?}");
        assert!(
            inner
                .query_selector(".delete-canvas[disabled]")
                .unwrap()
                .is_none()
        );
        assert!(cyan.width > 0.0 && aurora.width > 0.0 && blackbox.width > 0.0);
        assert!(cyan.x >= panel.x && blackbox.x + blackbox.width <= panel.x + panel.width);
        assert!(blackbox.x + blackbox.width <= panel.x + panel.width);
        assert!(inner.query_selector(".manage-canvas").unwrap().is_none());
        assert!(inner.query_selector(".output-settings").unwrap().is_none());
        assert!(inner.query_selector(".context-status").unwrap().is_none());
        assert!(
            inner
                .query_selector(
                    ".screen-picker .list-picker-trigger[aria-label='GAME SCREEN: Music Select']"
                )
                .unwrap()
                .is_some()
        );
        assert!(inner.query_selector(".stable-id").unwrap().is_none());
        assert!(inner.query_selector(".property-kind").unwrap().is_none());
        assert!(
            inner
                .query_selector(".property-section-title")
                .unwrap()
                .is_none()
        );
        assert!(
            inner
                .query_selector(".output-option small")
                .unwrap()
                .is_none()
        );
        assert!(inner.query_selector(".control-heading").unwrap().is_some());
        assert!((preview.x - 120.0).abs() < 1.0, "{preview:?}");
        let nested_canvas_point = [
            nested_canvas.x + nested_canvas.width / 2.0,
            nested_canvas.y + nested_canvas.height / 2.0,
        ];
        drop(inner);
        PointerInput::default().wheel(&mut document, nested_canvas_point, [0.0, -120.0]);
        while document.poll(Some(TaskContext::from_waker(Waker::noop()))) {}
        let mut inner = document.inner.borrow_mut();
        inner.resolve(1.0);
        let last_after = inner
            .get_client_bounding_rect(
                inner
                    .query_selector(".canvas-select[data-canvas-id='wayland-extra-5']")
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
        assert!(
            last_after.y < last_before.y,
            "{last_before:?} {last_after:?}"
        );
    }

    #[test]
    fn aggregate_canvas_visibility_preserves_explicit_screen_membership() {
        use scorepeek_overlay_ui::editor_model::SCREENS;
        let mut canvas = crate::config::visual_debug_config().canvases[0].presentation();
        canvas.show_on = None;
        let mut model = EditorModel::new(vec![canvas.clone()], [1920, 1080], "wayland");
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
    #[test]
    fn presentation_switch_and_undo_restore_background_before_another_edit() {
        let scenario: VisualDebugScenario =
            serde_json::from_str(include_str!("../tests/fixtures/visual-composition.json"))
                .unwrap();
        let session = VisualDebugSession::new(&scenario, [1920, 1080]).unwrap();
        let mut saved = session.managed.borrow()[0].clone();
        saved.background = scorepeek_overlay_ui::Background::None;
        let mut changed = saved.clone();
        changed.background = scorepeek_overlay_ui::Background::Animated;
        for presentation in [&changed, &saved, &changed, &saved] {
            let mut settings = session.settings.borrow_mut();
            settings.apply_presentation(presentation);
            settings.width += 4;
            let mut canvas = crate::config::empty_canvas(
                presentation.id.clone(),
                crate::runtime::Backend::Wayland,
            );
            canvas.apply_presentation(presentation);
            canvas.background = settings.background;
            canvas.width = settings.width;
            assert_eq!(canvas.presentation().background, presentation.background);
        }
    }
    #[test]
    fn aspect_options_are_separate_from_delete_and_show_every_selection() {
        for size in [[1280, 720], [1920, 1080]] {
            let mut scenario: VisualDebugScenario =
                serde_json::from_str(include_str!("../tests/fixtures/visual-composition.json"))
                    .unwrap();
            scenario.logical_size = size;
            scenario.editing = true;
            let mut session = VisualDebugSession::new(&scenario, size).unwrap();
            session.scroll(".navigator-scroll", 0.0, -200.0).unwrap();
            session.click(".widget-row[data-widget-id='cam']").unwrap();
            assert_eq!(session.selected.borrow().as_deref(), Some("cam"));
            session.scroll(".inspector-scroll", 0.0, -2000.0).unwrap();
            session.scroll(".inspector-scroll", 0.0, 60.0).unwrap();
            for index in [1, 2, 3, 0] {
                let selector = format!(".aspect-ratio[data-index='{index}']");
                session.click(&selector).unwrap();
                let widgets = session.widgets.borrow();
                let widget = widgets.iter().find(|widget| widget.id == "cam").unwrap();
                assert_eq!(
                    scorepeek_overlay_ui::editor::aspect_ratio_index(widget.settings.aspect_ratio),
                    index
                );
                if index == 3 {
                    assert_eq!(
                        widget.settings.aspect_ratio,
                        scorepeek_overlay_ui::AspectRatio::Current([widget.width, widget.height])
                    );
                }
                let doc = session.document.inner.borrow();
                let button = doc
                    .query_selector(&format!("{selector}.selected[aria-pressed='true']"))
                    .unwrap()
                    .unwrap();
                let button = doc.get_client_bounding_rect(button).unwrap();
                let delete = doc.query_selector(".delete-widget").unwrap().unwrap();
                let delete = doc.get_client_bounding_rect(delete).unwrap();
                assert!(button.width >= 40.0 && button.height >= 32.0);
                assert!(button.y + button.height < delete.y);
                assert!(button.y >= 0.0 && delete.y + delete.height <= f64::from(size[1]));
            }
        }
    }
}
