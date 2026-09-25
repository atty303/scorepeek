//! History and aperture renderers for the precision instrument design.

use super::{element, frame_node, panel, text_element, value};
use scorepeek_skin_sdk::{Node, Widget};
use serde_json::Value;

fn row_cell(key: &str, field: &str, tag: &str, value: &str) -> Node {
    text_element(
        &format!("{key}:{field}"),
        tag,
        &format!("history-cell {field}"),
        value,
    )
}

pub(super) fn history_list(key: &str, widget: &Widget, state: &Value) -> Node {
    let columns = ["DATE", "SCORE", "DJ LEVEL", "MISS", "CLEAR"];
    let mut rows = vec![element(
        &format!("{key}:columns"),
        "div",
        &[("class", "history-columns".into())],
        columns
            .iter()
            .enumerate()
            .map(|(index, name)| {
                text_element(
                    &format!("{key}:column:{index}"),
                    "span",
                    "history-column",
                    name,
                )
            })
            .collect(),
    )];
    let limit = usize::try_from(
        widget
            .settings
            .get("history_count")
            .and_then(Value::as_u64)
            .unwrap_or(5),
    )
    .unwrap_or(usize::MAX);
    if let Some(plays) = state.pointer("/history/plays").and_then(Value::as_array) {
        for (index, play) in plays.iter().take(limit).enumerate() {
            let row_key = format!("{key}:row:{index}");
            let rank = value(play, "/dj_level");
            let clear = value(play, "/clear");
            rows.push(element(
                &row_key,
                "div",
                &[("class", "history-record".into())],
                vec![
                    row_cell(&row_key, "date", "time", &value(play, "/notified_at")),
                    row_cell(&row_key, "score", "b", &value(play, "/score")),
                    element(
                        &format!("{row_key}:rank"),
                        "span",
                        &[
                            ("class", "history-cell rank".into()),
                            ("data-rank", rank.clone()),
                        ],
                        vec![Node::text(format!("{row_key}:rank:text"), rank)],
                    ),
                    row_cell(&row_key, "miss", "span", &value(play, "/miss")),
                    element(
                        &format!("{row_key}:clear"),
                        "span",
                        &[
                            ("class", "history-cell clear".into()),
                            ("data-clear", clear.clone()),
                        ],
                        vec![Node::text(format!("{row_key}:clear:text"), clear)],
                    ),
                ],
            ));
        }
    }
    panel(
        key,
        "history-list-panel",
        widget,
        vec![
            text_element(
                &format!("{key}:heading"),
                "h2",
                "history-heading",
                "HISTORY",
            ),
            element(
                &format!("{key}:table"),
                "div",
                &[("class", "history-table".into())],
                rows,
            ),
        ],
    )
}

fn plot_point(
    play: &Value,
    metric: &str,
    start: i64,
    end: i64,
    view_height: f64,
) -> Option<(f64, f64)> {
    let time = play.get("received_unix_ms")?.as_i64()?;
    if time < start || time > end {
        return None;
    }
    let ratio = play.get(metric)?.as_f64()?;
    let x = (time.saturating_sub(start) as f64 / end.saturating_sub(start).max(1) as f64)
        .clamp(0.0, 1.0)
        * 1000.0;
    let y = (1.0 - ratio.clamp(0.0, 1.0)) * view_height;
    Some((x, y))
}

fn line(key: &str, class: &str, points: &[(f64, f64)]) -> Node {
    let coords = points
        .iter()
        .map(|(x, y)| format!("{x:.2},{y:.2}"))
        .collect::<Vec<_>>()
        .join(" ");
    element(
        key,
        "polyline",
        &[
            ("class", class.into()),
            ("points", coords),
            ("fill", "none".into()),
            (
                "stroke",
                if class == "score-curve" {
                    "#4fe6ed"
                } else {
                    "#f1a287"
                }
                .into(),
            ),
            (
                "stroke-width",
                if class == "score-curve" { "2.4" } else { "1.8" }.into(),
            ),
            ("vector-effect", "non-scaling-stroke".into()),
        ],
        vec![],
    )
}

