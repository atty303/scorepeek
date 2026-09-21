use super::{chrome, label};
use crate::motion::{clear_motion_style, dot_style, time_ratio};
use crate::primitive::{el, heading, label_class, label_element, node_text, polyline};
use crate::theme::Skin;
use crate::value::{clear_role, integer, number, string};
use scorepeek_skin_sdk::{Node, Widget};
use serde_json::Value;
pub(crate) fn history_list(key: &str, widget: &Widget, state: &Value, skin: Skin) -> Node {
    let count = usize::try_from(
        widget
            .settings
            .get("history_count")
            .and_then(Value::as_u64)
            .unwrap_or(5),
    )
    .unwrap_or(usize::MAX);
    let mut rows = vec![
        heading(&format!("{key}:history:title"), "HISTORY", skin),
        el(
            &format!("{key}:history:head"),
            "div",
            &[("class", "history-row history-head".into())],
            ["DATE", "EX SCORE", "DJ LEVEL", "MISS", "CLEAR"]
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    label_element(&format!("{key}:history:head:{i}"), "span", name, skin, 12)
                })
                .collect(),
        ),
    ];
    if let Some(plays) = state.pointer("/history/plays").and_then(Value::as_array) {
        for (i, play) in plays.iter().take(count).enumerate() {
            let clear = string(play, "clear");
            rows.push(el(
                &format!("{key}:history:{i}"),
                "div",
                &[("class", "history-row".into())],
                vec![
                    node_text(
                        &format!("{key}:history:{i}:date"),
                        "time",
                        &[],
                        &string(play, "notified_at"),
                    ),
                    node_text(
                        &format!("{key}:history:{i}:score"),
                        "b",
                        &[],
                        &string(play, "score"),
                    ),
                    node_text(
                        &format!("{key}:history:{i}:rank"),
                        "span",
                        &[("data-rank", string(play, "dj_level"))],
                        &string(play, "dj_level"),
                    ),
                    node_text(
                        &format!("{key}:history:{i}:miss"),
                        "span",
                        &[],
                        &string(play, "miss"),
                    ),
                    el(
                        &format!("{key}:history:{i}:clear"),
                        "span",
                        &[
                            ("data-clear", clear_role(&clear).into()),
                            ("style", clear_motion_style(&clear)),
                        ],
                        vec![label(
                            &format!("{key}:history:{i}:clear:label"),
                            &clear,
                            skin,
                            16,
                        )],
                    ),
                ],
            ));
        }
    }
    el(
        &format!("{key}:history-widget"),
        "section",
        &[("class", "widget history-list-widget".into())],
        vec![
            chrome(key, widget, skin),
            el(
                &format!("{key}:content"),
                "div",
                &[("class", "widget-content history-content".into())],
                rows,
            ),
        ],
    )
}

