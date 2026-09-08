mod text;
use scorepeek_overlay_ui::editor_model::{Drag, Model as EditorModel};
use scorepeek_overlay_ui::editor_surface::{
    EditorCanvas, EditorSurface, PlacementPreview, SurfaceAction,
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
use blitz_dom::{BaseDocument, Document, DocumentConfig};
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};
use dioxus::prelude::*;
use dioxus_core::VirtualDom;
use dioxus_native_dom::DioxusDocument;
use scorepeek_overlay_handles::{CursorStyle, Event, OutputDescription, Shell};
use scorepeek_overlay_ui::editor::{
    EditorAccess, EditorAction, EditorChrome, EditorOutput, EditorPanel, EditorTitleState,
    EditorView, RefreshRateEditor,
};
use scorepeek_overlay_ui::{Appearance, OXANIUM, OverlayState, WidgetLayout};
use serde::{Deserialize, Serialize};
use smithay_client_toolkit::reexports::calloop::ping::{Ping, make_ping};

#[derive(Default)]
struct RendererInitCoordinator(std::sync::Mutex<()>);

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

impl RendererInitCoordinator {
    fn exclusive<T>(&self, operation: impl FnOnce() -> T) -> T {
        let _guard = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        operation()
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

fn editor_surface_host(
    surfaces: &std::collections::BTreeSet<String>,
    pinned: Option<&str>,
) -> Option<String> {
    pinned
        .filter(|id| surfaces.contains(*id))
        .map(str::to_owned)
        .or_else(|| surfaces.first().cloned())
}

fn shown_on(
    canvas: &scorepeek_overlay_ui::CanvasPresentation,
    screen: scorepeek_overlay_ui::ScreenKind,
) -> bool {
    canvas
        .show_on
        .as_ref()
        .is_none_or(|screens| screens.contains(&screen))
}

#[derive(Clone)]
struct NativeCanvasSettings {
    id: String,
    has_selection: bool,
    output: Option<String>,
    show_on: Option<Vec<scorepeek_overlay_ui::ScreenKind>>,
    background: scorepeek_overlay_ui::Background,
    opacity_percent: u8,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    preview_screen: scorepeek_overlay_ui::ScreenKind,
    panel_width: u32,
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

fn replace_canvas_selection(
    selected_canvas: &mut Option<String>,
    selected_widget: &mut Option<String>,
    next: Option<String>,
) {
    *selected_canvas = next;
    selected_widget.take();
}

fn scroll_editor_at(document: &mut BaseDocument, point: [f64; 2], delta: [f64; 2]) -> bool {
    let [x, y] = point;
    for selector in [".canvas-list", ".editor-tab-body"] {
        let Ok(Some(node)) = document.query_selector(selector) else {
            continue;
        };
        let Some(rect) = document.get_client_bounding_rect(node) else {
            continue;
        };
        if x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height {
            document.scroll_node_by(node, delta[0], delta[1], |_| {});
            return true;
        }
    }
    false
}

#[derive(Default)]
struct NativeWorkspace {
    ui: EditorWorkspaceUi,
    undo: Option<DraftUndo>,
    fallback: std::collections::BTreeSet<String>,
    reload_saved: bool,
    draft: Vec<scorepeek_overlay_ui::CanvasPresentation>,
    dirty: bool,
    selected_widget: Option<String>,
    pending_widget: Option<scorepeek_overlay_ui::WidgetKind>,
    outputs: std::collections::BTreeMap<String, OutputDescription>,
    surfaces: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    editor_hosts: std::collections::BTreeMap<String, String>,
    wayland_refresh_hz: scorepeek_overlay_ui::WaylandRefreshRate,
}

#[derive(Clone)]
struct NativeOverlayProps {
    appearance: Rc<Cell<Appearance>>,
    widgets: Rc<RefCell<Vec<WidgetLayout>>>,
    editing: Rc<Cell<bool>>,
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

    fn replace(&self, value: T) -> T {
        std::mem::replace(&mut *self.borrow_mut(), value)
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
        selected,
        managed,
        settings,
        ..
    } = reactive.clone();
    let current = reactive.state.borrow().clone();
    let sample = editing.get() && current.system == scorepeek_overlay_ui::LampState::Inactive;
    let current_settings = settings.borrow().clone();
    let refresh_error = reactive
        .refresh_edit
        .borrow()
        .as_ref()
        .and_then(|edit| parse_refresh_rate(&edit.text).err());
    let selected_visible = current_settings.has_selection
        && scorepeek_overlay_ui::canvas_visible(
            current_settings.show_on.as_deref(),
            scorepeek_overlay_ui::ScreenView {
                kind: Some(current_settings.preview_screen),
                suspended_since_unix_ms: None,
                revision: 0,
            },
        );
    rsx! {
      EditorSurface { onaction:onsurface,
        div { class:"canvas-content",style:if editing.get(){format!("display:{};opacity:{};position:absolute;left:{}px;top:{}px;width:{}px;height:{}px",if reactive.visible.get()&&selected_visible{"block"}else{"none"},f32::from(current_settings.opacity_percent)/100.0,current_settings.x,current_settings.y,current_settings.width,current_settings.height)}else{format!("display:{};opacity:{}",if reactive.visible.get(){"block"}else{"none"},f32::from(current_settings.opacity_percent)/100.0)},
            div { id:"scorepeek-skin-root", class:"scorepeek-skin-scope", "data-backend":"native", style:"position:absolute;inset:0" }
        }
        if editing.get() {
            for canvas in managed.borrow().iter().filter(|canvas| reactive.surface_canvas_ids.borrow().contains(&canvas.id) && shown_on(canvas,current_settings.preview_screen)) {
                EditorCanvas {key:"{canvas.id}",canvas:canvas.clone(),editing:true,selected:canvas.id==current_settings.id&&selected_visible,selected_widget:selected.borrow().clone(),onaction:onsurface,
                    div {}
                }
            }
        }
        if editing.get() {
            EditorPanel {
                    view: EditorView {
                        backend_label:"WAYLAND EDITOR".into(),
                        canvases: managed.borrow().clone(),
                        selected_canvas: current_settings.has_selection.then(||current_settings.id.clone()),
                        selected_widget:selected.borrow().clone(),
                        preview_screen:current_settings.preview_screen,
                        outputs:Some(reactive.outputs.borrow().iter().map(|output|EditorOutput {name:output.name.clone(),model:output.model.clone(),logical_size:output.logical_size}).collect()),
                        panel_width:current_settings.panel_width,
                        chrome:EditorChrome {panel_open:reactive.panel_open.get(),
                        widget_add_open:reactive.widget_add_open.get(),sample},
                        access:EditorAccess {dirty:reactive.dirty.get() || reactive.refresh_edit.borrow().is_some(),readonly:reactive.readonly.get(),
                        undo_available:reactive.undo_available.get()},
                        title:reactive.title_edit.borrow().as_ref().map_or(EditorTitleState::Closed,|edit|if edit.preedit.is_empty(){EditorTitleState::Editing}else{EditorTitleState::Composing}),
                        refresh_rate:Some(RefreshRateEditor {rate:reactive.refresh_rate.get(),editing:reactive.refresh_edit.borrow().is_some(),error:refresh_error}),
                        skins:installed_editor_skins(),
                    },
                    title_input:rsx! { if let Some(edit)=reactive.title_edit.borrow().as_ref() {
                        div { class:"empty-title-edit", role:"textbox", "aria-label":"Widget title", "aria-multiline":"false", {title_input_content(edit)} }
                    } },
                    refresh_rate_input:rsx! { if let Some(edit)=reactive.refresh_edit.borrow().as_ref() {
                        div { class:"refresh-rate-edit", role:"textbox", "aria-label":"Wayland refresh rate in Hz", "aria-multiline":"false", {title_input_content(edit)} }
                    } },
                    onaction: move |action| actions.borrow_mut().push(action),
            }
            if reactive.surface_canvas_ids.borrow().contains(&current_settings.id) { if let Some(kind) = reactive.pending_widget.get() { PlacementPreview {kind,point:reactive.pending_point.get()} } }
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

fn skin_deadline(schedule: &crate::skin::Schedule) -> Option<Instant> {
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
    failure: &(Option<String>, u64, Instant),
    canvas: &crate::config::Canvas,
) -> bool {
    failure.0 == canvas.output
        && failure.1 == canvas.revision
        && failure.2.elapsed() < Duration::from_secs(5)
}

/// Runs until the parent's lifetime lease closes.
/// # Errors
/// Returns Wayland, GPU or event-loop failures.
#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
pub fn run(config: Config, input: impl std::io::Read + Send + 'static) -> Result<(), String> {
    use std::collections::BTreeMap;
    struct Worker {
        output: Option<String>,
        stop: Arc<std::sync::atomic::AtomicBool>,
        join: std::thread::JoinHandle<Result<(), String>>,
    }
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
    let mut workers = BTreeMap::<String, Worker>::new();
    let mut failed = BTreeMap::<String, (Option<String>, u64, Instant)>::new();
    let preview = Arc::new(std::sync::Mutex::new(None::<String>));
    let workspace_open = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let workspace_ui = Arc::new(std::sync::Mutex::new(NativeWorkspace::default()));
    workspace_ui
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .wayland_refresh_hz = config.wayland_refresh_hz;
    let wayland_refresh_hz = Arc::new(std::sync::Mutex::new(config.wayland_refresh_hz));
    let renderer_init = Arc::new(RendererInitCoordinator::default());
    let suppressed = Arc::new(std::sync::Mutex::new(
        std::collections::BTreeSet::<String>::new(),
    ));
    if config.edit_on_start
        && let Some(canvas) = desired.first()
    {
        workspace_open.store(true, std::sync::atomic::Ordering::Release);
        *preview
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(canvas.id.clone());
    }
    while !stop.load(std::sync::atomic::Ordering::Acquire) {
        if let Ok((loaded, _)) = crate::config::load_or_create(&config.config_path) {
            *wayland_refresh_hz
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = loaded.wayland_refresh_hz;
            desired = loaded
                .canvases
                .into_iter()
                .filter(|canvas| canvas.backend == crate::runtime::Backend::Wayland)
                .collect();
        }
        if workspace_open.load(std::sync::atomic::Ordering::Acquire)
            && let Ok(response) = crate::control::request(
                &config.control_socket,
                &crate::control::Request::AcquireBackend {
                    backend: crate::runtime::Backend::Wayland,
                    editor_id: format!("wayland-{}", std::process::id()),
                },
            )
            && !response.readonly
            && !response.canvases.is_empty()
        {
            if let Some(refresh) = response.wayland_refresh_hz {
                *wayland_refresh_hz
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = refresh;
                workspace_ui
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .wayland_refresh_hz = refresh;
            }
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
        if workspace_open.load(std::sync::atomic::Ordering::Acquire) {
            let (outputs, surfaces) = {
                let workspace = workspace_ui
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                (
                    workspace.outputs.values().cloned().collect::<Vec<_>>(),
                    workspace.surfaces.clone(),
                )
            };
            for (index, output) in outputs.into_iter().enumerate() {
                let represented = desired
                    .iter()
                    .any(|canvas| canvas.output.as_deref() == Some(output.name.as_str()))
                    || surfaces.get(&output.name).is_some_and(|surface_ids| {
                        desired
                            .iter()
                            .any(|canvas| surface_ids.contains(&canvas.id))
                    });
                if represented {
                    continue;
                }
                let mut id = format!("__scorepeek-editor-surface-{index}");
                while desired.iter().any(|canvas| canvas.id == id) {
                    id.push('_');
                }
                let editor_skin = desired.first().map(|canvas| canvas.skin);
                let mut canvas = crate::config::empty_canvas(id, crate::runtime::Backend::Wayland);
                if let Some(skin) = editor_skin {
                    canvas.skin = skin;
                }
                canvas.output = Some(output.name);
                canvas.show_on = Some(Vec::new());
                if let Some([width, height]) = output.logical_size {
                    canvas.width = grid_floor(width).max(32);
                    canvas.height = grid_floor(height).max(32);
                }
                desired.push(canvas);
            }
        }
        let desired_ids: std::collections::BTreeSet<_> = desired
            .iter()
            .filter(|canvas| {
                !suppressed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contains(&canvas.id)
            })
            .map(|canvas| canvas.id.clone())
            .collect();
        let force_reload = {
            let mut workspace = workspace_ui
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut workspace.reload_saved)
        };
        let remove: Vec<_> = workers
            .iter()
            .filter(|(id, worker)| {
                force_reload
                    || !desired_ids.contains(*id)
                    || desired
                        .iter()
                        .find(|canvas| canvas.id == id.as_str())
                        .is_some_and(|canvas| canvas.output != worker.output)
                    || worker.join.is_finished()
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in remove {
            canvas_wakes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&id);
            if let Some(worker) = workers.remove(&id) {
                worker
                    .stop
                    .store(true, std::sync::atomic::Ordering::Release);
                match worker.join.join() {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        if let Some(canvas) = desired.iter().find(|canvas| canvas.id == id) {
                            failed.insert(
                                id.clone(),
                                (canvas.output.clone(), canvas.revision, Instant::now()),
                            );
                        }
                        crate::diagnostics::emit(
                            "native_canvas_failed",
                            &serde_json::json!({"canvas_id":id,"error":error}),
                        );
                    }
                    Err(_) => {
                        crate::diagnostics::emit(
                            "native_canvas_failed",
                            &serde_json::json!({"canvas_id":id,"error":"panicked"}),
                        );
                    }
                }
            }
        }
        for canvas in &desired {
            if suppressed
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
            let preview = Arc::clone(&preview);
            let workspace_open = Arc::clone(&workspace_open);
            let workspace_ui = Arc::clone(&workspace_ui);
            let suppressed = Arc::clone(&suppressed);
            let renderer_init = Arc::clone(&renderer_init);
            let refresh_rate = Arc::clone(&wayland_refresh_hz);
            let wakes = Arc::clone(&canvas_wakes);
            let join = std::thread::Builder::new()
                .name(format!("overlay-wayland-{}", canvas.id))
                .spawn(move || {
                    run_canvas(
                        &canvas_config,
                        stopping,
                        state,
                        stopped,
                        preview,
                        workspace_open,
                        workspace_ui,
                        suppressed,
                        renderer_init,
                        refresh_rate,
                        &wakes,
                    )
                })
                .map_err(|error| error.to_string())?;
            workers.insert(
                canvas.id.clone(),
                Worker {
                    output: canvas.output.clone(),
                    stop: canvas_stop,
                    join,
                },
            );
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    for (_, worker) in workers {
        worker
            .stop
            .store(true, std::sync::atomic::Ordering::Release);
        let _ = worker.join.join();
    }
    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn run_canvas(
    config: &Config,
    external_stop: Arc<std::sync::atomic::AtomicBool>,
    feed_state: Arc<std::sync::Mutex<OverlayState>>,
    feed_stop: Arc<std::sync::atomic::AtomicBool>,
    preview: Arc<std::sync::Mutex<Option<String>>>,
    workspace_open: Arc<std::sync::atomic::AtomicBool>,
    workspace_ui: Arc<std::sync::Mutex<NativeWorkspace>>,
    suppressed: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    renderer_init: Arc<RendererInitCoordinator>,
    wayland_refresh_hz: Arc<std::sync::Mutex<scorepeek_overlay_ui::WaylandRefreshRate>>,
    wakes: &std::sync::Mutex<std::collections::BTreeMap<String, Ping>>,
) -> Result<(), String> {
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
    let shell = Shell::open(
        canvas.output.as_deref(),
        canvas.width,
        canvas.height,
        canvas.x,
        canvas.y,
        canvas.initial_placement == Some(crate::config::InitialPlacement::UpperRight),
        ping.1,
    )?;
    let mut pending_resolved_output = None;
    if let Some(selected_output) = shell.output_name.as_deref()
        && canvas.output.as_deref() != Some(selected_output)
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
    let renderer = renderer_init.exclusive(|| {
        VelloWindowRenderer::with_options(
            VelloRendererOptions::default()
                .base_color(peniko::Color::TRANSPARENT)
                .composite_alpha_mode(CompositeAlphaMode::Transparent),
        )
    });
    report
        .borrow_mut()
        .operations
        .push("renderer_context_initialized");
    let appearance = Appearance { skin: canvas.skin };
    let widgets = canvas.widgets.iter().map(widget_layout).collect();
    let control_socket = config.control_socket.clone();
    let mut app = App::new(
        appearance,
        widgets,
        renderer,
        renderer_init,
        shell,
        Waker::from(Arc::new(CalloopWaker(ping.0))),
        feed_state,
        feed_stop,
        external_stop,
        canvas,
        control_socket,
        config.skin_store.clone(),
        outputs,
        Rc::clone(&report),
        pending_resolved_output,
        preview,
        workspace_open,
        workspace_ui,
        suppressed,
        wayland_refresh_hz,
    )?;
    let result = app.run();
    let renderer_coordinator = Arc::clone(&app.renderer_init);
    renderer_coordinator.exclusive(|| app.renderer.suspend());
    {
        let mut report = report.borrow_mut();
        report.paint_count = app.paint_count;
        report.render_calls = app.render_calls;
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
        report.failure = result.as_ref().err().cloned();
        report.operations.push("shutdown");
        crate::diagnostics::emit("native_summary", &*report);
    }
    renderer_coordinator.exclusive(|| drop(app));
    result
}

struct App {
    // Renderer is dropped before the shell; its own Arc handle also retains ownership.
    renderer: VelloWindowRenderer,
    renderer_init: Arc<RendererInitCoordinator>,
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
    cadence: FrameCadence,
    report: Rc<RefCell<RunReport>>,
    feed_state: Arc<std::sync::Mutex<OverlayState>>,
    feed_stop: Arc<std::sync::atomic::AtomicBool>,
    external_stop: Arc<std::sync::atomic::AtomicBool>,
    surface_canvas: crate::config::Canvas,
    surface_output: Option<String>,
    canvas: crate::config::Canvas,
    control_socket: std::path::PathBuf,
    appearance: Reactive<Appearance>,
    editor_id: String,
    editing: Reactive<bool>,
    panel_open: Reactive<bool>,
    widget_add_open: Reactive<bool>,
    dirty: Reactive<bool>,
    undo_available: Reactive<bool>,
    readonly: Reactive<bool>,
    editor_pointer_observation: EditorPointerObservation,
    selected: Reactive<Option<String>>,
    pending_widget: Reactive<Option<scorepeek_overlay_ui::WidgetKind>>,
    pending_point: Reactive<[f64; 2]>,
    shared_widgets: Reactive<Vec<WidgetLayout>>,
    interaction: Option<Drag>,
    next_keepalive: Instant,
    managed: Reactive<Vec<scorepeek_overlay_ui::CanvasPresentation>>,
    backend_revision: u64,
    outputs: Reactive<Vec<OutputDescription>>,
    visible: Reactive<bool>,
    pending_resolved_output: Option<String>,
    next_output_persist: Instant,
    preview: Arc<std::sync::Mutex<Option<String>>>,
    workspace_open: Arc<std::sync::atomic::AtomicBool>,
    workspace_ui: Arc<std::sync::Mutex<NativeWorkspace>>,
    surface_canvas_ids: Reactive<std::collections::BTreeSet<String>>,
    suppressed: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
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
    skin_store: std::path::PathBuf,
    skin_assets: Arc<std::sync::Mutex<Option<crate::skin::Package>>>,
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
        renderer_init: Arc<RendererInitCoordinator>,
        mut shell: Shell,
        waker: Waker,
        feed_state: Arc<std::sync::Mutex<OverlayState>>,
        feed_stop: Arc<std::sync::atomic::AtomicBool>,
        external_stop: Arc<std::sync::atomic::AtomicBool>,
        canvas: crate::config::Canvas,
        control_socket: std::path::PathBuf,
        skin_store: std::path::PathBuf,
        outputs: Vec<OutputDescription>,
        report: Rc<RefCell<RunReport>>,
        pending_resolved_output: Option<String>,
        preview: Arc<std::sync::Mutex<Option<String>>>,
        workspace_open: Arc<std::sync::atomic::AtomicBool>,
        workspace_ui: Arc<std::sync::Mutex<NativeWorkspace>>,
        suppressed: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
        wayland_refresh_hz: Arc<std::sync::Mutex<scorepeek_overlay_ui::WaylandRefreshRate>>,
    ) -> Result<Self, String> {
        let shared_state = Rc::new(RefCell::new(OverlayState::default()));
        let reactive = Rc::new(RefCell::new(None));
        let shared_widgets = Rc::new(RefCell::new(widgets));
        let editing = Rc::new(Cell::new(false));
        let ui = workspace_ui
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .ui;
        let panel_open = Rc::new(Cell::new(ui.panel_open));
        let widget_add_open = Rc::new(Cell::new(ui.widget_add_open));
        let dirty = Rc::new(Cell::new(false));
        let undo_available = Rc::new(Cell::new(false));
        let selected = Rc::new(RefCell::new(None));
        let pending_widget = Rc::new(Cell::new(None));
        let pending_point = Rc::new(Cell::new([340.0, 24.0]));
        let managed = Rc::new(RefCell::new(Vec::new()));
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
            output: canvas.output.clone(),
            show_on: canvas.show_on.clone(),
            background: canvas.background,
            opacity_percent: canvas.opacity_percent,
            x: canvas.x,
            y: canvas.y,
            width: canvas.width,
            height: canvas.height,
            preview_screen: ui.preview_screen,
            panel_width,
        }));
        let initially_visible = canvas.show_on.is_none();
        shell.set_input_enabled(initially_visible);
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
        let mut skin_runtime = crate::skin::Runtime::new(&package)?;
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
        let next_skin_render = skin_deadline(&initial.schedule);
        let reactive = reactive
            .borrow()
            .clone()
            .expect("native overlay must publish its reactive state during initial build");
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
            workspace
                .surfaces
                .entry(surface_output.clone().unwrap_or_default())
                .or_default()
                .insert(canvas.id.clone());
        }
        Ok(Self {
            renderer,
            renderer_init,
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
            cadence: FrameCadence::default(),
            report,
            feed_state,
            feed_stop,
            external_stop,
            surface_canvas: canvas.clone(),
            surface_output,
            canvas,
            control_socket,
            appearance: reactive.appearance,
            editor_id: format!("wayland-{}", std::process::id()),
            editing: reactive.editing,
            panel_open: reactive.panel_open,
            widget_add_open: reactive.widget_add_open,
            dirty: reactive.dirty,
            undo_available: reactive.undo_available,
            readonly: reactive.readonly,
            editor_pointer_observation: EditorPointerObservation::default(),
            selected: reactive.selected,
            pending_widget: reactive.pending_widget,
            pending_point: reactive.pending_point,
            shared_widgets: reactive.widgets,
            interaction: None,

            next_keepalive: Instant::now(),
            managed: reactive.managed,
            backend_revision: 0,
            outputs: reactive.outputs,
            pending_resolved_output,
            next_output_persist: Instant::now() + Duration::from_secs(1),
            visible: reactive.visible,
            preview,
            workspace_open,
            workspace_ui,
            surface_canvas_ids: reactive.surface_canvas_ids,
            suppressed,
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
            skin_manifest: package.manifest,
            skin_store,
            skin_assets,
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
            let editing_before = self.editing.get();
            self.sync_workspace_projection();
            let should_edit = self
                .workspace_open
                .load(std::sync::atomic::Ordering::Acquire)
                && self.is_editor_host();
            if should_edit && !self.editing.get() {
                self.set_editing(true);
                self.sync_workspace_projection();
            } else if self.editing.get() && !should_edit {
                self.set_editing(false);
            }
            self.retry_resolved_output();
            if self.editing.get() && Instant::now() >= self.next_keepalive {
                let _ = self.request(crate::control::Request::KeepAliveBackend {
                    backend: crate::runtime::Backend::Wayland,
                    editor_id: self.editor_id.clone(),
                });
                self.next_keepalive = Instant::now() + Duration::from_secs(5);
            }
            let events = self.shell.dispatch(Duration::from_millis(500))?;
            let mut wake = false;
            let mut frame = false;
            let mut configured = false;
            if *self.outputs.borrow() != self.shell.output_descriptions {
                self.outputs
                    .borrow_mut()
                    .clone_from(&self.shell.output_descriptions);
                let mut workspace = self
                    .workspace_ui
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                workspace.outputs = self
                    .shell
                    .output_descriptions
                    .iter()
                    .map(|output| (output.name.clone(), output.clone()))
                    .collect();
                drop(workspace);
                wake = true;
            }
            for event in events {
                match event {
                    Event::Configure {
                        logical,
                        physical,
                        scale_120,
                    } => {
                        self.configure(logical, physical, scale_120)?;
                        configured = true;
                        wake = true;
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
                        if self.editing.get()
                            && self.panel_open.get()
                            && scroll_editor_at(
                                &mut self.document.inner.borrow_mut(),
                                [x, y],
                                [dx, dy],
                            )
                        {
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
            let workspace_peer = self
                .workspace_open
                .load(std::sync::atomic::Ordering::Acquire)
                && !self.editing.get();
            let visible = !workspace_peer
                && (self.editing.get()
                    || scorepeek_overlay_ui::canvas_visible(
                        self.surface_canvas.show_on.as_deref(),
                        latest.screen,
                    ));
            let visibility_changed = self.visible.get() != visible;
            if visibility_changed {
                self.visible.set(visible);
                self.shell.set_input_enabled(visible);
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
                        self.update_editor_skin();
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
                    self.update_editor_skin();
                } else {
                    self.render_skin(&latest)?;
                }
                wake = true;
            }
            let changed =
                should_poll_dioxus(wake, editing_before, self.editing.get()) && self.poll_dioxus();
            self.pending_paint |= changed || visibility_changed;
            let reason = if configured {
                Some(if self.paint_count == 0 {
                    PaintReason::InitialConfigure
                } else {
                    PaintReason::Reconfigure
                })
            } else if visibility_changed && !visible {
                Some(PaintReason::VisibilityClear)
            } else if self.editing.get()
                && visible
                && (self.pending_paint || (frame && self.animating))
            {
                Some(PaintReason::Editor)
            } else if visible && (self.pending_paint || (frame && self.animating)) {
                Some(PaintReason::Steady)
            } else {
                None
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
        let fallback = self.canvas.clone();
        self.pending_resolved_output = None;
        self.workspace_open
            .store(true, std::sync::atomic::Ordering::Release);
        self.select_canvas(Some(self.canvas.id.clone()));
        self.set_editing(true);
        self.canvas.x = fallback.x;
        self.canvas.y = fallback.y;
        self.canvas.width = fallback.width;
        self.canvas.height = fallback.height;
        self.canvas.output = Some(output.clone());
        {
            let mut settings = self.settings.borrow_mut();
            settings.x = fallback.x;
            settings.y = fallback.y;
            settings.width = fallback.width;
            settings.height = fallback.height;
            settings.output = Some(output.clone());
        }
        self.persist_canvas();
        crate::diagnostics::emit(
            "native_output_fallback",
            &serde_json::json!({
                "canvas_id": self.canvas.id,
                "selected_output": output,
                "status": "editor_opened",
            }),
        );
    }
    #[allow(clippy::needless_pass_by_value)]
    fn request(&mut self, request: crate::control::Request) -> Option<crate::control::Response> {
        let updates_readonly = control_updates_readonly(&request);
        let response = crate::control::request(&self.control_socket, &request).ok()?;
        if updates_readonly {
            self.readonly.set(response.readonly);
        }
        if let Some(revision) = response.backend_revision {
            self.backend_revision = revision;
            self.dirty.set(response.dirty);
            self.managed.borrow_mut().clone_from(&response.canvases);
            {
                let mut workspace = self
                    .workspace_ui
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                workspace.draft.clone_from(&response.canvases);
                workspace.dirty = response.dirty;
            }
            let selected = if self.editing.get() {
                self.preview
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
            } else {
                Some(self.surface_canvas.id.clone())
            };
            if let Some(presentation) = response
                .canvases
                .iter()
                .find(|item| Some(item.id.as_str()) == selected.as_deref())
            {
                self.apply_selected_presentation(presentation);
            }
        }
        if let Some(refresh) = response.wayland_refresh_hz {
            self.set_refresh_rate_draft(refresh);
        }
        Some(response)
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

    fn is_editor_host(&self) -> bool {
        let key = self.surface_output.clone().unwrap_or_default();
        let mut workspace = self
            .workspace_ui
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pinned = workspace.editor_hosts.get(&key).map(String::as_str);
        let host = workspace
            .surfaces
            .get(&key)
            .and_then(|surfaces| editor_surface_host(surfaces, pinned));
        if let Some(host) = &host {
            workspace.editor_hosts.insert(key, host.clone());
        }
        host.as_deref() == Some(self.surface_canvas.id.as_str())
    }

    fn pin_editor_host(&self) {
        self.workspace_ui
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .editor_hosts
            .insert(
                self.surface_output.clone().unwrap_or_default(),
                self.surface_canvas.id.clone(),
            );
    }

    fn sync_workspace_projection(&mut self) {
        let (
            ui,
            draft,
            dirty,
            undo_available,
            selected_widget,
            pending_widget,
            surface_canvas_ids,
            wayland_refresh_hz,
        ) = {
            let workspace = self
                .workspace_ui
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let key = self.surface_output.clone().unwrap_or_default();
            (
                workspace.ui,
                workspace.draft.clone(),
                workspace.dirty,
                workspace.undo.is_some(),
                workspace.selected_widget.clone(),
                workspace.pending_widget,
                workspace.surfaces.get(&key).cloned().unwrap_or_default(),
                workspace.wayland_refresh_hz,
            )
        };
        self.refresh_rate.set_if_changed(wayland_refresh_hz);
        if *self.surface_canvas_ids.borrow() != surface_canvas_ids {
            *self.surface_canvas_ids.borrow_mut() = surface_canvas_ids;
        }
        self.panel_open.set_if_changed(ui.panel_open);
        self.widget_add_open.set_if_changed(ui.widget_add_open);
        let has_selection = self
            .preview
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        self.undo_available.set_if_changed(undo_available);
        if *self.selected.borrow() != selected_widget {
            self.finish_title_edit(false);
            *self.selected.borrow_mut() = selected_widget;
        }
        self.pending_widget.set_if_changed(pending_widget);
        if self.interaction.is_none() && !draft.is_empty() && *self.managed.borrow() != draft {
            self.managed.borrow_mut().clone_from(&draft);
        }
        if self.editing.get() && self.interaction.is_none() {
            let selected = self
                .preview
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let presentation = selected.as_deref().and_then(|id| {
                self.managed
                    .borrow()
                    .iter()
                    .find(|canvas| canvas.id == id)
                    .cloned()
            });
            if let Some(presentation) = presentation
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
    fn acquire(&mut self) {
        let _ = self.request(crate::control::Request::AcquireBackend {
            backend: crate::runtime::Backend::Wayland,
            editor_id: self.editor_id.clone(),
        });
    }
    fn set_editing(&mut self, value: bool) {
        if value {
            self.editing.set(true);
            self.editor_pointer_observation = EditorPointerObservation::AwaitingPress;
            self.preview_skin_runtime = None;
            self.acquire();
            self.next_keepalive = Instant::now() + Duration::from_secs(5);
            self.update_editor_skin();
        } else if !self
            .workspace_open
            .load(std::sync::atomic::Ordering::Acquire)
        {
            let _ = self.request(crate::control::Request::ReleaseBackend {
                backend: crate::runtime::Backend::Wayland,
                editor_id: self.editor_id.clone(),
            });
        }
        if !value {
            self.finish_title_edit(false);
            self.finish_refresh_edit(false);
            self.editing.set(false);
            self.dirty.set(false);
            self.canvas = self.surface_canvas.clone();
            let presentation = self.canvas.presentation();
            self.apply_selected_presentation(&presentation);
            self.update_editor_skin();
            self.preview_skin_runtime = None;
        }
        self.set_editor_geometry(value);
        if value && !self.visible.replace(true) {
            self.shell.set_input_enabled(true);
        }
        self.interaction = None;

        if !value {
            self.selected.borrow_mut().take();
            self.pending_widget.set(None);
        }
        self.sync_workspace_ui();
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
                self.pin_editor_host();
                self.workspace_open
                    .store(true, std::sync::atomic::Ordering::Release);
                if let Some(screen) = self.shared_state.borrow().screen.kind {
                    self.settings.borrow_mut().preview_screen = screen;
                }
                self.select_canvas(Some(self.canvas.id.clone()));
                self.set_editing(true);
            }
            return;
        }
        if pressed && self.editor_pointer_observation == EditorPointerObservation::AwaitingPress {
            self.editor_pointer_observation = EditorPointerObservation::AwaitingRelease;
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
        model.placing = self.pending_widget.get();
        model.drag.clone_from(&self.interaction);
        model
    }
    fn apply_editor_model(&mut self, model: EditorModel) {
        if let Some(canvas) = model.current() {
            self.apply_selected_presentation(canvas);
        }
        self.managed.set(model.draft);
        self.settings.borrow_mut().preview_screen = model.preview;
        self.panel_open.set(model.chrome.panel_open);
        self.widget_add_open.set(model.chrome.widget_add_open);
        self.pending_widget.set(model.placing);
        self.select_canvas(model.selected_canvas);
        self.selected.set(model.selected_widget);
        self.interaction = model.drag;
        self.update_editor_skin();
        self.sync_workspace_ui();
    }

    fn update_editor_skin(&mut self) {
        if let Err(error) = self.render_editor_skin() {
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
                &serde_json::json!({"skin_id":self.canvas.skin.name(),"canvas_id":self.canvas.id,"backend":"native","phase":if self.editing.get() { "editor-preview" } else { "render" },"status":"failed","error_type":skin_error_type(&error),"tree_applied":true}),
            );
        }
    }

    fn render_editor_skin(&mut self) -> Result<(), String> {
        let state = if self.editing.get()
            && self.shared_state.borrow().system == scorepeek_overlay_ui::LampState::Inactive
        {
            scorepeek_overlay_ui::editor_sample_state()
        } else {
            self.shared_state.borrow().clone()
        };
        let package =
            crate::skin::StoreRoot::new(self.skin_store.clone()).open(self.canvas.skin.name())?;
        let css = std::str::from_utf8(
            package
                .resource(crate::skin::STYLE_PATH)
                .ok_or("skin.css missing")?,
        )
        .map_err(|error| format!("skin.css is not UTF-8: {error}"))?
        .to_owned();
        if self.editing.get() {
            let same_preview =
                self.preview_skin_runtime
                    .as_ref()
                    .is_some_and(|(id, release, _)| {
                        id == &package.manifest.id && release == &package.manifest.release
                    });
            let input = native_skin_input(&self.canvas, &state, &package.manifest);
            let output = if same_preview {
                self.preview_skin_runtime
                    .as_mut()
                    .expect("matching preview runtime must exist")
                    .2
                    .render(&input)?
            } else {
                let mut runtime = crate::skin::Runtime::new(&package)?;
                let output = runtime.init(&input)?;
                self.preview_skin_runtime = Some((
                    package.manifest.id.clone(),
                    package.manifest.release.clone(),
                    runtime,
                ));
                output
            };
            *self
                .skin_assets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(package);
            if same_preview {
                self.skin_tree
                    .apply(&mut self.document.inner.borrow_mut(), &output);
            } else {
                self.skin_tree
                    .replace(&mut self.document.inner.borrow_mut(), &css, &output);
            }
            self.next_skin_render = skin_deadline(&output.schedule);
            self.animating = matches!(output.schedule, crate::skin::Schedule::NextFrame);
        } else if self.skin_manifest.id == package.manifest.id
            && self.skin_release == package.manifest.release
        {
            *self
                .skin_assets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(package);
            self.render_skin(&state)?;
            self.skin_tree
                .set_css(&mut self.document.inner.borrow_mut(), &css);
        } else {
            let mut runtime = crate::skin::Runtime::new(&package)?;
            let output =
                runtime.init(&native_skin_input(&self.canvas, &state, &package.manifest))?;
            *self
                .skin_assets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(package.clone());
            self.skin_tree
                .replace(&mut self.document.inner.borrow_mut(), &css, &output);
            self.next_skin_render = skin_deadline(&output.schedule);
            self.animating = matches!(output.schedule, crate::skin::Schedule::NextFrame);
            self.skin_release.clone_from(&package.manifest.release);
            self.skin_manifest = package.manifest;
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
    fn editor_action(&mut self, action: &EditorAction) {
        if !matches!(action, EditorAction::AcceptTitle | EditorAction::EditTitle) {
            self.finish_title_edit(false);
        }
        if !matches!(
            action,
            EditorAction::EditRefreshRate
                | EditorAction::AcceptRefreshRate
                | EditorAction::CancelRefreshRate
                | EditorAction::RefreshRateAuto
        ) {
            self.finish_refresh_edit(false);
        }
        match action {
            EditorAction::Save => {
                self.save_and_close();
                return;
            }
            EditorAction::Close | EditorAction::Discard => {
                let mut workspace = self
                    .workspace_ui
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if *action == EditorAction::Discard {
                    self.suppressed
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .extend(std::mem::take(&mut workspace.fallback));
                    workspace.reload_saved = true;
                }
                workspace.undo = None;
                drop(workspace);
                self.workspace_open
                    .store(false, std::sync::atomic::Ordering::Release);
                self.select_canvas(None);
                self.pending_widget.set(None);
                self.set_editing(false);
                return;
            }
            EditorAction::Undo => {
                self.finish_refresh_edit(false);
                self.undo_last_change();
                return;
            }
            EditorAction::RefreshRateAuto => {
                self.finish_refresh_edit(false);
                let before = self.undo_snapshot();
                self.set_refresh_rate_draft(scorepeek_overlay_ui::WaylandRefreshRate::Auto);
                self.finish_draft_change(before);
                return;
            }
            EditorAction::EditRefreshRate => {
                self.finish_refresh_edit(false);
                let text = self
                    .refresh_rate
                    .get()
                    .hz()
                    .map_or_else(String::new, |hz| hz.to_string());
                self.refresh_edit
                    .set(Some(TitleEdit::new("refresh-rate".into(), text)));
                self.update_refresh_input(false);
                return;
            }
            EditorAction::AcceptRefreshRate => {
                self.finish_refresh_edit(true);
                return;
            }
            EditorAction::CancelRefreshRate => {
                self.finish_refresh_edit(false);
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
    }

    fn select_canvas(&self, next: Option<String>) {
        replace_canvas_selection(
            &mut self
                .preview
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            &mut self.selected.borrow_mut(),
            next,
        );
        self.sync_workspace_ui();
    }
    fn update_draft(&mut self) {
        let canvases = self.managed.borrow().clone();
        let _ = self.request(crate::control::Request::UpdateBackendDraft {
            backend: crate::runtime::Backend::Wayland,
            editor_id: self.editor_id.clone(),
            canvases,
            wayland_refresh_hz: Some(self.refresh_rate.get()),
        });
    }
    fn save_and_close(&mut self) {
        if self.refresh_edit.borrow().is_some() && !self.finish_refresh_edit(true) {
            return;
        }
        self.persist_canvas();
        let canvases = self.managed.borrow().clone();
        let response = self.request(crate::control::Request::CommitBackend {
            backend: crate::runtime::Backend::Wayland,
            editor_id: self.editor_id.clone(),
            expected_revision: self.backend_revision,
            canvases,
            wayland_refresh_hz: Some(self.refresh_rate.get()),
        });
        if response.as_ref().is_some_and(|response| response.ok) {
            let mut workspace = self
                .workspace_ui
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            workspace.fallback.clear();
            workspace.undo = None;
            workspace.reload_saved = true;
            drop(workspace);
            self.workspace_open
                .store(false, std::sync::atomic::Ordering::Release);
            self.select_canvas(None);
            self.set_editing(false);
        }
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
        let mut changed = false;
        while self
            .document
            .poll(Some(TaskContext::from_waker(&self.waker)))
        {
            changed = true;
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
        self.next_skin_render = skin_deadline(&output.schedule);
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
    ) -> Result<(), String> {
        let [width, height] = physical;
        self.surface_logical = logical;
        let physical_changed = {
            let mut report = self.report.borrow_mut();
            let changed = report.physical_size != Some(physical);
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
            if physical_changed {
                let renderer_coordinator = Arc::clone(&self.renderer_init);
                renderer_coordinator.exclusive(|| self.renderer.set_size(width, height));
            }
        } else {
            let renderer_coordinator = Arc::clone(&self.renderer_init);
            let info = renderer_coordinator.exclusive(|| {
                self.renderer
                    .resume(self.shell.handles(), width, height, || {});
                if !self.renderer.complete_resume() {
                    return Err("gpu_adapter".to_owned());
                }
                self.renderer
                    .current_device_handle()
                    .map(|device| device.adapter.get_info())
                    .ok_or_else(|| "gpu_adapter".to_owned())
            })?;
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
        Ok(())
    }
    fn paint(&mut self, reason: PaintReason, now: Duration) -> Result<(), String> {
        let renderer_coordinator = Arc::clone(&self.renderer_init);
        renderer_coordinator.exclusive(|| self.paint_exclusive())?;
        self.cadence.record(now);
        self.pending_paint = false;
        if reason == PaintReason::Steady {
            self.steady_paint_count = self.steady_paint_count.saturating_add(1);
        } else {
            let mut report = self.report.borrow_mut();
            *report
                .cap_bypass_paints
                .entry(reason.name().to_owned())
                .or_default() += 1;
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
        resolve_with_loaded_resources(&mut inner, seconds);
        self.animating = self.visible.get();
        if self.animating {
            self.shell.request_frame();
        }
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

impl Drop for App {
    fn drop(&mut self) {
        {
            let mut workspace = self
                .workspace_ui
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let key = self.surface_output.clone().unwrap_or_default();
            if let Some(surfaces) = workspace.surfaces.get_mut(&key) {
                surfaces.remove(&self.surface_canvas.id);
                if surfaces.is_empty() {
                    workspace.surfaces.remove(&key);
                }
            }
            if workspace.editor_hosts.get(&key) == Some(&self.surface_canvas.id) {
                let next = workspace
                    .surfaces
                    .get(&key)
                    .and_then(|surfaces| surfaces.first())
                    .cloned();
                if let Some(next) = next {
                    workspace.editor_hosts.insert(key, next);
                } else {
                    workspace.editor_hosts.remove(&key);
                }
            }
        }
        let workspace_open = self
            .workspace_open
            .load(std::sync::atomic::Ordering::Acquire);
        if release_backend_on_drop(self.editing.get(), workspace_open) {
            let _ = crate::control::request(
                &self.control_socket,
                &crate::control::Request::ReleaseBackend {
                    backend: crate::runtime::Backend::Wayland,
                    editor_id: self.editor_id.clone(),
                },
            );
        }
    }
}

const fn release_backend_on_drop(editing: bool, workspace_open: bool) -> bool {
    editing && !workspace_open
}
const fn should_poll_dioxus(surface_wake: bool, editing_before: bool, editing_after: bool) -> bool {
    surface_wake || editing_before != editing_after
}
const fn control_updates_readonly(request: &crate::control::Request) -> bool {
    matches!(
        request,
        crate::control::Request::AcquireBackend { .. }
            | crate::control::Request::KeepAliveBackend { .. }
            | crate::control::Request::ReleaseBackend { .. }
            | crate::control::Request::UpdateBackendDraft { .. }
            | crate::control::Request::CommitBackend { .. }
    )
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
    wayland_refresh_hz: scorepeek_overlay_ui::WaylandRefreshRate,
    elapsed_ms: u64,
    effective_paint_hz: Option<f64>,
    effective_steady_paint_hz: Option<f64>,
    cap_bypass_paints: std::collections::BTreeMap<String, u64>,
    operations: Operations,
    status: &'static str,
    failure: Option<String>,
}

impl RunReport {
    fn new() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        Self {
            run_id: format!("{timestamp}-{}", std::process::id()),
            build_revision: option_env!("OVERLAY_BUILD_REVISION").unwrap_or("working-tree"),
            output_name: None,
            logical_size: None,
            physical_size: None,
            scale_120: 120,
            gpu_backend: None,
            gpu_adapter: None,
            paint_count: 0,
            render_calls: 0,
            wayland_refresh_hz: scorepeek_overlay_ui::WaylandRefreshRate::Auto,
            elapsed_ms: 0,
            effective_paint_hz: None,
            effective_steady_paint_hz: None,
            cap_bypass_paints: std::collections::BTreeMap::new(),
            operations: Operations::default(),
            status: "running",
            failure: None,
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
    panel_open: Reactive<bool>,
    widget_add_open: Reactive<bool>,
    selected: Reactive<Option<String>>,
    managed: Reactive<Vec<scorepeek_overlay_ui::CanvasPresentation>>,
    state: Reactive<OverlayState>,
    visible: Reactive<bool>,
    settings: Reactive<NativeCanvasSettings>,
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
            show_on: canvas.show_on.clone(),
            background: canvas.background,
            opacity_percent: canvas.opacity_percent,
            x: canvas.x,
            y: canvas.y,
            width: canvas.width,
            height: canvas.height,
            preview_screen: scorepeek_overlay_ui::ScreenKind::MusicSelect,
            panel_width: editor_panel_width(Some(scenario.logical_size[0])),
        }));
        let reactive = Rc::new(RefCell::new(None));
        let actions = Rc::new(RefCell::new(Vec::new()));
        let surface_actions = Rc::new(RefCell::new(Vec::new()));
        let props = NativeOverlayProps {
            appearance: Rc::clone(&appearance),
            widgets: Rc::clone(&widgets),
            editing: Rc::clone(&editing),
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
            panel_open: reactive.panel_open,
            widget_add_open: reactive.widget_add_open,
            selected: reactive.selected,
            managed: reactive.managed,
            state: reactive.state,
            visible: reactive.visible,
            settings: reactive.settings,
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
        *self.settings.borrow_mut() = NativeCanvasSettings {
            id: canvas.id,
            has_selection: true,
            output: canvas.output,
            show_on: canvas.show_on,
            background: canvas.background,
            opacity_percent: canvas.opacity_percent,
            x: canvas.x,
            y: canvas.y,
            width: canvas.width,
            height: canvas.height,
            preview_screen,
            panel_width: editor_panel_width(Some(self.logical_size[0])),
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
        model
    }
    fn apply_editor_model(&self, model: &EditorModel) {
        self.managed.set(model.draft.clone());
        self.settings.borrow_mut().preview_screen = model.preview;
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
        let mut inner = self.document.inner.borrow_mut();
        let node = inner
            .query_selector(selector)
            .map_err(|_| "invalid selector".to_owned())?
            .ok_or_else(|| format!("selector did not match: {selector}"))?;
        inner.scroll_node_by(node, dx, dy, |_| {});
        inner.resolve(1.0);
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
                ".native-panel-toggle",
                ".native-canvas-manager",
                ".editor-tab-body",
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
                VisualDebugAction::Capture { name } => sanitize_artifact_name(name),
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
        session.click(".preview-screen[data-index='4']").unwrap();
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
                session
                    .click(".screen-toggle[data-canvas-id='empty-output']")
                    .unwrap();
            }
            let inner = session.document.inner.borrow();
            assert_eq!(
                inner.query_selector_all(".editor-canvas").unwrap().len(),
                expected
            );
            let panel = inner
                .query_selector(".native-canvas-manager")
                .unwrap()
                .unwrap();
            let panel = inner.get_client_bounding_rect(panel).unwrap();
            assert_eq!((panel.width, panel.height), (384.0, 1080.0));
            let footer = inner.query_selector("footer").unwrap().unwrap();
            let footer = inner.get_client_bounding_rect(footer).unwrap();
            assert!(footer.width > 250.0 && footer.y > 900.0 && footer.y + footer.height <= 1080.0);
            let body = inner.query_selector(".editor-tab-body").unwrap().unwrap();
            assert!(inner.get_client_bounding_rect(body).unwrap().height > 100.0);
        }
    }
    #[test]
    fn unselected_canvas_consumes_first_widget_body_and_corner_gesture() {
        for corner in [false, true] {
            let mut scenario: VisualDebugScenario =
                serde_json::from_str(include_str!("../tests/fixtures/visual-composition.json"))
                    .unwrap();
            let mut other = scenario.canvases.as_ref().unwrap()[0].clone();
            other.id = "other".into();
            other.x = 800;
            other.y = 400;
            other.width = 256;
            other.height = 256;
            let mut widget = other
                .widgets
                .iter()
                .find(|w| w.id == "cam")
                .unwrap()
                .clone();
            widget.x = 24;
            widget.y = 24;
            widget.width = 100;
            widget.height = 100;
            other.widgets = vec![widget];
            scenario.canvases.as_mut().unwrap().push(other);
            let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
            session.editing.set(true);
            session.panel_open.set(false);
            session.resolve();
            let point = if corner {
                [919.0, 519.0]
            } else {
                [850.0, 450.0]
            };
            let to = [point[0] - 20.0, point[1] - 20.0];
            let mut model = session.editor_model();
            let before = model.draft.clone();
            session
                .pointer
                .dispatch(&mut session.document, point, 0x110, None);
            session.resolve();
            session
                .pointer
                .dispatch(&mut session.document, point, 0x110, Some(true));
            session.drain_editor_events(&mut model);
            assert!(model.drag.is_none());
            assert_eq!(model.selected_canvas.as_deref(), Some("other"));
            session.resolve();
            session
                .pointer
                .dispatch(&mut session.document, to, 0x110, None);
            session
                .pointer
                .dispatch(&mut session.document, to, 0x110, Some(false));
            session.drain_editor_events(&mut model);
            session.resolve();
            assert_eq!(
                model.draft, before,
                "first gesture only selects; corner={corner}"
            );
            session.drag(point, to, VisualDebugButton::Left).unwrap();
            assert_ne!(
                *session.managed.borrow(),
                before,
                "second gesture edits; corner={corner}"
            );
        }
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
    fn renderer_operations_are_serialized_across_surfaces() {
        use std::sync::{
            Barrier,
            atomic::{AtomicUsize, Ordering},
        };

        let coordinator = Arc::new(RendererInitCoordinator::default());
        let barrier = Arc::new(Barrier::new(3));
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let coordinator = Arc::clone(&coordinator);
            let barrier = Arc::clone(&barrier);
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                coordinator.exclusive(|| {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(current, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(25));
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }));
        }
        barrier.wait();
        for worker in workers {
            worker.join().unwrap();
        }

        assert_eq!(maximum.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn surface_handoff_keeps_the_backend_workspace_lease() {
        assert!(!release_backend_on_drop(true, true));
        assert!(release_backend_on_drop(true, false));
        assert!(!release_backend_on_drop(false, false));
    }

    #[test]
    fn peer_editor_transition_polls_without_a_local_surface_event() {
        assert!(should_poll_dioxus(false, true, false));
        assert!(should_poll_dioxus(false, false, true));
        assert!(should_poll_dioxus(true, false, false));
        assert!(!should_poll_dioxus(false, false, false));
    }

    #[test]
    fn added_surface_does_not_replace_the_pinned_editor_host() {
        let mut surfaces =
            std::collections::BTreeSet::from(["canvas-b".to_owned(), "canvas-a".to_owned()]);

        let pinned = editor_surface_host(&surfaces, None).unwrap();
        assert_eq!(pinned, "canvas-a");

        surfaces.insert("canvas-0-added".to_owned());
        assert_eq!(
            editor_surface_host(&surfaces, Some(&pinned)).as_deref(),
            Some("canvas-a")
        );
        assert_eq!(
            editor_surface_host(&surfaces, Some("removed-canvas")).as_deref(),
            Some("canvas-0-added")
        );
    }

    #[test]
    fn one_draft_snapshot_undoes_every_field_and_is_not_replaced_by_a_no_op() {
        let before = crate::config::OverlayConfig::initial()
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
    fn visual_debug_surface_contains_every_headless_canvas() {
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
        session.click(".preview-screen[data-index='4']").unwrap();
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
        assert_eq!(canvas_rects, 2);
        assert_eq!(widget_rects, 5);
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
        session.click(".preview-screen[data-index='4']").unwrap();
        session
            .click(".canvas-select[data-canvas-id='wayland-result']")
            .unwrap();
        session.click(".native-panel-toggle").unwrap();

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

        session.click(".preview-screen[data-index='4']").unwrap();
        session
            .click(".canvas-select[data-canvas-id='wayland-result']")
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
                            show_on: None,
                            background: scorepeek_overlay_ui::Background::None,
                            opacity_percent: 100,
                            x: 0,
                            y: 0,
                            width: 560,
                            height: 1040,
                            preview_screen: scorepeek_overlay_ui::ScreenKind::MusicSelect,
                            panel_width: 400,
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
                        show_on: None,
                        background: scorepeek_overlay_ui::Background::None,
                        opacity_percent: 100,
                        x: 0,
                        y: 0,
                        width: 560,
                        height: 72,
                        preview_screen: scorepeek_overlay_ui::ScreenKind::MusicSelect,
                        panel_width: 400,
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
    fn canvas_list_does_not_replace_the_lease_state() {
        assert!(control_updates_readonly(
            &crate::control::Request::KeepAliveBackend {
                backend: crate::runtime::Backend::Wayland,
                editor_id: "editor".into(),
            }
        ));
        assert!(!control_updates_readonly(
            &crate::control::Request::GetBackend {
                backend: crate::runtime::Backend::Wayland,
            }
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
    #[allow(clippy::too_many_lines)]
    fn output_picker_stays_inside_the_overlay_panel_at_actual_canvas_coordinates() {
        let canvas = scorepeek_overlay_ui::CanvasPresentation {
            id: "wayland-selection".into(),
            skin: Skin::CyanSystem,
            skin_properties: std::collections::BTreeMap::new(),
            show_on: None,
            background: scorepeek_overlay_ui::Background::None,
            opacity_percent: 100,
            output: Some("DP-1".into()),
            revision: 0,
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
                        show_on: None,
                        background: scorepeek_overlay_ui::Background::None,
                        opacity_percent: 100,
                        x: 120,
                        y: 80,
                        width: 560,
                        height: 120,
                        preview_screen: scorepeek_overlay_ui::ScreenKind::MusicSelect,
                        panel_width: 384,
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
        let panel = rect(".native-canvas-manager");
        let output = rect(".output-option");
        let canvas_section = rect(".canvas-section");
        let appearance = rect(".appearance-pane");
        let output_section = rect(".output-pane");
        let undo = rect(".undo-action");
        let delete = rect(".delete-canvas");
        let preview = rect(".editor-canvas.selected");
        let canvas_list = rect(".canvas-list");
        let cyan = rect(".skin-option[data-index='0']");
        let aurora = rect(".skin-option[data-index='1']");
        let blackbox = rect(".skin-option[data-index='2']");
        let last_before = rect(".canvas-select[data-canvas-id='wayland-extra-5']");
        assert!((panel.width - 384.0).abs() < 1.0, "{panel:?}");
        assert!(
            canvas_section.y < appearance.y,
            "{canvas_section:?} {appearance:?}"
        );
        assert!(
            appearance.y < output_section.y,
            "{appearance:?} {output_section:?}"
        );
        assert!(output.x >= panel.x && output.x + output.width <= panel.x + panel.width);
        assert!(undo.x >= panel.x && undo.y > canvas_section.y, "{undo:?}");
        assert!(delete.width > 0.0, "{delete:?}");
        assert!(
            inner
                .query_selector(".delete-canvas[disabled]")
                .unwrap()
                .is_none()
        );
        assert!(cyan.width > 0.0 && aurora.width > 0.0 && blackbox.width > 0.0);
        assert!(cyan.x >= panel.x && blackbox.x + blackbox.width <= panel.x + panel.width);
        assert!(blackbox.y + blackbox.height <= output_section.y);
        assert!(inner.query_selector(".manage-canvas").unwrap().is_none());
        assert!(inner.query_selector(".output-settings").unwrap().is_none());
        assert!((preview.x - 120.0).abs() < 1.0, "{preview:?}");
        assert!(scroll_editor_at(
            &mut inner,
            [canvas_list.x + 8.0, canvas_list.y + 8.0],
            [0.0, -120.0]
        ));
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
    fn screen_switch_is_the_only_canvas_visibility_state() {
        use scorepeek_overlay_ui::editor_model::SCREENS;
        let mut canvas = crate::config::OverlayConfig::initial().canvases[0].presentation();
        canvas.show_on = None;
        let mut model = EditorModel::new(vec![canvas.clone()], [1920, 1080], "wayland");
        model.readonly = false;
        for screen in SCREENS {
            model.preview = screen;
            model.action(&EditorAction::ToggleCanvas(canvas.id.clone()));
        }
        assert_eq!(model.draft[0].show_on, Some(Vec::new()));
        for screen in SCREENS {
            model.preview = screen;
            model.action(&EditorAction::ToggleCanvas(canvas.id.clone()));
        }
        assert_eq!(model.draft[0].show_on, None);
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
            session.scroll(".editor-tab-body", 0.0, -2000.0).unwrap();
            session.click(".widget-row[data-widget-id='cam']").unwrap();
            assert_eq!(session.selected.borrow().as_deref(), Some("cam"));
            session.scroll(".editor-tab-body", 0.0, -2000.0).unwrap();
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
