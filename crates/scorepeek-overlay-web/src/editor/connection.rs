use super::model::Model;
use dioxus::prelude::*;
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

#[derive(Clone, Copy, PartialEq)]
pub enum Compatibility {
    Checking,
    Ready,
    Mismatch,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Command {
    Acquire,
    KeepAlive,
    Update,
    Save,
    Discard,
    Close,
}
impl Command {
    fn name(self) -> &'static str {
        match self {
            Self::Acquire => "acquire_backend",
            Self::KeepAlive => "keep_alive_backend",
            Self::Update => "update_backend_draft",
            Self::Save => "commit_backend",
            Self::Discard | Self::Close => "release_backend",
        }
    }
}
#[derive(Deserialize)]
struct Reply {
    ok: bool,
    readonly: bool,
    error: Option<String>,
    #[serde(default)]
    canvases: Vec<CanvasPresentation>,
    backend_revision: Option<u64>,
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
    pub model: Signal<Model>,
    pub compatibility: Signal<Compatibility>,
    socket: RefCell<Option<SocketBinding>>,
    pending: RefCell<BTreeMap<u64, Command>>,
    next: Cell<u64>,
    latest_draft_request: Cell<u64>,
    editor_id: String,
    timer: Cell<Option<i32>>,
    tick: RefCell<Option<Closure<dyn FnMut()>>>,
}
impl Connection {
    pub fn new(model: Signal<Model>, compatibility: Signal<Compatibility>) -> Rc<Self> {
        let editor_id = format!(
            "obs-{}",
            web_sys::window()
                .and_then(|window| window.crypto().ok())
                .map(|crypto| crypto.random_uuid())
                .unwrap_or_default()
        );
        let this = Rc::new(Self {
            model,
            compatibility,
            socket: RefCell::new(None),
            pending: RefCell::new(BTreeMap::new()),
            next: Cell::new(1),
            latest_draft_request: Cell::new(0),
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
                } else if this.model.read().editing
                    && *this.compatibility.read() == Compatibility::Ready
                {
                    this.send(if this.model.read().readonly {
                        Command::Acquire
                    } else {
                        Command::KeepAlive
                    });
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
                        this.model.write_unchecked().notice =
                            Some(format!("Invalid overlay response: {error}"));
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        let weak = Rc::downgrade(self);
        let close = Closure::wrap(Box::new(move |_: web_sys::CloseEvent| {
            if let Some(this) = weak.upgrade() {
                this.pending.borrow_mut().clear();
                this.model.write_unchecked().readonly = true;
                this.model.write_unchecked().drag = None;
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
    pub fn send(&self, command: Command) {
        if *self.compatibility.read() != Compatibility::Ready {
            return;
        }
        let model = self.model.read();
        let mut request =
            json!({"command":command.name(),"backend":"obs","editor_id":self.editor_id});
        if matches!(command, Command::Update | Command::Save) {
            request["canvases"] = json!(model.draft);
        }
        if command == Command::Save {
            request["expected_revision"] = json!(model.backend_revision);
        }
        drop(model);
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
            if matches!(command, Command::Update | Command::Save) {
                self.latest_draft_request.set(id);
            }
            self.pending.borrow_mut().insert(id, command);
        } else {
            let mut model = self.model.write_unchecked();
            model.readonly = true;
            model.notice = Some("Editor connection lost; reconnecting.".into());
        }
    }
    fn mismatch(&self) {
        *self.compatibility.write_unchecked() = Compatibility::Mismatch;
        self.pending.borrow_mut().clear();
        let mut model = self.model.write_unchecked();
        let saved = model.saved.clone();
        let viewport = model.viewport;
        *model = Model::new(saved, viewport, "obs");
        model.selected_canvas = None;
        drop(model);
        if let Some(socket) = self.socket.borrow().as_ref() {
            let _ = socket.socket.close();
        }
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
                let first = *self.compatibility.read() == Compatibility::Checking;
                *self.compatibility.write_unchecked() = Compatibility::Ready;
                let mut model = self.model.write_unchecked();
                model.screen = state.screen.kind;
                model.chrome.sample = state.system == LampState::Inactive;
                model.receive_stage(canvases);
                let acquire = first && model.editing;
                drop(model);
                if acquire {
                    self.send(Command::Acquire);
                }
            }
            Message::Control {
                request_id,
                response,
            } => {
                let command = self.pending.borrow_mut().remove(&request_id);
                if command.is_none() {
                    return;
                }
                let acquire_discard;
                let mut release = false;
                {
                    let mut model = self.model.write_unchecked();
                    model.readonly = response.readonly;
                    model.notice = response.error;
                    if let Some(revision) = response.backend_revision {
                        model.backend_revision = revision;
                    }
                    if !response.canvases.is_empty()
                        && request_id >= self.latest_draft_request.get()
                    {
                        model.draft = response.canvases;
                        if !response.dirty {
                            let model = &mut *model;
                            model.saved.clone_from(&model.draft);
                        }
                        model.generation += 1;
                        if !model
                            .draft
                            .iter()
                            .any(|canvas| Some(&canvas.id) == model.selected_canvas.as_ref())
                        {
                            model.select_visible();
                        }
                    }
                    acquire_discard = response.ok
                        && !response.readonly
                        && command == Some(Command::Acquire)
                        && model.discard_pending;
                    if response.ok {
                        match command {
                            Some(Command::Save) => {
                                let model = &mut *model;
                                model.saved.clone_from(&model.draft);
                                release = true;
                            }
                            Some(Command::Discard | Command::Close) => model.close(),
                            _ => {}
                        }
                    } else if command == Some(Command::Discard) {
                        model.discard_pending = false;
                    }
                }
                if acquire_discard {
                    self.send(Command::Discard);
                }
                if release {
                    self.send(Command::Close);
                }
            }
        }
    }
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
