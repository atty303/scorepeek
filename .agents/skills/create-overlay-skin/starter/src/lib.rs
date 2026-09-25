//! Neutral, self-contained baseline skin tree for ABI v2.

mod extras;
mod metrics;

use extras::{empty, history_graph, history_list};
use metrics::ScoreMetrics;
use scorepeek_skin_sdk::{Input, Node, Output, Schedule, Widget};
use serde_json::Value;
use std::collections::BTreeMap;

fn element(key: &str, tag: &str, attributes: &[(&str, String)], children: Vec<Node>) -> Node {
    Node::element(
        key,
        tag,
        attributes
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect::<BTreeMap<_, _>>(),
        children,
    )
}

fn text_element(key: &str, tag: &str, class: &str, value: &str) -> Node {
    element(
        key,
        tag,
        &[("class", class.into())],
        vec![Node::text(format!("{key}:text"), value)],
    )
}

fn value(state: &Value, pointer: &str) -> String {
    state
        .pointer(pointer)
        .and_then(|item| match item {
            Value::String(value) => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "—".into())
}

fn panel(key: &str, kind: &str, widget: &Widget, content: Vec<Node>) -> Node {
    let frame = widget
        .properties
        .get("frame-width")
        .and_then(Value::as_str)
        .unwrap_or("m");
    element(
        &format!("{key}:panel"),
        "section",
        &[
            ("class", format!("base-panel {kind}")),
            ("data-frame-width", frame.into()),
        ],
        vec![
            frame_node(key),
            element(
                &format!("{key}:content"),
                "div",
                &[("class", "base-content".into())],
                content,
            ),
        ],
    )
}

fn frame_node(key: &str) -> Node {
    element(
        &format!("{key}:frame"),
        "div",
        &[
            ("class", "base-frame".into()),
            ("aria-hidden", "true".into()),
        ],
        ["top", "bottom", "left", "right", "tl", "tr", "bl", "br"]
            .iter()
            .map(|part| {
                element(
                    &format!("{key}:frame:{part}"),
                    "i",
                    &[("class", format!("frame-piece frame-{part}"))],
                    vec![],
                )
            })
            .collect(),
    )
}

fn status(key: &str, widget: &Widget, state: &Value) -> Node {
    let signals = [
        ("system", "SYSTEM", "system"),
        ("result", "RESULT", "result_signal"),
        ("select", "SELECT", "history.recorded"),
    ];
    let lamps = signals
        .iter()
        .map(|(id, label, field)| {
            let mode = if *id == "select" {
                if state
                    .pointer("/history/recorded")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    "active"
                } else {
                    "inactive"
                }
            } else {
                state
                    .get(field)
                    .and_then(Value::as_str)
                    .unwrap_or("inactive")
            };
            element(
                &format!("{key}:{id}"),
                "div",
                &[
                    ("class", "signal".into()),
                    ("data-signal", (*id).into()),
                    ("data-state", mode.into()),
                ],
                vec![
                    element(
                        &format!("{key}:{id}:lamp"),
                        "i",
                        &[
                            ("class", "signal-lamp".into()),
                            ("aria-hidden", "true".into()),
                        ],
                        vec![],
                    ),
                    text_element(&format!("{key}:{id}:label"), "span", "signal-label", label),
                ],
            )
        })
        .collect();
    panel(
        key,
        "status-panel",
        widget,
        vec![
            element(
                &format!("{key}:brand"),
                "div",
                &[
                    ("class", "brand".into()),
                    ("role", "img".into()),
                    ("aria-label", "scorepeek".into()),
                ],
                vec![],
            ),
            element(
                &format!("{key}:signals"),
                "div",
                &[("class", "signals".into())],
                lamps,
            ),
        ],
    )
}

fn selection(key: &str, widget: &Widget, state: &Value) -> Node {
    let mode = match state.pointer("/chart/play_type").and_then(Value::as_str) {
        Some("single") => "SP".to_owned(),
        Some("double") => "DP".to_owned(),
        Some(other) => other.to_owned(),
        None => "—".into(),
    };
    let difficulty = value(state, "/chart/difficulty").to_ascii_uppercase();
    let rail = vec![
        text_element(&format!("{key}:mode"), "span", "chart-field mode", &mode),
        element(
            &format!("{key}:difficulty"),
            "span",
            &[
                ("class", "chart-field difficulty".into()),
                ("data-difficulty", difficulty.clone()),
            ],
            vec![Node::text(format!("{key}:difficulty:text"), difficulty)],
        ),
        chart_labeled_field(key, "level", "LV", &value(state, "/chart/level")),
        chart_labeled_field(key, "notes", "NOTES", &value(state, "/chart/notes")),
    ];
    panel(
        key,
        "selection-panel",
        widget,
        vec![
            element(
                &format!("{key}:song"),
                "div",
                &[("class", "song-copy".into())],
                vec![
                    text_element(
                        &format!("{key}:title"),
                        "h1",
                        "song-title",
                        &value(state, "/chart/title"),
                    ),
                    text_element(
                        &format!("{key}:artist"),
                        "p",
                        "song-artist",
                        &value(state, "/chart/artist"),
                    ),
                ],
            ),
            element(
                &format!("{key}:rail"),
                "div",
                &[("class", "chart-rail".into())],
                rail,
            ),
        ],
    )
}

