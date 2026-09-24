use crate::bundle::embedded::Assets;
use crate::server::state::Shared;
use axum::{
    extract::{Path, RawQuery, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use std::{fmt::Write as _, sync::Arc, time::Duration};
pub(crate) async fn canvas_index(State(shared): State<Arc<Shared>>) -> Response {
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
pub(crate) async fn stage_editor_index(State(shared): State<Arc<Shared>>) -> Response {
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
    let skins = shared
        .skins
        .list()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|skin| {
            let package = shared.skins.open(&skin.id).ok()?;
            Some(scorepeek_overlay::editor::EditorSkin {
                id: skin.id.parse().ok()?,
                name: skin.name,
                release: skin.release,
                preview: format!("/skin/{}/{}", skin.id, crate::skin::PREVIEW_PATH),
                preview_video: package
                    .resource("preview.webm")
                    .map(|_| format!("/skin/{}/preview.webm", skin.id)),
                widget_defaults: serde_json::from_value(
                    serde_json::to_value(package.manifest.widget_defaults).ok()?,
                )
                .ok()?,
                canvas_properties: serde_json::from_value(
                    serde_json::to_value(package.manifest.canvas_properties).ok()?,
                )
                .ok()?,
                widget_properties: serde_json::from_value(
                    serde_json::to_value(package.manifest.widget_properties).ok()?,
                )
                .ok()?,
            })
        })
        .collect::<Vec<_>>();
    let skins = serde_json::to_string(&skins)
        .unwrap_or_else(|_| "[]".into())
        .replace('<', "\\u003c");
    let Some(asset) = Assets::get("index.html") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(html) = std::str::from_utf8(&asset.data) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let initial = format!(
        "<head><script id=\"scorepeek-stage\" type=\"application/json\">{canvases}</script><script id=\"scorepeek-skins\" type=\"application/json\">{skins}</script><style>{}{} </style>",
        scorepeek_overlay_runtime::style::EDITOR_CSS,
        include_str!("../../../scorepeek-overlay-runtime/styles/stage.css")
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

pub(crate) async fn index(
    Path(id): Path<String>,
    RawQuery(query): RawQuery,
    State(shared): State<Arc<Shared>>,
) -> Response {
    let editor_bootstrap = query
        .as_deref()
        .is_some_and(|query| query.split('&').any(|part| part == "editor=1"));
    let canvas = if editor_bootstrap {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let changed = shared.changed.notified();
            let canvas = shared
                .canvases
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .find(|canvas| canvas.id == id)
                .cloned();
            if canvas.is_some() {
                break canvas;
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                break None;
            }
        }
    } else {
        shared
            .canvases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|canvas| canvas.id == id)
            .cloned()
    };
    let Some(canvas) = canvas else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let skin_id = canvas.skin.name();
    let Ok(package) = shared.skins.open(skin_id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let specification = canvas_specification(&canvas, &package);
    let Ok(specification) = serde_json::to_string(&specification) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let specification = specification.replace('<', "\\u003c");
    let html = format!(
        "<!doctype html><html data-backend=\"obs\"><head><meta charset=\"utf-8\"><base href=\"/skin/{skin_id}/\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'self'; script-src 'self' 'wasm-unsafe-eval' blob:; worker-src blob:; style-src 'self'; img-src 'self' data:; media-src 'self'; connect-src 'self'\"><link rel=\"stylesheet\" href=\"/skin-host.css\"><link rel=\"stylesheet\" href=\"/skin/{skin_id}/{}\"></head><body><div id=\"skin-root\" class=\"scorepeek-skin-scope\"></div><script type=\"application/json\" id=\"scorepeek-skin\">{specification}</script><script src=\"/skin-runtime.js\"></script></body></html>",
        crate::skin::STYLE_PATH,
    );
    (
        [
            (header::CONTENT_TYPE, "text/html"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        html,
    )
        .into_response()
}

pub(crate) fn canvas_specification(
    canvas: &crate::config::Canvas,
    package: &crate::skin::Package,
) -> serde_json::Value {
    let skin_id = canvas.skin.name();
    let canvas_properties = effective_canvas_properties(canvas, &package.manifest);
    serde_json::json!({
        "canvas":{"id":canvas.id,"skin":skin_id,"width":canvas.width,"height":canvas.height,"properties":canvas_properties},
        "widgets":canvas.widgets.iter().map(|widget| { let kind=serde_json::to_value(widget.kind).ok().and_then(|value|value.as_str().map(str::to_owned)).unwrap_or_default(); let properties=package.manifest.effective_widget_properties(&kind,&widget.skin_properties); serde_json::json!({"id":widget.id,"kind":widget.kind,"x":widget.x,"y":widget.y,"width":widget.width,"height":widget.height,"settings":widget.settings,"properties":properties}) }).collect::<Vec<_>>(),
        "wasm":format!("/skin/{skin_id}/{}", crate::skin::MODULE_PATH),
    })
}

pub(crate) fn effective_canvas_properties(
    canvas: &crate::config::Canvas,
    manifest: &crate::skin::Manifest,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    manifest.effective_canvas_properties(&canvas.skin_properties)
}

#[cfg(test)]
#[test]
pub(crate) fn display_canvas_specification_uses_the_skin_property_authority() {
    let mut canvas = crate::config::empty_canvas(
        "browser-background".into(),
        crate::host::lifecycle::Backend::Obs,
        "dev.atty303.scorepeek.skin.cyan-system".parse().unwrap(),
    );
    canvas
        .skin_properties
        .insert("background".into(), serde_json::json!("none"));
    let manifest: crate::skin::Manifest =
        toml::from_str(include_str!("../../../../skins/cyan-system/skin.toml")).unwrap();

    assert_eq!(
        effective_canvas_properties(&canvas, &manifest)["background"],
        "none"
    );
}
