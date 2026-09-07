mod connection;
mod model;
use connection::{Command, Connection};
use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
use dioxus_web::WebEventExt;
use model::Model;
use scorepeek_overlay_ui::editor::{EditorAction, EditorPanel};
use scorepeek_overlay_ui::{CanvasPresentation, default_widget_size};
use std::rc::Rc;
use wasm_bindgen::{JsCast as _, closure::Closure};

pub fn app() -> Element {
    let mut model = use_signal(|| Model::new(read_initial(), viewport()));
    let connection = use_hook(move || Connection::new(model));
    let _resize = use_hook(move || Rc::new(ResizeListener::new(model)));
    let transport = connection.clone();
    let action = Callback::new(move |action: EditorAction| match action {
        EditorAction::Save if !model.read().readonly => transport.send(Command::Save),
        EditorAction::Discard if !model.read().readonly && !model.read().discard_pending => {
            model.write().discard_pending = true;
            transport.send(Command::Discard);
        }
        EditorAction::Close => transport.send(Command::Close),
        other => {
            let changed = model.write().action(&other);
            if changed {
                transport.send(Command::Update);
            }
        }
    });
    let transport = connection.clone();
    let enter = Callback::new(move |canvas: Option<String>| {
        if !model.read().editing {
            model.write().enter(canvas);
            transport.send(Command::Acquire);
        }
    });
    let transport = connection.clone();
    let end = Callback::new(move |(): ()| {
        let changed = model.write().end_drag();
        if changed {
            transport.send(Command::Update);
        }
    });
    let transport = connection.clone();
    let place = Callback::new(move |point: [i32; 2]| {
        let changed = model.write().place(point);
        if changed {
            transport.send(Command::Update);
        }
    });
    let start = Callback::new(
        move |(event, canvas, widget, corner): (
            PointerEvent,
            String,
            Option<String>,
            Option<String>,
        )| {
            if model.read().readonly || model.read().placing.is_some() {
                return;
            }
            let expected = if widget.is_none() && corner.is_none() {
                MouseButton::Secondary
            } else {
                MouseButton::Primary
            };
            if event.trigger_button() != Some(expected) {
                return;
            }
            event.prevent_default();
            event.stop_propagation();
            if let Some(raw) = event.data().try_as_web_event()
                && let Some(target) = raw
                    .target()
                    .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            {
                let _ = target.set_pointer_capture(raw.pointer_id());
            }
            let point = event.data().as_web_event();
            model
                .write()
                .begin_drag(canvas, widget, corner, [point.client_x(), point.client_y()]);
        },
    );
    let state = model.read().clone();
    let title_input = title_field(model, action);
    rsx! {
        div { class:"obs-workspace",
            onkeydown:move |event| {if event.key()==Key::Escape {model.write().placing=None;}},
            oncontextmenu:move |event| {event.prevent_default();enter.call(None);},
            onpointermove:move |event| {if model.read().drag.is_some()||model.read().placing.is_some(){let point=event.data().as_web_event();model.write().move_pointer([point.client_x(),point.client_y()]);}},
            onpointerup:move |_| end.call(()),
            onpointercancel:move |_| {let original=model.write().drag.take().map(|drag|drag.original);if let Some(original)=original{model.write().draft=original;}},
            div { id:"stage", onclick:move |event| {let point=event.data().as_web_event();place.call([point.client_x(),point.client_y()]);},
                for canvas in state.draft.iter().filter(|canvas|state.visible(canvas)) {
                    StageCanvas {key:"{canvas.id}",canvas:canvas.clone(),editing:state.editing,selected:state.editing&&state.selected_canvas.as_deref()==Some(canvas.id.as_str()),selected_widget:state.selected_widget.clone(),sample:state.editing&&state.chrome.sample,generation:state.generation,onstart:start,onenter:enter}
                }
            }
            if state.editing {EditorPanel {view:state.view(),title_input,onaction:action}}
            if let Some(kind)=state.placing {if state.editing {
                div {class:"placement-ghost",style:format!("left:{}px;top:{}px;width:{}px;height:{}px",state.point[0],state.point[1],default_widget_size(kind).0,default_widget_size(kind).1)}
            }}
            if let Some(notice)=state.notice {div {id:"notice",class:"show error","{notice}"}}
        }
    }
}

