use super::frame;
use crate::entry::expanded_widget;
use crate::primitive::{aperture_mask, el, frame_width, node_text};
use crate::theme::Skin;
use scorepeek_skin_sdk::{Node, Widget};
use serde_json::Value;
pub(crate) fn empty_widget(key: &str, widget: &Widget, skin: Skin) -> Node {
    let opacity = f64::from(
        u32::try_from(
            widget
                .properties
                .get("fill-opacity-percent")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        )
        .unwrap_or(0),
    ) / 100.0;
    let cut = (widget.width / 4).min(12).min(widget.height / 4);
    let title = widget
        .settings
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("");
    let edge = frame_width(widget);
    let expanded = expanded_widget(widget);
    let mask = aperture_mask([(
        i64::from(edge),
        i64::from(edge),
        widget.width,
        widget.height,
    )]);
    let mut nodes = vec![
        el(
            &format!("{key}:empty-frame"),
            "div",
            &[
                ("class", "empty-frame".into()),
                (
                    "style",
                    format!(
                        "position:absolute;left:-{edge}px;top:-{edge}px;width:{}px;height:{}px;{mask}",
                        widget.width.saturating_add(2 * edge),
                        widget.height.saturating_add(2 * edge)
                    ),
                ),
            ],
            vec![el(
                &format!("{key}:empty-frame:origin"),
                "div",
                &[(
                    "style",
                    format!(
                        "position:absolute;left:{}px;top:{}px;width:{}px;height:{}px",
                        i64::from(edge) - 8,
                        i64::from(edge) - 8,
                        expanded.width,
                        expanded.height
                    ),
                )],
                vec![frame(
                    &format!("{key}:empty-frame:material"),
                    &expanded,
                    skin,
                )],
            )],
        ),
        el(
            &format!("{key}:empty-fill"),
            "div",
            &[
                ("class", "empty-fill".into()),
                (
                    "style",
                    format!(
                        "position:absolute;inset:0;background:rgba(0,0,0,{opacity});clip-path:polygon({cut}px 0,calc(100% - {cut}px) 0,100% {cut}px,100% calc(100% - {cut}px),calc(100% - {cut}px) 100%,{cut}px 100%,0 calc(100% - {cut}px),0 {cut}px)"
                    ),
                ),
            ],
            vec![],
        ),
    ];
    if !title.is_empty() {
        nodes.push(node_text(
            &format!("{key}:empty-title"),
            "div",
            &[("class", "empty-title".into())],
            title,
        ));
    }
    el(
        &format!("{key}:empty"),
        "div",
        &[("class", "empty-widget".into())],
        nodes,
    )
}
