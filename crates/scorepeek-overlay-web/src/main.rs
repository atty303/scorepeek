use dioxus::prelude::*;
use scorepeek_overlay_ui::{
    Appearance, CanvasPresentation, OverlayState, WidgetKind, WidgetLayout, WidgetSettings,
    overlay_canvas,
};
use serde::Deserialize;
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    time::{SystemTime, UNIX_EPOCH},
};
use wasm_bindgen::{JsCast as _, closure::Closure};

fn main() {
    dioxus_web::launch::launch_cfg(app, dioxus_web::Config::default());
}

#[derive(Clone)]
struct Drag {
    id: String,
    start: [f64; 2],
    original: WidgetLayout,
    resize: bool,
}

#[allow(clippy::too_many_lines)]
fn app() -> Element {
    let Ok(initial) = use_hook(read_canvas) else {
        return rsx! { p { "表示設定を読み込めません。ページを再読み込みしてください。" } };
    };
    let mut canvas = use_signal(|| initial);
    let mut state = use_signal(OverlayState::default);
    let mut editing = use_signal(|| false);
    let mut readonly = use_signal(|| true);
    let mut selected = use_signal(|| None::<String>);
    let mut drag = use_signal(|| None::<Drag>);
    let mut pending_new = use_signal(|| None::<WidgetKind>);
    let mut managed_canvases = use_signal(Vec::<CanvasSummary>::new);
    let mut backend_revision = use_signal(|| 0_u64);
    let mut settings_revision = use_signal(|| 0_u64);
    let mut unknown_grace_ms = use_signal(|| 1000_u32);
    let editor_id = use_hook(|| {
        format!(
            "browser-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )
    });
    let mut server_canvas = canvas;
    let mut unavailable_canvas = canvas;
    let mut unavailable_editing = editing;
    let connection = use_hook(move || {
        BrowserConnection::new(
            canvas(),
            move |next| state.set(next),
            move |request, response| {
                if control_updates_readonly(request) {
                    readonly.set(response.readonly || !response.ok);
                }
                if let Some(next) = response.canvas {
                    server_canvas.set(next);
                }
                if let Some(revision) = response.backend_revision {
                    backend_revision.set(revision);
                    managed_canvases.set(response.canvases);
                }
                if let Some(revision) = response.settings_revision {
                    settings_revision.set(revision);
                }
                if let Some(value) = response.unknown_grace_ms {
                    unknown_grace_ms.set(value);
                }
            },
            move || {
                let mut next = unavailable_canvas();
                next.widgets.clear();
                unavailable_canvas.set(next);
                unavailable_editing.set(false);
            },
        )
    });
    let enter = {
        let connection = Rc::clone(&connection);
        let editor_id = editor_id.clone();
        move |event: Event<MouseData>| {
            event.prevent_default();
            editing.set(true);
            connection.acquire(&editor_id);
            connection.list_canvases();
            notify_parent("editing", &canvas().id);
        }
    };
    let done = {
        let connection = Rc::clone(&connection);
        let editor_id = editor_id.clone();
        move |_| {
            connection.release(&editor_id);
            editing.set(false);
            selected.set(None);
            drag.set(None);
            notify_parent("done", &canvas().id);
        }
    };
    let moving = move |event: Event<PointerData>| {
        let Some(active) = drag() else { return };
        if readonly() {
            return;
        }
        let p = event.client_coordinates();
        let mut next = canvas();
        let horizontal_edges: Vec<_> = next
            .widgets
            .iter()
            .filter(|other| other.id != active.id)
            .flat_map(|other| {
                [
                    other.x,
                    other
                        .x
                        .saturating_add(i32::try_from(other.width).unwrap_or(i32::MAX)),
                ]
            })
            .collect();
        let vertical_edges: Vec<_> = next
            .widgets
            .iter()
            .filter(|other| other.id != active.id)
            .flat_map(|other| {
                [
                    other.y,
                    other
                        .y
                        .saturating_add(i32::try_from(other.height).unwrap_or(i32::MAX)),
                ]
            })
            .collect();
        let [viewport_width, viewport_height] = viewport_size();
        if let Some(widget) = next.widgets.iter_mut().find(|w| w.id == active.id) {
            if active.resize {
                widget.width = snap_size(f64::from(active.original.width) + p.x - active.start[0]);
                widget.height =
                    snap_size(f64::from(active.original.height) + p.y - active.start[1]);
            } else {
                let candidate_x = snap(f64::from(active.original.x) + p.x - active.start[0]);
                let candidate_y = snap(f64::from(active.original.y) + p.y - active.start[1]);
                widget.x = magnetic_snap(
                    candidate_x,
                    widget.width,
                    viewport_width,
                    horizontal_edges.into_iter(),
                );
                widget.y = magnetic_snap(
                    candidate_y,
                    widget.height,
                    viewport_height,
                    vertical_edges.into_iter(),
                );
            }
        }
        canvas.set(next);
    };
    let finish = {
        let connection = Rc::clone(&connection);
        let editor_id = editor_id.clone();
        move |_| {
            if drag().is_some() && !readonly() {
                connection.replace(&editor_id, &canvas());
            }
            drag.set(None);
        }
    };
    let drop_widget = {
        let connection = Rc::clone(&connection);
        let editor_id = editor_id.clone();
        move |event: Event<DragData>| {
            event.prevent_default();
            let Some(kind) = pending_new() else { return };
            if readonly() {
                return;
            }
            let p = event.client_coordinates();
            let mut next = canvas();
            let id = scorepeek_overlay_ui::next_widget_id(kind, &next.widgets);
            let (width, height) = scorepeek_overlay_ui::default_widget_size(kind);
            let z = next
                .widgets
                .iter()
                .map(|w| w.z)
                .max()
                .unwrap_or(0)
                .saturating_add(1);
            next.widgets.push(WidgetLayout {
                id: id.clone(),
                kind,
                x: snap(p.x),
                y: snap(p.y),
                width,
                height,
                z,
                settings: WidgetSettings::default(),
            });
            selected.set(Some(id));
            canvas.set(next.clone());
            connection.replace(&editor_id, &next);
            pending_new.set(None);
        }
    };
    let selected_id = selected();
    let selected_widget = selected_id.as_ref().and_then(|id| {
        canvas()
            .widgets
            .iter()
            .find(|widget| &widget.id == id)
            .cloned()
    });
    let shown = editing()
        || scorepeek_overlay_ui::canvas_visible(canvas().show_on.as_deref(), state().screen);
    rsx! {
        div { class:"overlay-root",oncontextmenu:enter,onpointermove:moving,onpointerup:finish,ondragover:move|event|event.prevent_default(),ondrop:drop_widget,
            div { style:if shown{"display:block"}else{"display:none"},
                {overlay_canvas(&state.read(),Appearance{skin:canvas().skin},&canvas().widgets,editing(),selected_id.as_deref())}
            }
            if editing() {
                div { class:"editor-toolbar",strong{"CANVAS EDIT"}
                    for (label,skin) in [("CYAN",scorepeek_overlay_ui::Skin::CyanSystem),("AURORA",scorepeek_overlay_ui::Skin::ResultAurora),("BLACKBOX",scorepeek_overlay_ui::Skin::DjBlackbox)] { button { disabled:readonly(),onclick:{let connection=Rc::clone(&connection);let editor_id=editor_id.clone();move|_|{let mut next=canvas();next.skin=skin;canvas.set(next.clone());connection.replace(&editor_id,&next);}},"{label}"} }
                    span { class:"editor-state",if readonly(){"VIEW ONLY"}else{"EDITING"} } button { onclick:done,"DONE" }
                }
                div { class:"widget-palette",strong{"WIDGETS"} for kind in [WidgetKind::Status,WidgetKind::Selection,WidgetKind::Score,WidgetKind::HistoryList,WidgetKind::HistoryGraph] { button { draggable:!readonly(),ondragstart:move|_|pending_new.set(Some(kind)),"{kind_name(kind)}" } } }
                div { class:"canvas-manager", strong { "OBS CANVASES" }
                    for item in managed_canvases() { a { href:format!("/canvas/{}",item.id), class:if item.id==canvas().id{"current"}else{""}, onclick:{let id=item.id.clone();move|event|if is_framed(){event.prevent_default();notify_parent("select",&id)}}, "{item.id}" } }
                    button { disabled:readonly(), onclick:{let connection=Rc::clone(&connection);move|_|connection.add_canvas(backend_revision())}, "ADD EMPTY" }
                    button { disabled:readonly() || managed_canvases().len() <= 1, onclick:{let connection=Rc::clone(&connection);let id=canvas().id.clone();move|_|connection.delete_canvas(backend_revision(),&id)}, "DELETE CURRENT" }
                    button { disabled:readonly(), onclick:{let connection=Rc::clone(&connection);let id=canvas().id.clone();let enabled=managed_canvases().iter().find(|item|item.id==id).is_none_or(|item|item.enabled);move|_|connection.set_enabled(backend_revision(),&id,!enabled)}, "ENABLE / DISABLE" }
                }
                for widget in canvas().widgets {
                    div { key:"edit-{widget.id}",class:"editor-hitbox",style:format!("left:{}px;top:{}px;width:{}px;height:{}px;z-index:{}",widget.x,widget.y,widget.width,widget.height,widget.z.saturating_add(1000)),
                        onpointerdown:{let original=widget.clone();move|event:Event<PointerData>|{event.stop_propagation();if readonly(){return}let p=event.client_coordinates();selected.set(Some(original.id.clone()));let mut next=canvas();let top=next.widgets.iter().map(|w|w.z).max().unwrap_or(0).saturating_add(1);if let Some(w)=next.widgets.iter_mut().find(|w|w.id==original.id){w.z=top}canvas.set(next);drag.set(Some(Drag{id:original.id.clone(),start:[p.x,p.y],original:original.clone(),resize:false}));}},span{"{widget.id}"}
                        i { class:"editor-resize",onpointerdown:{let original=widget.clone();move|event:Event<PointerData>|{event.stop_propagation();if readonly(){return}let p=event.client_coordinates();selected.set(Some(original.id.clone()));drag.set(Some(Drag{id:original.id.clone(),start:[p.x,p.y],original:original.clone(),resize:true}));}} }
                    }
                }
                div { class:"editor-inspector",strong{"INSPECTOR"} if let Some(widget)=selected_widget { span{"{widget.id}"}
                    if widget.kind == WidgetKind::HistoryList { span { "ROWS" } div { class:"setting-options", for value in [5,10,20,50] { button { disabled:readonly() || widget.settings.history_count == value, onclick:{let connection=Rc::clone(&connection);let editor_id=editor_id.clone();let id=widget.id.clone();move|_|{let mut next=canvas();if let Some(w)=next.widgets.iter_mut().find(|w|w.id==id){w.settings.history_count=value}canvas.set(next.clone());connection.replace(&editor_id,&next);}},"{value}" } } } }
                    if widget.kind == WidgetKind::HistoryGraph { span { "RANGE" } div { class:"setting-options", for value in [1,3,6,12] { button { disabled:readonly() || widget.settings.graph_months == value, onclick:{let connection=Rc::clone(&connection);let editor_id=editor_id.clone();let id=widget.id.clone();move|_|{let mut next=canvas();if let Some(w)=next.widgets.iter_mut().find(|w|w.id==id){w.settings.graph_months=value}canvas.set(next.clone());connection.replace(&editor_id,&next);}},"{value}M" } } } }
                    button { disabled:readonly(),onclick:{let connection=Rc::clone(&connection);let editor_id=editor_id.clone();let id=widget.id.clone();move|_|{let mut next=canvas();next.widgets.retain(|w|w.id!=id);canvas.set(next.clone());selected.set(None);connection.replace(&editor_id,&next);}},"REMOVE" }
                    button { disabled:readonly(),onclick:{let connection=Rc::clone(&connection);let editor_id=editor_id.clone();let id=widget.id.clone();move|_|{let mut next=canvas();if let Some(w)=next.widgets.iter_mut().find(|w|w.id==id){w.x=w.x.max(0);w.y=w.y.max(0)}canvas.set(next.clone());connection.replace(&editor_id,&next);}},"キャンバス内へ戻す" }
                } else { span { "CANVAS" } }
                    span { "SHOW ON" }
                    div { class:"setting-options screens", for (label,kind) in [("SELECT",scorepeek_overlay_ui::ScreenKind::MusicSelect),("MODE",scorepeek_overlay_ui::ScreenKind::ModeSelect),("DECIDE",scorepeek_overlay_ui::ScreenKind::DecideTransition),("PLAY",scorepeek_overlay_ui::ScreenKind::Play),("RESULT",scorepeek_overlay_ui::ScreenKind::Result)] { button { disabled:readonly(),class:if canvas().show_on.as_ref().is_some_and(|values|values.contains(&kind)){"active"}else{""},onclick:{let connection=Rc::clone(&connection);let editor_id=editor_id.clone();move|_|{let mut next=canvas();let mut values=next.show_on.take().unwrap_or_default();if let Some(index)=values.iter().position(|value|*value==kind){values.remove(index);}else{values.push(kind)}next.show_on=(!values.is_empty()).then_some(values);canvas.set(next.clone());connection.replace(&editor_id,&next);}},"{label}" } } }
                    button { disabled:readonly(), onclick:{let connection=Rc::clone(&connection);let editor_id=editor_id.clone();move|_|{let mut next=canvas();next.show_on=None;canvas.set(next.clone());connection.replace(&editor_id,&next);}}, "ALWAYS" }
                    div { class:"setting-options", button { disabled:readonly(),onclick:{let connection=Rc::clone(&connection);let editor_id=editor_id.clone();move|_|{let mut next=canvas();next.z=next.z.saturating_sub(1);canvas.set(next.clone());connection.replace(&editor_id,&next);}},"Z−" } button { disabled:readonly(),onclick:{let connection=Rc::clone(&connection);let editor_id=editor_id.clone();move|_|{let mut next=canvas();next.z=next.z.saturating_add(1);canvas.set(next.clone());connection.replace(&editor_id,&next);}},"Z+" } }
                    span { "UNKNOWN GRACE {unknown_grace_ms}ms" }
                    div { class:"setting-options", for value in [0,500,1000,2000] { button { disabled:readonly(),onclick:{let connection=Rc::clone(&connection);let editor_id=editor_id.clone();move|_|connection.set_unknown_grace(&editor_id,settings_revision(),value)},"{value}" } } }
                }
            }
        }
        style { "{EDITOR_CSS}" }
    }
}

#[allow(clippy::cast_possible_truncation)]
fn snap(value: f64) -> i32 {
    ((value / 4.0).round() * 4.0).clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn snap_size(value: f64) -> u32 {
    ((value.max(32.0) / 4.0).round() * 4.0).min(f64::from(u32::MAX)) as u32
}
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn viewport_size() -> [u32; 2] {
    let value = |width: bool| {
        web_sys::window()
            .and_then(|window| {
                if width {
                    window.inner_width()
                } else {
                    window.inner_height()
                }
                .ok()
            })
            .and_then(|value| value.as_f64())
            .unwrap_or(0.0)
            .clamp(0.0, f64::from(u32::MAX)) as u32
    };
    [value(true), value(false)]
}
fn magnetic_snap(
    candidate: i32,
    size: u32,
    canvas_size: u32,
    other_edges: impl Iterator<Item = i32>,
) -> i32 {
    let size = i32::try_from(size).unwrap_or(i32::MAX);
    let canvas = i32::try_from(canvas_size).unwrap_or(i32::MAX);
    let mut anchors = vec![0, canvas / 2, canvas];
    anchors.extend(other_edges);
    let points = [
        candidate,
        candidate.saturating_add(size / 2),
        candidate.saturating_add(size),
    ];
    let mut best = (7, candidate);
    for point in points {
        for anchor in &anchors {
            let distance = point.abs_diff(*anchor);
            if distance < best.0 {
                best = (distance, candidate.saturating_add(*anchor - point));
            }
        }
    }
    best.1
}
const fn kind_name(kind: WidgetKind) -> &'static str {
    match kind {
        WidgetKind::Status => "status",
        WidgetKind::Selection => "selection",
        WidgetKind::Score => "score",
        WidgetKind::HistoryList => "history-list",
        WidgetKind::HistoryGraph => "history-graph",
    }
}
fn read_canvas() -> Result<CanvasPresentation, String> {
    let element = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.query_selector("#scorepeek-canvas").ok().flatten())
        .ok_or("missing canvas")?;
    serde_json::from_str(&element.text_content().ok_or("missing canvas body")?)
        .map_err(|e| e.to_string())
}
fn notify_parent(kind: &str, canvas_id: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(Some(parent)) = window.parent() else {
        return;
    };
    let _ = parent.post_message(
        &wasm_bindgen::JsValue::from_str(&format!("scorepeek:{kind}:{canvas_id}")),
        "*",
    );
}
fn is_framed() -> bool {
    web_sys::window().is_some_and(|window| {
        window
            .parent()
            .ok()
            .flatten()
            .is_some_and(|parent| parent != window)
    })
}
const EDITOR_CSS: &str = r".native-editor-shell,.native-widget-palette,.native-editor-inspector,.native-canvas-resize{display:none}.overlay-root{width:100vw;height:100vh;overflow:hidden}.editor-toolbar,.widget-palette,.editor-inspector,.canvas-manager{position:fixed;z-index:100000;background:#071019ee;border:1px solid #27e5f3;color:#eef;padding:8px;display:flex;gap:7px;align-items:center;font:12px Oxanium,sans-serif}.editor-toolbar{left:8px;right:8px;top:8px}.editor-toolbar .editor-state{margin-left:auto}.widget-palette{left:8px;top:58px;flex-direction:column;align-items:stretch}.canvas-manager{left:8px;top:250px;width:145px;flex-direction:column;align-items:stretch}.canvas-manager a{color:#a9f7ff;text-decoration:none;overflow:hidden;text-overflow:ellipsis}.canvas-manager a.current{color:#fff;background:#147083}.editor-inspector{right:8px;top:58px;width:180px;flex-direction:column;align-items:stretch}.setting-options{display:grid;grid-template-columns:repeat(4,1fr);gap:4px}.editor-toolbar button,.widget-palette button,.editor-inspector button,.canvas-manager button{background:#122531;color:#fff;border:1px solid #65808c;padding:5px}.editor-hitbox{position:absolute;border:1px dashed #fff9;cursor:move}.editor-hitbox>span{background:#000c;padding:2px 4px;font:10px Oxanium;color:#fff}.editor-resize{position:absolute;right:0;bottom:0;width:18px;height:18px;border-right:4px solid #27e5f3;border-bottom:4px solid #27e5f3;cursor:nwse-resize}";

