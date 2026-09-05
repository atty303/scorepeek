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
use blitz_dom::{Document, DocumentConfig};
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};
use dioxus::prelude::*;
use dioxus_core::{VirtualDom, schedule_update};
use dioxus_native_dom::DioxusDocument;
use scorepeek_overlay_handles::{CursorStyle, Event, OutputDescription, Shell};
use scorepeek_overlay_ui::{Appearance, OXANIUM, OverlayState, WidgetLayout, overlay_canvas};
use serde::Serialize;
use smithay_client_toolkit::reexports::calloop::ping::{Ping, make_ping};

type NativeUpdate = Rc<RefCell<Option<Arc<dyn Fn() + Send + Sync>>>>;
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

#[derive(Clone)]
struct NativeCanvasSettings {
    id: String,
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

#[derive(Clone, Copy, Eq, PartialEq)]
enum EditorTab {
    Widgets,
    Canvas,
}

#[derive(Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
struct EditorWorkspaceUi {
    panel_open: bool,
    tab: EditorTab,
    output_open: bool,
    manage_open: bool,
    widget_add_open: bool,
}

impl Default for EditorWorkspaceUi {
    fn default() -> Self {
        Self {
            panel_open: true,
            tab: EditorTab::Widgets,
            output_open: false,
            manage_open: false,
            widget_add_open: false,
        }
    }
}

#[derive(Clone)]
struct GeometryUndo(scorepeek_overlay_ui::CanvasPresentation);

#[derive(Default)]
struct NativeWorkspace {
    ui: EditorWorkspaceUi,
    undo: Option<GeometryUndo>,
    fallback: std::collections::BTreeSet<String>,
    reload_saved: bool,
}

#[derive(Clone)]
struct NativeOverlayProps {
    appearance: Rc<Cell<Appearance>>,
    widgets: Rc<RefCell<Vec<WidgetLayout>>>,
    editing: Rc<Cell<bool>>,
    actual_preview: Rc<Cell<bool>>,
    panel_open: Rc<Cell<bool>>,
    editor_tab: Rc<Cell<EditorTab>>,
    output_open: Rc<Cell<bool>>,
    manage_open: Rc<Cell<bool>>,
    widget_add_open: Rc<Cell<bool>>,
    dirty: Rc<Cell<bool>>,
    selected: Rc<RefCell<Option<String>>>,
    pending_widget: Rc<Cell<Option<scorepeek_overlay_ui::WidgetKind>>>,
    pending_point: Rc<Cell<[f64; 2]>>,
    pending_delete: Rc<RefCell<Option<String>>>,
    managed: Rc<RefCell<Vec<scorepeek_overlay_ui::CanvasPresentation>>>,
    outputs: Rc<RefCell<Vec<OutputDescription>>>,
    state: Rc<RefCell<OverlayState>>,
    visible: Rc<Cell<bool>>,
    settings: Rc<RefCell<NativeCanvasSettings>>,
    update: NativeUpdate,
}

#[allow(clippy::cast_precision_loss)]
fn native_overlay(
    NativeOverlayProps {
        state,
        visible,
        settings,
        update,
        appearance,
        widgets,
        editing,
        actual_preview,
        panel_open,
        editor_tab,
        output_open,
        manage_open,
        widget_add_open,
        dirty,
        selected,
        pending_widget,
        pending_point,
        pending_delete,
        managed,
        outputs,
    }: NativeOverlayProps,
) -> Element {
    *update.borrow_mut() = Some(schedule_update());
    let current = state.borrow().clone();
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
    let canvas_enabled = managed
        .borrow()
        .iter()
        .find(|canvas| canvas.id == current_settings.id)
        .is_some_and(|canvas| canvas.enabled);
    rsx! {
        div { class: if editing.get() { "canvas-content editor-preview-canvas selected" } else { "canvas-content" },style:format!("display:{};opacity:{};{}",if visible.get(){"block"}else{"none"},f32::from(current_settings.opacity_percent)/100.0,if editing.get(){format!("left:{}px;top:{}px;width:{}px;height:{}px",current_settings.x,current_settings.y,current_settings.width,current_settings.height)}else{String::new()}),
            {overlay_canvas(
                &shown,
                appearance.get(),
                &widgets.borrow(),
                editing.get(),
                selected.borrow().as_deref(),
            )}
            if editing.get() && !actual_preview.get() { for corner in ["nw","ne","sw","se"] { i { class:"native-canvas-handle {corner}" } } }
        }
        if editing.get() {
            for canvas in managed.borrow().iter().filter(|canvas| canvas.id != current_settings.id && canvas.enabled && (canvas.output == current_settings.output || canvas.output.is_none()) && scorepeek_overlay_ui::canvas_visible(canvas.show_on.as_deref(), scorepeek_overlay_ui::ScreenView { kind:Some(current_settings.preview_screen), suspended_since_unix_ms:None, revision:0 })) {
                div { class:"canvas-content editor-preview-canvas preview-only", style:format!("left:{}px;top:{}px;width:{}px;height:{}px;opacity:{}",canvas.x,canvas.y,canvas.width,canvas.height,f32::from(canvas.opacity_percent)/100.0),
                    {overlay_canvas(&shown, Appearance { skin: canvas.skin }, &canvas.widgets, false, None)}
                }
            }
        }
        if editing.get() && !actual_preview.get() {
            button { class:if dirty.get(){"native-panel-toggle dirty"}else{"native-panel-toggle"}, "aria-label":if panel_open.get(){"Hide editor panel"}else{"Show editor panel"}, "data-state":if panel_open.get(){"open"}else{"closed"}, if panel_open.get(){"‹"}else{"›"} span { class:"dirty-dot" } }
            if panel_open.get() { div { class:"native-canvas-manager", style:format!("width:{}px",current_settings.panel_width),
                header { strong { "SCOREPEEK OVERLAY" } small { "WAYLAND EDITOR" } if sample { b { "SAMPLE DATA" } } if dirty.get() { i { class:"unsaved-dot" } } }
                div { class:"editor-fixed-top", p { "PREVIEW DATA" } div { class:"preview-tabs",
                    for (index,(label,kind)) in [("SELECT",scorepeek_overlay_ui::ScreenKind::MusicSelect),("MODE",scorepeek_overlay_ui::ScreenKind::ModeSelect),("DECIDE",scorepeek_overlay_ui::ScreenKind::DecideTransition),("PLAY",scorepeek_overlay_ui::ScreenKind::Play),("RESULT",scorepeek_overlay_ui::ScreenKind::Result)].into_iter().enumerate() {
                        button { class:if current_settings.preview_screen==kind{"selected preview-screen"}else{"preview-screen"}, "aria-selected":current_settings.preview_screen==kind, "data-index":index, "{label}" }
                    }
                } }
                nav { class:"canvas-list", for canvas in managed.borrow().iter() { button { class:if canvas.id==current_settings.id{"canvas-row selected"}else if canvas.enabled{"canvas-row"}else{"canvas-row disabled"}, "aria-selected":canvas.id==current_settings.id, "data-canvas-id":"{canvas.id}", span { "{canvas.id}" } i { class:if canvas.enabled{"canvas-enabled-lamp on"}else{"canvas-enabled-lamp off"} } } } }
                div { class:"editor-tabs", button { class:if editor_tab.get()==EditorTab::Widgets{"editor-tab selected"}else{"editor-tab"}, "aria-selected":editor_tab.get()==EditorTab::Widgets, "data-tab":"widgets", "WIDGETS" } button { class:if editor_tab.get()==EditorTab::Canvas{"editor-tab selected"}else{"editor-tab"}, "aria-selected":editor_tab.get()==EditorTab::Canvas, "data-tab":"canvas", "CANVAS" } }
                div { class:"editor-tab-body",
                if editor_tab.get()==EditorTab::Widgets { section { class:"widgets-pane",
                    for widget in widgets.borrow().iter() { button { class:if selected.borrow().as_deref()==Some(widget.id.as_str()){"widget-row selected"}else{"widget-row"}, "aria-selected":selected.borrow().as_deref()==Some(widget.id.as_str()), "data-widget-id":"{widget.id}", "{widget.id}" } }
                    details { class:"widget-add", open:widget_add_open.get(), summary { class:"widget-add-summary", "+ ADD WIDGET" } if widget_add_open.get() { div { class:"button-grid", for (index,label) in ["STATUS","SELECTION","SCORE","HISTORY LIST","HISTORY GRAPH"].into_iter().enumerate() { button { class:"add-widget", "data-index":index, "+ {label}" } } } } }
                    if let Some(widget) = selected_widget {
                        div { class:"native-widget-settings",
                            strong { "{widget.id}" }
                            if widget.kind == scorepeek_overlay_ui::WidgetKind::HistoryList {
                                for value in [5,10,20,50] { button { class:if widget.settings.history_count==value{"history-count selected"}else{"history-count"}, "aria-pressed":widget.settings.history_count==value, "data-value":value, if widget.settings.history_count==value{"✓ "} "{value}" } }
                            }
                            if widget.kind == scorepeek_overlay_ui::WidgetKind::HistoryGraph {
                                for value in [1,3,6,12] { button { class:if widget.settings.graph_months==value{"graph-months selected"}else{"graph-months"}, "aria-pressed":widget.settings.graph_months==value, "data-value":value, if widget.settings.graph_months==value{"✓ "} "{value}M" } }
                            }
                            button { class:"delete-widget danger", if pending_delete.borrow().as_deref()==Some(widget.id.as_str()) { "CONFIRM DELETE" } else { "DELETE WIDGET" } }
                        }
                    }
                } } else { section { class:"canvas-pane",
                    h2 { "VISIBILITY" }
                    div { class:"visibility-mode", button { class:if current_settings.show_on.is_none(){"visibility-all selected"}else{"visibility-all"}, "aria-pressed":current_settings.show_on.is_none(), "ALL SCREENS" } button { class:if current_settings.show_on.is_some(){"visibility-specific selected"}else{"visibility-specific"}, "aria-pressed":current_settings.show_on.is_some(), "SPECIFIC SCREENS" } }
                    if let Some(show_on)=current_settings.show_on.as_ref() { div { class:"screen-filter", for (index,(label,kind)) in [("SELECT",scorepeek_overlay_ui::ScreenKind::MusicSelect),("MODE",scorepeek_overlay_ui::ScreenKind::ModeSelect),("DECIDE",scorepeek_overlay_ui::ScreenKind::DecideTransition),("PLAY",scorepeek_overlay_ui::ScreenKind::Play),("RESULT",scorepeek_overlay_ui::ScreenKind::Result)].into_iter().enumerate() { button { class:if show_on.contains(&kind){"visibility-screen selected"}else{"visibility-screen"}, "aria-pressed":show_on.contains(&kind), "data-index":index, if show_on.contains(&kind){"✓ "} "{label}" } } } }
                    h2 { "APPEARANCE" }
                    div { class:"native-skin-options button-grid three", for (index,(label,skin)) in [("CYAN",scorepeek_overlay_ui::Skin::CyanSystem),("AURORA",scorepeek_overlay_ui::Skin::ResultAurora),("BLACKBOX",scorepeek_overlay_ui::Skin::DjBlackbox)].into_iter().enumerate() { button { class:if appearance.get().skin==skin{"skin-option selected"}else{"skin-option"}, "aria-pressed":appearance.get().skin==skin, "data-index":index, if appearance.get().skin==skin{"✓ "} "{label}" } } }
                    h2 { "OPACITY {current_settings.opacity_percent}%" }
                    div { class:"native-opacity button-grid four", for value in [25,50,75,100] { button { class:if current_settings.opacity_percent==value{"opacity-option selected"}else{"opacity-option"}, "aria-pressed":current_settings.opacity_percent==value, "data-value":value, if current_settings.opacity_percent==value{"✓ "} "{value}" } } }
                    details { class:"output-settings", open:output_open.get(), summary { class:"output-summary", "OUTPUT" } if output_open.get() { div { class:"output-list", for output in outputs.borrow().iter() { button { class:if current_settings.output.as_deref()==Some(output.name.as_str()){"output-option selected"}else{"output-option"}, "aria-selected":current_settings.output.as_deref()==Some(output.name.as_str()), "data-output":"{output.name}", strong { if current_settings.output.as_deref()==Some(output.name.as_str()){"✓ "} "{output.name}" } small { "{output.model}" if let Some([width,height])=output.logical_size { " · {width}×{height}" } } } } } } }
                    details { class:"manage-canvas", open:manage_open.get(), summary { class:"manage-summary", "MANAGE CANVAS" } if manage_open.get() { button { class:if canvas_enabled{"toggle-canvas canvas-switch selected"}else{"toggle-canvas canvas-switch"}, "aria-pressed":canvas_enabled, span { "CANVAS ENABLED" } i { if canvas_enabled{"✓"} } } button { class:"add-canvas", "ADD EMPTY CANVAS" } button { class:"delete-canvas danger", if pending_delete.borrow().as_deref()==Some(current_settings.id.as_str()) { "CONFIRM DELETE" } else { "DELETE CANVAS" } } } }
                } }
                }
                footer { button { class:"undo-action", "UNDO GEOMETRY" } button { class:"actual-action", "PREVIEW ACTUAL" } button { class:"discard-action", disabled:!dirty.get(), "DISCARD" } button { class:if dirty.get(){"primary save-action"}else{"save-action"}, disabled:!dirty.get(), "SAVE AND CLOSE" } }
            } }
            if let Some(kind) = pending_widget.get() { div { class:"native-placement-ghost", style:format!("left:{}px;top:{}px",pending_point.get()[0],pending_point.get()[1]), "PLACE {kind:?}" } }
        }
        if editing.get() && actual_preview.get() { button { class:"native-return-editor", "RETURN TO EDITOR" } }
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
    let workspace_ui = Arc::new(std::sync::Mutex::new(NativeWorkspace::default()));
    let suppressed = Arc::new(std::sync::Mutex::new(
        std::collections::BTreeSet::<String>::new(),
    ));
    if config.edit_on_start
        && let Some(canvas) = desired.first()
    {
        *preview
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(canvas.id.clone());
    }
    while !stop.load(std::sync::atomic::Ordering::Acquire) {
        if let Ok((loaded, _)) = crate::config::load_or_create(&config.config_path) {
            desired = loaded
                .canvases
                .into_iter()
                .filter(|canvas| {
                    canvas.backend == crate::runtime::Backend::Wayland && canvas.enabled
                })
                .collect();
        }
        if preview
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
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
            let target = preview
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            desired = response
                .canvases
                .into_iter()
                .filter(|canvas| canvas.enabled || target.as_deref() == Some(canvas.id.as_str()))
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
            let workspace_ui = Arc::clone(&workspace_ui);
            let suppressed = Arc::clone(&suppressed);
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
                        workspace_ui,
                        suppressed,
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
    workspace_ui: Arc<std::sync::Mutex<NativeWorkspace>>,
    suppressed: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    wakes: &std::sync::Mutex<std::collections::BTreeMap<String, Ping>>,
) -> Result<(), String> {
    let report = Rc::new(RefCell::new(RunReport::new()));
    let ping = make_ping().map_err(|e| e.to_string())?;
    let wake = ping.0.clone();
    let mut canvas = config
        .canvases
        .first()
        .ok_or("Wayland overlay has no enabled canvas")?
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
    let appearance = Appearance { skin: canvas.skin };
    let widgets = canvas.widgets.iter().map(widget_layout).collect();
    let control_socket = config.control_socket.clone();
    let mut app = App::new(
        appearance,
        widgets,
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
        workspace_ui,
        suppressed,
    );
    let result = app.run();
    app.renderer.suspend();
    let mut report = report.borrow_mut();
    report.paint_count = app.paint_count;
    report.render_calls = app.render_calls;
    report.status = if result.is_ok() { "complete" } else { "failed" };
    report.failure = result.as_ref().err().cloned();
    report.operations.push("shutdown");
    crate::diagnostics::emit("native_summary", &*report);
    result
}

struct App {
    // Renderer is dropped before the shell; its own Arc handle also retains ownership.
    renderer: VelloWindowRenderer,
    shell: Shell,
    document: DioxusDocument,
    shared_state: Rc<RefCell<OverlayState>>,
    native_update: NativeUpdate,
    waker: Waker,
    started: Instant,
    animating: bool,
    paint_count: u32,
    render_calls: u32,
    report: Rc<RefCell<RunReport>>,
    feed_state: Arc<std::sync::Mutex<OverlayState>>,
    feed_stop: Arc<std::sync::atomic::AtomicBool>,
    external_stop: Arc<std::sync::atomic::AtomicBool>,
    canvas: crate::config::Canvas,
    control_socket: std::path::PathBuf,
    appearance: Rc<Cell<Appearance>>,
    editor_id: String,
    editing: Rc<Cell<bool>>,
    actual_preview: Rc<Cell<bool>>,
    panel_open: Rc<Cell<bool>>,
    editor_tab: Rc<Cell<EditorTab>>,
    output_open: Rc<Cell<bool>>,
    manage_open: Rc<Cell<bool>>,
    widget_add_open: Rc<Cell<bool>>,
    dirty: Rc<Cell<bool>>,
    readonly: bool,
    selected: Rc<RefCell<Option<String>>>,
    pending_widget: Rc<Cell<Option<scorepeek_overlay_ui::WidgetKind>>>,
    pending_point: Rc<Cell<[f64; 2]>>,
    pending_delete: Rc<RefCell<Option<String>>>,
    shared_widgets: Rc<RefCell<Vec<WidgetLayout>>>,
    interaction: Option<NativeInteraction>,
    next_keepalive: Instant,
    managed: Rc<RefCell<Vec<scorepeek_overlay_ui::CanvasPresentation>>>,
    backend_revision: u64,
    outputs: Rc<RefCell<Vec<OutputDescription>>>,
    visible: Rc<Cell<bool>>,
    pending_resolved_output: Option<String>,
    next_output_persist: Instant,
    preview: Arc<std::sync::Mutex<Option<String>>>,
    workspace_ui: Arc<std::sync::Mutex<NativeWorkspace>>,
    suppressed: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    settings: Rc<RefCell<NativeCanvasSettings>>,
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
#[derive(Clone, Copy)]
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
        workspace_ui: Arc<std::sync::Mutex<NativeWorkspace>>,
        suppressed: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    ) -> Self {
        let shared_state = Rc::new(RefCell::new(OverlayState::default()));
        let native_update = Rc::new(RefCell::new(None));
        let shared_widgets = Rc::new(RefCell::new(widgets));
        let editing = Rc::new(Cell::new(false));
        let actual_preview = Rc::new(Cell::new(false));
        let ui = workspace_ui
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .ui;
        let panel_open = Rc::new(Cell::new(ui.panel_open));
        let editor_tab = Rc::new(Cell::new(ui.tab));
        let output_open = Rc::new(Cell::new(ui.output_open));
        let manage_open = Rc::new(Cell::new(ui.manage_open));
        let widget_add_open = Rc::new(Cell::new(ui.widget_add_open));
        let dirty = Rc::new(Cell::new(false));
        let selected = Rc::new(RefCell::new(None));
        let pending_widget = Rc::new(Cell::new(None));
        let pending_point = Rc::new(Cell::new([340.0, 24.0]));
        let pending_delete = Rc::new(RefCell::new(None));
        let managed = Rc::new(RefCell::new(Vec::new()));
        let outputs = Rc::new(RefCell::new(outputs));
        let appearance = Rc::new(Cell::new(appearance));
        let panel_width = editor_panel_width(shell.output_logical_size.map(|[width, _]| width));
        let settings = Rc::new(RefCell::new(NativeCanvasSettings {
            id: canvas.id.clone(),
            output: canvas.output.clone(),
            show_on: canvas.show_on.clone(),
            opacity_percent: canvas.opacity_percent,
            x: canvas.x,
            y: canvas.y,
            width: canvas.width,
            height: canvas.height,
            preview_screen: scorepeek_overlay_ui::ScreenKind::MusicSelect,
            panel_width,
        }));
        let initially_visible = canvas.show_on.is_none();
        shell.set_input_enabled(initially_visible);
        let visible = Rc::new(Cell::new(initially_visible));
        let vdom = VirtualDom::new_with_props(
            native_overlay,
            NativeOverlayProps {
                appearance: Rc::clone(&appearance),
                widgets: Rc::clone(&shared_widgets),
                editing: Rc::clone(&editing),
                actual_preview: Rc::clone(&actual_preview),
                panel_open: Rc::clone(&panel_open),
                editor_tab: Rc::clone(&editor_tab),
                output_open: Rc::clone(&output_open),
                manage_open: Rc::clone(&manage_open),
                widget_add_open: Rc::clone(&widget_add_open),
                dirty: Rc::clone(&dirty),
                selected: Rc::clone(&selected),
                pending_widget: Rc::clone(&pending_widget),
                pending_point: Rc::clone(&pending_point),
                pending_delete: Rc::clone(&pending_delete),
                managed: Rc::clone(&managed),
                outputs: Rc::clone(&outputs),
                state: Rc::clone(&shared_state),
                visible: Rc::clone(&visible),
                settings: Rc::clone(&settings),
                update: Rc::clone(&native_update),
            },
        );
        let mut document = DioxusDocument::new(vdom, document_config());
        document.initial_build();
        let surface_logical = [canvas.width, canvas.height];

        if pending_resolved_output.is_some() {
            workspace_ui
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .fallback
                .insert(canvas.id.clone());
        }
        Self {
            renderer: VelloWindowRenderer::with_options(
                VelloRendererOptions::default()
                    .base_color(peniko::Color::TRANSPARENT)
                    .composite_alpha_mode(CompositeAlphaMode::Transparent),
            ),
            shell,
            document,
            shared_state,
            native_update,
            waker,
            started: Instant::now(),
            animating: false,
            paint_count: 0,
            render_calls: 0,
            report,
            feed_state,
            feed_stop,
            external_stop,
            canvas,
            control_socket,
            appearance,
            editor_id: format!("wayland-{}", std::process::id()),
            editing,
            actual_preview,
            panel_open,
            editor_tab,
            output_open,
            manage_open,
            widget_add_open,
            dirty,
            readonly: true,
            selected,
            pending_widget,
            pending_point,
            pending_delete,
            shared_widgets,
            interaction: None,
            next_keepalive: Instant::now(),
            managed,
            backend_revision: 0,
            outputs,
            pending_resolved_output,
            next_output_persist: Instant::now() + Duration::from_secs(1),
            visible,
            preview,
            workspace_ui,
            suppressed,
            settings,
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
            let preview_target = self
                .preview
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if preview_target.as_deref() == Some(self.canvas.id.as_str()) && !self.editing.get() {
                self.set_editing(true);
            } else if self.editing.get()
                && preview_target.as_deref() != Some(self.canvas.id.as_str())
            {
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
            if *self.outputs.borrow() != self.shell.output_descriptions {
                self.outputs
                    .borrow_mut()
                    .clone_from(&self.shell.output_descriptions);
                if let Some(update) = self.native_update.borrow().as_ref() {
                    update();
                }
                wake = true;
            }
            for event in events {
                match event {
                    Event::Configure {
                        logical,
                        physical,
                        scale_120,
                    } => self.configure(logical, physical, scale_120)?,
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
                    Event::Frame => frame = true,
                    Event::Closed => return Ok(()),
                }
            }
            let latest = self
                .feed_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let preview_target = self
                .preview
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let workspace_peer = preview_target
                .as_deref()
                .is_some_and(|target| target != self.canvas.id);
            let visible = !workspace_peer
                && (self.editing.get()
                    || scorepeek_overlay_ui::canvas_visible(
                        self.canvas.show_on.as_deref(),
                        latest.screen,
                    ));
            if self.visible.get() != visible {
                self.visible.set(visible);
                self.shell.set_input_enabled(visible);
                if let Some(update) = self.native_update.borrow().as_ref() {
                    update();
                }
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
                if let Some(update) = self.native_update.borrow().as_ref() {
                    update();
                }
                wake = true;
            }
            let changed = wake && self.poll_dioxus();
            if self.renderer.is_active() && (changed || (frame && self.animating)) {
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
        *self
            .preview
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(self.canvas.id.clone());
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
            if let Some(presentation) = response
                .canvases
                .iter()
                .find(|item| item.id == self.canvas.id)
            {
                self.canvas.apply_presentation(presentation);
                self.appearance.set(Appearance {
                    skin: presentation.skin,
                });
                self.shared_widgets
                    .borrow_mut()
                    .clone_from(&presentation.widgets);
                let mut settings = self.settings.borrow_mut();
                settings.output.clone_from(&presentation.output);
                settings.show_on.clone_from(&presentation.show_on);
                settings.opacity_percent = presentation.opacity_percent;
                settings.x = presentation.x;
                settings.y = presentation.y;
                settings.width = presentation.width;
                settings.height = presentation.height;
            }
        }
        Some(response)
    }
    fn acquire(&mut self) {
        let _ = self.request(crate::control::Request::AcquireBackend {
            backend: crate::runtime::Backend::Wayland,
            editor_id: self.editor_id.clone(),
        });
    }
    fn set_editing(&mut self, value: bool) {
        if value {
            self.acquire();
            self.next_keepalive = Instant::now() + Duration::from_secs(5);
        } else if self
            .preview
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none()
        {
            let _ = self.request(crate::control::Request::ReleaseBackend {
                backend: crate::runtime::Backend::Wayland,
                editor_id: self.editor_id.clone(),
            });
        }
        self.editing.set(value);
        if !value {
            self.actual_preview.set(false);
            self.dirty.set(false);
        }
        self.set_editor_geometry(value);
        if value && !self.visible.replace(true) {
            self.shell.set_input_enabled(true);
        }
        self.interaction = None;
        if !value {
            self.selected.borrow_mut().take();
        }
        self.sync_workspace_ui();
        if let Some(update) = self.native_update.borrow().as_ref() {
            update();
        }
    }
    fn set_editor_geometry(&mut self, editing: bool) {
        if !editing {
            self.shell.set_geometry(
                self.canvas.x,
                self.canvas.y,
                self.canvas.width,
                self.canvas.height,
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
                    *self
                        .preview
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(self.canvas.id.clone());
                    self.set_editing(true);
                    return;
                }
                if self.readonly
                    || self.actual_preview.get()
                    || (self.panel_open.get() && x < f64::from(self.settings.borrow().panel_width))
                {
                    return;
                }
                let target = self
                    .managed
                    .borrow()
                    .iter()
                    .rfind(|canvas| {
                        canvas.enabled
                            && (canvas.output == self.canvas.output || canvas.output.is_none())
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
                    if target.id == self.canvas.id {
                        self.remember_geometry();
                        self.interaction = Some(NativeInteraction::CanvasMove {
                            start: [x, y],
                            origin: [target.x, target.y],
                        });
                    } else {
                        self.workspace_ui
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .undo = Some(GeometryUndo(target.clone()));
                        self.interaction = Some(NativeInteraction::ManagedCanvasMove {
                            id: target.id,
                            start: [x, y],
                            origin: [target.x, target.y],
                        });
                    }
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
            if self.actual_preview.get() {
                if self.hit_selector(".native-return-editor", x, y) {
                    self.actual_preview.set(false);
                    if let Some(update) = self.native_update.borrow().as_ref() {
                        update();
                    }
                }
                return;
            }
            if self.hit_selector(".native-panel-toggle", x, y) {
                self.panel_open.set(!self.panel_open.get());
                self.sync_workspace_ui();
                if let Some(update) = self.native_update.borrow().as_ref() {
                    update();
                }
                return;
            }
            let outputs = self.outputs.borrow().clone();
            for output in outputs {
                if self.hit_selector(
                    &format!(".output-option[data-output='{}']", output.name),
                    x,
                    y,
                ) {
                    self.canvas.output = Some(output.name.clone());
                    self.settings.borrow_mut().output = Some(output.name);
                    self.persist_canvas();
                    return;
                }
            }
            if self.panel_open.get() && x < f64::from(self.settings.borrow().panel_width) {
                if self.hit_selector(".undo-action", x, y) {
                    self.undo_last_geometry();
                    return;
                }
                if self.hit_selector(".actual-action", x, y) {
                    self.actual_preview.set(true);
                    if let Some(update) = self.native_update.borrow().as_ref() {
                        update();
                    }
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
                    self.preview
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take();
                    self.pending_widget.set(None);
                    self.pending_delete.borrow_mut().take();
                    self.set_editing(false);
                    return;
                }
                if self.hit_selector(".save-action", x, y) {
                    if self.dirty.get() {
                        self.save_and_close();
                    }
                    return;
                }
                if self.hit_selector(".editor-tab[data-tab='widgets']", x, y) {
                    self.editor_tab.set(EditorTab::Widgets);
                    self.sync_workspace_ui();
                    if let Some(update) = self.native_update.borrow().as_ref() {
                        update();
                    }
                    return;
                }
                if self.hit_selector(".editor-tab[data-tab='canvas']", x, y) {
                    self.editor_tab.set(EditorTab::Canvas);
                    self.sync_workspace_ui();
                    if let Some(update) = self.native_update.borrow().as_ref() {
                        update();
                    }
                    return;
                }
                if self.hit_selector(".widget-add-summary", x, y) {
                    self.widget_add_open.set(!self.widget_add_open.get());
                    self.sync_workspace_ui();
                    if let Some(update) = self.native_update.borrow().as_ref() {
                        update();
                    }
                    return;
                }
                if self.hit_selector(".output-summary", x, y) {
                    self.output_open.set(!self.output_open.get());
                    self.sync_workspace_ui();
                    if let Some(update) = self.native_update.borrow().as_ref() {
                        update();
                    }
                    return;
                }
                if self.hit_selector(".manage-summary", x, y) {
                    self.manage_open.set(!self.manage_open.get());
                    self.sync_workspace_ui();
                    if let Some(update) = self.native_update.borrow().as_ref() {
                        update();
                    }
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
                        if let Some(update) = self.native_update.borrow().as_ref() {
                            update();
                        }
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
                    if self.hit_selector(&format!(".canvas-row[data-canvas-id='{id}']"), x, y) {
                        if id != self.canvas.id {
                            *self
                                .preview
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(id);
                        }
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
                        if let Some(update) = self.native_update.borrow().as_ref() {
                            update();
                        }
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
                        self.pending_delete.borrow_mut().take();
                        if let Some(update) = self.native_update.borrow().as_ref() {
                            update();
                        }
                        return;
                    }
                }
                if self.hit_selector(".delete-widget", x, y) {
                    let selected_id = self.selected.borrow().clone();
                    if let Some(id) = selected_id {
                        if self.pending_delete.borrow().as_deref() == Some(id.as_str()) {
                            self.shared_widgets
                                .borrow_mut()
                                .retain(|widget| widget.id != id);
                            self.selected.borrow_mut().take();
                            self.pending_delete.borrow_mut().take();
                            self.persist_canvas();
                        } else {
                            *self.pending_delete.borrow_mut() = Some(id);
                            if let Some(update) = self.native_update.borrow().as_ref() {
                                update();
                            }
                        }
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
                if self.hit_selector(".toggle-canvas", x, y) {
                    if let Some(canvas) = self
                        .managed
                        .borrow_mut()
                        .iter_mut()
                        .find(|canvas| canvas.id == self.canvas.id)
                    {
                        canvas.enabled = !canvas.enabled;
                    }
                    self.update_draft();
                    return;
                }
                if self.hit_selector(".add-canvas", x, y) {
                    let suffix = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis();
                    let mut canvas = crate::config::empty_canvas(
                        format!("wayland-{suffix}"),
                        crate::runtime::Backend::Wayland,
                    )
                    .presentation();
                    canvas.output.clone_from(&self.canvas.output);
                    canvas.x = 0;
                    canvas.y = 0;
                    canvas.width = 560.min(grid_floor(self.surface_logical[0]));
                    canvas.height = 1040.min(grid_floor(self.surface_logical[1]));
                    let id = canvas.id.clone();
                    self.managed.borrow_mut().push(canvas);
                    self.update_draft();
                    *self
                        .preview
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(id);
                    return;
                }
                if self.hit_selector(".delete-canvas", x, y) {
                    if self.managed.borrow().len() <= 1 {
                        return;
                    }
                    if self.pending_delete.borrow().as_deref() == Some(self.canvas.id.as_str()) {
                        self.managed
                            .borrow_mut()
                            .retain(|canvas| canvas.id != self.canvas.id);
                        self.pending_delete.borrow_mut().take();
                        self.update_draft();
                        let next = self
                            .managed
                            .borrow()
                            .first()
                            .map(|canvas| canvas.id.clone());
                        *self
                            .preview
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
                    } else {
                        *self.pending_delete.borrow_mut() = Some(self.canvas.id.clone());
                        if let Some(update) = self.native_update.borrow().as_ref() {
                            update();
                        }
                    }
                    return;
                }
                if self.hit_selector(".visibility-all", x, y) {
                    self.settings.borrow_mut().show_on = None;
                    self.persist_canvas();
                    return;
                }
                if self.hit_selector(".visibility-specific", x, y) {
                    if self.settings.borrow().show_on.is_none() {
                        let preview_screen = self.settings.borrow().preview_screen;
                        self.settings.borrow_mut().show_on = Some(vec![preview_screen]);
                        self.persist_canvas();
                    }
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
                    if self.hit_selector(&format!(".visibility-screen[data-index='{index}']"), x, y)
                    {
                        let mut settings = self.settings.borrow_mut();
                        let values = settings.show_on.get_or_insert_with(Vec::new);
                        if let Some(position) = values.iter().position(|value| *value == kind) {
                            values.remove(position);
                        } else {
                            values.push(kind);
                        }
                        if values.is_empty() {
                            settings.show_on = None;
                        }
                        drop(settings);
                        self.persist_canvas();
                        return;
                    }
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
                        self.appearance.set(Appearance { skin });
                        self.canvas.skin = skin;
                        self.persist_canvas();
                        return;
                    }
                }
                for value in [25_u8, 50, 75, 100] {
                    if self.hit_selector(&format!(".opacity-option[data-value='{value}']"), x, y) {
                        self.settings.borrow_mut().opacity_percent = value;
                        self.persist_canvas();
                        return;
                    }
                }
                return;
            }
            let output_point = [x, y];
            let x = x - f64::from(self.canvas.x);
            let y = y - f64::from(self.canvas.y);
            if let Some(kind) = self.pending_widget.take() {
                self.place_widget(kind, x, y);
                return;
            }
            let corner = if x < 18.0 && y < 18.0 {
                Some(ResizeCorner::NorthWest)
            } else if x >= f64::from(self.canvas.width.saturating_sub(18)) && y < 18.0 {
                Some(ResizeCorner::NorthEast)
            } else if x < 18.0 && y >= f64::from(self.canvas.height.saturating_sub(18)) {
                Some(ResizeCorner::SouthWest)
            } else if x >= f64::from(self.canvas.width.saturating_sub(18))
                && y >= f64::from(self.canvas.height.saturating_sub(18))
            {
                Some(ResizeCorner::SouthEast)
            } else {
                None
            };
            if let Some(corner) = corner {
                self.remember_geometry();
                self.interaction = Some(NativeInteraction::CanvasResize {
                    start: output_point,
                    position: [self.canvas.x, self.canvas.y],
                    origin: [self.canvas.width, self.canvas.height],
                    corner,
                });
                return;
            }
            let hit = self
                .shared_widgets
                .borrow()
                .iter()
                .rfind(|widget| {
                    x >= f64::from(widget.x)
                        && y >= f64::from(widget.y)
                        && x < f64::from(widget.x) + f64::from(widget.width)
                        && y < f64::from(widget.y) + f64::from(widget.height)
                })
                .cloned();
            if let Some(original) = hit {
                let local_x = x - f64::from(original.x);
                let local_y = y - f64::from(original.y);
                let corner = if local_x < 18.0 && local_y < 18.0 {
                    Some(ResizeCorner::NorthWest)
                } else if local_x >= f64::from(original.width.saturating_sub(18)) && local_y < 18.0
                {
                    Some(ResizeCorner::NorthEast)
                } else if local_x < 18.0 && local_y >= f64::from(original.height.saturating_sub(18))
                {
                    Some(ResizeCorner::SouthWest)
                } else if local_x >= f64::from(original.width.saturating_sub(18))
                    && local_y >= f64::from(original.height.saturating_sub(18))
                {
                    Some(ResizeCorner::SouthEast)
                } else {
                    None
                };
                *self.selected.borrow_mut() = Some(original.id.clone());
                self.remember_geometry();
                self.interaction = Some(NativeInteraction::Widget {
                    id: original.id.clone(),
                    start: [x, y],
                    original,
                    corner,
                });
            }
        } else {
            self.persist_interaction();
        }
    }
    #[allow(clippy::too_many_lines)]
    fn pointer_motion(&mut self, x: f64, y: f64) {
        if self.pending_widget.get().is_some() {
            self.pending_point.set([x, y]);
            if let Some(update) = self.native_update.borrow().as_ref() {
                update();
            }
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
        if let Some(update) = self.native_update.borrow().as_ref() {
            update();
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
        self.settings.borrow_mut().x = self.canvas.x;
        self.settings.borrow_mut().y = self.canvas.y;
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
        let west = matches!(corner, ResizeCorner::NorthWest | ResizeCorner::SouthWest);
        let north = matches!(corner, ResizeCorner::NorthWest | ResizeCorner::NorthEast);
        let east = matches!(corner, ResizeCorner::NorthEast | ResizeCorner::SouthEast);
        let south = matches!(corner, ResizeCorner::SouthWest | ResizeCorner::SouthEast);
        if west {
            let maximum = i32::try_from(origin[0].saturating_sub(min_width)).unwrap_or(i32::MAX);
            let applied = dx.clamp(-position[0], maximum);
            self.canvas.x = position[0].saturating_add(applied);
            self.canvas.width = origin[0].saturating_sub_signed(applied);
        } else if east {
            self.canvas.width = origin[0].saturating_add_signed(dx).max(min_width);
        }
        if north {
            let maximum = i32::try_from(origin[1].saturating_sub(min_height)).unwrap_or(i32::MAX);
            let applied = dy.clamp(-position[1], maximum);
            self.canvas.y = position[1].saturating_add(applied);
            self.canvas.height = origin[1].saturating_sub_signed(applied);
        } else if south {
            self.canvas.height = origin[1].saturating_add_signed(dy).max(min_height);
        }
        if let Some([output_width, output_height]) = self.shell.output_logical_size {
            self.canvas.width = self.canvas.width.min(grid_floor(
                output_width.saturating_sub(self.canvas.x.cast_unsigned()),
            ));
            self.canvas.height = self.canvas.height.min(grid_floor(
                output_height.saturating_sub(self.canvas.y.cast_unsigned()),
            ));
        }
        self.settings.borrow_mut().width = self.canvas.width;
        self.settings.borrow_mut().height = self.canvas.height;
        self.set_editor_geometry(self.editing.get());
    }
    fn persist_interaction(&mut self) {
        let Some(interaction) = self.interaction.take() else {
            return;
        };
        if let NativeInteraction::ManagedCanvasMove { id, .. } = interaction {
            self.update_draft();
            *self
                .preview
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(id);
        } else {
            self.persist_canvas();
        }
        if !self.editing.get() {
            let _ = self.request(crate::control::Request::ReleaseBackend {
                backend: crate::runtime::Backend::Wayland,
                editor_id: self.editor_id.clone(),
            });
        }
    }
    fn persist_canvas(&mut self) {
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
        self.update_draft();
        if let Some(update) = self.native_update.borrow().as_ref() {
            update();
        }
    }
    fn remember_geometry(&mut self) {
        let mut presentation = self.canvas.presentation();
        presentation.skin = self.appearance.get().skin;
        presentation
            .widgets
            .clone_from(&self.shared_widgets.borrow());
        self.workspace_ui
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .undo = Some(GeometryUndo(presentation));
    }
    fn undo_last_geometry(&mut self) {
        let Some(undo) = self
            .workspace_ui
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .undo
            .take()
        else {
            return;
        };
        let GeometryUndo(canvas) = undo;
        let restored = self
            .managed
            .borrow_mut()
            .iter_mut()
            .find(|existing| existing.id == canvas.id)
            .map(|existing| {
                existing.x = canvas.x;
                existing.y = canvas.y;
                existing.width = canvas.width;
                existing.height = canvas.height;
                for snapshot in &canvas.widgets {
                    if let Some(widget) = existing
                        .widgets
                        .iter_mut()
                        .find(|widget| widget.id == snapshot.id)
                    {
                        widget.x = snapshot.x;
                        widget.y = snapshot.y;
                        widget.width = snapshot.width;
                        widget.height = snapshot.height;
                    }
                }
                existing.clone()
            });
        if canvas.id == self.canvas.id {
            self.canvas.x = canvas.x;
            self.canvas.y = canvas.y;
            self.canvas.width = canvas.width;
            self.canvas.height = canvas.height;
            if let Some(restored) = restored {
                self.shared_widgets
                    .borrow_mut()
                    .clone_from(&restored.widgets);
            }
            let mut settings = self.settings.borrow_mut();
            settings.x = canvas.x;
            settings.y = canvas.y;
            settings.width = canvas.width;
            settings.height = canvas.height;
            drop(settings);
            self.set_editor_geometry(true);
        }
        self.update_draft();
        if let Some(update) = self.native_update.borrow().as_ref() {
            update();
        }
    }

    fn sync_workspace_ui(&self) {
        self.workspace_ui
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .ui = EditorWorkspaceUi {
            panel_open: self.panel_open.get(),
            tab: self.editor_tab.get(),
            output_open: self.output_open.get(),
            manage_open: self.manage_open.get(),
            widget_add_open: self.widget_add_open.get(),
        };
    }
    fn place_widget(&mut self, kind: scorepeek_overlay_ui::WidgetKind, x: f64, y: f64) {
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
        self.persist_canvas();
    }
    fn update_selected_widget_setting(
        &mut self,
        history_count: Option<u32>,
        graph_months: Option<u32>,
    ) {
        let selected = self.selected.borrow().clone();
        if let Some(widget) = self
            .shared_widgets
            .borrow_mut()
            .iter_mut()
            .find(|widget| Some(&widget.id) == selected.as_ref())
        {
            if let Some(value) = history_count {
                widget.settings.history_count = value;
            }
            if let Some(value) = graph_months {
                widget.settings.graph_months = value;
            }
        }
        self.persist_canvas();
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
            drop(workspace);
            self.preview
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
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
        {
            let mut report = self.report.borrow_mut();
            report.logical_size = Some(logical);
            report.physical_size = Some(physical);
            report.scale_120 = scale_120;
        }
        self.document.inner.borrow_mut().set_viewport(Viewport::new(
            width,
            height,
            f32::from(u16::try_from(scale_120).map_err(|e| e.to_string())?) / 120.0,
            ColorScheme::Dark,
        ));
        if self.renderer.is_active() {
            self.renderer.set_size(width, height);
        } else {
            self.renderer
                .resume(self.shell.handles(), width, height, || {});
            if !self.renderer.complete_resume() {
                return Err("gpu_adapter".into());
            }
            let device = self.renderer.current_device_handle().ok_or("gpu_adapter")?;
            let info = device.adapter.get_info();
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
        self.paint()
    }
    fn paint(&mut self) -> Result<(), String> {
        if !self.renderer.is_active() {
            return Err("native renderer inactive".into());
        }
        let mut inner = self.document.inner.borrow_mut();
        inner.resolve(self.started.elapsed().as_secs_f64());
        // Embedded images complete synchronously during resolve; ingest them before painting.
        inner.handle_messages();
        self.animating = inner.is_animating();
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
        let workspace_target = self
            .preview
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if release_backend_on_drop(self.editing.get(), workspace_target.as_deref()) {
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

const fn release_backend_on_drop(editing: bool, workspace_target: Option<&str>) -> bool {
    editing && workspace_target.is_none()
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

/// Registers embedded artwork and the Latin font, preserving Japanese system fallbacks.
#[must_use]
pub fn document_config() -> DocumentConfig {
    let mut font_ctx = blitz_dom::FontContext::default();
    font_ctx
        .collection
        .register_fonts(peniko::Blob::new(Arc::new(OXANIUM)), None);
    DocumentConfig {
        font_ctx: Some(font_ctx),
        base_url: Some("http://scorepeek.invalid/".into()),
        net_provider: Some(Arc::new(EmbeddedSkinAssets)),
        ..DocumentConfig::default()
    }
}

#[cfg(test)]
mod skin_tests {
    use super::*;

    #[test]
    fn surface_handoff_keeps_the_backend_workspace_lease() {
        assert!(!release_backend_on_drop(true, Some("wayland-result")));
        assert!(release_backend_on_drop(true, None));
        assert!(!release_backend_on_drop(false, None));
    }
    use scorepeek_overlay_ui::Skin;

    #[test]
    fn embedded_artwork_decodes_with_the_native_png_feature() {
        for (path, bytes) in scorepeek_overlay_ui::SKIN_ASSETS {
            let image = image::load_from_memory(bytes).expect(path);
            assert!(image.width() >= 1024 && image.height() >= 768, "{path}");
        }
    }

    #[test]
    fn every_skin_lays_out_all_initial_widgets_and_stops_animating() {
        for skin in [Skin::CyanSystem, Skin::ResultAurora, Skin::DjBlackbox] {
            let mut document = DioxusDocument::new(
                VirtualDom::new_with_props(
                    native_overlay,
                    NativeOverlayProps {
                        appearance: Rc::new(Cell::new(Appearance { skin })),
                        widgets: Rc::new(RefCell::new(scorepeek_overlay_ui::default_widgets())),
                        editing: Rc::new(Cell::new(false)),
                        actual_preview: Rc::new(Cell::new(false)),
                        panel_open: Rc::new(Cell::new(true)),
                        editor_tab: Rc::new(Cell::new(EditorTab::Widgets)),
                        output_open: Rc::new(Cell::new(false)),
                        manage_open: Rc::new(Cell::new(false)),
                        widget_add_open: Rc::new(Cell::new(false)),
                        dirty: Rc::new(Cell::new(false)),
                        selected: Rc::new(RefCell::new(None)),
                        pending_widget: Rc::new(Cell::new(None)),
                        pending_point: Rc::new(Cell::new([0.0, 0.0])),
                        pending_delete: Rc::new(RefCell::new(None)),
                        managed: Rc::new(RefCell::new(Vec::new())),
                        outputs: Rc::new(RefCell::new(Vec::new())),
                        state: Rc::new(RefCell::new(OverlayState::default())),
                        visible: Rc::new(Cell::new(true)),
                        settings: Rc::new(RefCell::new(NativeCanvasSettings {
                            id: "test".into(),
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
                        update: Rc::new(RefCell::new(None)),
                    },
                ),
                document_config(),
            );
            document.initial_build();
            let mut inner = document.inner.borrow_mut();
            inner.set_viewport(Viewport::new(560, 1040, 1.25, ColorScheme::Dark));
            inner.resolve(0.0);
            inner.resolve(1.0);
            assert!(!inner.is_animating(), "{skin:?}");
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
                    actual_preview: Rc::new(Cell::new(false)),
                    panel_open: Rc::new(Cell::new(true)),
                    editor_tab: Rc::new(Cell::new(EditorTab::Widgets)),
                    output_open: Rc::new(Cell::new(false)),
                    manage_open: Rc::new(Cell::new(false)),
                    widget_add_open: Rc::new(Cell::new(false)),
                    dirty: Rc::new(Cell::new(false)),
                    selected: Rc::new(RefCell::new(None)),
                    pending_widget: Rc::new(Cell::new(None)),
                    pending_point: Rc::new(Cell::new([0.0, 0.0])),
                    pending_delete: Rc::new(RefCell::new(None)),
                    managed: Rc::new(RefCell::new(Vec::new())),
                    outputs: Rc::new(RefCell::new(Vec::new())),
                    state: Rc::new(RefCell::new(OverlayState::default())),
                    visible: Rc::new(Cell::new(true)),
                    settings: Rc::new(RefCell::new(NativeCanvasSettings {
                        id: "test".into(),
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
                    update: Rc::new(RefCell::new(None)),
                },
            ),
            document_config(),
        );
        document.initial_build();
        let mut inner = document.inner.borrow_mut();
        inner.set_viewport(Viewport::new(560, 72, 1.0, ColorScheme::Dark));
        inner.resolve(0.0);
        inner.resolve(1.0);

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
    fn output_picker_stays_inside_the_overlay_panel_at_actual_canvas_coordinates() {
        let canvas = scorepeek_overlay_ui::CanvasPresentation {
            id: "wayland-selection".into(),
            enabled: true,
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
        let mut document = DioxusDocument::new(
            VirtualDom::new_with_props(
                native_overlay,
                NativeOverlayProps {
                    appearance: Rc::new(Cell::new(Appearance {
                        skin: Skin::CyanSystem,
                    })),
                    widgets: Rc::new(RefCell::new(Vec::new())),
                    editing: Rc::new(Cell::new(true)),
                    actual_preview: Rc::new(Cell::new(false)),
                    panel_open: Rc::new(Cell::new(true)),
                    editor_tab: Rc::new(Cell::new(EditorTab::Canvas)),
                    output_open: Rc::new(Cell::new(true)),
                    manage_open: Rc::new(Cell::new(false)),
                    widget_add_open: Rc::new(Cell::new(false)),
                    dirty: Rc::new(Cell::new(false)),
                    selected: Rc::new(RefCell::new(None)),
                    pending_widget: Rc::new(Cell::new(None)),
                    pending_point: Rc::new(Cell::new([0.0, 0.0])),
                    pending_delete: Rc::new(RefCell::new(None)),
                    managed: Rc::new(RefCell::new(vec![canvas])),
                    outputs: Rc::new(RefCell::new(vec![OutputDescription {
                        name: "DP-1".into(),
                        model: "Odyssey G9".into(),
                        logical_size: Some([1920, 1080]),
                    }])),
                    state: Rc::new(RefCell::new(OverlayState::default())),
                    visible: Rc::new(Cell::new(true)),
                    settings: Rc::new(RefCell::new(NativeCanvasSettings {
                        id: "wayland-selection".into(),
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
                    update: Rc::new(RefCell::new(None)),
                },
            ),
            document_config(),
        );
        document.initial_build();
        let mut inner = document.inner.borrow_mut();
        inner.set_viewport(Viewport::new(1920, 1080, 1.0, ColorScheme::Dark));
        inner.resolve(0.0);
        inner.resolve(1.0);

        let panel = inner
            .get_client_bounding_rect(
                inner
                    .query_selector(".native-canvas-manager")
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
        let output = inner
            .get_client_bounding_rect(inner.query_selector(".output-option").unwrap().unwrap())
            .unwrap();
        let preview = inner
            .get_client_bounding_rect(inner.query_selector(".canvas-content").unwrap().unwrap())
            .unwrap();
        assert!((panel.width - 384.0).abs() < 1.0, "{panel:?}");
        assert!(output.x >= panel.x && output.x + output.width <= panel.x + panel.width);
        assert!((preview.x - 120.0).abs() < 1.0, "{preview:?}");
    }
}
