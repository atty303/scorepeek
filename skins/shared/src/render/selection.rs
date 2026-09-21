use super::{chart_rail, chrome, label, lamp};
use crate::primitive::{el, field, node_text};
use crate::theme::Skin;
use crate::value::{shown, string, value_text};
use scorepeek_skin_sdk::{Node, Widget};
use serde_json::Value;
pub(crate) fn selection(key: &str, widget: &Widget, state: &Value, skin: Skin) -> Node {
    let chart = state.get("chart").unwrap_or(&Value::Null);
    let play_type = string(chart, "play_type");
    let mode = match play_type.as_str() {
        "single" => "SP",
        "double" => "DP",
        other => other,
    };
    let difficulty = string(chart, "difficulty").to_ascii_uppercase();
    let recorded = state
        .pointer("/history/recorded")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    el(
        &format!("{key}:selection"),
        "section",
        &[("class", "widget selection-widget".into())],
        vec![
            chrome(key, widget, skin),
            el(
                &format!("{key}:content"),
                "div",
                &[("class", "widget-content selection-content".into())],
                vec![
                    lamp(
                        &format!("{key}:recorded"),
                        if recorded { "active" } else { "inactive" },
                        None,
                        true,
                        skin,
                    ),
                    el(
                        &format!("{key}:song"),
                        "div",
                        &[("class", "song-copy".into())],
                        vec![
                            node_text(
                                &format!("{key}:title"),
                                "h1",
                                &[("title", string(chart, "title"))],
                                &string(chart, "title"),
                            ),
                            node_text(
                                &format!("{key}:artist"),
                                "p",
                                &[("title", string(chart, "artist"))],
                                &string(chart, "artist"),
                            ),
                        ],
                    ),
                    el(
                        &format!("{key}:rail"),
                        "div",
                        &[("class", "chart-rail".into())],
                        vec![
                            chart_rail(
                                &format!("{key}:rail:svg"),
                                widget.width.saturating_sub(56),
                                skin,
                            ),
                            el(
                                &format!("{key}:mode"),
                                "span",
                                &[("class", "play-type".into())],
                                vec![label(&format!("{key}:mode:label"), mode, skin, 24)],
                            ),
                            el(
                                &format!("{key}:difficulty"),
                                "span",
                                &[
                                    ("class", "difficulty".into()),
                                    ("data-difficulty", difficulty.clone()),
                                ],
                                vec![label(
                                    &format!("{key}:difficulty:label"),
                                    &difficulty,
                                    skin,
                                    20,
                                )],
                            ),
                            field(
                                &format!("{key}:level"),
                                "LV",
                                &shown(value_text(chart.get("level"))),
                                skin,
                            ),
                            field(
                                &format!("{key}:notes"),
                                "NOTES",
                                &shown(value_text(chart.get("notes"))),
                                skin,
                            ),
                        ],
                    ),
                ],
            ),
        ],
    )
}
