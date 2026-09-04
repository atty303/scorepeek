use dioxus::prelude::*;
use scorepeek_overlay_ui::{Appearance, OverlayState, overlay_panel};
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};
use wasm_bindgen::{JsCast as _, closure::Closure};

fn main() {
    dioxus_web::launch::launch_cfg(app, dioxus_web::Config::default());
}

fn app() -> Element {
    let appearance = use_hook(read_appearance);
    let Ok(appearance) = appearance else {
        return rsx! { p { "表示設定を読み込めません。ページを再読み込みしてください。" } };
    };
    let mut state = use_signal(OverlayState::default);
    let _connection = use_hook(move || BrowserConnection::new(move |next| state.set(next)));
    overlay_panel(&state.read(), appearance)
}

fn read_appearance() -> Result<Appearance, String> {
    let element = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| {
            document
                .query_selector("meta[name='scorepeek-appearance']")
                .ok()
                .flatten()
        })
        .ok_or("missing appearance")?;
    Ok(Appearance {
        skin: element
            .get_attribute("data-skin")
            .ok_or("missing skin")?
            .parse()?,
        layout: element
            .get_attribute("data-layout")
            .ok_or("missing layout")?
            .parse()?,
    })
}

struct BrowserConnection {
    socket: RefCell<Option<web_sys::WebSocket>>,
    latest: RefCell<OverlayState>,
    publish: RefCell<Box<dyn FnMut(OverlayState)>>,
    frame_id: Cell<Option<i32>>,
    interval_id: Cell<Option<i32>>,
    message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    close: Closure<dyn FnMut(web_sys::Event)>,
    frame: Closure<dyn FnMut(f64)>,
    reconnect: Closure<dyn FnMut()>,
}

impl BrowserConnection {
    fn new(publish: impl FnMut(OverlayState) + 'static) -> Rc<Self> {
        let connection = Rc::new_cyclic(|weak: &Weak<Self>| {
            let owner = weak.clone();
            let message = Closure::new(move |event: web_sys::MessageEvent| {
                let Some(owner) = owner.upgrade() else {
                    return;
                };
                let Some(text) = event.data().as_string() else {
                    return;
                };
                let Ok(next) = serde_json::from_str::<OverlayState>(&text) else {
                    return;
                };
                *owner.latest.borrow_mut() = next;
                owner.schedule();
            });
            let owner = weak.clone();
            let close = Closure::new(move |_: web_sys::Event| {
                if let Some(owner) = owner.upgrade() {
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
                }
            });
            Self {
                socket: RefCell::new(None),
                latest: RefCell::new(OverlayState::default()),
                publish: RefCell::new(Box::new(publish)),
                frame_id: Cell::new(None),
                interval_id: Cell::new(None),
                message,
                close,
                frame,
                reconnect,
            }
        });
        connection.connect();
        if let Some(window) = web_sys::window() {
            connection.interval_id.set(
                window
                    .set_interval_with_callback_and_timeout_and_arguments_0(
                        connection.reconnect.as_ref().unchecked_ref(),
                        1000,
                    )
                    .ok(),
            );
        }
        connection
    }

    fn connect(&self) {
        if self
            .socket
            .borrow()
            .as_ref()
            .is_some_and(|socket| socket.ready_state() < web_sys::WebSocket::CLOSING)
        {
            return;
        }
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(host) = window.location().host() else {
            return;
        };
        let Ok(socket) = web_sys::WebSocket::new(&format!("ws://{host}/ws")) else {
            return;
        };
        socket.set_onmessage(Some(self.message.as_ref().unchecked_ref()));
        socket.set_onclose(Some(self.close.as_ref().unchecked_ref()));
        if let Some(previous) = self.socket.replace(Some(socket)) {
            previous.set_onmessage(None);
            previous.set_onclose(None);
        }
    }

    fn schedule(&self) {
        if self.frame_id.get().is_none()
            && let Some(window) = web_sys::window()
        {
            self.frame_id.set(
                window
                    .request_animation_frame(self.frame.as_ref().unchecked_ref())
                    .ok(),
            );
        }
    }
}

impl Drop for BrowserConnection {
    fn drop(&mut self) {
        if let Some(socket) = self.socket.take() {
            socket.set_onmessage(None);
            socket.set_onclose(None);
            let _ = socket.close();
        }
        if let Some(window) = web_sys::window() {
            if let Some(id) = self.frame_id.get() {
                let _ = window.cancel_animation_frame(id);
            }
            if let Some(id) = self.interval_id.get() {
                window.clear_interval_with_handle(id);
            }
        }
    }
}
