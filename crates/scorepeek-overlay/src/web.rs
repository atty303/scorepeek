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
        sync::{Arc, Mutex, atomic::Ordering},
        time::Duration,
    };
    use tokio::sync::Notify;

    struct Shared {
        canvases: Mutex<Vec<crate::config::Canvas>>,
        config_path: std::path::PathBuf,
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
        let shared = Arc::new(Shared {
            canvases: Mutex::new(config.canvases.clone()),
            config_path: config.config_path.clone(),
            control_socket: config.control_socket.clone(),
            feed,
            changed,
        });
        let app = Router::new()
            .route("/", get(canvas_index))
            .route("/overlay", get(stage_index))
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
        let links = canvases
            .iter()
            .map(|canvas| {
                format!(
                    "<li><a href=\"/canvas/{}\">{}</a></li>",
                    canvas.id, canvas.id
                )
            })
            .collect::<String>();
        (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            format!("<!doctype html><title>scorepeek canvases</title><ul>{links}</ul>"),
        )
            .into_response()
    }
    async fn stage_index(State(shared): State<Arc<Shared>>) -> Response {
        let canvases = shared
            .canvases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let layout = canvases
            .iter()
            .map(|canvas| {
                serde_json::json!({
                    "id": canvas.id,
                    "x": canvas.x,
                    "y": canvas.y,
                    "width": canvas.width,
                    "height": canvas.height,
                    "z": canvas.z,
                    "show_on": canvas.show_on,
                })
            })
            .collect::<Vec<_>>();
        let Ok(layout) = serde_json::to_string(&layout) else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        let layout = layout.replace('<', "\\u003c");
        let html = format!(
            r#"<!doctype html><meta charset="utf-8"><title>scorepeek OBS overlay</title>
<style>html,body{{margin:0;width:100%;height:100%;overflow:hidden;background:transparent}}iframe{{position:absolute;border:0;background:transparent}}#menu{{display:none;position:fixed;z-index:2147483647;padding:8px;background:#071019ee;border:1px solid #27e5f3;color:#fff;font:13px sans-serif}}#menu button{{display:block;width:100%;margin:3px 0;background:#122531;color:#fff;border:1px solid #65808c;padding:6px}}</style>
<div id="stage"></div><div id="menu"></div><script id="layout" type="application/json">{layout}</script>
<script>
const stage=document.querySelector('#stage'),menu=document.querySelector('#menu');let layout=JSON.parse(document.querySelector('#layout').textContent),screen=null,preview=null,editing=null;
function visible(c){{return preview===c.id||!c.show_on||c.show_on.includes(screen)}}
function render(){{const existing=new Map([...stage.children].map(n=>[n.dataset.id,n]));for(const c of layout){{let f=existing.get(c.id);if(!f){{f=document.createElement('iframe');f.dataset.id=c.id;f.src='/canvas/'+encodeURIComponent(c.id);stage.append(f)}}const edit=editing===c.id;f.style.cssText=edit?`left:0;top:0;width:100vw;height:100vh;z-index:2147483646;display:block`:`left:${{c.x}}px;top:${{c.y}}px;width:${{c.width}}px;height:${{c.height}}px;z-index:${{c.z}};display:${{visible(c)?'block':'none'}}`;existing.delete(c.id)}}for(const f of existing.values())f.remove()}}
function openMenu(e){{e.preventDefault();menu.replaceChildren();for(const c of layout){{const b=document.createElement('button');b.textContent=c.id;b.onclick=()=>{{if(editing&&editing!==c.id){{const old=[...stage.children].find(f=>f.dataset.id===editing);if(old)old.src=old.src;editing=null}}preview=c.id;menu.style.display='none';render()}};menu.append(b)}}menu.style.left=e.clientX+'px';menu.style.top=e.clientY+'px';menu.style.display='block'}}
document.body.addEventListener('contextmenu',openMenu);document.body.addEventListener('pointerdown',e=>{{if(!menu.contains(e.target))menu.style.display='none'}});render();
addEventListener('message',e=>{{if(typeof e.data!=='string'||!e.data.startsWith('scorepeek:'))return;const [,kind,id]=e.data.split(':');if(kind==='editing'){{editing=id;render()}}else if(kind==='select'){{if(editing&&editing!==id){{const old=[...stage.children].find(f=>f.dataset.id===editing);if(old)old.src=old.src;editing=null}}preview=id;render()}}else if(kind==='done'&&editing===id){{editing=null;preview=null;render()}}}});
function connect(){{const ws=new WebSocket(`ws://${{location.host}}/ws/stage`);ws.onmessage=e=>{{const m=JSON.parse(e.data);if(m.type==='stage'){{layout=m.canvases;screen=m.state.screen.kind;render()}}}};ws.onclose=()=>setTimeout(connect,1000)}}connect();
</script>"#
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
                                let mut request_kind = None;
                                let response = serde_json::from_str::<crate::control::Request>(&text).map_err(|error| error.to_string()).and_then(|request| {
                                    request_kind = Some(match &request {
                                        crate::control::Request::Acquire { .. } | crate::control::Request::KeepAlive { .. } | crate::control::Request::Release { .. } => "lease",
                                        crate::control::Request::ReplaceCanvas { .. } | crate::control::Request::SetGeometry { .. } | crate::control::Request::SetOutput { .. } => "canvas_mutation",
                                        crate::control::Request::SetUnknownGrace { .. } => "settings",
                                        crate::control::Request::ListCanvases { .. } => "canvas_list",
                                        crate::control::Request::AddCanvas { .. } | crate::control::Request::DeleteCanvas { .. } | crate::control::Request::SetCanvasEnabled { .. } => "canvas_manager",
                                    });
                                    let targets_canvas = match &request {
                                        crate::control::Request::Acquire { canvas_id, .. } | crate::control::Request::KeepAlive { canvas_id, .. } | crate::control::Request::Release { canvas_id, .. } => canvas_id == &id,
                                        crate::control::Request::ReplaceCanvas { canvas_id, .. } | crate::control::Request::SetGeometry { canvas_id, .. } | crate::control::Request::SetOutput { canvas_id, .. } | crate::control::Request::SetUnknownGrace { canvas_id, .. } => canvas_id == &id,
                                        crate::control::Request::ListCanvases { backend } | crate::control::Request::AddCanvas { backend, .. } | crate::control::Request::DeleteCanvas { backend, .. } | crate::control::Request::SetCanvasEnabled { backend, .. } => *backend == crate::runtime::Backend::Obs,
                                    };
                                    if !targets_canvas { return Err("control request targets another canvas".into()); }
                                    crate::control::request(&shared.control_socket, &request)
                                }).unwrap_or_else(|error| crate::control::Response { ok:false, readonly:true, canvas:None, error:Some(error), canvases:Vec::new(), backend_revision:None, settings_revision:None, unknown_grace_ms:None });
                                if let Some(presentation) = &response.canvas {
                                    let mut canvases = shared.canvases.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                                    if let Some(current) = canvases.iter_mut().find(|current| current.id == presentation.id) {
                                        current.apply_presentation(presentation);
                                    }
                                }
                                if response.backend_revision.is_some()
                                    && let Ok((config, _)) = crate::config::load_or_create(&shared.config_path)
                                {
                                    *shared.canvases.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = config.canvases.into_iter().filter(|canvas| canvas.backend == crate::runtime::Backend::Obs && canvas.enabled).collect();
                                }
                                shared.changed.notify_waiters();
                                let Ok(reply) = serde_json::to_string(&serde_json::json!({"type":"control", "request":request_kind, "response":response})) else { break; };
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
                    .map(|canvas| {
                        serde_json::json!({
                            "id": canvas.id, "x": canvas.x, "y": canvas.y,
                            "width": canvas.width, "height": canvas.height,
                            "z": canvas.z, "show_on": canvas.show_on,
                        })
                    })
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
                    message = socket.recv() => if message.is_none() { break; },
                }
            }
        })
    }
}
