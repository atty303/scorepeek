use crate::primitive::node::{el, text};
use crate::render::label;
use crate::theme::Skin;
use scorepeek_skin_sdk::{Node, Widget};
use serde_json::Value;
pub(crate) fn field(key: &str, name: &str, value: &str, skin: Skin) -> Node {
    el(
        key,
        "span",
        &[],
        vec![
            el(
                &format!("{key}:name"),
                "span",
                &[("class", "field-label".into())],
                vec![label(&format!("{key}:label"), name, skin, 18)],
            ),
            text(&format!("{key}:text"), value),
        ],
    )
}
pub(crate) fn heading(key: &str, value: &str, skin: Skin) -> Node {
    label_element(key, "h2", value, skin, 18)
}
pub(crate) fn label_element(key: &str, tag: &str, value: &str, skin: Skin, height: u32) -> Node {
    el(
        key,
        tag,
        &[],
        vec![label(&format!("{key}:label"), value, skin, height)],
    )
}
pub(crate) fn label_class(key: &str, class: &str, value: &str, skin: Skin, height: u32) -> Node {
    el(
        key,
        "span",
        &[("class", class.into())],
        vec![label(&format!("{key}:label"), value, skin, height)],
    )
}
pub(crate) fn polyline(key: &str, class: &str, color: &str, points: &str) -> Node {
    el(
        key,
        "polyline",
        &[
            ("class", class.into()),
            ("fill", "none".into()),
            ("stroke", color.into()),
            ("stroke-width", "1.25".into()),
            ("vector-effect", "non-scaling-stroke".into()),
            ("points", points.into()),
        ],
        vec![],
    )
}
pub(crate) fn shape(key: &str, tag: &str, attrs: &[(&str, &str)]) -> Node {
    el(
        key,
        tag,
        &attrs
            .iter()
            .map(|(k, v)| (*k, (*v).to_owned()))
            .collect::<Vec<_>>(),
        vec![],
    )
}
pub(crate) fn frame_width(widget: &Widget) -> u32 {
    match widget
        .properties
        .get("frame-width")
        .and_then(Value::as_str)
        .unwrap_or("m")
    {
        "s" => 4,
        "l" => 16,
        _ => 8,
    }
}