#[derive(Deserialize)]
struct ControlResponse {
    ok: bool,
    readonly: bool,
    canvas: Option<CanvasPresentation>,
    #[serde(default)]
    canvases: Vec<CanvasSummary>,
    backend_revision: Option<u64>,
    #[serde(default)]
    settings_revision: Option<u64>,
    #[serde(default)]
    unknown_grace_ms: Option<u32>,
}
#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ControlKind {
    Lease,
    CanvasMutation,
    CanvasList,
    CanvasManager,
    Settings,
}
const fn control_updates_readonly(request: Option<ControlKind>) -> bool {
    matches!(
        request,
        Some(ControlKind::Lease | ControlKind::CanvasMutation | ControlKind::Settings)
    )
}
fn reconcile_canvas_response(
    request: Option<ControlKind>,
    response: &mut ControlResponse,
    confirmed: &mut CanvasPresentation,
    has_pending_replacement: bool,
) {
    if response.ok
        && let Some(canvas) = &response.canvas
    {
        confirmed.clone_from(canvas);
    }
    if request == Some(ControlKind::CanvasMutation) {
        if response.ok && has_pending_replacement {
            response.canvas = None;
        } else if !response.ok {
            response.canvas = Some(confirmed.clone());
        }
    }
}
#[derive(Clone, Deserialize)]
struct CanvasSummary {
    id: String,
    enabled: bool,
}
#[derive(Clone)]
struct Replacement {
    editor: String,
    canvas: CanvasPresentation,
}
#[derive(Default)]
struct ReplacementQueue {
    in_flight: bool,
    pending: Option<Replacement>,
}
impl ReplacementQueue {
    fn enqueue(&mut self, replacement: Replacement) -> Option<Replacement> {
        if self.in_flight {
            self.pending = Some(replacement);
            None
        } else {
            self.in_flight = true;
            Some(replacement)
        }
    }