fn chart_labeled_field(key: &str, id: &str, label: &str, field_value: &str) -> Node {
    element(
        &format!("{key}:{id}"),
        "span",
        &[("class", format!("chart-field {id}"))],
        vec![
            text_element(&format!("{key}:{id}:label"), "span", "field-label", label),
            text_element(
                &format!("{key}:{id}:value"),
                "b",
                "field-value",
                field_value,
            ),
        ],
    )
}

fn rank_delta(key: &str, delta: &str) -> Node {
    let mut parts = Vec::new();
    if let Some(sign) = delta.find(['+', '-']) {
        let (prefix, digits) = delta.split_at(sign + 1);
        let zeros = digits
            .bytes()
            .take_while(|digit| *digit == b'0')
            .count()
            .min(digits.len().saturating_sub(1));
        parts.push(text_element(
            &format!("{key}:prefix"),
            "span",
            "rank-delta-prefix",
            prefix,
        ));
        if zeros > 0 {
            parts.push(text_element(
                &format!("{key}:zeros"),
                "span",
                "rank-delta-zeros",
                &digits[..zeros],
            ));
        }
        parts.push(text_element(
            &format!("{key}:digits"),
            "span",
            "rank-delta-digits",
            &digits[zeros..],
        ));
    } else {
        parts.push(Node::text(format!("{key}:text"), delta));
    }
    element(key, "span", &[("class", "rank-delta".into())], parts)
}

fn score(key: &str, widget: &Widget, state: &Value) -> Node {
    let rank = value(state, "/best/dj_level");
    let clear = value(state, "/best/clear");
    let metrics = ScoreMetrics::new(
        state
            .pointer("/best/score")
            .and_then(Value::as_str)
            .unwrap_or(""),
        state.pointer("/chart/notes").and_then(Value::as_u64),
        &rank,
    );
    let fill_width = metrics
        .as_ref()
        .map_or_else(|| "0%".to_owned(), ScoreMetrics::css_width);
    let delta = metrics
        .as_ref()
        .and_then(|metrics| metrics.delta.as_deref())
        .unwrap_or("—");
    let rows = [
        ("PGREAT", "pgreat"),
        ("GREAT", "great"),
        ("GOOD", "good"),
        ("BAD", "bad"),
        ("POOR", "poor"),
        ("FAST", "fast"),
        ("SLOW", "slow"),
        ("COMBO BREAK", "combo_break"),
        ("PLAY OPTIONS", "play_options"),
    ];
    let detail = rows
        .iter()
        .map(|(label, field)| {
            element(
                &format!("{key}:detail:{field}"),
                "div",
                &[("class", format!("detail-row {field}"))],
                vec![
                    text_element(
                        &format!("{key}:detail:{field}:label"),
                        "span",
                        "detail-label",
                        label,
                    ),
                    text_element(
                        &format!("{key}:detail:{field}:value"),
                        "b",
                        "detail-value",
                        &value(state, &format!("/detail/{field}")),
                    ),
                ],
            )
        })
        .collect();
    panel(
        key,
        "score-panel",
        widget,
        vec![
            element(
                &format!("{key}:best"),
                "div",
                &[("class", "best-group".into())],
                vec![
                    element(
                        &format!("{key}:best:primary"),
                        "div",
                        &[("class", "best-primary".into())],
                        vec![
                            element(
                                &format!("{key}:score"),
                                "div",
                                &[("class", "score-block".into())],
                                vec![
                                    text_element(
                                        &format!("{key}:score:label"),
                                        "span",
                                        "small-label",
                                        "SCORE",
                                    ),
                                    text_element(
                                        &format!("{key}:score:value"),
                                        "strong",
                                        "score-value",
                                        &value(state, "/best/score"),
                                    ),
                                ],
                            ),
                            element(
                                &format!("{key}:rank"),
                                "div",
                                &[("class", "rank-block".into()), ("data-rank", rank.clone())],
                                vec![
                                    text_element(
                                        &format!("{key}:rank:label"),
                                        "span",
                                        "small-label",
                                        "DJ LEVEL",
                                    ),
                                    text_element(
                                        &format!("{key}:rank:value"),
                                        "b",
                                        "rank-value",
                                        &rank,
                                    ),
                                    rank_delta(&format!("{key}:rank:delta"), delta),
                                ],
                            ),
                        ],
                    ),
                    element(
                        &format!("{key}:best:result"),
                        "div",
                        &[("class", "best-result".into())],
                        vec![
                            element(
                                &format!("{key}:miss"),
                                "div",
                                &[("class", "miss-block".into())],
                                vec![
                                    text_element(
                                        &format!("{key}:miss:label"),
                                        "span",
                                        "small-label",
                                        "MISS COUNT",
                                    ),
                                    text_element(
                                        &format!("{key}:miss:value"),
                                        "b",
                                        "miss-value",
                                        &value(state, "/best/miss"),
                                    ),
                                ],
                            ),
                            element(
                                &format!("{key}:clear"),
                                "div",
                                &[
                                    ("class", "clear-block".into()),
                                    ("data-clear", clear.clone()),
                                ],
                                vec![
                                    text_element(
                                        &format!("{key}:clear:label"),
                                        "span",
                                        "small-label",
                                        "CLEAR TYPE",
                                    ),
                                    text_element(
                                        &format!("{key}:clear:value"),
                                        "b",
                                        "clear-value",
                                        &clear,
                                    ),
                                ],
                            ),
                        ],
                    ),
                    score_progress(key, &fill_width),
                ],
            ),
            element(
                &format!("{key}:detail"),
                "div",
                &[("class", "detail-group".into())],
                vec![element(
                    &format!("{key}:detail:rows"),
                    "div",
                    &[("class", "detail-rows".into())],
                    detail,
                )],
            ),
        ],
    )
}

