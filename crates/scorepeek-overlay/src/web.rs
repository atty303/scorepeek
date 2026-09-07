use crate::runtime::Config;
#[cfg(feature = "embedded-web")]
#[derive(rust_embed::Embed)]
#[folder = "$SCOREPEEK_WEB_ASSET_DIR/"]
struct Assets;

#[cfg(all(test, feature = "embedded-web"))]
mod tests {
    use super::Assets;
    #[test]
    fn real_bundle_contains_html_javascript_and_wasm_with_mime_types() {
        assert_eq!(
            Assets::get("index.html").unwrap().metadata.mimetype(),
            "text/html"
        );
        for (suffix, mime) in [(".wasm", "application/wasm"), (".js", "text/javascript")] {
            let path = Assets::iter()
                .find(|path| path.ends_with(suffix))
                .expect("real bundle asset");
            let asset = Assets::get(&path).unwrap();
            assert_eq!(asset.metadata.mimetype(), mime);
            assert!(!asset.data.is_empty());
        }
    }
}

/// Serves only local embedded UI assets and display snapshots.
/// # Errors
/// Returns runtime, bind or worker errors.
pub fn run(config: Config, input: impl std::io::Read + Send + 'static) -> Result<(), String> {
    #[cfg(not(feature = "embedded-web"))]
    {
        let _ = (config, input);
        Err("OBS overlay requires the embedded-web build (mise run dist:build)".into())
    }
    #[cfg(feature = "embedded-web")]
    {
        if Assets::get("index.html").is_none() {
            return Err("embedded index.html is missing".into());
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        runtime.block_on(server::serve(config, input))
    }
}

#[cfg(feature = "embedded-web")]
mod server {
    use super::Assets;
    use crate::runtime::{Config, Feed};
    use axum::{
        Router,
        extract::{Path, RawQuery, State, WebSocketUpgrade, ws::Message},
        http::{StatusCode, header},
        response::{IntoResponse, Response},
        routing::get,
    };
    use std::{
        fmt::Write as _,
        sync::{Arc, Mutex, atomic::Ordering},
        time::Duration,
    };
    use tokio::sync::Notify;

    struct Shared {
        canvases: Mutex<Vec<crate::config::Canvas>>,
        control_socket: std::path::PathBuf,
        feed: Feed,
        changed: Arc<Notify>,
    }

    #[derive(serde::Deserialize)]
    struct StageControlEnvelope {
        request_id: u64,
        #[serde(default)]
        asset_version: String,
        request: serde_json::Value,
    }

    const ASSET_VERSION: &str = env!("SCOREPEEK_OVERLAY_BUILD_ID");

    struct EditorSession {
        shared: Arc<Shared>,
        id: String,
        owns_lease: std::cell::Cell<bool>,
    }
    impl EditorSession {
        fn new(shared: Arc<Shared>) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(1);
            Self {
                shared,
                owns_lease: std::cell::Cell::new(false),
                id: format!(
                    "obs-socket-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ),
            }
        }
        fn request(
            &self,
            mut request: crate::control::Request,
        ) -> Result<crate::control::Response, String> {
            use crate::control::Request;
            let (backend, editor) = match &mut request {
                Request::AcquireBackend { backend, editor_id }
                | Request::KeepAliveBackend { backend, editor_id }
                | Request::ReleaseBackend { backend, editor_id }
                | Request::UpdateBackendDraft {
                    backend, editor_id, ..
                }
                | Request::CommitBackend {
                    backend, editor_id, ..
                } => (backend, Some(editor_id)),
                Request::GetBackend { backend } => (backend, None),
            };
            if *backend != crate::runtime::Backend::Obs {
                return Err("stage control only accepts the OBS backend".into());
            }
            if let Some(editor) = editor {
                editor.clone_from(&self.id);
            }
            let publishes =
                !matches!(request, Request::ReleaseBackend { .. }) || self.owns_lease.get();
            let response = crate::control::request(&self.shared.control_socket, &request)?;
            if matches!(request, Request::AcquireBackend { .. })
                && response.ok
                && !response.readonly
            {
                self.owns_lease.set(true);
            }
            if response.readonly || matches!(request, Request::ReleaseBackend { .. }) && response.ok
            {
                self.owns_lease.set(false);
            }
            if publishes && response.ok && !response.readonly && response.backend_revision.is_some()
            {
                let mut canvases = self
                    .shared
                    .canvases
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for presentation in &response.canvases {
                    if let Some(canvas) = canvases
                        .iter_mut()
                        .find(|canvas| canvas.id == presentation.id)
                    {
                        canvas.apply_presentation(presentation);
                    } else {
                        let mut canvas = crate::config::empty_canvas(
                            presentation.id.clone(),
                            crate::runtime::Backend::Obs,
                        );
                        canvas.apply_presentation(presentation);
                        canvases.push(canvas);
                    }
                }
                canvases.retain(|canvas| response.canvases.iter().any(|item| item.id == canvas.id));
            }
            self.shared.changed.notify_waiters();
            Ok(response)
        }
    }
    impl Drop for EditorSession {
        fn drop(&mut self) {
            if !self.owns_lease.get() {
                return;
            }
            let result = self.request(crate::control::Request::ReleaseBackend {
                backend: crate::runtime::Backend::Obs,
                editor_id: self.id.clone(),
            });
            crate::diagnostics::emit(
                "overlay_editor_connection",
                &serde_json::json!({
                    "status": if result.as_ref().is_ok_and(|response| response.ok) { "released" } else { "release_failed" },
                }),
            );
        }
    }

    fn version_mismatch() -> String {
        serde_json::json!({"type":"version_mismatch", "asset_version":ASSET_VERSION}).to_string()
    }

    pub(super) async fn serve(
        config: Config,
        mut input: impl std::io::Read + Send + 'static,
    ) -> Result<(), String> {
        let changed = Arc::new(Notify::new());
        let wake = Arc::clone(&changed);
        let feed = Feed::start(config.clone(), Arc::new(move || wake.notify_waiters()))
            .map_err(|error| error.to_string())?;
        let stop = Arc::clone(&feed.stop);
        std::thread::Builder::new()
            .name("overlay-parent".into())
            .spawn(move || {
                let mut byte = [0];
                let _ = input.read(&mut byte);
                stop.store(true, Ordering::Release);
            })
            .map_err(|error| error.to_string())?;
        let managed_canvases = config
            .canvases
            .iter()
            .filter(|canvas| canvas.backend == crate::runtime::Backend::Obs)
            .cloned()
            .collect();
        let shared = Arc::new(Shared {
            canvases: Mutex::new(managed_canvases),
            control_socket: config.control_socket.clone(),
            feed,
            changed,
        });
        let app = Router::new()
            .route("/", get(canvas_index))
            .route("/overlay", get(stage_editor_index))
            .route("/canvas/{id}", get(index))
            .route("/ws/stage", get(stage_socket))
            .route("/ws/{id}", get(socket))
            .route("/fonts/oxanium.ttf", get(font))
            .route("/fonts/{name}", get(extra_font))
            .route("/motion.js", get(motion_script))
            .route("/fonts/OFL.txt", get(font_license))
            .route("/{*path}", get(asset))
            .with_state(Arc::clone(&shared));
        let listener = tokio::net::TcpListener::bind(config.listen)
            .await
            .map_err(|error| error.to_string())?;
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                while !shared.feed.stop.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await
            .map_err(|error| error.to_string())
    }

    async fn canvas_index(State(shared): State<Arc<Shared>>) -> Response {
        let canvases = shared
            .canvases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let links = canvases.iter().fold(String::new(), |mut links, canvas| {
            let _ = write!(
                links,
                "<li><a href=\"/canvas/{}\">{}</a></li>",
                canvas.id, canvas.id
            );
            links
        });
        (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            format!("<!doctype html><title>scorepeek canvases</title><ul>{links}</ul>"),
        )
            .into_response()
    }
    async fn stage_editor_index(State(shared): State<Arc<Shared>>) -> Response {
        let canvases = shared
            .canvases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(crate::config::Canvas::presentation)
            .collect::<Vec<_>>();
        let Ok(canvases) = serde_json::to_string(&canvases) else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        let canvases = canvases.replace('<', "\\u003c");
        let Some(asset) = Assets::get("index.html") else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Ok(html) = std::str::from_utf8(&asset.data) else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        let initial = format!(
            "<head><script id=\"scorepeek-stage\" type=\"application/json\">{canvases}</script><style>@font-face{{font-family:Oxanium;src:url('/fonts/oxanium.ttf');font-weight:200 800}}{}{}{} </style>",
            scorepeek_overlay_ui::FONT_CSS,
            scorepeek_overlay_ui::EDITOR_CSS,
            include_str!("../../scorepeek-overlay-ui/styles/stage.css")
        );
        let html = html.replacen("<head>", &initial, 1);
        (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            html,
        )
            .into_response()
    }

    async fn index(Path(id): Path<String>, State(shared): State<Arc<Shared>>) -> Response {
        let Some(asset) = Assets::get("index.html") else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Ok(html) = std::str::from_utf8(&asset.data) else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        let canvases = shared
            .canvases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(canvas) = canvases.iter().find(|canvas| canvas.id == id) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Ok(canvas_json) = serde_json::to_string(&canvas.presentation()) else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        let canvas_json = canvas_json.replace('<', "\\u003c");
        let initial = format!(
            "<head><script type=\"application/json\" id=\"scorepeek-canvas\">{canvas_json}</script><style>@font-face{{font-family:Oxanium;src:url('/fonts/oxanium.ttf') format('truetype');font-weight:200 800;font-style:normal;font-display:swap}}</style>"
        );
        (
            [
                (header::CONTENT_TYPE, "text/html"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            html.replacen("<head>", &initial, 1).replace("</head>", &format!("<style>{}</style><script id=\"scorepeek-motion\" type=\"application/json\">{}</script><script defer src=\"/motion.js\"></script></head>", scorepeek_overlay_ui::FONT_CSS, scorepeek_overlay_ui::motion::SPEC)),
        )
            .into_response()
    }
    async fn extra_font(Path(name): Path<String>) -> Response {
        if let Some((_, text)) = scorepeek_overlay_ui::FONT_LICENSES
            .iter()
            .find(|(path, _)| *path == name)
        {
            return ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], *text).into_response();
        }
        scorepeek_overlay_ui::FONT_ASSETS
            .iter()
            .find(|(path, _)| *path == name)
            .map_or_else(
                || StatusCode::NOT_FOUND.into_response(),
                |(_, bytes)| ([(header::CONTENT_TYPE, "font/ttf")], *bytes).into_response(),
            )
    }
    async fn motion_script() -> Response {
        (
            [(header::CONTENT_TYPE, "text/javascript")],
            scorepeek_overlay_ui::motion::BROWSER_DRIVER,
        )
            .into_response()
    }
    async fn font() -> Response {
        (
            [(header::CONTENT_TYPE, "font/ttf")],
            scorepeek_overlay_ui::OXANIUM,
        )
            .into_response()
    }
    async fn font_license() -> Response {
        (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            include_str!("../../scorepeek-overlay-ui/assets/fonts/OFL.txt"),
        )
            .into_response()
    }
    async fn asset(Path(path): Path<String>) -> Response {
        embedded(&path)
    }
    fn embedded(path: &str) -> Response {
        if let Some(svg) = scorepeek_overlay_ui::composition::aperture_asset(&format!("/{path}")) {
            return ([(header::CONTENT_TYPE, "image/svg+xml")], svg).into_response();
        }
        if let Some(bytes) = scorepeek_overlay_ui::skin_asset(&format!("/{path}")) {
            return ([(header::CONTENT_TYPE, "image/png")], bytes).into_response();
        }
        #[cfg(feature = "embedded-web")]
        if let Some(asset) = Assets::get(path) {
            return (
                [(header::CONTENT_TYPE, asset.metadata.mimetype())],
                asset.data,
            )
                .into_response();
        }
        let _ = path;
        StatusCode::NOT_FOUND.into_response()
    }
    async fn socket(
        Path(id): Path<String>,
        ws: WebSocketUpgrade,
        State(shared): State<Arc<Shared>>,
    ) -> Response {
        if !shared
            .canvases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|canvas| canvas.id == id)
        {
            return StatusCode::NOT_FOUND.into_response();
        }
        ws.on_upgrade(move |mut socket| async move {
            let mut sent = None;
            loop {
                let available = shared
                    .canvases
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .any(|canvas| canvas.id == id);
                if !available {
                    let message = serde_json::json!({"type":"canvas_unavailable"}).to_string();
                    let _ = tokio::time::timeout(
                        Duration::from_secs(2),
                        socket.send(Message::Text(message.into())),
                    )
                    .await;
                    break;
                }
                let notified = shared.changed.notified();
                let state = shared
                    .feed
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                if sent.as_ref() != Some(&state) {
                    let Ok(bytes) = serde_json::to_string(&serde_json::json!({"type":"state", "state":state})) else {
                        break;
                    };
                    if tokio::time::timeout(
                        Duration::from_secs(2),
                        socket.send(Message::Text(bytes.into())),
                    )
                    .await
                    .map_or(true, |result| result.is_err())
                    {
                        break;
                    }
                    sent = Some(state);
                }
                tokio::select! {
                    () = notified => {},
                    () = tokio::time::sleep(Duration::from_millis(250)) => {
                        if shared.feed.stop.load(Ordering::Acquire) { break; }
                    },
                    message = socket.recv() => {
                        match message {
                            Some(Ok(Message::Text(text))) => {
                                let reply = serde_json::json!({
                                    "type":"control",
                                    "request":"display_only",
                                    "response":crate::control::Response {
                                        ok:false,
                                        readonly:true,
                                        error:Some("このURLは表示専用です。編集には /overlay をOBS Browser SourceのInteractionで開いてください。".into()),
                                        canvases:Vec::new(),
                                        backend_revision:None,
                                        dirty:false,
                                        wayland_refresh_hz:None,
                                    }
                                });
                                let Ok(reply) = serde_json::to_string(&reply) else { break; };
                                let _ = text;
                                if socket.send(Message::Text(reply.into())).await.is_err() { break; }
                            }
                            None | Some(Err(_) | Ok(Message::Close(_))) => break,
                            _ => {}
                        }
                    }
                }
            }
        })
    }

    async fn stage_socket(
        ws: WebSocketUpgrade,
        RawQuery(version): RawQuery,
        State(shared): State<Arc<Shared>>,
    ) -> Response {
        ws.on_upgrade(move |mut socket| async move {
            let session = EditorSession::new(Arc::clone(&shared));
            if version.as_deref() != Some(format!("asset_version={ASSET_VERSION}").as_str()) {
                crate::diagnostics::emit("overlay_editor_version", &serde_json::json!({"status":"error", "error_type":"version_mismatch"}));
                let _ = socket.send(Message::Text(version_mismatch().into())).await;
                return;
            }
            crate::diagnostics::emit("overlay_editor_version", &serde_json::json!({"status":"success"}));
            let mut sent = String::new();
            loop {
                let notified = shared.changed.notified();
                let state = shared
                    .feed
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                let canvases = shared
                    .canvases
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .map(crate::config::Canvas::presentation)
                    .collect::<Vec<_>>();
                let message =
                    serde_json::json!({"type":"stage", "asset_version":ASSET_VERSION, "state":state, "canvases":canvases})
                        .to_string();
                if message != sent {
                    if socket
                        .send(Message::Text(message.clone().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    sent = message;
                }
                tokio::select! {
                    () = notified => {},
                    () = tokio::time::sleep(Duration::from_millis(250)) => {
                        if shared.feed.stop.load(Ordering::Acquire) { break; }
                    },
                    message = socket.recv() => {
                        match message {
                            Some(Ok(Message::Text(text))) => {
                                let envelope = serde_json::from_str::<StageControlEnvelope>(&text);
                                let request_id = envelope.as_ref().map_or(0, |value| value.request_id);
                                if envelope.as_ref().is_ok_and(|value| value.asset_version != ASSET_VERSION) {
                                    crate::diagnostics::emit("overlay_editor_version", &serde_json::json!({"status":"error", "error_type":"version_mismatch"}));
                                    let _ = socket.send(Message::Text(version_mismatch().into())).await;
                                    break;
                                }
                                let response = envelope
                                    .map_err(|error| error.to_string())
                                    .and_then(|envelope| serde_json::from_value(envelope.request).map_err(|error| error.to_string()))
                                    .and_then(|request| session.request(request))
                                    .unwrap_or_else(|error| crate::control::Response {
                                        ok:false, readonly:true, error:Some(error),
                                        canvases:Vec::new(), backend_revision:None, dirty:false,
                                        wayland_refresh_hz:None,
                                    });
                                let reply = serde_json::json!({"type":"control", "request_id":request_id, "response":response}).to_string();
                                if socket.send(Message::Text(reply.into())).await.is_err() { break; }
                            }
                            None | Some(Err(_) | Ok(Message::Close(_))) => break,
                            _ => {}
                        }
                    },
                }
            }
        })
    }

    #[cfg(test)]
    mod tests;
}
