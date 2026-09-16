use dioxus::prelude::*;
use scorepeek_overlay_ui::editor_model::{
    EditorBackendReply, EditorEffect, EditorEffectKind, EditorInput,
};
use scorepeek_overlay_ui::{CanvasPresentation, LampState, OverlayState};
use serde::Deserialize;
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};
use wasm_bindgen::{JsCast as _, closure::Closure};

pub const ASSET_VERSION: &str = env!("SCOREPEEK_OVERLAY_BUILD_ID");
const VERSION_RELOAD_KEY: &str = "scorepeek-overlay-version-reload";

#[derive(Clone, Copy, PartialEq)]
pub enum Compatibility {
    Checking,
    Ready,
    Mismatch,
}

#[derive(Deserialize)]
struct Reply {
    ok: bool,
    readonly: bool,
    error: Option<String>,
    #[serde(default)]
    canvases: Vec<CanvasPresentation>,
    #[serde(default)]
    dirty: bool,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Message {
    VersionMismatch,
    Stage {
        #[serde(default)]
        asset_version: String,
        state: Box<OverlayState>,
        canvases: Vec<CanvasPresentation>,
    },
    Control {
        request_id: u64,
        response: Reply,
    },
}
struct SocketBinding {
    socket: web_sys::WebSocket,
    _open: Closure<dyn FnMut(web_sys::Event)>,
    _message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    _close: Closure<dyn FnMut(web_sys::CloseEvent)>,
}
impl Drop for SocketBinding {
    fn drop(&mut self) {
        self.socket.set_onopen(None);
        self.socket.set_onmessage(None);
        self.socket.set_onclose(None);
        let _ = self.socket.close();
    }
}
pub struct Connection {
    dispatch: Callback<EditorInput, Vec<EditorEffect>>,
    pub compatibility: Signal<Compatibility>,
    socket: RefCell<Option<SocketBinding>>,
    pending: RefCell<BTreeMap<u64, EditorEffect>>,
    next: Cell<u64>,
    editor_id: String,
    timer: Cell<Option<i32>>,
    tick: RefCell<Option<Closure<dyn FnMut()>>>,
}
impl Connection {
    pub fn new(
        dispatch: Callback<EditorInput, Vec<EditorEffect>>,
        compatibility: Signal<Compatibility>,
    ) -> Rc<Self> {
        let editor_id = format!(
            "obs-{}",
            web_sys::window()
                .and_then(|window| window.crypto().ok())
                .map(|crypto| crypto.random_uuid())
                .unwrap_or_default()
        );
        let this = Rc::new(Self {
            dispatch,
            compatibility,
            socket: RefCell::new(None),
            pending: RefCell::new(BTreeMap::new()),
            next: Cell::new(1),
            editor_id,
            timer: Cell::new(None),
            tick: RefCell::new(None),
        });
        this.connect();
        let weak = Rc::downgrade(&this);
        let tick = Closure::wrap(Box::new(move || {
            if let Some(this) = weak.upgrade() {
                if *this.compatibility.read() == Compatibility::Mismatch {
                    return;
                }
                let ready = this
                    .socket
                    .borrow()
                    .as_ref()
                    .map(|socket| socket.socket.ready_state());
                if ready.is_none_or(|state| state == web_sys::WebSocket::CLOSED) {
                    this.connect();
                } else if *this.compatibility.read() == Compatibility::Ready {
                    this.dispatch_and_send(EditorInput::KeepAliveTick);
                }
            }
        }) as Box<dyn FnMut()>);
        if let Some(window) = web_sys::window() {
            this.timer.set(
                window
                    .set_interval_with_callback_and_timeout_and_arguments_0(
                        tick.as_ref().unchecked_ref(),
                        5000,
                    )
                    .ok(),
            );
        }
        *this.tick.borrow_mut() = Some(tick);
        this
    }
    fn connect(self: &Rc<Self>) {
        *self.compatibility.write_unchecked() = Compatibility::Checking;
        let Some(window) = web_sys::window() else {
            return;
        };
        let location = window.location();
        let Ok(host) = location.host() else {
            return;
        };
        let scheme = if location.protocol().ok().as_deref() == Some("https:") {
            "wss"
        } else {
            "ws"
        };
        let Ok(socket) = web_sys::WebSocket::new(&format!(
            "{scheme}://{host}/ws/stage?asset_version={ASSET_VERSION}"
        )) else {
            return;
        };
        let open = Closure::wrap(Box::new(move |_: web_sys::Event| {}) as Box<dyn FnMut(_)>);
        let weak = Rc::downgrade(self);
        let message = Closure::wrap(Box::new(move |event: web_sys::MessageEvent| {
            if let Some(this) = weak.upgrade()
                && let Some(text) = event.data().as_string()
            {
                let decoded = serde_json::from_str::<serde_json::Value>(&text);
                if let Ok(value) = &decoded
                    && value["type"] == "stage"
                    && value["asset_version"].as_str() != Some(ASSET_VERSION)
                {
                    this.mismatch();
                    return;
                }
                match decoded.and_then(serde_json::from_value::<Message>) {
                    Ok(message) => this.receive(message),
                    Err(error) => {
                        this.dispatch_and_send(EditorInput::BackendCompleted {
                            effect: EditorEffectKind::KeepAlive,
                            requested_draft: None,
                            reply: EditorBackendReply {
                                ok: false,
                                readonly: true,
                                error: Some(format!("Invalid overlay response: {error}")),
                                canvases: Vec::new(),
                                dirty: false,
                            },
                        });
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        let weak = Rc::downgrade(self);
        let close = Closure::wrap(Box::new(move |_: web_sys::CloseEvent| {
            if let Some(this) = weak.upgrade() {
                this.pending.borrow_mut().clear();
                this.dispatch_and_send(EditorInput::TransportLost);
            }
        }) as Box<dyn FnMut(_)>);
        socket.set_onopen(Some(open.as_ref().unchecked_ref()));
        socket.set_onmessage(Some(message.as_ref().unchecked_ref()));
        socket.set_onclose(Some(close.as_ref().unchecked_ref()));
        *self.socket.borrow_mut() = Some(SocketBinding {
            socket,
            _open: open,
            _message: message,
            _close: close,
        });
    }
    pub fn send(&self, effect: &EditorEffect) {
        if *self.compatibility.read() != Compatibility::Ready {
            return;
        }
        let kind = effect.kind();
        let command = match kind {
            EditorEffectKind::Acquire => "acquire_backend",
            EditorEffectKind::KeepAlive => "keep_alive_backend",
            EditorEffectKind::Update => "update_backend_draft",
            EditorEffectKind::Save => "commit_backend",
            EditorEffectKind::Discard | EditorEffectKind::Close => "release_backend",
        };
        let mut request = json!({"command":command,"backend":"obs","editor_id":self.editor_id});
        if let EditorEffect::Update { canvases } | EditorEffect::Save { canvases } = effect {
            request["canvases"] = json!(canvases);
        }
        let id = self.next.get();
        self.next.set(id + 1);
        let sent = self.socket.borrow().as_ref().is_some_and(|socket| {
            socket.socket.ready_state() == web_sys::WebSocket::OPEN
                && socket
                    .socket
                    .send_with_str(
                        &json!({"request_id":id,"asset_version":ASSET_VERSION,"request":request})
                            .to_string(),
                    )
                    .is_ok()
        });
        if sent {
            self.pending.borrow_mut().insert(id, effect.clone());
        } else {
            self.dispatch_and_send(EditorInput::TransportLost);
        }
    }

    fn dispatch_and_send(&self, input: EditorInput) {
        for effect in self.dispatch.call(input) {
            self.send(&effect);
        }
    }
    fn mismatch(&self) {
        *self.compatibility.write_unchecked() = Compatibility::Mismatch;
        self.pending.borrow_mut().clear();
        self.dispatch_and_send(EditorInput::VersionMismatch);
        if let Some(socket) = self.socket.borrow().as_ref() {
            let _ = socket.socket.close();
        }
        reload_once_for_version_mismatch();
    }
    fn receive(&self, message: Message) {
        if *self.compatibility.read() == Compatibility::Mismatch {
            return;
        }
        match message {
            Message::VersionMismatch => self.mismatch(),
            Message::Stage {
                asset_version,
                state,
                canvases,
            } => {
                if asset_version != ASSET_VERSION {
                    self.mismatch();
                    return;
                }
                clear_version_reload_guard();
                let first = *self.compatibility.read() == Compatibility::Checking;
                *self.compatibility.write_unchecked() = Compatibility::Ready;
                self.dispatch_and_send(EditorInput::TransportReady {
                    screen: state.screen.kind,
                    sample: state.system == LampState::Inactive,
                    canvases,
                    first,
                });
            }
            Message::Control {
                request_id,
                response,
            } => {
                let command = self.pending.borrow_mut().remove(&request_id);
                if command.is_none() {
                    return;
                }
                if let Some(effect) = command {
                    let kind = effect.kind();
                    self.dispatch_and_send(EditorInput::BackendCompleted {
                        effect: kind,
                        requested_draft: effect.requested_draft().map(<[_]>::to_vec),
                        reply: EditorBackendReply {
                            ok: response.ok,
                            readonly: response.readonly,
                            error: response.error,
                            canvases: response.canvases,
                            dirty: response.dirty,
                        },
                    });
                }
            }
        }
    }
}

fn reload_once_for_version_mismatch() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(Some(storage)) = window.session_storage() else {
        return;
    };
    if storage
        .get_item(VERSION_RELOAD_KEY)
        .ok()
        .flatten()
        .as_deref()
        == Some(ASSET_VERSION)
    {
        return;
    }
    if storage.set_item(VERSION_RELOAD_KEY, ASSET_VERSION).is_err() {
        return;
    }
    if window.location().reload().is_err() {
        let _ = storage.remove_item(VERSION_RELOAD_KEY);
    }
}

fn clear_version_reload_guard() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(Some(storage)) = window.session_storage() else {
        return;
    };
    let _ = storage.remove_item(VERSION_RELOAD_KEY);
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(window) = web_sys::window()
            && let Some(timer) = self.timer.get()
        {
            window.clear_interval_with_handle(timer);
        }
    }
}
