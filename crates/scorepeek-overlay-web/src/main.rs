use dioxus::prelude::*;
use scorepeek_overlay_ui::{
    Appearance, CanvasPresentation, LampState, OverlayState, overlay_canvas,
};
use serde::Deserialize;
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};
use wasm_bindgen::{JsCast as _, closure::Closure};

fn main() {
    dioxus_web::launch::launch_cfg(app, dioxus_web::Config::default());
}

fn app() -> Element {
    let Ok(canvas) = use_hook(read_canvas) else {
        return rsx! { p { "表示設定を読み込めません。ページを再読み込みしてください。" } };
    };
    let mut state = use_signal(OverlayState::default);
    let mut available = use_signal(|| true);
    let _connection = use_hook(move || {
        Rc::new(DisplayConnection::new(
            &canvas.id,
            move |next| state.set(next),
            move || available.set(false),
        ))
    });
    let sample = sample_requested() && state().system == LampState::Inactive;
    let shown_state = if sample {
        scorepeek_overlay_ui::editor_sample_state()
    } else {
        state()
    };
    let visible = available()
        && scorepeek_overlay_ui::canvas_visible(canvas.show_on.as_deref(), shown_state.screen);
    let guidance = move |event: Event<MouseData>| {
        event.prevent_default();
        if let Some(window) = web_sys::window() {
            let _ = window.alert_with_message(
                "このURLは表示専用です。OBS Browser Sourceには /overlay を設定し、Interactionから編集してください。",
            );
        }
    };
    rsx! {
        div { class:"overlay-root", oncontextmenu:guidance,
            div { class:"canvas-content", style:if visible{"display:block"}else{"display:none"},
                {overlay_canvas(&shown_state, Appearance { skin:canvas.skin }, &canvas.widgets, false, None)}
            }
        }
    }
}

fn read_canvas() -> Result<CanvasPresentation, String> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or("document unavailable")?;
    let text = document
        .query_selector("#scorepeek-canvas")
        .map_err(|error| format!("query initial canvas: {error:?}"))?
        .and_then(|node| node.text_content())
        .ok_or("initial canvas missing")?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

fn sample_requested() -> bool {
    web_sys::window()
        .and_then(|window| window.location().search().ok())
        .is_some_and(|query| {
            query
                .split('&')
                .any(|part| part == "?sample=1" || part == "sample=1")
        })
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    state: Option<OverlayState>,
}

type AnimationFrame = Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>;
type WeakAnimationFrame = Weak<RefCell<Option<Closure<dyn FnMut(f64)>>>>;

struct DisplayConnection {
    socket: web_sys::WebSocket,
    message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    close: Closure<dyn FnMut(web_sys::CloseEvent)>,
    frame: AnimationFrame,
    frame_id: Rc<Cell<Option<i32>>>,
}

impl DisplayConnection {
    fn new(
        canvas_id: &str,
        publish: impl FnMut(OverlayState) + 'static,
        mut unavailable: impl FnMut() + 'static,
    ) -> Result<Self, String> {
        let window = web_sys::window().ok_or("window unavailable")?;
        let location = window.location();
        let host = location.host().map_err(|_| "location host unavailable")?;
        let protocol = if location.protocol().ok().as_deref() == Some("https:") {
            "wss"
        } else {
            "ws"
        };
        let socket = web_sys::WebSocket::new(&format!("{protocol}://{host}/ws/{canvas_id}"))
            .map_err(|error| format!("websocket: {error:?}"))?;
        let pending = Rc::new(RefCell::new(None::<OverlayState>));
        let frame_id = Rc::new(Cell::new(None));
        let frame = Rc::new(RefCell::new(None::<Closure<dyn FnMut(f64)>>));
        let weak_frame: WeakAnimationFrame = Rc::downgrade(&frame);
        let pending_for_message = Rc::clone(&pending);
        let frame_id_for_message = Rc::clone(&frame_id);
        let window_for_message = window.clone();
        let mut publish_for_frame = publish;
        *frame.borrow_mut() = Some(Closure::wrap(Box::new(move |_timestamp: f64| {
            frame_id_for_message.set(None);
            if let Some(next) = pending_for_message.borrow_mut().take() {
                publish_for_frame(next);
            }
        }) as Box<dyn FnMut(f64)>));
        let pending_for_event = Rc::clone(&pending);
        let frame_id_for_event = Rc::clone(&frame_id);
        let message = Closure::wrap(Box::new(move |event: web_sys::MessageEvent| {
            let Some(text) = event.data().as_string() else {
                return;
            };
            let Ok(envelope) = serde_json::from_str::<Envelope>(&text) else {
                return;
            };
            if envelope.kind == "canvas_unavailable" {
                unavailable();
                return;
            }
            let Some(next) = envelope.state else { return };
            *pending_for_event.borrow_mut() = Some(next);
            if frame_id_for_event.get().is_none()
                && let Some(frame) = weak_frame.upgrade()
                && let Some(callback) = frame.borrow().as_ref()
                && let Ok(id) =
                    window_for_message.request_animation_frame(callback.as_ref().unchecked_ref())
            {
                frame_id_for_event.set(Some(id));
            }
        }) as Box<dyn FnMut(web_sys::MessageEvent)>);
        socket.set_onmessage(Some(message.as_ref().unchecked_ref()));
        let close =
            Closure::wrap(Box::new(move |_event: web_sys::CloseEvent| {}) as Box<dyn FnMut(_)>);
        socket.set_onclose(Some(close.as_ref().unchecked_ref()));
        Ok(Self {
            socket,
            message,
            close,
            frame,
            frame_id,
        })
    }
}

impl Drop for DisplayConnection {
    fn drop(&mut self) {
        self.socket.set_onmessage(None);
        self.socket.set_onclose(None);
        let _ = (&self.message, &self.close, &self.frame);
        if let Some(id) = self.frame_id.get()
            && let Some(window) = web_sys::window()
        {
            let _ = window.cancel_animation_frame(id);
        }
    }
}
