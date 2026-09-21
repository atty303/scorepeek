use crate::server::state::Shared;
use axum::{
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use std::sync::Arc;
pub(crate) async fn skin_asset(
    Path((id, path)): Path<(String, String)>,
    State(shared): State<Arc<Shared>>,
) -> Response {
    let Ok(package) = shared.skins.open(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(bytes) = package.resource(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let content_type = match path.as_str() {
        crate::skin::MODULE_PATH => "application/wasm",
        crate::skin::STYLE_PATH => "text/css; charset=utf-8",
        crate::skin::PREVIEW_PATH => "image/png",
        crate::skin::PREVIEW_VIDEO_PATH => "video/webm",
        _ => package
            .resource_media_type(&path)
            .unwrap_or("application/octet-stream"),
    };
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes.to_vec(),
    )
        .into_response()
}
pub(crate) async fn skin_runtime_script() -> Response {
    (
        [(header::CONTENT_TYPE, "text/javascript")],
        include_str!("../bundle/skin_browser.js"),
    )
        .into_response()
}
pub(crate) async fn skin_host_style() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        "html,body,#skin-root{margin:0;width:100%;height:100%;overflow:hidden;background:transparent}",
    )
        .into_response()
}
