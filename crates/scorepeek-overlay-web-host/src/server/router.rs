use crate::bridge::data::Feed;
use crate::host::lifecycle::{Backend, Config};
use axum::{Router, routing::get};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

use crate::server::state::Shared;

pub(crate) async fn serve(
    config: Config,
    input: impl std::io::Read + Send + 'static,
) -> Result<(), String> {
    let skins = crate::skin::StoreRoot::new(config.skin_store.clone());
    for skin in config
        .canvases
        .iter()
        .map(|canvas| canvas.skin.name())
        .collect::<std::collections::BTreeSet<_>>()
    {
        skins.open(skin)?;
    }
    let changed = Arc::new(Notify::new());
    let wake = Arc::clone(&changed);
    let feed = Feed::start(
        config.clone().into(),
        Arc::new(move || wake.notify_waiters()),
        Arc::new(crate::diagnostics::emit),
    )
    .map_err(|error| error.to_string())?;
    let stop = Arc::clone(&feed.stop);
    crate::host::shutdown::watch_parent(input, Arc::clone(&stop))?;
    let managed_canvases = config
        .canvases
        .iter()
        .filter(|canvas| canvas.backend == Backend::Obs)
        .cloned()
        .collect();
    let shared = Arc::new(Shared {
        canvases: Mutex::new(managed_canvases),
        control_socket: config.control_socket.clone(),
        feed,
        changed,
        skins,
    });
    let app = Router::new()
        .route("/health", get(crate::http::health::health))
        .route("/", get(canvas_index))
        .route("/overlay", get(stage_editor_index))
        .route("/canvas/{id}", get(index))
        .route("/skin/{id}/{*path}", get(skin_asset))
        .route("/ws/stage", get(stage_socket))
        .route("/ws/{id}", get(socket))
        .route("/skin-runtime.js", get(skin_runtime_script))
        .route("/skin-host.css", get(skin_host_style))
        .route("/{*path}", get(asset))
        .with_state(Arc::clone(&shared));
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|error| error.to_string())?;
    crate::diagnostics::emit(
        "child_ready",
        &serde_json::json!({"backend":"obs","status":"success"}),
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(crate::host::shutdown::requested(stop))
        .await
        .map_err(|error| error.to_string())
}

use crate::http::assets::asset;
use crate::http::overlay::{canvas_index, index, stage_editor_index};
use crate::http::skin::{skin_asset, skin_host_style, skin_runtime_script};
use crate::websocket::session::{socket, stage_socket};

#[cfg(test)]
mod tests;
