use crate::bundle::embedded::Assets;
use axum::{
    extract::Path,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
pub(crate) async fn asset(Path(path): Path<String>) -> Response {
    embedded(&path)
}
pub(crate) fn embedded(path: &str) -> Response {
    if let Some(asset) = Assets::get(path) {
        return (
            [(header::CONTENT_TYPE, asset.metadata.mimetype())],
            asset.data,
        )
            .into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}
