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
    if !config.listen.ip().is_loopback() {
        return Err("overlay listen must be loopback".into());
    }
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
        sync::{Arc, atomic::Ordering},
        time::Duration,
    };
    use tokio::sync::Notify;

    struct Shared {
        appearance: scorepeek_overlay_ui::Appearance,
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
        let shared = Arc::new(Shared {
            appearance: config.appearance,
            feed,
            changed,
        });
        let app = Router::new()
            .route("/", get(index))
            .route("/ws", get(socket))
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

    async fn index(State(shared): State<Arc<Shared>>) -> Response {
        let Some(asset) = Assets::get("index.html") else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Ok(html) = std::str::from_utf8(&asset.data) else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        let appearance = shared.appearance;
        let initial = format!(
            "<head><meta name=\"scorepeek-appearance\" data-skin=\"{}\" data-layout=\"{}\"><style>@font-face{{font-family:Oxanium;src:url('/fonts/oxanium.ttf') format('truetype');font-weight:200 800;font-style:normal;font-display:swap}}</style>",
            appearance.skin.name(),
            appearance.layout.name()
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
    async fn socket(ws: WebSocketUpgrade, State(shared): State<Arc<Shared>>) -> Response {
        ws.on_upgrade(move |mut socket| async move {
            let mut sent = None;
            loop {
                let notified = shared.changed.notified();
                let state = shared
                    .feed
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                if sent.as_ref() != Some(&state) {
                    let Ok(bytes) = serde_json::to_string(&state) else {
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
                        if matches!(message, None | Some(Err(_) | Ok(Message::Close(_)))) { break; }
                    }
                }
            }
        })
    }
}
