use super::{chrome, lamp};
use crate::primitive::{el, text};
use crate::theme::Skin;
use scorepeek_skin_sdk::{Node, Widget};
use serde_json::Value;
pub(crate) fn status(key: &str, widget: &Widget, state: &Value, skin: Skin) -> Node {
    el(
        &format!("{key}:status"),
        "section",
        &[("class", "widget status-widget".into())],
        vec![
            chrome(key, widget, skin),
            el(
                &format!("{key}:content"),
                "div",
                &[("class", "widget-content status-content".into())],
                vec![
                    el(
                        &format!("{key}:wordmark"),
                        "span",
                        &[("class", "wordmark".into())],
                        vec![
                            text(&format!("{key}:wordmark:score"), "score"),
                            el(
                                &format!("{key}:wordmark:peek"),
                                "span",
                                &[],
                                vec![text(&format!("{key}:wordmark:peek:text"), "peek")],
                            ),
                        ],
                    ),
                    el(
                        &format!("{key}:lamps"),
                        "div",
                        &[("class", "status-lamps".into())],
                        vec![
                            lamp(
                                &format!("{key}:system"),
                                state
                                    .get("system")
                                    .and_then(Value::as_str)
                                    .unwrap_or("inactive"),
                                Some("SYSTEM"),
                                false,
                                skin,
                            ),
                            lamp(
                                &format!("{key}:result"),
                                state
                                    .get("result_signal")
                                    .and_then(Value::as_str)
                                    .unwrap_or("inactive"),
                                Some("RESULT"),
                                false,
                                skin,
                            ),
                        ],
                    ),
                ],
            ),
        ],
    )
}