fn score_progress(key: &str, fill_width: &str) -> Node {
    let segments = ["F", "E", "D", "C", "B", "A", "AA", "AAA"]
        .iter()
        .map(|level| {
            text_element(
                &format!("{key}:progress:{level}"),
                "span",
                &format!("progress-segment level-{}", level.to_ascii_lowercase()),
                level,
            )
        })
        .collect();
    element(
        &format!("{key}:progress"),
        "div",
        &[("class", "score-progress".into())],
        vec![element(
            &format!("{key}:progress:track"),
            "div",
            &[("class", "progress-track".into())],
            vec![
                element(
                    &format!("{key}:progress:fill"),
                    "i",
                    &[
                        ("class", "progress-fill".into()),
                        ("style", format!("width:{fill_width}")),
                        ("aria-hidden", "true".into()),
                    ],
                    vec![],
                ),
                element(
                    &format!("{key}:progress:segments"),
                    "div",
                    &[("class", "progress-segments".into())],
                    segments,
                ),
            ],
        )],
    )
}

fn render(input: Input) -> Output {
    let background = input
        .canvas
        .properties
        .get("background")
        .and_then(Value::as_str)
        .unwrap_or("none");
    let mut children = Vec::new();
    if background != "none" {
        children.push(element(
            "background",
            "div",
            &[
                ("class", "canvas-background".into()),
                ("data-motion", background.into()),
                ("aria-hidden", "true".into()),
            ],
            vec![],
        ));
    }
    for widget in &input.widgets {
        let key = format!("widget:{}", widget.id);
        let content = match widget.kind.as_str() {
            "status" => status(&key, widget, &input.state),
            "selection" => selection(&key, widget, &input.state),
            "score" => score(&key, widget, &input.state),
            "history-list" => history_list(&key, widget, &input.state),
            "history-graph" => history_graph(&key, widget, &input.state),
            "empty" => empty(&key, widget),
            _ => element(
                &format!("{key}:pending"),
                "div",
                &[("class", "prototype-pending".into())],
                vec![],
            ),
        };
        children.push(element(
            &key,
            "div",
            &[
                ("class", "widget-slot".into()),
                ("data-widget-id", widget.id.clone()),
                (
                    "style",
                    format!(
                        "left:{}px;top:{}px;width:{}px;height:{}px",
                        widget.x, widget.y, widget.width, widget.height
                    ),
                ),
            ],
            vec![content],
        ));
    }
    Output {
        schedule: Schedule::Idle,
        tree: element(
            "canvas",
            "main",
            &[
                ("class", "overlay-canvas".into()),
                ("data-backend", input.backend),
                (
                    "style",
                    format!(
                        "width:{}px;height:{}px",
                        input.canvas.width, input.canvas.height
                    ),
                ),
            ],
            children,
        ),
    }
}

fn invoke(pointer: i32, length: i32) -> i64 {
    let output = scorepeek_skin_sdk::decode(pointer, length).map_or_else(
        |error| Output {
            schedule: Schedule::Idle,
            tree: text_element("error", "div", "skin-error", &error),
        },
        render,
    );
    scorepeek_skin_sdk::encode(&output)
}

#[unsafe(no_mangle)]
pub extern "C" fn scorepeek_alloc(length: i32) -> i32 {
    scorepeek_skin_sdk::allocate(length)
}

#[unsafe(no_mangle)]
pub extern "C" fn scorepeek_dealloc(pointer: i32, length: i32) {
    scorepeek_skin_sdk::deallocate(pointer, length);
}

#[unsafe(no_mangle)]
pub extern "C" fn scorepeek_init(pointer: i32, length: i32) -> i64 {
    invoke(pointer, length)
}

#[unsafe(no_mangle)]
pub extern "C" fn scorepeek_render(pointer: i32, length: i32) -> i64 {
    invoke(pointer, length)
}
