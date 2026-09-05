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
        extract::{Path, State, WebSocketUpgrade, ws::Message},
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
            .route("/stage.js", get(stage_script))
            .route("/canvas/{id}", get(index))
            .route("/ws/stage", get(stage_socket))
            .route("/ws/{id}", get(socket))
            .route("/fonts/oxanium.ttf", get(font))
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
        let html = format!(
            r#"<!doctype html><html><head><meta charset="utf-8"><title>scorepeek OBS overlay</title><style>{}{}</style></head><body><div id="stage"></div><aside id="editor"></aside><button id="return">RETURN TO EDITOR</button><div id="notice"></div><script id="initial" type="application/json">{canvases}</script><script src="/stage.js"></script></body></html>"#,
            scorepeek_overlay_ui::EDITOR_CSS,
            include_str!("../../scorepeek-overlay-ui/styles/stage.css")
        );
        (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            html,
        )
            .into_response()
    }

    async fn stage_script() -> Response {
        (
            [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
            include_str!("stage.js"),
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
            html.replacen("<head>", &initial, 1),
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

    async fn stage_socket(ws: WebSocketUpgrade, State(shared): State<Arc<Shared>>) -> Response {
        ws.on_upgrade(move |mut socket| async move {
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
                    serde_json::json!({"type":"stage", "state":state, "canvases":canvases})
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
                                let response = serde_json::from_str::<crate::control::Request>(&text)
                                    .map_err(|error| error.to_string())
                                    .and_then(|request| {
                                        let obs = match &request {
                                            crate::control::Request::AcquireBackend { backend, .. }
                                            | crate::control::Request::KeepAliveBackend { backend, .. }
                                            | crate::control::Request::ReleaseBackend { backend, .. }
                                            | crate::control::Request::GetBackend { backend }
                                            | crate::control::Request::UpdateBackendDraft { backend, .. }
                                            | crate::control::Request::CommitBackend { backend, .. } => {
                                                *backend == crate::runtime::Backend::Obs
                                            }
                                        };
                                        if !obs { return Err("stage control only accepts the OBS backend".into()); }
                                        crate::control::request(&shared.control_socket, &request)
                                    })
                                    .unwrap_or_else(|error| crate::control::Response {
                                        ok:false,
                                        readonly:true,
                                        error:Some(error),
                                        canvases:Vec::new(),
                                        backend_revision:None,
                                    });
                                if !response.canvases.is_empty() {
                                    let mut canvases = shared.canvases.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                                    for presentation in &response.canvases {
                                        if let Some(canvas) = canvases.iter_mut().find(|canvas| canvas.id == presentation.id) {
                                            canvas.apply_presentation(presentation);
                                        } else {
                                            let mut canvas = crate::config::empty_canvas(presentation.id.clone(), crate::runtime::Backend::Obs);
                                            canvas.apply_presentation(presentation);
                                            canvases.push(canvas);
                                        }
                                    }
                                    canvases.retain(|canvas| response.canvases.iter().any(|item| item.id == canvas.id));
                                }
                                shared.changed.notify_waiters();
                                let reply = serde_json::json!({"type":"control", "response":response}).to_string();
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
}