pub(super) fn history_graph(key: &str, widget: &Widget, state: &Value) -> Node {
    let months = widget
        .settings
        .get("graph_months")
        .and_then(Value::as_u64)
        .unwrap_or(6);
    let start_index = match months {
        1 => 0,
        3 => 1,
        12 => 3,
        _ => 2,
    };
    let start = state
        .pointer("/history/graph_start_unix_ms")
        .and_then(Value::as_array)
        .and_then(|starts| starts.get(start_index))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let end = state
        .pointer("/history/graph_end_unix_ms")
        .and_then(Value::as_i64)
        .unwrap_or(start.saturating_add(1))
        .max(start.saturating_add(1));
    let plot_width = widget.width.saturating_sub(122).max(1);
    let plot_height = widget.height.saturating_sub(80).max(1);
    let view_height = 1000.0 * f64::from(plot_height) / f64::from(plot_width);
    let plays = state.pointer("/history/graph").and_then(Value::as_array);
    let score_points = plays
        .into_iter()
        .flatten()
        .filter_map(|play| plot_point(play, "score_ratio", start, end, view_height))
        .collect::<Vec<_>>();
    let mut miss_runs = Vec::<Vec<(f64, f64)>>::new();
    let mut current_run = Vec::new();
    for play in plays.into_iter().flatten() {
        if let Some(point) = plot_point(play, "miss_ratio", start, end, view_height) {
            current_run.push(point);
        } else if !current_run.is_empty() {
            miss_runs.push(std::mem::take(&mut current_run));
        }
    }
    if !current_run.is_empty() {
        miss_runs.push(current_run);
    }
    let mut curves = vec![line(
        &format!("{key}:score-line"),
        "score-curve",
        &score_points,
    )];
    curves.extend(
        miss_runs
            .iter()
            .enumerate()
            .map(|(index, run)| line(&format!("{key}:miss-line:{index}"), "miss-curve", run)),
    );
    let ranks = [
        ("AAA", 8_u8),
        ("AA", 7),
        ("A", 6),
        ("B", 5),
        ("C", 4),
        ("D", 3),
        ("E", 2),
        ("F", 0),
    ];
    let rank_axis = element(
        &format!("{key}:levels"),
        "div",
        &[("class", "graph-level-axis".into())],
        ranks
            .iter()
            .enumerate()
            .map(|(index, (name, threshold))| {
                element(
                    &format!("{key}:level:{index}"),
                    "span",
                    &[
                        ("class", "graph-level".into()),
                        (
                            "style",
                            format!("top:{:.3}%", (1.0 - f64::from(*threshold) / 9.0) * 100.0),
                        ),
                    ],
                    vec![Node::text(format!("{key}:level:{index}:text"), *name)],
                )
            })
            .collect(),
    );
    let miss_axis = element(
        &format!("{key}:miss-axis"),
        "div",
        &[("class", "graph-miss-axis".into())],
        ["100%", "75%", "50%", "25%", "0%"]
            .iter()
            .enumerate()
            .map(|(index, name)| {
                text_element(
                    &format!("{key}:miss-tick:{index}"),
                    "span",
                    "graph-miss-tick",
                    name,
                )
            })
            .collect(),
    );
    let ticks = state
        .pointer("/history/graph_ticks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, tick)| {
            let time = tick.get("unix_ms")?.as_i64()?;
            (start..=end).contains(&time).then(|| {
                element(
                    &format!("{key}:time:{index}"),
                    "span",
                    &[
                        ("class", "graph-time-tick".into()),
                        (
                            "style",
                            format!(
                                "left:{:.2}%",
                                (time.saturating_sub(start) as f64
                                    / end.saturating_sub(start).max(1) as f64)
                                    * 100.0
                            ),
                        ),
                    ],
                    vec![Node::text(
                        format!("{key}:time:{index}:text"),
                        value(tick, "/label"),
                    )],
                )
            })
        })
        .collect();
    panel(
        key,
        "history-graph-panel",
        widget,
        vec![
            element(
                &format!("{key}:graph-head"),
                "div",
                &[("class", "graph-head".into())],
                vec![
                    text_element(
                        &format!("{key}:heading"),
                        "h2",
                        "history-heading",
                        "HISTORY GRAPH",
                    ),
                    text_element(
                        &format!("{key}:score-legend"),
                        "span",
                        "graph-legend score-legend",
                        "DJ LEVEL",
                    ),
                    text_element(
                        &format!("{key}:miss-legend"),
                        "span",
                        "graph-legend miss-legend",
                        "MISS RATE",
                    ),
                ],
            ),
            element(
                &format!("{key}:graph-body"),
                "div",
                &[("class", "graph-body".into())],
                vec![
                    rank_axis,
                    element(
                        &format!("{key}:plot"),
                        "div",
                        &[("class", "graph-plot".into())],
                        vec![
                            element(
                                &format!("{key}:grid"),
                                "div",
                                &[("class", "graph-grid".into())],
                                vec![],
                            ),
                            element(
                                &format!("{key}:svg"),
                                "svg",
                                &[
                                    ("class", "graph-svg".into()),
                                    ("viewBox", format!("0 0 1000 {view_height:.2}")),
                                    ("preserveAspectRatio", "none".into()),
                                ],
                                curves,
                            ),
                        ],
                    ),
                    miss_axis,
                ],
            ),
            element(
                &format!("{key}:time-axis"),
                "div",
                &[("class", "graph-time-axis".into())],
                ticks,
            ),
        ],
    )
}

pub(super) fn empty(key: &str, widget: &Widget) -> Node {
    let frame = widget
        .properties
        .get("frame-width")
        .and_then(Value::as_str)
        .unwrap_or("m");
    let opacity = widget
        .properties
        .get("fill-opacity-percent")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(100);
    let title = widget
        .settings
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut children = vec![
        element(
            &format!("{key}:aperture"),
            "div",
            &[
                ("class", "empty-aperture".into()),
                (
                    "style",
                    format!("background:rgba(4,13,22,{:.2})", opacity as f64 / 100.0),
                ),
            ],
            vec![],
        ),
        frame_node(key),
    ];
    if !title.is_empty() {
        children.push(text_element(
            &format!("{key}:title"),
            "div",
            "empty-title",
            title,
        ));
    }
    element(
        &format!("{key}:empty"),
        "section",
        &[
            ("class", "instrument-panel empty-panel".into()),
            ("data-frame-width", frame.into()),
        ],
        children,
    )
}
