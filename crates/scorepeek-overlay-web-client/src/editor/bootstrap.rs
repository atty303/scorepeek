//! Browser editor bootstrap data embedded by the Web host.

use scorepeek_overlay::CanvasPresentation;

pub(crate) fn read_initial() -> (
    Vec<CanvasPresentation>,
    Vec<scorepeek_overlay::editor::EditorSkin>,
) {
    let document = web_sys::window().and_then(|window| window.document());
    let read = |id: &str| {
        document
            .as_ref()
            .and_then(|doc| doc.get_element_by_id(id))
            .and_then(|node| node.text_content())
    };
    (
        read("scorepeek-stage")
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default(),
        read("scorepeek-skins")
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default(),
    )
}