#[allow(clippy::too_many_lines)]
pub(crate) fn history_graph(key: &str, widget: &Widget, state: &Value, skin: Skin) -> Node {
    let (px, py, score_axis, miss_axis, header) = (18_u32, 12_u32, 36_u32, 39_u32, 68_u32);
    let width = widget
        .width
        .saturating_sub(2 * px + score_axis + miss_axis)
        .max(1);
    let height = widget.height.saturating_sub(2 * py + header).max(1);
    let months = widget
        .settings
        .get("graph_months")
        .and_then(Value::as_u64)
        .unwrap_or(6);
    let index = match months {
        1 => 0,
        3 => 1,
        12 => 3,
        _ => 2,
    };
    let start = state
        .pointer("/history/graph_start_unix_ms")
        .and_then(Value::as_array)
        .and_then(|v| v.get(index))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let end = state
        .pointer("/history/graph_end_unix_ms")
        .and_then(Value::as_i64)
        .unwrap_or(start + 1);
    let values: Vec<&Value> = state
        .pointer("/history/graph")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|v| integer(v, "received_unix_ms") >= start)
        .collect();
    let point = |v: &Value, ratio: f64| {
        format!(
            "{:.2},{:.2}",
            time_ratio(integer(v, "received_unix_ms"), start, end) * 1000.0,
            100.0 - ratio.clamp(0.0, 1.0) * 100.0
        )
    };
    let score_points = values
        .iter()
        .map(|v| point(v, number(v, "score_ratio")))
        .collect::<Vec<_>>()
        .join(" ");
    let mut miss_segments = Vec::new();
    let mut segment = Vec::new();
    for value in &values {
        if let Some(ratio) = value.get("miss_ratio").and_then(Value::as_f64) {
            segment.push(point(value, ratio));
        } else if !segment.is_empty() {
            miss_segments.push(std::mem::take(&mut segment).join(" "));
        }
    }
    if !segment.is_empty() {
        miss_segments.push(segment.join(" "));
    }
    let levels = [
        ("AAA", 88.889),
        ("AA", 77.778),
        ("A", 66.667),
        ("B", 55.556),
        ("C", 44.444),
        ("D", 33.333),
        ("E", 22.222),
    ];
    let mut plot = Vec::new();
    for (i, (name, threshold)) in levels.iter().enumerate() {
        plot.push(el(
            &format!("{key}:graph:threshold:{i}"),
            "i",
            &[
                ("class", "threshold".into()),
                ("data-level", (*name).into()),
                ("style", format!("top:{:.3}%", 100.0 - threshold)),
            ],
            vec![],
        ));
    }
    for (i, value) in values.iter().enumerate() {
        plot.push(el(
            &format!("{key}:graph:score-dot:{i}"),
            "i",
            &[
                ("class", "graph-dot score-dot".into()),
                (
                    "style",
                    dot_style(value, number(value, "score_ratio"), start, end),
                ),
            ],
            vec![],
        ));
        if let Some(ratio) = value.get("miss_ratio").and_then(Value::as_f64) {
            plot.push(el(
                &format!("{key}:graph:miss-dot:{i}"),
                "i",
                &[
                    ("class", "graph-dot miss-dot".into()),
                    ("style", dot_style(value, ratio, start, end)),
                ],
                vec![],
            ));
        }
    }
    let (score_color, miss_color) = (skin.graph_score, skin.graph_miss);
    let mut lines = vec![polyline(
        &format!("{key}:graph:score-line"),
        "score-line",
        score_color,
        &score_points,
    )];
    for (i, points) in miss_segments.iter().enumerate() {
        lines.push(polyline(
            &format!("{key}:graph:miss-line:{i}"),
            "miss-line",
            miss_color,
            points,
        ));
    }
    plot.push(el(
        &format!("{key}:graph:svg"),
        "svg",
        &[
            ("width", width.to_string()),
            ("height", height.to_string()),
            ("viewBox", "0 0 1000 100".into()),
            ("preserveAspectRatio", "none".into()),
        ],
        lines,
    ));
    let level_axis = el(
        &format!("{key}:graph:levels"),
        "div",
        &[("class", "level-axis".into())],
        levels
            .iter()
            .enumerate()
            .map(|(i, (name, threshold))| {
                el(
                    &format!("{key}:graph:level:{i}"),
                    "span",
                    &[("style", format!("top:{:.3}%", 100.0 - threshold))],
                    vec![label(
                        &format!("{key}:graph:level:{i}:label"),
                        name,
                        skin,
                        14,
                    )],
                )
            })
            .collect(),
    );
    let miss_axis_node = el(
        &format!("{key}:graph:miss-axis"),
        "div",
        &[("class", "miss-axis".into())],
        ["100%", "75%", "50%", "25%", "0%"]
            .iter()
            .enumerate()
            .map(|(i, name)| {
                label_element(
                    &format!("{key}:graph:miss-axis:{i}"),
                    "span",
                    name,
                    skin,
                    14,
                )
            })
            .collect(),
    );
    let mut ticks = Vec::new();
    if let Some(items) = state
        .pointer("/history/graph_ticks")
        .and_then(Value::as_array)
    {
        for (i, tick) in items.iter().enumerate() {
            let time = integer(tick, "unix_ms");
            if time >= start && time <= end {
                ticks.push(node_text(
                    &format!("{key}:graph:tick:{i}"),
                    "span",
                    &[(
                        "style",
                        format!("left:{:.3}%", time_ratio(time, start, end) * 100.0),
                    )],
                    &string(tick, "label"),
                ));
            }
        }
    }
    let geometry = format!(
        "--graph-padding-x:{px}px;--graph-padding-y:{py}px;--score-axis:{score_axis}px;--miss-axis:{miss_axis}px;--plot-width:{width}px;--plot-height:{height}px"
    );
    el(
        &format!("{key}:graph-widget"),
        "section",
        &[
            ("class", "widget history-graph-widget".into()),
            ("style", geometry),
        ],
        vec![
            chrome(key, widget, skin),
            el(
                &format!("{key}:content"),
                "div",
                &[("class", "widget-content graph-content".into())],
                vec![
                    heading(&format!("{key}:graph:title"), "HISTORY GRAPH", skin),
                    el(
                        &format!("{key}:graph:legend"),
                        "div",
                        &[("class", "graph-legend".into())],
                        vec![
                            label_class(
                                &format!("{key}:graph:score-key"),
                                "score-key",
                                "DJ LEVEL",
                                skin,
                                14,
                            ),
                            label_class(
                                &format!("{key}:graph:miss-key"),
                                "miss-key",
                                "MISS RATE",
                                skin,
                                14,
                            ),
                        ],
                    ),
                    el(
                        &format!("{key}:graph:plot"),
                        "div",
                        &[("class", "plot".into())],
                        vec![
                            level_axis,
                            el(
                                &format!("{key}:graph:plot-area"),
                                "div",
                                &[("class", "plot-area".into())],
                                plot,
                            ),
                            miss_axis_node,
                        ],
                    ),
                    el(
                        &format!("{key}:graph:time"),
                        "div",
                        &[("class", "time-axis".into())],
                        ticks,
                    ),
                ],
            ),
        ],
    )
}
