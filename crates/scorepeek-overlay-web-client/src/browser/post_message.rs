//! Same-origin canvas iframe presentation publishing.

use wasm_bindgen::{JsCast as _, JsValue};

pub(crate) fn publish_canvas_replica(
    canvas_id: &str,
    specification: &serde_json::Value,
    session_id: u64,
    revision: u64,
) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Some(element) =
        document.get_element_by_id(&format!("scorepeek-replica-{}", encode_id(canvas_id)))
    else {
        return;
    };
    let Ok(frame) = element.dyn_into::<web_sys::HtmlIFrameElement>() else {
        return;
    };
    let Some(target) = frame.content_window() else {
        return;
    };
    let message = serde_json::json!({
        "type": "scorepeek-editor-presentation",
        "session_id": session_id,
        "revision": revision,
        "specification": specification,
    });
    let Ok(message) = serde_json::to_string(&message) else {
        return;
    };
    let origin = window.location().origin().unwrap_or_else(|_| "/".into());
    let _ = target.post_message(&JsValue::from_str(&message), &origin);
}

pub(crate) fn encode_id(id: &str) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::new();
    for byte in id.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}
