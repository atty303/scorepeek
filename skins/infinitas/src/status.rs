use crate::{element, group, letters, text};
use scorepeek_skin_sdk::{Node, Widget};
use serde_json::Value;
pub fn render(w: &Widget, state: &Value) -> Node {
    let mut signals = vec![];
    for (name, value) in [
        ("SYSTEM", text(&state["system"])),
        ("RESULT", text(&state["result_signal"])),
        (
            "SELECT",
            if state["history"]["recorded"].as_bool() == Some(true) {
                "active"
            } else {
                "inactive"
            },
        ),
    ] {
        let value = if matches!(value, "active" | "error") {
            value
        } else {
            "inactive"
        };
        signals.push(group(
            &format!("{}-{name}", w.id),
            "signal",
            "",
            vec![
                element(
                    &format!("{}-{name}-lamp", w.id),
                    "span",
                    &[("class", &format!("lamp {value}")), ("aria-label", value)],
                    vec![],
                ),
                letters(&format!("{}-{name}-label", w.id), name, 16.0, "silver"),
            ],
        ));
    }
    group(
        &format!("{}-content", w.id),
        "status-content",
        "",
        vec![
            element(
                &format!("{}-logo", w.id),
                "img",
                &[
                    ("class", "status-logo"),
                    ("src", "scorepeek-logo.png"),
                    ("alt", "scorepeek"),
                ],
                vec![],
            ),
            group(&format!("{}-signals", w.id), "signals", "", signals),
        ],
    )
}