    fn complete(&mut self, ok: bool, revision: Option<u64>) -> Option<Replacement> {
        self.in_flight = false;
        if !ok {
            self.pending.take();
            return None;
        }
        let mut next = self.pending.take()?;
        if let Some(revision) = revision {
            next.canvas.revision = revision;
        }
        self.in_flight = true;
        Some(next)
    }

    fn retry(&mut self, replacement: Replacement) {
        self.in_flight = false;
        self.pending = Some(replacement);
    }

    fn ready(&mut self) -> Option<Replacement> {
        if self.in_flight {
            None
        } else {
            let next = self.pending.take()?;
            self.in_flight = true;
            Some(next)
        }
    }

    const fn busy(&self) -> bool {
        self.in_flight || self.pending.is_some()
    }
}
type ControlCallback = dyn FnMut(Option<ControlKind>, ControlResponse);
struct BrowserConnection {
    canvas_id: String,
    confirmed_canvas: RefCell<CanvasPresentation>,
    socket: RefCell<Option<web_sys::WebSocket>>,
    latest: RefCell<OverlayState>,
    publish: RefCell<Box<dyn FnMut(OverlayState)>>,
    control: RefCell<Box<ControlCallback>>,
    unavailable: RefCell<Box<dyn FnMut()>>,
    lease: RefCell<Option<String>>,
    replacements: RefCell<ReplacementQueue>,
    pending_release: RefCell<Option<String>>,
    retired: Cell<bool>,
    frame_id: Cell<Option<i32>>,
    interval_id: Cell<Option<i32>>,
    message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    close: Closure<dyn FnMut(web_sys::Event)>,
    frame: Closure<dyn FnMut(f64)>,
    reconnect: Closure<dyn FnMut()>,
}
impl BrowserConnection {
    #[allow(clippy::too_many_lines)]
    fn new(
        initial_canvas: CanvasPresentation,
        publish: impl FnMut(OverlayState) + 'static,
        control: impl FnMut(Option<ControlKind>, ControlResponse) + 'static,
        unavailable: impl FnMut() + 'static,
    ) -> Rc<Self> {
        let connection = Rc::new_cyclic(|weak: &Weak<Self>| {
            let owner = weak.clone();
            let message = Closure::new(move |event: web_sys::MessageEvent| {
                let Some(owner) = owner.upgrade() else { return };
                let Some(text) = event.data().as_string() else {
                    return;
                };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                    return;
                };
                match value["type"].as_str() {
                    Some("state") => {
                        if let Ok(next) = serde_json::from_value(value["state"].clone()) {
                            *owner.latest.borrow_mut() = next;
                            owner.schedule();
                        }
                    }
                    Some("control") => {
                        let request = serde_json::from_value(value["request"].clone()).ok();
                        if let Ok(mut response) =
                            serde_json::from_value::<ControlResponse>(value["response"].clone())
                        {
                            let next = if request == Some(ControlKind::CanvasMutation) {
                                owner.replacements.borrow_mut().complete(
                                    response.ok,
                                    response.canvas.as_ref().map(|canvas| canvas.revision),
                                )
                            } else {
                                None
                            };
                            reconcile_canvas_response(
                                request,
                                &mut response,
                                &mut owner.confirmed_canvas.borrow_mut(),
                                next.is_some(),
                            );
                            let failed_mutation =
                                request == Some(ControlKind::CanvasMutation) && !response.ok;
                            (owner.control.borrow_mut())(request, response);
                            if let Some(next) = next {
                                owner.send_replacement(next);
                            } else if request == Some(ControlKind::CanvasMutation)
                                && let Some(editor) = owner.pending_release.borrow_mut().take()
                            {
                                owner.send_release(&editor);
                            } else if failed_mutation
                                && let Some(editor) = owner.lease.borrow().clone()
                            {
                                owner.acquire(&editor);
                            }
                        }
                    }
                    Some("canvas_unavailable") => {
                        owner.retired.set(true);
                        (owner.unavailable.borrow_mut())();
                    }
                    _ => {}
                }
            });
            let owner = weak.clone();
            let close = Closure::new(move |_: web_sys::Event| {
                if let Some(owner) = owner.upgrade() {
                    if owner.retired.get() {
                        return;
                    }
                    owner.latest.borrow_mut().connected = false;
                    owner.schedule();
                }
            });
            let owner = weak.clone();
            let frame = Closure::new(move |_| {
                if let Some(owner) = owner.upgrade() {
                    owner.frame_id.set(None);
                    (owner.publish.borrow_mut())(owner.latest.borrow().clone());
                }
            });
            let owner = weak.clone();
            let reconnect = Closure::new(move || {
                if let Some(owner) = owner.upgrade() {
                    owner.connect();
                    if let Some(editor) = owner.lease.borrow().clone() {
                        owner.send(json!({"command":"keep_alive","canvas_id":owner.canvas_id,"editor_id":editor}));
                    }
                    let next = owner.replacements.borrow_mut().ready();
                    if let Some(next) = next {
                        owner.send_replacement(next);
                    }
                }
            });
            Self {
                canvas_id: initial_canvas.id.clone(),
                confirmed_canvas: RefCell::new(initial_canvas),
                socket: RefCell::new(None),
                latest: RefCell::new(OverlayState::default()),
                publish: RefCell::new(Box::new(publish)),
                control: RefCell::new(Box::new(control)),
                unavailable: RefCell::new(Box::new(unavailable)),
                lease: RefCell::new(None),
                replacements: RefCell::new(ReplacementQueue::default()),
                pending_release: RefCell::new(None),
                retired: Cell::new(false),
                frame_id: Cell::new(None),
                interval_id: Cell::new(None),
                message,
                close,
                frame,
                reconnect,
            }
        });
        connection.connect();
        if let Some(w) = web_sys::window() {
            connection.interval_id.set(
                w.set_interval_with_callback_and_timeout_and_arguments_0(
                    connection.reconnect.as_ref().unchecked_ref(),
                    5000,
                )
                .ok(),
            );
        }
        connection
    }
    fn acquire(&self, editor: &str) {
        self.pending_release.borrow_mut().take();
        *self.lease.borrow_mut() = Some(editor.into());
        self.send(json!({"command":"acquire","canvas_id":self.canvas_id,"editor_id":editor}));
    }
    fn release(&self, editor: &str) {
        self.lease.borrow_mut().take();
        if self.replacements.borrow().busy() {
            *self.pending_release.borrow_mut() = Some(editor.into());
        } else {
            self.send_release(editor);
        }
    }
    fn send_release(&self, editor: &str) {
        self.send(json!({"command":"release","canvas_id":self.canvas_id,"editor_id":editor}));
    }
    fn replace(&self, editor: &str, canvas: &CanvasPresentation) {
        let next = self.replacements.borrow_mut().enqueue(Replacement {
            editor: editor.into(),
            canvas: canvas.clone(),
        });
        if let Some(next) = next {
            self.send_replacement(next);
        }
    }
    fn send_replacement(&self, replacement: Replacement) {
        let sent = self.send(json!({"command":"replace_canvas","canvas_id":self.canvas_id,"editor_id":replacement.editor,"expected_revision":replacement.canvas.revision,"presentation":replacement.canvas}));
        if !sent {
            self.replacements.borrow_mut().retry(replacement);
        }
    }
    fn list_canvases(&self) {
        self.send(json!({"command":"list_canvases","backend":"obs"}));
    }
    fn add_canvas(&self, revision: u64) {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        self.send(json!({"command":"add_canvas","backend":"obs","expected_revision":revision,"canvas_id":format!("obs-{suffix}")}));
    }
    fn delete_canvas(&self, revision: u64, canvas_id: &str) {
        self.send(json!({"command":"delete_canvas","backend":"obs","expected_revision":revision,"canvas_id":canvas_id}));
    }
    fn set_enabled(&self, revision: u64, canvas_id: &str, enabled: bool) {
        self.send(json!({"command":"set_canvas_enabled","backend":"obs","expected_revision":revision,"canvas_id":canvas_id,"enabled":enabled}));
    }
    fn set_unknown_grace(&self, editor: &str, revision: u64, value: u32) {
        self.send(json!({"command":"set_unknown_grace","canvas_id":self.canvas_id,"editor_id":editor,"expected_revision":revision,"unknown_grace_ms":value}));
    }
    #[allow(clippy::needless_pass_by_value)]
    fn send(&self, value: serde_json::Value) -> bool {
        if let (Some(socket), Ok(text)) =
            (self.socket.borrow().as_ref(), serde_json::to_string(&value))
        {
            return socket.ready_state() == web_sys::WebSocket::OPEN
                && socket.send_with_str(&text).is_ok();
        }
        false
    }
    fn connect(&self) {
        if self.retired.get() {
            return;
        }
        if self
            .socket
            .borrow()
            .as_ref()
            .is_some_and(|s| s.ready_state() < web_sys::WebSocket::CLOSING)
        {
            return;
        }
        let Some(w) = web_sys::window() else { return };
        let Ok(host) = w.location().host() else {
            return;
        };
        let Ok(socket) = web_sys::WebSocket::new(&format!("ws://{host}/ws/{}", self.canvas_id))
        else {
            return;
        };
        socket.set_onmessage(Some(self.message.as_ref().unchecked_ref()));
        socket.set_onclose(Some(self.close.as_ref().unchecked_ref()));
        if let Some(old) = self.socket.replace(Some(socket)) {
            old.set_onmessage(None);
            old.set_onclose(None);
        }
    }
    fn schedule(&self) {
        if self.frame_id.get().is_none()
            && let Some(w) = web_sys::window()
        {
            self.frame_id.set(
                w.request_animation_frame(self.frame.as_ref().unchecked_ref())
                    .ok(),
            );
        }
    }
}
impl Drop for BrowserConnection {
    fn drop(&mut self) {
        if let Some(editor) = self.lease.take() {
            self.send_release(&editor);
        }
        if let Some(socket) = self.socket.take() {
            socket.set_onmessage(None);
            socket.set_onclose(None);
            let _ = socket.close();
        }
        if let Some(w) = web_sys::window() {
            if let Some(id) = self.frame_id.get() {
                let _ = w.cancel_animation_frame(id);
            }
            if let Some(id) = self.interval_id.get() {
                w.clear_interval_with_handle(id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replacement(revision: u64, skin: scorepeek_overlay_ui::Skin) -> Replacement {
        Replacement {
            editor: "editor".into(),
            canvas: CanvasPresentation {
                id: "obs-main".into(),
                skin,
                revision,
                show_on: None,
                opacity_percent: 100,
                z: 0,
                widgets: Vec::new(),
            },
        }
    }

    #[test]
    fn canvas_list_does_not_replace_the_lease_state() {
        assert!(control_updates_readonly(Some(ControlKind::Lease)));
        assert!(control_updates_readonly(Some(ControlKind::CanvasMutation)));
        assert!(!control_updates_readonly(Some(ControlKind::CanvasList)));
        assert!(!control_updates_readonly(Some(ControlKind::CanvasManager)));
    }

    #[test]
    fn replacement_queue_coalesces_and_advances_the_revision() {
        let mut queue = ReplacementQueue::default();
        assert!(
            queue
                .enqueue(replacement(4, scorepeek_overlay_ui::Skin::CyanSystem))
                .is_some()
        );
        assert!(
            queue
                .enqueue(replacement(4, scorepeek_overlay_ui::Skin::ResultAurora))
                .is_none()
        );
        assert!(
            queue
                .enqueue(replacement(4, scorepeek_overlay_ui::Skin::DjBlackbox))
                .is_none()
        );
        assert!(queue.busy());

        let next = queue.complete(true, Some(5)).unwrap();
        assert_eq!(next.canvas.revision, 5);
        assert_eq!(next.canvas.skin, scorepeek_overlay_ui::Skin::DjBlackbox);
        assert!(queue.complete(true, Some(6)).is_none());
        assert!(!queue.busy());
    }

    #[test]
    fn failed_replacement_restores_the_last_confirmed_canvas_before_release() {
        let mut confirmed = replacement(4, scorepeek_overlay_ui::Skin::CyanSystem).canvas;
        let mut response = ControlResponse {
            ok: false,
            readonly: true,
            canvas: None,
            canvases: Vec::new(),
            backend_revision: None,
            settings_revision: None,
            unknown_grace_ms: None,
        };

        reconcile_canvas_response(
            Some(ControlKind::CanvasMutation),
            &mut response,
            &mut confirmed,
            false,
        );

        assert_eq!(response.canvas, Some(confirmed));
    }
}