type DragStart = (PointerEvent, String, Option<String>, Option<String>);
#[component]
fn StageCanvas(
    canvas: CanvasPresentation,
    editing: bool,
    selected: bool,
    selected_widget: Option<String>,
    sample: bool,
    generation: u64,
    onstart: EventHandler<DragStart>,
    onenter: EventHandler<Option<String>>,
) -> Element {
    let id = canvas.id.clone();
    let context_id = canvas.id.clone();
    rsx! {
        div { class:if selected{"stage-canvas selected"}else{"stage-canvas"},"data-canvas":"{canvas.id}",style:format!("left:{}px;top:{}px;width:{}px;height:{}px",canvas.x,canvas.y,canvas.width,canvas.height),
            oncontextmenu:move |event|{event.prevent_default();event.stop_propagation();onenter.call(Some(context_id.clone()));},
            onpointerdown:move |event|{if editing{onstart.call((event,id.clone(),None,None));}},
            iframe {src:format!("/canvas/{}?sample={}&presentation={generation}",encode_id(&canvas.id),u8::from(sample)),tabindex:-1},
            if selected {
                for widget in &canvas.widgets {
                    div {key:"{widget.id}",class:if selected_widget.as_deref()==Some(widget.id.as_str()){"stage-widget-hit selected"}else{"stage-widget-hit"},"data-widget":"{widget.id}",title:"{widget.id}",style:format!("left:{}px;top:{}px;width:{}px;height:{}px",widget.x,widget.y,widget.width,widget.height),
                        onpointerdown:{let canvas=canvas.id.clone();let widget=widget.id.clone();move |event|onstart.call((event,canvas.clone(),Some(widget.clone()),None))},
                        for corner in ["nw","ne","sw","se"] {i {class:"resize-handle {corner}",onpointerdown:{let canvas=canvas.id.clone();let widget=widget.id.clone();move |event|onstart.call((event,canvas.clone(),Some(widget.clone()),Some(corner.into())))}}}
                    }
                }
                for corner in ["nw","ne","sw","se"] {i {class:"resize-handle canvas-resize {corner}",onpointerdown:{let canvas=canvas.id.clone();move |event|onstart.call((event,canvas.clone(),None,Some(corner.into())))}}}
            }
        }
    }
}
fn encode_id(id: &str) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::new();
    for byte in id.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}
fn read_initial() -> Vec<CanvasPresentation> {
    web_sys::window()
        .and_then(|window| window.document())
        .and_then(|doc| doc.get_element_by_id("scorepeek-stage"))
        .and_then(|node| node.text_content())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}
fn viewport() -> [u32; 2] {
    web_sys::window()
        .and_then(|window| window.document())
        .and_then(|doc| doc.document_element())
        .map_or([1920, 1080], |element| {
            [
                u32::try_from(element.client_width()).unwrap_or(1920),
                u32::try_from(element.client_height()).unwrap_or(1080),
            ]
        })
}
struct ResizeListener(Closure<dyn FnMut(web_sys::Event)>);
impl ResizeListener {
    fn new(mut model: Signal<Model>) -> Self {
        let callback =
            Closure::wrap(
                Box::new(move |_: web_sys::Event| model.write().viewport = viewport())
                    as Box<dyn FnMut(_)>,
            );
        if let Some(window) = web_sys::window() {
            let _ = window
                .add_event_listener_with_callback("resize", callback.as_ref().unchecked_ref());
        }
        Self(callback)
    }
}
impl Drop for ResizeListener {
    fn drop(&mut self) {
        if let Some(window) = web_sys::window() {
            let _ = window
                .remove_event_listener_with_callback("resize", self.0.as_ref().unchecked_ref());
        }
    }
}

fn title_field(mut model: Signal<Model>, action: EventHandler<EditorAction>) -> Element {
    let state = model.read().clone();
    rsx! { if let Some(title)=&state.title {
        input { class:"empty-title-edit", r#type:"text", "aria-label":"Widget title", value:"{title.text}", disabled:state.readonly,
            onmounted:move |event| async move {let _=event.set_focus(true).await;},
            oninput:move |event| {if let Some(title)=model.write().title.as_mut(){title.text=event.value();}},
            oncompositionstart:move |_| {if let Some(title)=model.write().title.as_mut(){title.composing=true;}},
            oncompositionend:move |_| {if let Some(title)=model.write().title.as_mut(){title.composing=false;}},
            onkeydown:move |event| {if event.key()==Key::Escape{event.prevent_default();action.call(EditorAction::CancelTitle);}else if event.key()==Key::Enter&&!event.is_composing(){event.prevent_default();action.call(EditorAction::AcceptTitle);}},
        }
    } }
}
