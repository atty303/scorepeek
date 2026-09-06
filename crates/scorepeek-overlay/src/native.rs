use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
    task::{Context as TaskContext, Wake, Waker},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::runtime::{Config, Feed};
use anyrender::{CompositeAlphaMode, WindowRenderer};
use anyrender_vello::{VelloRendererOptions, VelloWindowRenderer};
use blitz_dom::{BaseDocument, Document, DocumentConfig};
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};
use dioxus::prelude::*;
use dioxus_core::VirtualDom;
use dioxus_native_dom::DioxusDocument;
use scorepeek_overlay_handles::{CursorStyle, Event, OutputDescription, Shell};
use scorepeek_overlay_ui::{Appearance, OXANIUM, OverlayState, WidgetLayout, overlay_canvas};
use serde::{Deserialize, Serialize};
use smithay_client_toolkit::reexports::calloop::ping::{Ping, make_ping};

#[derive(Default)]
struct RendererInitCoordinator(std::sync::Mutex<()>);

impl RendererInitCoordinator {
    fn exclusive<T>(&self, operation: impl FnOnce() -> T) -> T {
        let _guard = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        operation()
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

fn unselected_canvas_at(
    canvases: &[scorepeek_overlay_ui::CanvasPresentation],
    surface_canvas_ids: &std::collections::BTreeSet<String>,
    selected: &scorepeek_overlay_ui::CanvasPresentation,
    screen: scorepeek_overlay_ui::ScreenKind,
    point: [f64; 2],
) -> Option<String> {
    let [x, y] = point;
    let contains = |canvas: &scorepeek_overlay_ui::CanvasPresentation| {
        x >= f64::from(canvas.x)
            && y >= f64::from(canvas.y)
            && x < f64::from(canvas.x) + f64::from(canvas.width)
            && y < f64::from(canvas.y) + f64::from(canvas.height)
    };
    if surface_canvas_ids.contains(&selected.id) && shown_on(selected, screen) && contains(selected)
    {
        return None;
    }
    canvases
        .iter()
        .find(|canvas| {
            canvas.id != selected.id
                && surface_canvas_ids.contains(&canvas.id)
                && shown_on(canvas, screen)
                && contains(canvas)
        })
        .map(|canvas| canvas.id.clone())
}

const EDITOR_SCREENS: [scorepeek_overlay_ui::ScreenKind; 5] = [
    scorepeek_overlay_ui::ScreenKind::MusicSelect,
    scorepeek_overlay_ui::ScreenKind::ModeSelect,
    scorepeek_overlay_ui::ScreenKind::DecideTransition,
    scorepeek_overlay_ui::ScreenKind::Play,
    scorepeek_overlay_ui::ScreenKind::Result,
];

fn shown_on(
    canvas: &scorepeek_overlay_ui::CanvasPresentation,
    screen: scorepeek_overlay_ui::ScreenKind,
) -> bool {
    canvas
        .show_on
        .as_ref()
        .is_none_or(|screens| screens.contains(&screen))
}

fn set_shown_on(
    canvas: &mut scorepeek_overlay_ui::CanvasPresentation,
    screen: scorepeek_overlay_ui::ScreenKind,
    shown: bool,
) {
    if shown {
        let Some(screens) = canvas.show_on.as_mut() else {
            return;
        };
        if !screens.contains(&screen) {
            screens.push(screen);
        }
        if EDITOR_SCREENS.iter().all(|screen| screens.contains(screen)) {
            canvas.show_on = None;
        }
    } else {
        let screens = canvas
            .show_on
            .get_or_insert_with(|| EDITOR_SCREENS.to_vec());
        screens.retain(|candidate| *candidate != screen);
    }
}

#[derive(Clone)]
struct NativeCanvasSettings {
    id: String,
    has_selection: bool,
    output: Option<String>,
    show_on: Option<Vec<scorepeek_overlay_ui::ScreenKind>>,
    opacity_percent: u8,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    preview_screen: scorepeek_overlay_ui::ScreenKind,
    panel_width: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CanvasGeometry {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl NativeCanvasSettings {
    fn set_geometry(&mut self, geometry: CanvasGeometry) {
        self.x = geometry.x;
        self.y = geometry.y;
        self.width = geometry.width;
        self.height = geometry.height;
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
struct DraftUndo(Vec<scorepeek_overlay_ui::CanvasPresentation>);

fn remember_draft_change(
    undo: &mut Option<DraftUndo>,
    before: Vec<scorepeek_overlay_ui::CanvasPresentation>,
    after: &[scorepeek_overlay_ui::CanvasPresentation],
) -> bool {
    if before == after {
        return false;
    }
    *undo = Some(DraftUndo(before));
    true
}

fn selected_canvas_for_delete(
    canvases: &[scorepeek_overlay_ui::CanvasPresentation],
    selected: Option<&str>,
) -> Option<String> {
    if canvases.len() <= 1 {
        return None;
    }
    selected
        .filter(|id| canvases.iter().any(|canvas| canvas.id == *id))
        .map(str::to_owned)
}

fn replace_canvas_selection(
    selected_canvas: &mut Option<String>,
    selected_widget: &mut Option<String>,
    next: Option<String>,
) {
    *selected_canvas = next;
    selected_widget.take();
}

fn discard_undo(workspace: &mut NativeWorkspace) {
    workspace.undo = None;
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
}

fn delete_canvas_button(disabled: bool) -> Element {
    if disabled {
        rsx! { button { class:"delete-canvas danger", disabled:true, "DELETE SELECTED" } }
    } else {
        rsx! { button { class:"delete-canvas danger", "DELETE SELECTED" } }
    }
}

fn undo_button(available: bool) -> Element {
    if available {
        rsx! { button { class:"undo-action", "UNDO" } }
    } else {
        rsx! { button { class:"undo-action", disabled:true, "UNDO" } }
    }
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
    }: NativeOverlayProps,
) -> NativeReactiveState {
    let reactive_state = NativeReactiveState {
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
    let reactive = use_native_reactive_state(props);
    let NativeReactiveState {
        appearance,
        widgets,
        editing,
        selected,
        managed,
        settings,
        ..
    } = reactive.clone();
    let current = reactive.state.borrow().clone();
    let sample = editing.get() && current.system == scorepeek_overlay_ui::LampState::Inactive;
    let shown = if sample {
        scorepeek_overlay_ui::editor_sample_state()
    } else {
        current
    };
    let current_settings = settings.borrow().clone();
    let selected_widget = selected.borrow().as_ref().and_then(|id| {
        widgets
            .borrow()
            .iter()
            .find(|widget| &widget.id == id)
            .cloned()
    });
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
        div { class: if editing.get() { "canvas-content editor-preview-canvas selected" } else { "canvas-content" },style:format!("display:{};opacity:{};{}",if reactive.visible.get() && (!editing.get() || (selected_visible && reactive.surface_canvas_ids.borrow().contains(&current_settings.id))){"block"}else{"none"},f32::from(current_settings.opacity_percent)/100.0,if editing.get(){format!("left:{}px;top:{}px;width:{}px;height:{}px",current_settings.x,current_settings.y,current_settings.width,current_settings.height)}else{String::new()}),
            {overlay_canvas(
                &shown,
                appearance.get(),
                &widgets.borrow(),
                editing.get(),
                selected.borrow().as_deref(),
            )}
            if editing.get() && selected_visible { for corner in ["nw","ne","sw","se"] { i { class:"native-canvas-handle {corner}" } } }
        }
        if editing.get() {
            for canvas in managed.borrow().iter().filter(|canvas| canvas.id != current_settings.id && reactive.surface_canvas_ids.borrow().contains(&canvas.id) && scorepeek_overlay_ui::canvas_visible(canvas.show_on.as_deref(), scorepeek_overlay_ui::ScreenView { kind:Some(current_settings.preview_screen), suspended_since_unix_ms:None, revision:0 })) {
                div { class:"canvas-content editor-preview-canvas preview-only", style:format!("left:{}px;top:{}px;width:{}px;height:{}px;opacity:{}",canvas.x,canvas.y,canvas.width,canvas.height,f32::from(canvas.opacity_percent)/100.0),
                    {overlay_canvas(&shown, Appearance { skin: canvas.skin }, &canvas.widgets, false, None)}
                }
            }
        }
        if editing.get() {
            button { class:if reactive.dirty.get(){"native-panel-toggle dirty"}else{"native-panel-toggle"}, "aria-label":if reactive.panel_open.get(){"Hide editor panel"}else{"Show editor panel"}, "data-state":if reactive.panel_open.get(){"open"}else{"closed"}, if reactive.panel_open.get(){"‹"}else{"›"} span { class:"dirty-dot" } }
            if reactive.panel_open.get() { div { class:"native-canvas-manager", style:format!("width:{}px",current_settings.panel_width),
                header { strong { "SCOREPEEK OVERLAY" } small { "WAYLAND EDITOR" } if sample { b { "SAMPLE DATA" } } if reactive.dirty.get() { i { class:"unsaved-dot" } } }
                div { class:"editor-fixed-top", p { "GAME SCREEN" } div { class:"preview-tabs",
                    for (index,(label,kind)) in [("MUSIC SELECT",scorepeek_overlay_ui::ScreenKind::MusicSelect),("MODE SELECT",scorepeek_overlay_ui::ScreenKind::ModeSelect),("DECIDE",scorepeek_overlay_ui::ScreenKind::DecideTransition),("PLAY",scorepeek_overlay_ui::ScreenKind::Play),("RESULT",scorepeek_overlay_ui::ScreenKind::Result)].into_iter().enumerate() {
                        button { class:if current_settings.preview_screen==kind{"selected preview-screen"}else{"preview-screen"}, "aria-selected":current_settings.preview_screen==kind, "data-index":index, "{label}" }
                    }
                }
                section { class:"canvas-section", h2 { "CANVASES" }
                    nav { class:"canvas-list", for canvas in managed.borrow().iter() {
                        div { class:"canvas-row",
                            button { class:if current_settings.has_selection && canvas.id==current_settings.id{"canvas-select selected"}else{"canvas-select"}, "aria-selected":current_settings.has_selection && canvas.id==current_settings.id, "data-canvas-id":"{canvas.id}", "{canvas.id}" }
                            button { class:if scorepeek_overlay_ui::canvas_visible(canvas.show_on.as_deref(), scorepeek_overlay_ui::ScreenView { kind:Some(current_settings.preview_screen), suspended_since_unix_ms:None, revision:0 }){"screen-toggle selected"}else{"screen-toggle"}, "aria-pressed":scorepeek_overlay_ui::canvas_visible(canvas.show_on.as_deref(), scorepeek_overlay_ui::ScreenView { kind:Some(current_settings.preview_screen), suspended_since_unix_ms:None, revision:0 }), "data-canvas-id":"{canvas.id}", if scorepeek_overlay_ui::canvas_visible(canvas.show_on.as_deref(), scorepeek_overlay_ui::ScreenView { kind:Some(current_settings.preview_screen), suspended_since_unix_ms:None, revision:0 }){"ON"}else{"OFF"} }
                        }
                    } }
                    div { class:"canvas-actions", button { class:"add-canvas", "+ ADD CANVAS" } {delete_canvas_button(managed.borrow().len()<=1 || !current_settings.has_selection)} }
                }
                }
                div { class:"editor-tab-body",
                if current_settings.has_selection { section { class:"appearance-pane", h2 { "APPEARANCE" } h3 { "SKIN" }
                    div { class:"native-skin-options button-grid three", for (index,(label,skin)) in [("CYAN",scorepeek_overlay_ui::Skin::CyanSystem),("AURORA",scorepeek_overlay_ui::Skin::ResultAurora),("BLACKBOX",scorepeek_overlay_ui::Skin::DjBlackbox)].into_iter().enumerate() { button { class:if appearance.get().skin==skin{"skin-option selected"}else{"skin-option"}, "aria-pressed":appearance.get().skin==skin, "data-index":index, if appearance.get().skin==skin{"✓ "} "{label}" } } }
                    h3 { "OPACITY" }
                    div { class:"native-opacity button-grid four", for value in [25,50,75,100] { button { class:if current_settings.opacity_percent==value{"opacity-option selected"}else{"opacity-option"}, "aria-pressed":current_settings.opacity_percent==value, "data-value":value, if current_settings.opacity_percent==value{"✓ "} "{value}" } } }
                }
                section { class:"output-pane", h2 { "OUTPUT" } div { class:"output-list", for output in reactive.outputs.borrow().iter() { button { class:if current_settings.output.as_deref()==Some(output.name.as_str()){"output-option selected"}else{"output-option"}, "aria-selected":current_settings.output.as_deref()==Some(output.name.as_str()), "data-output":"{output.name}", strong { if current_settings.output.as_deref()==Some(output.name.as_str()){"✓ "} "{output.name}" } small { "{output.model}" if let Some([width,height])=output.logical_size { " · {width}×{height}" } } } } } }
                if selected_visible { section { class:"widgets-pane", h2 { "WIDGETS" }
                    for widget in widgets.borrow().iter() { button { class:if selected.borrow().as_deref()==Some(widget.id.as_str()){"widget-row selected"}else{"widget-row"}, "aria-selected":selected.borrow().as_deref()==Some(widget.id.as_str()), "data-widget-id":"{widget.id}", "{widget.id}" } }
                    details { class:"widget-add", open:reactive.widget_add_open.get(), summary { class:"widget-add-summary", "+ ADD WIDGET" } if reactive.widget_add_open.get() { div { class:"button-grid", for (index,label) in ["STATUS","SELECTION","SCORE","HISTORY LIST","HISTORY GRAPH"].into_iter().enumerate() { button { class:"add-widget", "data-index":index, "+ {label}" } } } } }
                    if let Some(widget) = selected_widget {
                        div { class:"native-widget-settings",
                            strong { "{widget.id}" }
                            if widget.kind == scorepeek_overlay_ui::WidgetKind::HistoryList {
                                for value in [5,10,20,50] { button { class:if widget.settings.history_count==value{"history-count selected"}else{"history-count"}, "aria-pressed":widget.settings.history_count==value, "data-value":value, if widget.settings.history_count==value{"✓ "} "{value}" } }
                            }
                            if widget.kind == scorepeek_overlay_ui::WidgetKind::HistoryGraph {
                                for value in [1,3,6,12] { button { class:if widget.settings.graph_months==value{"graph-months selected"}else{"graph-months"}, "aria-pressed":widget.settings.graph_months==value, "data-value":value, if widget.settings.graph_months==value{"✓ "} "{value}M" } }
                            }
                            button { class:"delete-widget danger", "DELETE WIDGET" }
                        }
                    }
                } } else { div { class:"canvas-hidden-state", strong { "HIDDEN ON THIS GAME SCREEN" } span { "Turn this canvas ON in the list to edit its widgets." } } } } else { div { class:"canvas-hidden-state", strong { "NO CANVAS ON THIS GAME SCREEN" } span { "Turn a canvas ON or add one for this game screen." } } }
                }
                footer { {undo_button(reactive.undo_available.get())} div { class:"footer-actions", if reactive.dirty.get() { button { class:"discard-action", "DISCARD CHANGES" } button { class:"primary save-action", "SAVE ALL CHANGES AND CLOSE" } } else { button { class:"close-action", "CLOSE EDITOR" } } } }
            } }
            if reactive.surface_canvas_ids.borrow().contains(&current_settings.id) { if let Some(kind) = reactive.pending_widget.get() { div { class:"native-placement-ghost", style:format!("left:{}px;top:{}px",reactive.pending_point.get()[0],reactive.pending_point.get()[1]), "PLACE {kind:?}" } } }
        }
    }
}

struct CalloopWaker(Ping);

fn widget_layout(widget: &crate::config::Widget) -> WidgetLayout {
    WidgetLayout {
        id: widget.id.clone(),
        kind: match widget.kind {
            crate::config::WidgetKind::Status => scorepeek_overlay_ui::WidgetKind::Status,
            crate::config::WidgetKind::Selection => scorepeek_overlay_ui::WidgetKind::Selection,
            crate::config::WidgetKind::Score => scorepeek_overlay_ui::WidgetKind::Score,
            crate::config::WidgetKind::HistoryList => scorepeek_overlay_ui::WidgetKind::HistoryList,
            crate::config::WidgetKind::HistoryGraph => {
                scorepeek_overlay_ui::WidgetKind::HistoryGraph
            }
        },
        x: widget.x,
        y: widget.y,
        width: widget.width,
        height: widget.height,
        settings: scorepeek_overlay_ui::WidgetSettings {
            history_count: widget.settings.history_count,
            graph_months: widget.settings.graph_months,
        },
    }
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

fn resized_canvas_geometry(
    position: [i32; 2],
    origin: [u32; 2],
    minimum: [u32; 2],
    corner: ResizeCorner,
    delta: [i32; 2],
    output: Option<[u32; 2]>,
) -> CanvasGeometry {
    let mut geometry = CanvasGeometry {
        x: position[0],
        y: position[1],
        width: origin[0],
        height: origin[1],
    };
    let west = matches!(corner, ResizeCorner::NorthWest | ResizeCorner::SouthWest);
    let north = matches!(corner, ResizeCorner::NorthWest | ResizeCorner::NorthEast);
    let east = matches!(corner, ResizeCorner::NorthEast | ResizeCorner::SouthEast);
    let south = matches!(corner, ResizeCorner::SouthWest | ResizeCorner::SouthEast);
    if west {
        let maximum = i32::try_from(origin[0].saturating_sub(minimum[0])).unwrap_or(i32::MAX);
        let applied = delta[0].clamp(-position[0], maximum);
        geometry.x = position[0].saturating_add(applied);
        geometry.width = origin[0].saturating_sub_signed(applied);
    } else if east {
        geometry.width = origin[0].saturating_add_signed(delta[0]).max(minimum[0]);
    }
    if north {
        let maximum = i32::try_from(origin[1].saturating_sub(minimum[1])).unwrap_or(i32::MAX);
        let applied = delta[1].clamp(-position[1], maximum);
        geometry.y = position[1].saturating_add(applied);
        geometry.height = origin[1].saturating_sub_signed(applied);
    } else if south {
        geometry.height = origin[1].saturating_add_signed(delta[1]).max(minimum[1]);
    }
    if let Some([output_width, output_height]) = output {
        geometry.width = geometry.width.min(grid_floor(
            output_width.saturating_sub(geometry.x.cast_unsigned()),
        ));
        geometry.height = geometry.height.min(grid_floor(
            output_height.saturating_sub(geometry.y.cast_unsigned()),
        ));
    }
    geometry
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
    let dx = snap_i32(x - start[0]);
    let dy = snap_i32(y - start[1]);
    let mut left = original.x;
    let mut top = original.y;
    let mut right = original
        .x
        .saturating_add(i32::try_from(original.width).unwrap_or(i32::MAX));
    let mut bottom = original
        .y
        .saturating_add(i32::try_from(original.height).unwrap_or(i32::MAX));
    if matches!(corner, ResizeCorner::NorthWest | ResizeCorner::SouthWest) {
        left = original
            .x
            .saturating_add(dx)
            .clamp(0, right.saturating_sub(32));
    } else {
        right = right.saturating_add(dx).clamp(
            left.saturating_add(32),
            i32::try_from(canvas[0]).unwrap_or(i32::MAX),
        );
    }
    if matches!(corner, ResizeCorner::NorthWest | ResizeCorner::NorthEast) {
        top = original
            .y
            .saturating_add(dy)
            .clamp(0, bottom.saturating_sub(32));
    } else {
        bottom = bottom.saturating_add(dy).clamp(
            top.saturating_add(32),
            i32::try_from(canvas[1]).unwrap_or(i32::MAX),
        );
    }
    widget.x = left;
    widget.y = top;
    widget.width = right.saturating_sub(left).cast_unsigned();
    widget.height = bottom.saturating_sub(top).cast_unsigned();
}

fn resize_corner_at(width: u32, height: u32, x: f64, y: f64) -> Option<ResizeCorner> {
    if x < 18.0 && y < 18.0 {
        Some(ResizeCorner::NorthWest)
    } else if x >= f64::from(width.saturating_sub(18)) && y < 18.0 {
        Some(ResizeCorner::NorthEast)
    } else if x < 18.0 && y >= f64::from(height.saturating_sub(18)) {
        Some(ResizeCorner::SouthWest)
    } else if x >= f64::from(width.saturating_sub(18)) && y >= f64::from(height.saturating_sub(18))
    {
        Some(ResizeCorner::SouthEast)
    } else {
        None
    }
}

fn widget_interaction_at(
    widgets: &[WidgetLayout],
    point: [f64; 2],
) -> Option<(WidgetLayout, Option<ResizeCorner>)> {
    let [x, y] = point;
    let widget = widgets
        .iter()
        .rfind(|widget| {
            x >= f64::from(widget.x)
                && y >= f64::from(widget.y)
                && x < f64::from(widget.x) + f64::from(widget.width)
                && y < f64::from(widget.y) + f64::from(widget.height)
        })?
        .clone();
    let corner = resize_corner_at(
        widget.width,
        widget.height,
        x - f64::from(widget.x),
        y - f64::from(widget.y),
    );
    Some((widget, corner))
}

enum DirectManipulationHit {
    Canvas(ResizeCorner),
    Widget(WidgetLayout, Option<ResizeCorner>),
}

fn direct_manipulation_at(
    canvas: [u32; 2],
    widgets: &[WidgetLayout],
    selected_widget: Option<&str>,
    point: [f64; 2],
) -> Option<DirectManipulationHit> {
    if let Some(selected) = selected_widget
        && let Some(widget) = widgets.iter().find(|widget| widget.id == selected)
        && point[0] >= f64::from(widget.x)
        && point[1] >= f64::from(widget.y)
        && point[0] < f64::from(widget.x) + f64::from(widget.width)
        && point[1] < f64::from(widget.y) + f64::from(widget.height)
        && let Some(corner) = resize_corner_at(
            widget.width,
            widget.height,
            point[0] - f64::from(widget.x),
            point[1] - f64::from(widget.y),
        )
    {
        return Some(DirectManipulationHit::Widget(widget.clone(), Some(corner)));
    }
    let widget_hit = widget_interaction_at(widgets, point);
    if let Some(corner) = resize_corner_at(canvas[0], canvas[1], point[0], point[1]) {
        return Some(DirectManipulationHit::Canvas(corner));
    }
    widget_hit.map(|(widget, corner)| DirectManipulationHit::Widget(widget, corner))
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
                let mut canvas = crate::config::empty_canvas(id, crate::runtime::Backend::Wayland);
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
        outputs,
        Rc::clone(&report),
        pending_resolved_output,
        preview,
        workspace_open,
        workspace_ui,
        suppressed,
    );
    let result = app.run();
    let renderer_coordinator = Arc::clone(&app.renderer_init);
    renderer_coordinator.exclusive(|| app.renderer.suspend());
    {
        let mut report = report.borrow_mut();
        report.paint_count = app.paint_count;
        report.render_calls = app.render_calls;
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
    shared_state: Reactive<OverlayState>,
    waker: Waker,
    started: Instant,
    animating: bool,
    paint_count: u32,
    render_calls: u32,
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
    readonly: bool,
    selected: Reactive<Option<String>>,
    pending_widget: Reactive<Option<scorepeek_overlay_ui::WidgetKind>>,
    pending_point: Reactive<[f64; 2]>,
    shared_widgets: Reactive<Vec<WidgetLayout>>,
    interaction: Option<NativeInteraction>,
    interaction_snapshot: Option<Vec<scorepeek_overlay_ui::CanvasPresentation>>,
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
}

enum NativeInteraction {
    CanvasMove {
        start: [f64; 2],
        origin: [i32; 2],
    },
    ManagedCanvasMove {
        id: String,
        start: [f64; 2],
        origin: [i32; 2],
    },
    CanvasResize {
        start: [f64; 2],
        position: [i32; 2],
        origin: [u32; 2],
        corner: ResizeCorner,
    },
    Widget {
        id: String,
        start: [f64; 2],
        original: WidgetLayout,
        corner: Option<ResizeCorner>,
    },
}

fn managed_canvas_move_target(interaction: &NativeInteraction) -> Option<&str> {
    let NativeInteraction::ManagedCanvasMove { id, .. } = interaction else {
        return None;
    };
    Some(id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResizeCorner {
    NorthWest,
    NorthEast,
    SouthWest,
    SouthEast,
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
        outputs: Vec<OutputDescription>,
        report: Rc<RefCell<RunReport>>,
        pending_resolved_output: Option<String>,
        preview: Arc<std::sync::Mutex<Option<String>>>,
        workspace_open: Arc<std::sync::atomic::AtomicBool>,
        workspace_ui: Arc<std::sync::Mutex<NativeWorkspace>>,
        suppressed: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    ) -> Self {
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
        let panel_width = editor_panel_width(shell.output_logical_size.map(|[width, _]| width));
        let settings = Rc::new(RefCell::new(NativeCanvasSettings {
            id: canvas.id.clone(),
            has_selection: true,
            output: canvas.output.clone(),
            show_on: canvas.show_on.clone(),
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
            },
        );
        let mut document = DioxusDocument::new(vdom, document_config());
        document.initial_build();
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
        Self {
            renderer,
            renderer_init,
            shell,
            document,
            shared_state: reactive.state,
            waker,
            started: Instant::now(),
            animating: false,
            paint_count: 0,
            render_calls: 0,
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
            readonly: true,
            selected: reactive.selected,
            pending_widget: reactive.pending_widget,
            pending_point: reactive.pending_point,
            shared_widgets: reactive.widgets,
            interaction: None,
            interaction_snapshot: None,
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
        }
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
            if self.visible.get() != visible {
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
                *self.shared_state.borrow_mut() = latest;
                wake = true;
            }
            let changed = wake && self.poll_dioxus();
            if self.renderer.is_active() && (configured || changed || (frame && self.animating)) {
                self.paint()?;
                if changed {
                    crate::diagnostics::emit(
                        "native_state_paint",
                        &serde_json::json!({"paint_count":self.paint_count,"render_calls":self.render_calls}),
                    );
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
            self.readonly = response.readonly;
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
        let mut settings = self.settings.borrow_mut();
        settings.id.clone_from(&presentation.id);
        settings.output.clone_from(&presentation.output);
        settings.show_on.clone_from(&presentation.show_on);
        settings.opacity_percent = presentation.opacity_percent;
        settings.x = presentation.x;
        settings.y = presentation.y;
        settings.width = presentation.width;
        settings.height = presentation.height;
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
        let (ui, draft, dirty, undo_available, selected_widget, pending_widget, surface_canvas_ids) = {
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
            )
        };
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
                self.apply_selected_presentation(&presentation);
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
            self.acquire();
            self.next_keepalive = Instant::now() + Duration::from_secs(5);
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
            self.editing.set(false);
            self.dirty.set(false);
            self.canvas = self.surface_canvas.clone();
            let presentation = self.canvas.presentation();
            self.apply_selected_presentation(&presentation);
        }
        self.set_editor_geometry(value);
        if value && !self.visible.replace(true) {
            self.shell.set_input_enabled(true);
        }
        self.interaction = None;
        self.interaction_snapshot = None;
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
        if button == 0x111 {
            if pressed {
                if !self.editing.get() {
                    self.pin_editor_host();
                    self.workspace_open
                        .store(true, std::sync::atomic::Ordering::Release);
                    if let Some(screen) = self.shared_state.borrow().screen.kind {
                        self.settings.borrow_mut().preview_screen = screen;
                        self.sync_workspace_ui();
                    }
                    self.select_canvas(Some(self.canvas.id.clone()));
                    self.set_editing(true);
                    return;
                }
                if self.readonly
                    || (self.panel_open.get() && x < f64::from(self.settings.borrow().panel_width))
                {
                    return;
                }
                let target = self
                    .managed
                    .borrow()
                    .iter()
                    .rfind(|canvas| {
                        self.surface_canvas_ids.borrow().contains(&canvas.id)
                            && scorepeek_overlay_ui::canvas_visible(
                                canvas.show_on.as_deref(),
                                scorepeek_overlay_ui::ScreenView {
                                    kind: Some(self.settings.borrow().preview_screen),
                                    suspended_since_unix_ms: None,
                                    revision: 0,
                                },
                            )
                            && x >= f64::from(canvas.x)
                            && y >= f64::from(canvas.y)
                            && x < f64::from(canvas.x) + f64::from(canvas.width)
                            && y < f64::from(canvas.y) + f64::from(canvas.height)
                    })
                    .cloned();
                if let Some(target) = target {
                    self.interaction_snapshot = Some(self.draft_snapshot());
                    if target.id == self.canvas.id {
                        self.interaction = Some(NativeInteraction::CanvasMove {
                            start: [x, y],
                            origin: [target.x, target.y],
                        });
                    } else {
                        self.interaction = Some(NativeInteraction::ManagedCanvasMove {
                            id: target.id,
                            start: [x, y],
                            origin: [target.x, target.y],
                        });
                    }
                } else {
                    let screen = self.settings.borrow().preview_screen;
                    self.select_first_canvas(screen);
                }
            } else {
                self.persist_interaction();
            }
            return;
        }
        if button != 0x110 {
            return;
        }
        if pressed {
            if !self.editing.get() {
                return;
            }
            if self.readonly {
                return;
            }
            if self.hit_selector(".native-panel-toggle", x, y) {
                self.panel_open.set(!self.panel_open.get());
                self.sync_workspace_ui();
                return;
            }
            let outputs = self.outputs.borrow().clone();
            for output in outputs {
                if self.hit_selector(
                    &format!(".output-option[data-output='{}']", output.name),
                    x,
                    y,
                ) {
                    if self.settings.borrow().output.as_deref() == Some(output.name.as_str()) {
                        return;
                    }
                    let before = self.draft_snapshot();
                    self.canvas.output = Some(output.name.clone());
                    self.settings.borrow_mut().output = Some(output.name);
                    self.persist_canvas_change(before);
                    return;
                }
            }
            if self.panel_open.get() && x < f64::from(self.settings.borrow().panel_width) {
                if self.hit_selector(".undo-action", x, y) {
                    self.undo_last_change();
                    return;
                }
                if self.hit_selector(".close-action", x, y) {
                    let mut workspace = self
                        .workspace_ui
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    discard_undo(&mut workspace);
                    drop(workspace);
                    self.undo_available.set(false);
                    self.workspace_open
                        .store(false, std::sync::atomic::Ordering::Release);
                    self.select_canvas(None);
                    self.set_editing(false);
                    return;
                }
                if self.hit_selector(".discard-action", x, y) {
                    if !self.dirty.get() {
                        return;
                    }
                    let mut workspace = self
                        .workspace_ui
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    self.suppressed
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .extend(std::mem::take(&mut workspace.fallback));
                    workspace.undo = None;
                    workspace.reload_saved = true;
                    drop(workspace);
                    self.workspace_open
                        .store(false, std::sync::atomic::Ordering::Release);
                    self.select_canvas(None);
                    self.pending_widget.set(None);
                    self.set_editing(false);
                    return;
                }
                if self.hit_selector(".save-action", x, y) {
                    if self.dirty.get() {
                        self.save_and_close();
                    }
                    return;
                }
                if self.hit_selector(".widget-add-summary", x, y) {
                    self.widget_add_open.set(!self.widget_add_open.get());
                    self.sync_workspace_ui();
                    return;
                }
                for (index, kind) in [
                    scorepeek_overlay_ui::ScreenKind::MusicSelect,
                    scorepeek_overlay_ui::ScreenKind::ModeSelect,
                    scorepeek_overlay_ui::ScreenKind::DecideTransition,
                    scorepeek_overlay_ui::ScreenKind::Play,
                    scorepeek_overlay_ui::ScreenKind::Result,
                ]
                .into_iter()
                .enumerate()
                {
                    if self.hit_selector(&format!(".preview-screen[data-index='{index}']"), x, y) {
                        self.settings.borrow_mut().preview_screen = kind;
                        self.sync_workspace_ui();
                        self.select_first_canvas(kind);
                        return;
                    }
                }
                let canvas_ids = self
                    .managed
                    .borrow()
                    .iter()
                    .map(|item| item.id.clone())
                    .collect::<Vec<_>>();
                for id in canvas_ids {
                    if self.hit_selector(&format!(".screen-toggle[data-canvas-id='{id}']"), x, y) {
                        let before = self.draft_snapshot();
                        let screen = self.settings.borrow().preview_screen;
                        let was_shown = self
                            .managed
                            .borrow()
                            .iter()
                            .find(|canvas| canvas.id == id)
                            .is_some_and(|canvas| shown_on(canvas, screen));
                        if let Some(canvas) = self
                            .managed
                            .borrow_mut()
                            .iter_mut()
                            .find(|canvas| canvas.id == id)
                        {
                            set_shown_on(canvas, screen, !was_shown);
                        }
                        self.finish_draft_change(before);
                        if was_shown && id == self.canvas.id {
                            self.select_first_canvas(screen);
                        }
                        return;
                    }
                    if self.hit_selector(&format!(".canvas-select[data-canvas-id='{id}']"), x, y) {
                        let target_screen = self
                            .managed
                            .borrow()
                            .iter()
                            .find(|canvas| canvas.id == id)
                            .and_then(|canvas| {
                                EDITOR_SCREENS
                                    .into_iter()
                                    .find(|screen| shown_on(canvas, *screen))
                            });
                        if !self
                            .managed
                            .borrow()
                            .iter()
                            .find(|canvas| canvas.id == id)
                            .is_some_and(|canvas| {
                                shown_on(canvas, self.settings.borrow().preview_screen)
                            })
                            && let Some(screen) = target_screen
                        {
                            self.settings.borrow_mut().preview_screen = screen;
                            self.sync_workspace_ui();
                        }
                        self.select_canvas(Some(id));
                        return;
                    }
                }
                let widget_ids = self
                    .shared_widgets
                    .borrow()
                    .iter()
                    .map(|item| item.id.clone())
                    .collect::<Vec<_>>();
                for id in widget_ids {
                    if self.hit_selector(&format!(".widget-row[data-widget-id='{id}']"), x, y) {
                        *self.selected.borrow_mut() = Some(id);
                        self.sync_workspace_ui();
                        return;
                    }
                }
                for (index, kind) in [
                    scorepeek_overlay_ui::WidgetKind::Status,
                    scorepeek_overlay_ui::WidgetKind::Selection,
                    scorepeek_overlay_ui::WidgetKind::Score,
                    scorepeek_overlay_ui::WidgetKind::HistoryList,
                    scorepeek_overlay_ui::WidgetKind::HistoryGraph,
                ]
                .into_iter()
                .enumerate()
                {
                    if self.hit_selector(&format!(".add-widget[data-index='{index}']"), x, y) {
                        self.pending_widget.set(Some(kind));
                        self.sync_workspace_ui();
                        return;
                    }
                }
                if self.hit_selector(".delete-widget", x, y) {
                    let selected_id = self.selected.borrow().clone();
                    if let Some(id) = selected_id {
                        let before = self.draft_snapshot();
                        self.shared_widgets
                            .borrow_mut()
                            .retain(|widget| widget.id != id);
                        self.selected.borrow_mut().take();
                        self.sync_workspace_ui();
                        self.persist_canvas_change(before);
                    }
                    return;
                }
                for value in [5_u32, 10, 20, 50] {
                    if self.hit_selector(&format!(".history-count[data-value='{value}']"), x, y) {
                        self.update_selected_widget_setting(Some(value), None);
                        return;
                    }
                }
                for value in [1_u32, 3, 6, 12] {
                    if self.hit_selector(&format!(".graph-months[data-value='{value}']"), x, y) {
                        self.update_selected_widget_setting(None, Some(value));
                        return;
                    }
                }
                if self.hit_selector(".add-canvas", x, y) {
                    let before = self.draft_snapshot();
                    let suffix = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis();
                    let mut canvas = crate::config::empty_canvas(
                        format!("wayland-{suffix}"),
                        crate::runtime::Backend::Wayland,
                    )
                    .presentation();
                    canvas.output.clone_from(&self.surface_output);
                    canvas.show_on = Some(vec![self.settings.borrow().preview_screen]);
                    canvas.x = 0;
                    canvas.y = 0;
                    canvas.width = 560.min(grid_floor(self.surface_logical[0]));
                    canvas.height = 1040.min(grid_floor(self.surface_logical[1]));
                    let id = canvas.id.clone();
                    self.managed.borrow_mut().push(canvas);
                    self.finish_draft_change(before);
                    self.select_canvas(Some(id));
                    return;
                }
                if self.hit_selector(".delete-canvas", x, y) {
                    let selected = self
                        .preview
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    let Some(selected) =
                        selected_canvas_for_delete(&self.managed.borrow(), selected.as_deref())
                    else {
                        return;
                    };
                    let before = self.draft_snapshot();
                    self.managed
                        .borrow_mut()
                        .retain(|canvas| canvas.id != selected);
                    self.finish_draft_change(before);
                    let next = self
                        .managed
                        .borrow()
                        .first()
                        .map(|canvas| canvas.id.clone());
                    self.select_canvas(next);
                    return;
                }
                for (index, skin) in [
                    scorepeek_overlay_ui::Skin::CyanSystem,
                    scorepeek_overlay_ui::Skin::ResultAurora,
                    scorepeek_overlay_ui::Skin::DjBlackbox,
                ]
                .into_iter()
                .enumerate()
                {
                    if self.hit_selector(&format!(".skin-option[data-index='{index}']"), x, y) {
                        if self.appearance.get().skin == skin {
                            return;
                        }
                        let before = self.draft_snapshot();
                        self.appearance.set(Appearance { skin });
                        self.canvas.skin = skin;
                        self.persist_canvas_change(before);
                        return;
                    }
                }
                for value in [25_u8, 50, 75, 100] {
                    if self.hit_selector(&format!(".opacity-option[data-value='{value}']"), x, y) {
                        if self.settings.borrow().opacity_percent == value {
                            return;
                        }
                        let before = self.draft_snapshot();
                        self.settings.borrow_mut().opacity_percent = value;
                        self.persist_canvas_change(before);
                        return;
                    }
                }
                return;
            }
            let target = unselected_canvas_at(
                &self.managed.borrow(),
                &self.surface_canvas_ids.borrow(),
                &self.canvas.presentation(),
                self.settings.borrow().preview_screen,
                [x, y],
            );
            if let Some(id) = target {
                self.select_canvas(Some(id));
                return;
            }
            let output_point = [x, y];
            let x = x - f64::from(self.canvas.x);
            let y = y - f64::from(self.canvas.y);
            if let Some(kind) = self.pending_widget.take() {
                self.place_widget(kind, x, y);
                return;
            }
            let hit = direct_manipulation_at(
                [self.canvas.width, self.canvas.height],
                &self.shared_widgets.borrow(),
                self.selected.borrow().as_deref(),
                [x, y],
            );
            match hit {
                Some(DirectManipulationHit::Canvas(corner)) => {
                    self.interaction_snapshot = Some(self.draft_snapshot());
                    self.interaction = Some(NativeInteraction::CanvasResize {
                        start: output_point,
                        position: [self.canvas.x, self.canvas.y],
                        origin: [self.canvas.width, self.canvas.height],
                        corner,
                    });
                }
                Some(DirectManipulationHit::Widget(original, corner)) => {
                    *self.selected.borrow_mut() = Some(original.id.clone());
                    self.sync_workspace_ui();
                    self.interaction_snapshot = Some(self.draft_snapshot());
                    self.interaction = Some(NativeInteraction::Widget {
                        id: original.id.clone(),
                        start: [x, y],
                        original,
                        corner,
                    });
                }
                None => {}
            }
        } else {
            self.persist_interaction();
        }
    }
    #[allow(clippy::too_many_lines)]
    fn pointer_motion(&mut self, x: f64, y: f64) {
        if self.pending_widget.get().is_some() {
            self.pending_point.set([x, y]);
        }
        self.shell.set_cursor(match &self.interaction {
            Some(
                NativeInteraction::CanvasMove { .. } | NativeInteraction::ManagedCanvasMove { .. },
            ) => CursorStyle::Move,
            Some(
                NativeInteraction::CanvasResize { .. }
                | NativeInteraction::Widget {
                    corner: Some(_), ..
                },
            ) => CursorStyle::Resize,
            Some(NativeInteraction::Widget { corner: None, .. }) => CursorStyle::Grabbing,
            None if self.editing.get() => CursorStyle::Grab,
            None => CursorStyle::Default,
        });
        let Some(interaction) = &self.interaction else {
            return;
        };
        match interaction {
            NativeInteraction::CanvasMove { start, origin } => {
                self.move_canvas(*start, *origin, x, y);
            }
            NativeInteraction::ManagedCanvasMove { id, start, origin } => {
                let dx = snap_i32(x - start[0]);
                let dy = snap_i32(y - start[1]);
                if let Some(canvas) = self
                    .managed
                    .borrow_mut()
                    .iter_mut()
                    .find(|canvas| &canvas.id == id)
                {
                    canvas.x = origin[0].saturating_add(dx);
                    canvas.y = origin[1].saturating_add(dy);
                    if let Some([output_width, output_height]) = self.shell.output_logical_size {
                        canvas.x = canvas
                            .x
                            .clamp(0, maximum_grid_position(output_width, canvas.width));
                        canvas.y = canvas
                            .y
                            .clamp(0, maximum_grid_position(output_height, canvas.height));
                    }
                }
            }
            NativeInteraction::CanvasResize {
                start,
                position,
                origin,
                corner,
            } => {
                self.resize_canvas(*start, *position, *origin, *corner, x, y);
            }
            NativeInteraction::Widget {
                id,
                start,
                original,
                corner,
            } => {
                let [x, y] = self.editor_local_point(x, y);
                if let Some(widget) = self
                    .shared_widgets
                    .borrow_mut()
                    .iter_mut()
                    .find(|widget| &widget.id == id)
                {
                    if let Some(corner) = corner {
                        resize_widget(
                            widget,
                            original,
                            *start,
                            *corner,
                            x,
                            y,
                            [self.canvas.width, self.canvas.height],
                        );
                    } else {
                        widget.x = snap_i32(f64::from(original.x) + x - start[0]).clamp(
                            0,
                            i32::try_from(self.canvas.width.saturating_sub(widget.width))
                                .unwrap_or(i32::MAX),
                        );
                        widget.y = snap_i32(f64::from(original.y) + y - start[1]).clamp(
                            0,
                            i32::try_from(self.canvas.height.saturating_sub(widget.height))
                                .unwrap_or(i32::MAX),
                        );
                    }
                }
            }
        }
    }

    fn editor_local_point(&self, x: f64, y: f64) -> [f64; 2] {
        [x - f64::from(self.canvas.x), y - f64::from(self.canvas.y)]
    }

    fn move_canvas(&mut self, start: [f64; 2], origin: [i32; 2], x: f64, y: f64) {
        self.canvas.x = snap_i32(f64::from(origin[0]) + x - start[0]);
        self.canvas.y = snap_i32(f64::from(origin[1]) + y - start[1]);
        if let Some([output_width, output_height]) = self.shell.output_logical_size {
            self.canvas.x = self
                .canvas
                .x
                .clamp(0, maximum_grid_position(output_width, self.canvas.width));
            self.canvas.y = self
                .canvas
                .y
                .clamp(0, maximum_grid_position(output_height, self.canvas.height));
        }
        self.settings.borrow_mut().set_geometry(CanvasGeometry {
            x: self.canvas.x,
            y: self.canvas.y,
            width: self.canvas.width,
            height: self.canvas.height,
        });
        self.set_editor_geometry(self.editing.get());
    }

    fn resize_canvas(
        &mut self,
        start: [f64; 2],
        position: [i32; 2],
        origin: [u32; 2],
        corner: ResizeCorner,
        x: f64,
        y: f64,
    ) {
        let dx = snap_i32(x - start[0]);
        let dy = snap_i32(y - start[1]);
        let min_width = self
            .shared_widgets
            .borrow()
            .iter()
            .map(|widget| widget.x.cast_unsigned().saturating_add(widget.width))
            .max()
            .unwrap_or(32)
            .max(32);
        let min_height = self
            .shared_widgets
            .borrow()
            .iter()
            .map(|widget| widget.y.cast_unsigned().saturating_add(widget.height))
            .max()
            .unwrap_or(32)
            .max(32);
        let geometry = resized_canvas_geometry(
            position,
            origin,
            [min_width, min_height],
            corner,
            [dx, dy],
            self.shell.output_logical_size,
        );
        self.canvas.x = geometry.x;
        self.canvas.y = geometry.y;
        self.canvas.width = geometry.width;
        self.canvas.height = geometry.height;
        self.settings.borrow_mut().set_geometry(geometry);
        self.set_editor_geometry(self.editing.get());
    }
    fn persist_interaction(&mut self) {
        let Some(interaction) = self.interaction.take() else {
            return;
        };
        let before = self.interaction_snapshot.take();
        if let Some(id) = managed_canvas_move_target(&interaction) {
            self.select_canvas(Some(id.to_owned()));
        } else {
            self.sync_canvas_to_managed();
        }
        if let Some(before) = before {
            self.finish_draft_change(before);
        }
        if !self.editing.get() {
            let _ = self.request(crate::control::Request::ReleaseBackend {
                backend: crate::runtime::Backend::Wayland,
                editor_id: self.editor_id.clone(),
            });
        }
    }
    fn persist_canvas(&mut self) {
        self.sync_canvas_to_managed();
        self.update_draft();
    }
    fn persist_canvas_change(&mut self, before: Vec<scorepeek_overlay_ui::CanvasPresentation>) {
        self.sync_canvas_to_managed();
        self.finish_draft_change(before);
    }
    fn sync_canvas_to_managed(&mut self) {
        {
            let settings = self.settings.borrow();
            self.canvas.show_on.clone_from(&settings.show_on);
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

    fn finish_draft_change(&mut self, before: Vec<scorepeek_overlay_ui::CanvasPresentation>) {
        let changed = {
            let after = self.managed.borrow();
            let mut workspace = self
                .workspace_ui
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            remember_draft_change(&mut workspace.undo, before, &after)
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
        let DraftUndo(restored) = undo;
        self.managed.borrow_mut().clone_from(&restored);
        self.undo_available.set(false);
        let selected = self
            .preview
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let next = restored
            .iter()
            .find(|canvas| Some(canvas.id.as_str()) == selected.as_deref())
            .or_else(|| restored.first())
            .cloned();
        if let Some(next) = next {
            if selected.as_deref() != Some(next.id.as_str()) {
                self.select_canvas(Some(next.id.clone()));
            }
            self.apply_selected_presentation(&next);
            self.set_editor_geometry(true);
        }
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

    fn select_first_canvas(&mut self, screen: scorepeek_overlay_ui::ScreenKind) {
        let next = self
            .managed
            .borrow()
            .iter()
            .find(|canvas| shown_on(canvas, screen))
            .map(|canvas| canvas.id.clone());
        self.select_canvas(next);
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
    fn place_widget(&mut self, kind: scorepeek_overlay_ui::WidgetKind, x: f64, y: f64) {
        let before = self.draft_snapshot();
        let id = scorepeek_overlay_ui::next_widget_id(kind, &self.shared_widgets.borrow());
        let (natural_width, natural_height) = scorepeek_overlay_ui::default_widget_size(kind);
        let width = natural_width.min(self.canvas.width);
        let height = natural_height.min(self.canvas.height);
        self.shared_widgets.borrow_mut().push(WidgetLayout {
            id: id.clone(),
            kind,
            x: snap_i32(x).clamp(
                0,
                i32::try_from(self.canvas.width.saturating_sub(width)).unwrap_or(i32::MAX),
            ),
            y: snap_i32(y).clamp(
                0,
                i32::try_from(self.canvas.height.saturating_sub(height)).unwrap_or(i32::MAX),
            ),
            width,
            height,
            settings: scorepeek_overlay_ui::WidgetSettings::default(),
        });
        *self.selected.borrow_mut() = Some(id);
        self.sync_workspace_ui();
        self.persist_canvas_change(before);
    }
    fn update_selected_widget_setting(
        &mut self,
        history_count: Option<u32>,
        graph_months: Option<u32>,
    ) {
        let before = self.draft_snapshot();
        let mut changed = false;
        let selected = self.selected.borrow().clone();
        if let Some(widget) = self
            .shared_widgets
            .borrow_mut()
            .iter_mut()
            .find(|widget| Some(&widget.id) == selected.as_ref())
        {
            if let Some(value) = history_count
                && widget.settings.history_count != value
            {
                widget.settings.history_count = value;
                changed = true;
            }
            if let Some(value) = graph_months
                && widget.settings.graph_months != value
            {
                widget.settings.graph_months = value;
                changed = true;
            }
        }
        if changed {
            self.persist_canvas_change(before);
        }
    }
    fn update_draft(&mut self) {
        let canvases = self.managed.borrow().clone();
        let _ = self.request(crate::control::Request::UpdateBackendDraft {
            backend: crate::runtime::Backend::Wayland,
            editor_id: self.editor_id.clone(),
            canvases,
        });
    }
    fn save_and_close(&mut self) {
        self.persist_canvas();
        let canvases = self.managed.borrow().clone();
        let response = self.request(crate::control::Request::CommitBackend {
            backend: crate::runtime::Backend::Wayland,
            editor_id: self.editor_id.clone(),
            expected_revision: self.backend_revision,
            canvases,
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
    fn hit_selector(&self, selector: &str, x: f64, y: f64) -> bool {
        let inner = self.document.inner.borrow();
        let Ok(Some(node)) = inner.query_selector(selector) else {
            return false;
        };
        let Some(rect) = inner.get_client_bounding_rect(node) else {
            return false;
        };
        x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height
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
    fn paint(&mut self) -> Result<(), String> {
        let renderer_coordinator = Arc::clone(&self.renderer_init);
        renderer_coordinator.exclusive(|| self.paint_exclusive())
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
            paint_scene(scene, &mut inner, scale, width, height, 0, 0);
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

struct EmbeddedSkinAssets;

impl blitz_traits::net::NetProvider for EmbeddedSkinAssets {
    fn fetch(
        &self,
        _doc_id: usize,
        request: blitz_traits::net::Request,
        handler: Box<dyn blitz_traits::net::NetHandler>,
    ) {
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
    let mut font_ctx = blitz_dom::FontContext::default();
    font_ctx
        .collection
        .register_fonts(peniko::Blob::new(Arc::new(OXANIUM)), None);
    for (_, bytes) in scorepeek_overlay_ui::FONT_ASSETS {
        font_ctx
            .collection
            .register_fonts(peniko::Blob::new(Arc::new(*bytes)), None);
    }
    DocumentConfig {
        font_ctx: Some(font_ctx),
        base_url: Some("http://scorepeek.invalid/".into()),
        net_provider: Some(Arc::new(EmbeddedSkinAssets)),
        ..DocumentConfig::default()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisualDebugScenario {
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
    Motion {
        seconds: f64,
    },
    SetEditing {
        value: bool,
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
    settings: Reactive<NativeCanvasSettings>,
}

impl VisualDebugSession {
    #[allow(clippy::too_many_lines)]
    fn new(scenario: &VisualDebugScenario, physical_size: [u32; 2]) -> Result<Self, String> {
        let config = crate::config::OverlayConfig::initial();
        let mut managed = config
            .canvases
            .into_iter()
            .filter(|canvas| canvas.backend == crate::runtime::Backend::Wayland)
            .map(|canvas| canvas.presentation())
            .collect::<Vec<_>>();
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
            id: canvas.id,
            has_selection: true,
            output: canvas.output,
            show_on: canvas.show_on,
            opacity_percent: canvas.opacity_percent,
            x: canvas.x,
            y: canvas.y,
            width: canvas.width,
            height: canvas.height,
            preview_screen: scorepeek_overlay_ui::ScreenKind::MusicSelect,
            panel_width: editor_panel_width(Some(scenario.logical_size[0])),
        }));
        let reactive = Rc::new(RefCell::new(None));
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
        };
        let mut document = DioxusDocument::new(
            VirtualDom::new_with_props(native_overlay, props),
            document_config(),
        );
        document.initial_build();
        let reactive = reactive
            .borrow()
            .clone()
            .ok_or_else(|| "native overlay did not publish its reactive state".to_owned())?;
        let mut session = Self {
            document,
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
            settings: reactive.settings,
        };
        session.resolve();
        Ok(session)
    }

    fn resolve(&mut self) {
        while self
            .document
            .poll(Some(TaskContext::from_waker(Waker::noop())))
        {}
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

    fn load_canvas(&self, canvas: scorepeek_overlay_ui::CanvasPresentation) {
        self.appearance.set(Appearance { skin: canvas.skin });
        self.widgets.borrow_mut().clone_from(&canvas.widgets);
        let preview_screen = self.settings.borrow().preview_screen;
        *self.settings.borrow_mut() = NativeCanvasSettings {
            id: canvas.id,
            has_selection: true,
            output: canvas.output,
            show_on: canvas.show_on,
            opacity_percent: canvas.opacity_percent,
            x: canvas.x,
            y: canvas.y,
            width: canvas.width,
            height: canvas.height,
            preview_screen,
            panel_width: editor_panel_width(Some(self.logical_size[0])),
        };
    }

    fn click(&mut self, selector: &str) -> Result<(), String> {
        let matched = {
            let inner = self.document.inner.borrow();
            inner
                .query_selector(selector)
                .map_err(|_| "invalid selector".to_owned())?
                .is_some()
        };
        if !matched {
            return Err(format!("selector did not match: {selector}"));
        }
        match selector {
            ".native-panel-toggle" => self.panel_open.set(!self.panel_open.get()),
            ".widget-add-summary" => self.widget_add_open.set(!self.widget_add_open.get()),
            _ if selector.contains("preview-screen") => {
                let index = selector_index(selector)?;
                let screens = [
                    scorepeek_overlay_ui::ScreenKind::MusicSelect,
                    scorepeek_overlay_ui::ScreenKind::ModeSelect,
                    scorepeek_overlay_ui::ScreenKind::DecideTransition,
                    scorepeek_overlay_ui::ScreenKind::Play,
                    scorepeek_overlay_ui::ScreenKind::Result,
                ];
                let screen = *screens
                    .get(index)
                    .ok_or_else(|| "preview screen index is out of range".to_owned())?;
                self.settings.borrow_mut().preview_screen = screen;
                let first = self
                    .managed
                    .borrow()
                    .iter()
                    .find(|canvas| shown_on(canvas, screen))
                    .cloned();
                if let Some(canvas) = first {
                    self.load_canvas(canvas);
                }
            }
            _ if selector.contains("skin-option") => {
                let skins = [
                    scorepeek_overlay_ui::Skin::CyanSystem,
                    scorepeek_overlay_ui::Skin::ResultAurora,
                    scorepeek_overlay_ui::Skin::DjBlackbox,
                ];
                let skin = *skins
                    .get(selector_index(selector)?)
                    .ok_or_else(|| "skin index is out of range".to_owned())?;
                self.appearance.set(Appearance { skin });
                self.sync_selected_canvas();
            }
            _ if selector.contains("screen-toggle") => {
                let id = selector_attribute(selector, "data-canvas-id")?;
                let screen = self.settings.borrow().preview_screen;
                let mut managed = self.managed.borrow_mut();
                let canvas = managed
                    .iter_mut()
                    .find(|canvas| canvas.id == id)
                    .ok_or_else(|| "canvas row is not in the workspace".to_owned())?;
                let shown = shown_on(canvas, screen);
                set_shown_on(canvas, screen, !shown);
            }
            _ if selector.contains("canvas-select") => {
                let id = selector_attribute(selector, "data-canvas-id")?;
                let canvas = self
                    .managed
                    .borrow()
                    .iter()
                    .find(|canvas| canvas.id == id)
                    .cloned()
                    .ok_or_else(|| "canvas row is not in the workspace".to_owned())?;
                if !shown_on(&canvas, self.settings.borrow().preview_screen)
                    && let Some(screen) = EDITOR_SCREENS
                        .into_iter()
                        .find(|screen| shown_on(&canvas, *screen))
                {
                    self.settings.borrow_mut().preview_screen = screen;
                }
                self.load_canvas(canvas);
                self.selected.borrow_mut().take();
            }
            _ if selector.contains("widget-row") => {
                *self.selected.borrow_mut() = Some(selector_attribute(selector, "data-widget-id")?);
            }
            _ => {
                return Err(format!(
                    "visual debugger does not implement click for: {selector}"
                ));
            }
        }
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
            return Err("drag requires the editable preview".into());
        }
        let panel_width = f64::from(self.settings.borrow().panel_width);
        if self.panel_open.get() && from[0] < panel_width {
            return Err("drag start is inside the editor panel".into());
        }
        let dx = snap_i32(to[0] - from[0]);
        let dy = snap_i32(to[1] - from[1]);
        match button {
            VisualDebugButton::Right => self.drag_canvas(from, [dx, dy])?,
            VisualDebugButton::Left => self.drag_widget_or_canvas(from, [dx, dy])?,
        }
        self.sync_selected_canvas();
        self.resolve();
        Ok(())
    }

    fn drag_canvas(&self, from: [f64; 2], [dx, dy]: [i32; 2]) -> Result<(), String> {
        let mut settings = self.settings.borrow_mut();
        let inside = from[0] >= f64::from(settings.x)
            && from[1] >= f64::from(settings.y)
            && from[0] < f64::from(settings.x) + f64::from(settings.width)
            && from[1] < f64::from(settings.y) + f64::from(settings.height);
        if !inside {
            return Err("right drag did not start on the selected canvas".into());
        }
        settings.x = settings.x.saturating_add(dx).clamp(
            0,
            maximum_grid_position(self.logical_size[0], settings.width),
        );
        settings.y = settings.y.saturating_add(dy).clamp(
            0,
            maximum_grid_position(self.logical_size[1], settings.height),
        );
        Ok(())
    }

    fn drag_widget_or_canvas(&self, from: [f64; 2], delta: [i32; 2]) -> Result<(), String> {
        let settings = self.settings.borrow().clone();
        let local = [
            from[0] - f64::from(settings.x),
            from[1] - f64::from(settings.y),
        ];
        let hit = direct_manipulation_at(
            [settings.width, settings.height],
            &self.widgets.borrow(),
            self.selected.borrow().as_deref(),
            local,
        );
        match hit {
            Some(DirectManipulationHit::Canvas(corner)) => {
                let minimum =
                    self.widgets
                        .borrow()
                        .iter()
                        .fold([32, 32], |[width, height], widget| {
                            [
                                width.max(widget.x.cast_unsigned().saturating_add(widget.width)),
                                height.max(widget.y.cast_unsigned().saturating_add(widget.height)),
                            ]
                        });
                let geometry = resized_canvas_geometry(
                    [settings.x, settings.y],
                    [settings.width, settings.height],
                    minimum,
                    corner,
                    delta,
                    Some(self.logical_size),
                );
                self.settings.borrow_mut().set_geometry(geometry);
            }
            Some(DirectManipulationHit::Widget(original, corner)) => {
                self.drag_widget(&settings, local, delta, &original, corner)?;
            }
            None => return Err("left drag did not start on a canvas or widget".to_owned()),
        }
        Ok(())
    }

    fn drag_widget(
        &self,
        settings: &NativeCanvasSettings,
        local: [f64; 2],
        [dx, dy]: [i32; 2],
        original: &WidgetLayout,
        corner: Option<ResizeCorner>,
    ) -> Result<(), String> {
        let mut widgets = self.widgets.borrow_mut();
        let widget = widgets
            .iter_mut()
            .find(|widget| widget.id == original.id)
            .ok_or_else(|| "left drag widget disappeared".to_owned())?;
        if let Some(corner) = corner {
            resize_widget(
                widget,
                original,
                local,
                corner,
                local[0] + f64::from(dx),
                local[1] + f64::from(dy),
                [settings.width, settings.height],
            );
        } else {
            widget.x = original.x.saturating_add(dx).clamp(
                0,
                i32::try_from(settings.width.saturating_sub(widget.width)).unwrap_or(i32::MAX),
            );
            widget.y = original.y.saturating_add(dy).clamp(
                0,
                i32::try_from(settings.height.saturating_sub(widget.height)).unwrap_or(i32::MAX),
            );
        }
        *self.selected.borrow_mut() = Some(widget.id.clone());
        Ok(())
    }

    fn sync_selected_canvas(&self) {
        let settings = self.settings.borrow();
        let widgets = self.widgets.borrow();
        if let Some(canvas) = self
            .managed
            .borrow_mut()
            .iter_mut()
            .find(|canvas| canvas.id == settings.id)
        {
            canvas.skin = self.appearance.get().skin;
            canvas.widgets.clone_from(&widgets);
            canvas.x = settings.x;
            canvas.y = settings.y;
            canvas.width = settings.width;
            canvas.height = settings.height;
        }
    }

    fn render(&mut self, path: &std::path::Path) -> Result<(), String> {
        use anyrender::ImageRenderer as _;
        let mut renderer =
            anyrender_vello::VelloImageRenderer::new(self.physical_size[0], self.physical_size[1]);
        let mut pixels = Vec::new();
        let mut inner = self.document.inner.borrow_mut();
        renderer.render_to_vec(
            |scene| {
                paint_scene(
                    scene,
                    &mut inner,
                    f64::from(self.scale),
                    self.physical_size[0],
                    self.physical_size[1],
                    0,
                    0,
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
        && !crate::config::OverlayConfig::initial()
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

fn selector_index(selector: &str) -> Result<usize, String> {
    selector_attribute(selector, "data-index")?
        .parse()
        .map_err(|_| "selector data-index must be an integer".to_owned())
}

fn selector_attribute(selector: &str, name: &str) -> Result<String, String> {
    let prefix = format!("{name}='");
    let start = selector
        .find(&prefix)
        .map(|position| position + prefix.len())
        .ok_or_else(|| format!("selector must include {name}='…'"))?;
    let end = selector[start..]
        .find('\'')
        .map(|position| start + position)
        .ok_or_else(|| format!("selector must close {name}"))?;
    Ok(selector[start..end].to_owned())
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
            output,
            &selectors,
            &mut manifest,
            0,
            "initial",
        )?;
        for (index, action) in scenario.actions.iter().enumerate() {
            let name = match action {
                VisualDebugAction::Motion { seconds } => {
                    if !seconds.is_finite() || *seconds < 0.0 {
                        return Err("motion seconds must be finite and nonnegative".into());
                    }
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
    output: &std::path::Path,
    selectors: &[String],
    manifest: &mut VisualDebugManifest,
    sequence: usize,
    name: &str,
) -> Result<(), String> {
    let stem = format!("{sequence:03}-{}", sanitize_artifact_name(name));
    let image_name = format!("{stem}.png");
    let layout_name = format!("{stem}.layout.json");
    session.render(&output.join(&image_name))?;
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
    fn left_click_selects_an_unselected_visible_canvas_only_outside_the_current_canvas() {
        let mut selected = crate::config::OverlayConfig::initial().canvases[0].presentation();
        selected.id = "selected".into();
        selected.x = 0;
        selected.y = 0;
        selected.width = 100;
        selected.height = 100;
        let mut preview = selected.clone();
        preview.id = "preview".into();
        preview.x = 120;
        let surfaces = std::collections::BTreeSet::from([selected.id.clone(), preview.id.clone()]);
        let canvases = vec![selected.clone(), preview];

        assert_eq!(
            unselected_canvas_at(
                &canvases,
                &surfaces,
                &selected,
                scorepeek_overlay_ui::ScreenKind::MusicSelect,
                [140.0, 20.0],
            ),
            Some("preview".into())
        );
        assert_eq!(
            unselected_canvas_at(
                &canvases,
                &surfaces,
                &selected,
                scorepeek_overlay_ui::ScreenKind::MusicSelect,
                [20.0, 20.0],
            ),
            None
        );

        let mut local_preview = canvases[1].clone();
        local_preview.x = 0;
        let local_canvases = vec![selected.clone(), local_preview];
        let local_surfaces = std::collections::BTreeSet::from(["preview".into()]);
        assert_eq!(
            unselected_canvas_at(
                &local_canvases,
                &local_surfaces,
                &selected,
                scorepeek_overlay_ui::ScreenKind::MusicSelect,
                [20.0, 20.0],
            ),
            Some("preview".into()),
            "a selected canvas on another output must not block the local preview"
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
        assert!(remember_draft_change(&mut undo, before.clone(), &after));
        assert!(!remember_draft_change(&mut undo, after.clone(), &after));
        let DraftUndo(restored) = undo.take().unwrap();
        assert_eq!(restored, before);
        assert!(undo.is_none());
    }

    #[test]
    fn canvas_delete_requires_a_current_selection_and_clean_close_discards_undo() {
        let canvases = crate::config::OverlayConfig::initial()
            .canvases
            .into_iter()
            .filter(|canvas| canvas.backend == crate::runtime::Backend::Wayland)
            .map(|canvas| canvas.presentation())
            .collect::<Vec<_>>();
        assert_eq!(selected_canvas_for_delete(&canvases, None), None);
        assert_eq!(
            selected_canvas_for_delete(&canvases, Some(&canvases[1].id)),
            Some(canvases[1].id.clone())
        );

        let mut workspace = NativeWorkspace {
            undo: Some(DraftUndo(canvases)),
            ..NativeWorkspace::default()
        };
        discard_undo(&mut workspace);
        assert!(workspace.undo.is_none());
    }

    #[test]
    fn visual_debug_surface_contains_every_headless_canvas() {
        let scenario = VisualDebugScenario {
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
            .query_selector_all(".canvas-content")
            .unwrap()
            .into_iter()
            .filter_map(|id| inner.get_client_bounding_rect(id))
            .filter(|rect| rect.width > 0.0 && rect.height > 0.0)
            .count();
        let widget_rects = inner
            .query_selector_all(".widget-slot")
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
            .drag([19.0, 99.0], [3.0, 83.0], VisualDebugButton::Left)
            .unwrap();

        let settings = session.settings.borrow();
        assert_eq!(
            (settings.x, settings.y, settings.width, settings.height),
            (4, 84, 576, 976)
        );
        drop(settings);
        let inner = session.document.inner.borrow();
        let canvas = inner
            .query_selector(".canvas-content.selected")
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
            skin: None,
            logical_size: [1920, 1080],
            scale: 1.0,
            canvas_id: Some("wayland-result".into()),
            editing: true,
            selectors: Vec::new(),
            actions: Vec::new(),
        };
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();

        *session.selected.borrow_mut() = Some("selection".into());
        session.resolve();
        assert!(
            session
                .document
                .inner
                .borrow()
                .query_selector(".widget-slot.selected[data-widget-id='selection']")
                .unwrap()
                .is_some()
        );

        *session.selected.borrow_mut() = Some("history-graph".into());
        session.resolve();
        let inner = session.document.inner.borrow();
        assert!(
            inner
                .query_selector(".widget-slot.selected[data-widget-id='selection']")
                .unwrap()
                .is_none()
        );
        assert!(
            inner
                .query_selector(".widget-slot.selected[data-widget-id='history-graph']")
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

    #[test]
    fn managed_canvas_move_clears_a_reused_widget_selection() {
        let scenario = VisualDebugScenario {
            skin: None,
            logical_size: [1920, 1080],
            scale: 1.0,
            canvas_id: Some("wayland-result".into()),
            editing: true,
            selectors: Vec::new(),
            actions: Vec::new(),
        };
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        *session.selected.borrow_mut() = Some("selection".into());
        session.resolve();

        let interaction = NativeInteraction::ManagedCanvasMove {
            id: "wayland-selection".to_owned(),
            start: [0.0, 0.0],
            origin: [0, 0],
        };
        let mut selected_canvas = Some("wayland-result".to_owned());
        replace_canvas_selection(
            &mut selected_canvas,
            &mut session.selected.borrow_mut(),
            managed_canvas_move_target(&interaction).map(str::to_owned),
        );
        session.resolve();

        assert_eq!(selected_canvas.as_deref(), Some("wayland-selection"));
        assert!(session.selected.borrow().is_none());
        assert!(
            session
                .document
                .inner
                .borrow()
                .query_selector(".widget-slot.selected")
                .unwrap()
                .is_none()
        );
        let widget = scorepeek_overlay_ui::default_widgets()
            .into_iter()
            .find(|widget| widget.id == "selection")
            .unwrap();
        assert!(matches!(
            direct_manipulation_at(
                [widget.width, widget.height],
                std::slice::from_ref(&widget),
                session.selected.borrow().as_deref(),
                [f64::from(widget.width - 1), f64::from(widget.height - 1)],
            ),
            Some(DirectManipulationHit::Canvas(ResizeCorner::SouthEast))
        ));
    }

    #[test]
    fn selected_widget_corners_win_over_canvas_corners_for_every_kind() {
        for mut widget in scorepeek_overlay_ui::default_widgets() {
            widget.x = 0;
            widget.y = 0;
            widget.width = 560;
            widget.height = 140;
            let selected = widget.id.clone();
            let point = [559.0, 139.0];
            let hit = direct_manipulation_at(
                [widget.width, widget.height],
                std::slice::from_ref(&widget),
                Some(&selected),
                point,
            )
            .unwrap();
            let DirectManipulationHit::Widget(actual, corner) = hit else {
                panic!("selected {selected} widget corner was routed to the canvas");
            };
            assert_eq!(actual.id, selected);
            assert_eq!(corner, Some(ResizeCorner::SouthEast));

            assert!(matches!(
                direct_manipulation_at(
                    [widget.width, widget.height],
                    std::slice::from_ref(&widget),
                    None,
                    point,
                ),
                Some(DirectManipulationHit::Canvas(ResizeCorner::SouthEast))
            ));
        }
    }

    #[test]
    fn selected_widget_does_not_capture_another_widgets_corner() {
        let mut widgets = scorepeek_overlay_ui::default_widgets();
        let selected = &mut widgets[0];
        selected.x = 0;
        selected.y = 0;
        selected.width = 100;
        selected.height = 100;
        let selected_id = selected.id.clone();
        let other = &mut widgets[1];
        other.x = 200;
        other.y = 200;
        other.width = 100;
        other.height = 100;
        let other_id = other.id.clone();

        let Some(DirectManipulationHit::Widget(actual, corner)) = direct_manipulation_at(
            [500, 500],
            &widgets[..2],
            Some(&selected_id),
            [299.0, 299.0],
        ) else {
            panic!("another widget's visible corner must keep its own interaction");
        };
        assert_eq!(actual.id, other_id);
        assert_eq!(corner, Some(ResizeCorner::SouthEast));
    }

    use scorepeek_overlay_ui::Skin;

    #[test]
    fn embedded_artwork_decodes_with_the_native_png_feature() {
        for (path, bytes) in scorepeek_overlay_ui::SKIN_ASSETS {
            let image = image::load_from_memory(bytes).expect(path);
            assert!(image.width() > 0 && image.height() > 0, "{path}");
        }
    }

    fn assert_native_graph_viewport(inner: &blitz_dom::BaseDocument) {
        let svg_id = inner.query_selector(".plot-area svg").unwrap().unwrap();
        let plot_id = inner.query_selector(".plot-area").unwrap().unwrap();
        let plot = inner.get_client_bounding_rect(plot_id).unwrap();
        let data = &inner
            .get_node(svg_id)
            .unwrap()
            .element_data()
            .unwrap()
            .special_data;
        let blitz_dom::node::SpecialElementData::Image(image) = data else {
            panic!("SVG must become a native image")
        };
        let blitz_dom::node::ImageData::Svg(svg) = image.as_ref() else {
            panic!("SVG feature must be enabled")
        };
        assert!(
            !svg.tree.root().children().is_empty(),
            "graph strokes must survive standalone SVG parsing"
        );
        assert!((f64::from(svg.tree.size().width()) - plot.width).abs() < 1.0);
        assert!((f64::from(svg.tree.size().height()) - plot.height).abs() < 1.0);
    }

    #[test]
    fn every_skin_keeps_widget_geometry_while_motion_advances() {
        for skin in [Skin::CyanSystem, Skin::ResultAurora, Skin::DjBlackbox] {
            let mut document = DioxusDocument::new(
                VirtualDom::new_with_props(
                    native_overlay,
                    NativeOverlayProps {
                        appearance: Rc::new(Cell::new(Appearance { skin })),
                        widgets: Rc::new(RefCell::new(scorepeek_overlay_ui::default_widgets())),
                        editing: Rc::new(Cell::new(false)),
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
            assert_native_graph_viewport(&inner);
            let glint = inner.query_selector(".skin-glint").unwrap().unwrap();
            let widget = inner.query_selector(".score-widget").unwrap().unwrap();
            let before = inner.get_client_bounding_rect(widget).unwrap();
            apply_motion(&mut inner, 2.0);
            inner.resolve(2.0);
            let first = inner.get_client_bounding_rect(glint).unwrap();
            apply_motion(&mut inner, 3.0);
            inner.resolve(3.0);
            let second = inner.get_client_bounding_rect(glint).unwrap();
            let after = inner.get_client_bounding_rect(widget).unwrap();
            assert!(
                (first.x - second.x).abs() > 1.0,
                "{skin:?} motion must reach native layout"
            );
            assert_eq!(
                (before.x, before.y, before.width, before.height),
                (after.x, after.y, after.width, after.height)
            );
            for selector in [
                ".status-widget",
                ".selection-widget",
                ".score-widget",
                ".history-list-widget",
                ".history-graph-widget",
            ] {
                let id = inner.query_selector(selector).unwrap().unwrap();
                let rect = inner.get_client_bounding_rect(id).unwrap();
                assert!(rect.width > 0.0 && rect.height > 0.0, "{skin:?} {selector}");
            }
        }
    }

    #[test]
    fn selected_widget_handle_center_starts_widget_resize() {
        let scenario = VisualDebugScenario {
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
        *session.selected.borrow_mut() = Some("selection".to_owned());
        session.resolve();

        let (canvas_rect, handle_rect) = {
            let inner = session.document.inner.borrow();
            let canvas = inner
                .query_selector(".canvas-content.selected")
                .unwrap()
                .unwrap();
            let handle = inner.query_selector(".resize-handle.se").unwrap().unwrap();
            (
                inner.get_client_bounding_rect(canvas).unwrap(),
                inner.get_client_bounding_rect(handle).unwrap(),
            )
        };
        let visible_left = handle_rect.x.max(canvas_rect.x);
        let visible_top = handle_rect.y.max(canvas_rect.y);
        let visible_right =
            (handle_rect.x + handle_rect.width).min(canvas_rect.x + canvas_rect.width);
        let visible_bottom =
            (handle_rect.y + handle_rect.height).min(canvas_rect.y + canvas_rect.height);
        assert!(visible_left < visible_right && visible_top < visible_bottom);
        let visible_center = [
            visible_left.midpoint(visible_right),
            visible_top.midpoint(visible_bottom),
        ];
        assert!(
            visible_center[0] >= handle_rect.x
                && visible_center[0] < handle_rect.x + handle_rect.width
                && visible_center[1] >= handle_rect.y
                && visible_center[1] < handle_rect.y + handle_rect.height
                && visible_center[0] >= canvas_rect.x
                && visible_center[0] < canvas_rect.x + canvas_rect.width
                && visible_center[1] >= canvas_rect.y
                && visible_center[1] < canvas_rect.y + canvas_rect.height
        );
        let point = [
            visible_center[0] - canvas_rect.x,
            visible_center[1] - canvas_rect.y,
        ];
        let hit = direct_manipulation_at(
            [
                session.settings.borrow().width,
                session.settings.borrow().height,
            ],
            &session.widgets.borrow(),
            session.selected.borrow().as_deref(),
            point,
        );
        let Some(DirectManipulationHit::Widget(original, Some(corner))) = hit else {
            panic!("the visible selected-widget handle must start widget resize");
        };
        assert_eq!(corner, ResizeCorner::SouthEast);
        let mut resized = original.clone();
        resize_widget(
            &mut resized,
            &original,
            point,
            corner,
            point[0] - 20.0,
            point[1] - 20.0,
            [
                session.settings.borrow().width,
                session.settings.borrow().height,
            ],
        );
        assert_eq!(resized.width, original.width - 20);
        assert_eq!(resized.height, original.height - 20);
    }

    #[test]
    fn compact_canvas_content_fills_the_viewport_and_does_not_clip_widgets() {
        let mut document = DioxusDocument::new(
            VirtualDom::new_with_props(
                native_overlay,
                NativeOverlayProps {
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

        for selector in [".canvas-content", ".overlay-canvas", ".status-widget"] {
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
            show_on: None,
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
        let preview = rect(".canvas-content");
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
        assert!((cyan.y - aurora.y).abs() < 1.0, "{cyan:?} {aurora:?}");
        assert!(
            (aurora.y - blackbox.y).abs() < 1.0,
            "{aurora:?} {blackbox:?}"
        );
        assert!(cyan.x + cyan.width <= aurora.x, "{cyan:?} {aurora:?}");
        assert!(
            aurora.x + aurora.width <= blackbox.x,
            "{aurora:?} {blackbox:?}"
        );
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
        let mut canvas = crate::config::OverlayConfig::initial().canvases[0].presentation();
        for screen in EDITOR_SCREENS {
            set_shown_on(&mut canvas, screen, false);
        }
        assert_eq!(canvas.show_on, Some(Vec::new()));
        assert!(
            EDITOR_SCREENS
                .into_iter()
                .all(|screen| !shown_on(&canvas, screen))
        );

        for screen in EDITOR_SCREENS {
            set_shown_on(&mut canvas, screen, true);
        }
        assert_eq!(canvas.show_on, None);
        assert!(
            EDITOR_SCREENS
                .into_iter()
                .all(|screen| shown_on(&canvas, screen))
        );
    }
}
