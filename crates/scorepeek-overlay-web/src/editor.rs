mod connection;
mod model;
use connection::{Command, Connection};
use dioxus::prelude::*;
use dioxus_web::WebEventExt;
use model::Model;
use scorepeek_overlay_ui::CanvasPresentation;
use scorepeek_overlay_ui::editor::{EditorAction, EditorPanel};
use scorepeek_overlay_ui::editor_surface::{
    EditorCanvas, EditorSurface, PlacementPreview, SurfaceAction,
};
use std::rc::Rc;
use wasm_bindgen::{JsCast as _, closure::Closure};

pub fn app() -> Element {
    let mut model = use_signal(|| Model::new(read_initial(), viewport(), "obs"));
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
    let surface = Callback::new(move |action: SurfaceAction| {
        if let SurfaceAction::Enter(canvas) = action {
            if !model.read().editing {
                model.write().enter(canvas);
                transport.send(Command::Acquire);
            }
        } else if model.write().surface(action) {
            transport.send(Command::Update);
        }
    });
    let capture = Callback::new(move |event: PointerEvent| {
        let raw = event.data().as_web_event();
        if let Some(target) = raw
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        {
            let _ = target.set_pointer_capture(raw.pointer_id());
        }
    });
    let state = model.read().clone();
    let title_input = title_field(model, action);
    rsx! {
        EditorSurface {onaction:surface,
            div { id:"stage",
                for canvas in state.draft.iter().filter(|canvas|state.visible(canvas)) {
                    EditorCanvas {key:"{canvas.id}",canvas:canvas.clone(),editing:state.editing,selected:state.editing&&state.selected_canvas.as_deref()==Some(canvas.id.as_str()),selected_widget:state.selected_widget.clone(),onaction:surface,oncapture:capture,
                        iframe {src:format!("/canvas/{}?sample={}&presentation={}",encode_id(&canvas.id),u8::from(state.editing&&state.chrome.sample),state.generation),tabindex:-1}
                    }
                }
            }
            if state.editing {EditorPanel {view:state.view(),title_input,onaction:action}}
            if let Some(kind)=state.placing {if state.editing {
                PlacementPreview {kind,point:state.point.map(f64::from)}
            }}
            if let Some(notice)=state.notice {div {id:"notice",class:"show error","{notice}"}}
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
