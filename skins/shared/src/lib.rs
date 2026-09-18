use scorepeek_skin_sdk::{Input, Node, Output, Schedule, Widget};
use serde_json::Value;
use std::{cell::Cell, collections::BTreeMap};

thread_local! {
    static MOTION: Cell<(bool, u64)> = const { Cell::new((false, 0)) };
}

const GLYPHS: &str = "0123456789ABCDEFG-";
const LABELS: &[&str] = &[
    "SYSTEM",
    "RESULT",
    "BEST",
    "RESULT DETAIL",
    "HISTORY",
    "HISTORY GRAPH",
    "EX SCORE",
    "DJ LEVEL",
    "MISS COUNT",
    "LV",
    "NOTES",
    "SP",
    "DP",
    "NORMAL",
    "HYPER",
    "ANOTHER",
    "LEGGENDARIA",
    "PGREAT",
    "GREAT",
    "GOOD",
    "BAD",
    "POOR",
    "FAST",
    "SLOW",
    "COMBO BREAK",
    "PLAY OPTIONS",
    "DATE",
    "MISS",
    "CLEAR",
    "MISS RATE",
    "NO PLAY",
    "FAILED",
    "ASSIST",
    "EASY",
    "HARD",
    "EX HARD",
    "ASSIST CLEAR",
    "EASY CLEAR",
    "HARD CLEAR",
    "EX HARD CLEAR",
    "FULL COMBO",
    "AAA",
    "AA",
    "A",
    "B",
    "C",
    "D",
    "E",
    "100%",
    "75%",
    "50%",
    "25%",
    "0%",
];

#[derive(Clone, Copy)]
pub struct Theme {
    pub graph_score: &'static str,
    pub graph_miss: &'static str,
    pub frame_source: [f64; 2],
    pub frame_factor: f64,
    pub selection_illumination: bool,
    pub status_edge: &'static str,
    pub lamp_accent: &'static str,
    pub rail_edge: &'static str,
    pub rail_inner: &'static str,
    pub rank_material: &'static str,
    pub label_font: &'static str,
    pub label_descent: f64,
}

type Skin = &'static Theme;

#[must_use]
pub fn allocate(length: i32) -> i32 {
    scorepeek_skin_sdk::allocate(length)
}

pub fn deallocate(pointer: i32, length: i32) {
    scorepeek_skin_sdk::deallocate(pointer, length);
}

#[must_use]
pub fn render(pointer: i32, length: i32, theme: &'static Theme) -> i64 {
    let output = scorepeek_skin_sdk::decode(pointer, length)
        .map_or_else(error_tree, |input| tree(input, theme));
    scorepeek_skin_sdk::encode(&output)
}

fn tree(input: Input, theme: Skin) -> Output {
    let native = input.backend == "native";
    MOTION.set((native, input.monotonic_ms));
    let (score, miss) = (theme.graph_score, theme.graph_miss);
    let background = input
        .canvas
        .properties
        .get("background")
        .and_then(Value::as_str)
        .unwrap_or("none");
    let mut children = Vec::new();
    if background != "none" {
        children.push(canvas_background(background, &input.widgets));
    }
    children.extend(
        input
            .widgets
            .iter()
            .map(|widget| widget_slot(widget, &input.state, theme)),
    );
    Output {
        schedule: if native {
            Schedule::NextFrame
        } else {
            Schedule::Idle
        },
        tree: el(
            "canvas",
            "main",
            &[
                ("class", "overlay-canvas".into()),
                ("data-backend", input.backend),
                ("data-canvas-id", input.canvas.id),
                (
                    "style",
                    format!(
                        "width:{}px;height:{}px;--graph-score:{score};--graph-miss:{miss}",
                        input.canvas.width, input.canvas.height
                    ),
                ),
            ],
            children,
        ),
    }
}

fn widget_slot(widget: &Widget, state: &Value, theme: Skin) -> Node {
    let key = format!("widget:{}", widget.id);
    let child = if widget.kind == "empty" {
        empty_widget(&key, widget, theme)
    } else {
        let expanded = expanded_widget(widget);
        el(
            &format!("{key}:origin"),
            "div",
            &[
                ("class", "widget-content-origin".into()),
                (
                    "style",
                    format!(
                        "position:absolute;left:-8px;top:-8px;width:{}px;height:{}px",
                        widget.width + 16,
                        widget.height + 16
                    ),
                ),
            ],
            vec![render_widget(&key, &expanded, state, theme)],
        )
    };
    el(
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
        vec![child],
    )
}

fn expanded_widget(widget: &Widget) -> Widget {
    Widget {
        id: widget.id.clone(),
        kind: widget.kind.clone(),
        x: widget.x,
        y: widget.y,
        width: widget.width.saturating_add(16),
        height: widget.height.saturating_add(16),
        settings: widget.settings.clone(),
        properties: widget.properties.clone(),
    }
}

fn render_widget(key: &str, widget: &Widget, state: &Value, skin: Skin) -> Node {
    match widget.kind.as_str() {
        "status" => status(key, widget, state, skin),
        "selection" => selection(key, widget, state, skin),
        "score" => score_widget(key, widget, state, skin),
        "history-list" => history_list(key, widget, state, skin),
        "history-graph" => history_graph(key, widget, state, skin),
        _ => empty_widget(key, widget, skin),
    }
}

fn status(key: &str, widget: &Widget, state: &Value, skin: Skin) -> Node {
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

fn selection(key: &str, widget: &Widget, state: &Value, skin: Skin) -> Node {
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

#[allow(clippy::too_many_lines)]
fn score_widget(key: &str, widget: &Widget, state: &Value, skin: Skin) -> Node {
    let score = shown(path_text(state, "/best/score"));
    let rank = shown(path_text(state, "/best/dj_level"));
    let miss = shown(path_text(state, "/best/miss"));
    let clear = shown(path_text(state, "/best/clear"));
    let fields = [
        ("PGREAT", "/detail/pgreat", "pgreat"),
        ("GREAT", "/detail/great", "great"),
        ("GOOD", "/detail/good", "good"),
        ("BAD", "/detail/bad", "bad"),
        ("POOR", "/detail/poor", "poor"),
        ("FAST", "/detail/fast", "fast"),
        ("SLOW", "/detail/slow", "slow"),
        ("COMBO BREAK", "/detail/combo_break", "combo"),
        ("PLAY OPTIONS", "/detail/play_options", "options"),
    ];
    let mut detail = vec![heading(
        &format!("{key}:detail:title"),
        "RESULT DETAIL",
        skin,
    )];
    for (i, (name, path, class)) in fields.iter().enumerate() {
        detail.push(el(
            &format!("{key}:detail:{i}"),
            "div",
            &[("class", format!("detail-row {class}"))],
            vec![
                el(
                    &format!("{key}:detail:{i}:name"),
                    "span",
                    &[],
                    vec![label(&format!("{key}:detail:{i}:label"), name, skin, 15)],
                ),
                node_text(
                    &format!("{key}:detail:{i}:value"),
                    "b",
                    &[],
                    &shown(path_text(state, path)),
                ),
            ],
        ));
    }
    el(
        &format!("{key}:score"),
        "section",
        &[("class", "widget score-widget".into())],
        vec![
            chrome(key, widget, skin),
            el(
                &format!("{key}:content"),
                "div",
                &[("class", "widget-content score-content".into())],
                vec![
                    el(
                        &format!("{key}:best"),
                        "div",
                        &[("class", "best-section".into())],
                        vec![
                            heading(&format!("{key}:best:title"), "BEST", skin),
                            el(
                                &format!("{key}:best:grid"),
                                "div",
                                &[("class", "best-grid".into())],
                                vec![
                                    el(
                                        &format!("{key}:score-main"),
                                        "div",
                                        &[("class", "score-main".into())],
                                        vec![
                                            label_element(
                                                &format!("{key}:score-label"),
                                                "label",
                                                "EX SCORE",
                                                skin,
                                                15,
                                            ),
                                            el(
                                                &format!("{key}:score-value"),
                                                "strong",
                                                &[],
                                                vec![metallic(
                                                    &format!("{key}:score-value:type"),
                                                    &score,
                                                    skin,
                                                    66,
                                                    false,
                                                )],
                                            ),
                                            el(
                                                &format!("{key}:clear"),
                                                "span",
                                                &[
                                                    ("class", "clear-value".into()),
                                                    ("data-clear", clear_role(&clear).into()),
                                                    ("style", clear_motion_style(&clear)),
                                                ],
                                                vec![label(
                                                    &format!("{key}:clear:label"),
                                                    &clear,
                                                    skin,
                                                    20,
                                                )],
                                            ),
                                        ],
                                    ),
                                    el(
                                        &format!("{key}:best-side"),
                                        "div",
                                        &[("class", "best-side".into())],
                                        vec![
                                            label_element(
                                                &format!("{key}:rank-label"),
                                                "label",
                                                "DJ LEVEL",
                                                skin,
                                                15,
                                            ),
                                            el(
                                                &format!("{key}:rank"),
                                                "strong",
                                                &[
                                                    ("class", "dj-level".into()),
                                                    ("data-rank", rank.clone()),
                                                ],
                                                vec![metallic(
                                                    &format!("{key}:rank:type"),
                                                    &rank,
                                                    skin,
                                                    48,
                                                    true,
                                                )],
                                            ),
                                            label_element(
                                                &format!("{key}:miss-label"),
                                                "label",
                                                "MISS COUNT",
                                                skin,
                                                15,
                                            ),
                                            node_text(&format!("{key}:miss"), "b", &[], &miss),
                                        ],
                                    ),
                                ],
                            ),
                        ],
                    ),
                    el(
                        &format!("{key}:detail"),
                        "div",
                        &[("class", "detail-section".into())],
                        detail,
                    ),
                ],
            ),
        ],
    )
}

fn history_list(key: &str, widget: &Widget, state: &Value, skin: Skin) -> Node {
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
fn history_graph(key: &str, widget: &Widget, state: &Value, skin: Skin) -> Node {
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

fn chrome(key: &str, widget: &Widget, skin: Skin) -> Node {
    el(
        &format!("{key}:chrome"),
        "div",
        &[
            ("class", "skin-frame".into()),
            ("aria-hidden", "true".into()),
        ],
        vec![
            frame(&format!("{key}:frame"), widget, skin),
            el(
                &format!("{key}:energy"),
                "div",
                &[
                    ("class", "skin-energy".into()),
                    ("style", energy_motion_style()),
                ],
                vec![],
            ),
            el(
                &format!("{key}:glint"),
                "div",
                &[
                    ("class", "skin-glint".into()),
                    ("style", glint_motion_style(&widget.kind)),
                ],
                vec![],
            ),
        ],
    )
}
fn frame(key: &str, widget: &Widget, skin: Skin) -> Node {
    let image = "frame.png";
    let [source_x, source_y] = skin.frame_source;
    let factor = skin.frame_factor;
    let edge = f64::from(frame_width(widget));
    let width = f64::from(widget.width) - 16.0 + edge * 2.0;
    let height = f64::from(widget.height) - 16.0 + edge * 2.0;
    let aperture_scale = if widget.kind == "empty" { 0.55 } else { 1.0 };
    let scale = (factor * aperture_scale)
        .min(width / (2.0 * source_x))
        .min(height / (2.0 * source_y));
    let sx = [0.0, source_x, 1254.0 - source_x, 1254.0];
    let sy = [0.0, source_y, 1254.0 - source_y, 1254.0];
    let corner_x = (source_x * scale + edge - 8.0).max(1.0).min(width / 2.0);
    let corner_y = (source_y * scale + edge - 8.0).max(1.0).min(height / 2.0);
    let tx = [0.0, corner_x, width - corner_x, width];
    let ty = [0.0, corner_y, height - corner_y, height];
    let mut pieces = Vec::new();
    for row in 0..3 {
        for column in 0..3 {
            let w = tx[column + 1] - tx[column];
            let h = ty[row + 1] - ty[row];
            if w > 0.0 && h > 0.0 {
                let xs = w / (sx[column + 1] - sx[column]);
                let ys = h / (sy[row + 1] - sy[row]);
                pieces.push(el(&format!("{key}:slice:{row}:{column}"), "div", &[("class", "material-slice".into()), ("style", format!("left:{}px;top:{}px;width:{w}px;height:{h}px;background-image:url('{image}');background-size:{}px {}px;background-position:{}px {}px", tx[column], ty[row], 1254.0 * xs, 1254.0 * ys, -sx[column] * xs, -sy[row] * ys))], vec![]));
            }
        }
    }
    let mut nodes = vec![el(
        &format!("{key}:material"),
        "div",
        &[
            ("class", "material-frame".into()),
            (
                "style",
                format!(
                    "left:{}px;top:{}px;width:{width}px;height:{height}px",
                    8.0 - edge,
                    8.0 - edge
                ),
            ),
        ],
        pieces,
    )];
    if skin.selection_illumination && widget.kind == "selection" {
        nodes.push(el(
            &format!("{key}:illumination"),
            "div",
            &[("class", "material-illumination".into())],
            vec![],
        ));
    }
    if widget.kind == "status" {
        let edge = skin.status_edge;
        nodes.push(el(
            &format!("{key}:panel-frame"),
            "svg",
            &[
                ("class", "panel-frame".into()),
                ("width", width.to_string()),
                ("height", height.to_string()),
                ("viewBox", format!("0 0 {width} {height}")),
            ],
            vec![shape(
                &format!("{key}:panel-frame:path"),
                "path",
                &[
                    (
                        "d",
                        &format!(
                            "M{} 12l-30 {}h-6l30 -{}",
                            width * 0.43,
                            height - 24.0,
                            height - 24.0
                        ),
                    ),
                    ("fill", "none"),
                    ("stroke", edge),
                    ("stroke-width", "0.7"),
                ],
            )],
        ));
    }
    el(key, "div", &[], nodes)
}

fn lamp(key: &str, state: &str, caption: Option<&str>, vertical: bool, skin: Skin) -> Node {
    let mut children = vec![el(
        &format!("{key}:lamp"),
        "span",
        &[
            ("class", "lamp".into()),
            ("data-state", state.into()),
            ("aria-hidden", "true".into()),
            ("style", lamp_motion_style(state)),
        ],
        vec![lamp_svg(key, state, vertical, skin)],
    )];
    if let Some(value) = caption {
        children.push(label_class(
            &format!("{key}:caption"),
            "lamp-label",
            value,
            skin,
            15,
        ));
    }
    let role = if caption == Some("SYSTEM") {
        "system"
    } else if caption == Some("RESULT") {
        "result"
    } else {
        "recorded"
    };
    el(
        key,
        "div",
        &[("class", format!("lamp-group {role}-lamp"))],
        children,
    )
}

fn lamp_svg(key: &str, state: &str, vertical: bool, skin: Skin) -> Node {
    let accent = skin.lamp_accent;
    let light = match (state, vertical) {
        ("active", true) => accent,
        ("active", false) => "#54e56b",
        ("error", _) => "#ff5369",
        _ => "#36434a",
    };
    let (width, height) = if vertical { (16, 64) } else { (24, 24) };
    let gradient_id = format!("lamp-gradient-{}", fragment_id(key));
    let glow = format!("url(#{gradient_id})");
    let mut art = vec![lamp_gradient(key, state, light, &gradient_id)];
    if vertical {
        art.extend([
            shape(
                &format!("{key}:outer"),
                "rect",
                &[
                    ("x", "0.5"),
                    ("y", "0.5"),
                    ("width", "15"),
                    ("height", "63"),
                    ("fill", "#020709"),
                    ("stroke", accent),
                    ("stroke-width", "1"),
                ],
            ),
            shape(
                &format!("{key}:inner"),
                "rect",
                &[
                    ("x", "3"),
                    ("y", "3"),
                    ("width", "10"),
                    ("height", "58"),
                    ("fill", light),
                ],
            ),
            shape(
                &format!("{key}:highlight"),
                "path",
                &[
                    ("d", "M5 5V59"),
                    ("stroke", "#d5faff"),
                    ("stroke-width", "1"),
                    ("opacity", if state == "inactive" { "0.15" } else { "0.8" }),
                ],
            ),
        ]);
    } else {
        art.extend([
            shape(
                &format!("{key}:outer"),
                "circle",
                &[
                    ("cx", "12"),
                    ("cy", "12"),
                    ("r", "11"),
                    ("fill", "#030a0d"),
                    ("stroke", light),
                    ("stroke-width", "1"),
                ],
            ),
            shape(
                &format!("{key}:inner"),
                "circle",
                &[
                    ("cx", "12"),
                    ("cy", "12"),
                    ("r", "8"),
                    ("fill", &glow),
                    ("stroke", "#87949b"),
                    ("stroke-width", "0.8"),
                ],
            ),
        ]);
    }
    el(
        &format!("{key}:svg"),
        "svg",
        &[
            ("width", width.to_string()),
            ("height", height.to_string()),
            ("viewBox", format!("0 0 {width} {height}")),
        ],
        art,
    )
}

fn lamp_gradient(key: &str, state: &str, light: &str, gradient_id: &str) -> Node {
    el(
        &format!("{key}:defs"),
        "defs",
        &[],
        vec![el(
            &format!("{key}:gradient"),
            "radialGradient",
            &[
                ("id", gradient_id.into()),
                ("cx", "40%".into()),
                ("cy", "35%".into()),
                ("r", "65%".into()),
            ],
            vec![
                shape(
                    &format!("{key}:gradient:0"),
                    "stop",
                    &[
                        ("offset", "0"),
                        (
                            "stop-color",
                            if state == "inactive" {
                                "#6c7981"
                            } else {
                                "#ffffff"
                            },
                        ),
                    ],
                ),
                shape(
                    &format!("{key}:gradient:1"),
                    "stop",
                    &[("offset", "0.4"), ("stop-color", light)],
                ),
                shape(
                    &format!("{key}:gradient:2"),
                    "stop",
                    &[("offset", "1"), ("stop-color", "#051015")],
                ),
            ],
        )],
    )
}
fn chart_rail(key: &str, width: u32, skin: Skin) -> Node {
    let edge = skin.rail_edge;
    let inner = skin.rail_inner;
    let width_f64 = f64::from(width);
    el(
        key,
        "svg",
        &[
            ("class", "rail-frame".into()),
            ("width", width.to_string()),
            ("height", "34".into()),
            ("viewBox", format!("0 0 {width} 34")),
        ],
        vec![
            shape(
                &format!("{key}:outer"),
                "path",
                &[
                    ("d", &contour(width_f64, 34.0, 0.5, 9.0)),
                    ("fill", "#020508"),
                    ("stroke", edge),
                    ("stroke-width", "1"),
                ],
            ),
            shape(
                &format!("{key}:inner"),
                "path",
                &[
                    ("d", &contour(width_f64, 34.0, 3.0, 7.0)),
                    ("fill", "none"),
                    ("stroke", inner),
                    ("stroke-width", "0.7"),
                ],
            ),
            shape(
                &format!("{key}:badge"),
                "path",
                &[
                    ("d", "M34 4H88L97 13V21L88 30H34L25 21V13Z"),
                    ("fill", "#0b1115"),
                    ("stroke", edge),
                    ("stroke-width", "1"),
                ],
            ),
            shape(
                &format!("{key}:end"),
                "path",
                &[
                    ("d", &format!("M{} 8l5 5v8l-5 5", width_f64 - 14.0)),
                    ("fill", "none"),
                    ("stroke", edge),
                    ("stroke-width", "1"),
                ],
            ),
        ],
    )
}

fn contour(width: f64, height: f64, inset: f64, cut: f64) -> String {
    let right = width - inset;
    let bottom = height - inset;
    let near = inset + cut;
    let far_x = right - cut;
    let far_y = bottom - cut;
    format!(
        "M{near} {inset}H{far_x}L{right} {near}V{far_y}L{far_x} {bottom}H{near}L{inset} {far_y}V{near}Z"
    )
}

fn fragment_id(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.bytes() {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn metallic(key: &str, value: &str, skin: Skin, height: u32, rank: bool) -> Node {
    if value.is_empty() || !value.chars().all(|c| GLYPHS.contains(c)) {
        return text(key, value);
    }
    let width = 44.0 * f64::from(height) / 80.0;
    let sheet = width * 18.0;
    let material = if rank { skin.rank_material } else { "type.png" };
    let mut nodes = vec![node_text(
        &format!("{key}:value"),
        "span",
        &[("class", "type-value".into())],
        value,
    )];
    for (i, glyph) in value.chars().enumerate() {
        let index = GLYPHS.chars().position(|v| v == glyph).unwrap_or(0);
        nodes.push(el(&format!("{key}:glyph:{i}"), "span", &[("class", "metal-glyph".into()), ("aria-hidden", "true".into()), ("style", format!("width:{width}px;height:{height}px;background-image:url('{material}');background-size:{sheet}px {height}px;background-position:-{}px 0", f64::from(u32::try_from(index).unwrap_or(0)) * width))], vec![]));
    }
    el(key, "span", &[("class", "metal-type".into())], nodes)
}
fn label(key: &str, value: &str, skin: Skin, height: u32) -> Node {
    let Some(index) = LABELS.iter().position(|item| *item == value) else {
        return text(key, value);
    };
    let scale = f64::from(height) / 24.0;
    let font = skin.label_font;
    el(
        key,
        "span",
        &[
            ("class", "material-label".into()),
            (
                "style",
                format!(
                    "margin-bottom:-{}px;font-family:{font};font-size:{}px;line-height:{height}px;height:{height}px",
                    scale * skin.label_descent,
                    scale * 20.0
                ),
            ),
        ],
        vec![
            node_text(
                &format!("{key}:value"),
                "span",
                &[("class", "label-value".into())],
                value,
            ),
            el(
                &format!("{key}:art"),
                "span",
                &[
                    ("class", "label-art".into()),
                    ("aria-hidden", "true".into()),
                    (
                        "style",
                        format!(
                            "width:100%;height:{height}px;background-image:url('labels.png');background-size:{}px {}px;background-position:0 -{}px",
                            256.0 * scale,
                            f64::from(u32::try_from(LABELS.len()).unwrap_or(0)) * 24.0 * scale,
                            u32::try_from(index).unwrap_or(0) * height
                        ),
                    ),
                ],
                vec![],
            ),
        ],
    )
}

fn empty_widget(key: &str, widget: &Widget, skin: Skin) -> Node {
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
fn canvas_background(mode: &str, widgets: &[Widget]) -> Node {
    let mask = aperture_mask(widgets.iter().filter(|widget| widget.kind == "empty").map(
        |widget| {
            (
                i64::from(widget.x),
                i64::from(widget.y),
                widget.width,
                widget.height,
            )
        },
    ));
    el(
        "background",
        "div",
        &[
            ("class", "canvas-background".into()),
            ("data-motion", mode.into()),
            ("aria-hidden", "true".into()),
            ("style", mask),
        ],
        vec![
            el(
                "background:art",
                "div",
                &[
                    ("class", "canvas-background-art".into()),
                    ("style", "background-image:url('background.png')".into()),
                ],
                vec![],
            ),
            el(
                "background:light",
                "div",
                &[
                    ("class", "canvas-background-light".into()),
                    ("style", background_motion_style(mode)),
                ],
                vec![],
            ),
        ],
    )
}

fn aperture_mask(holes: impl IntoIterator<Item = (i64, i64, u32, u32)>) -> String {
    let mut images = vec!["linear-gradient(white,white)".to_owned()];
    let mut sizes = vec!["100% 100%".to_owned()];
    let mut positions = vec!["0px 0px".to_owned()];
    for (x, y, width, height) in holes {
        let cut = (width / 4).min(12).min(height / 4);
        let right = width.saturating_sub(cut);
        let bottom = height.saturating_sub(cut);
        images.push(format!(
            "url(\"data:image/svg+xml,%3Csvg%20xmlns=%27http://www.w3.org/2000/svg%27%20width=%27{width}%27%20height=%27{height}%27%3E%3Cpath%20fill=%27white%27%20d=%27M{cut}%200H{right}L{width}%20{cut}V{bottom}L{right}%20{height}H{cut}L0%20{bottom}V{cut}Z%27/%3E%3C/svg%3E\")"
        ));
        sizes.push(format!("{width}px {height}px"));
        positions.push(format!("{x}px {y}px"));
    }
    if images.len() == 1 {
        return String::new();
    }
    let mut composites = vec!["subtract"; images.len()];
    composites[1..].fill("add");
    format!(
        "mask-image:{};mask-size:{};mask-position:{};mask-repeat:no-repeat;mask-composite:{};",
        images.join(","),
        sizes.join(","),
        positions.join(","),
        composites.join(",")
    )
}

fn field(key: &str, name: &str, value: &str, skin: Skin) -> Node {
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
fn heading(key: &str, value: &str, skin: Skin) -> Node {
    label_element(key, "h2", value, skin, 18)
}
fn label_element(key: &str, tag: &str, value: &str, skin: Skin, height: u32) -> Node {
    el(
        key,
        tag,
        &[],
        vec![label(&format!("{key}:label"), value, skin, height)],
    )
}
fn label_class(key: &str, class: &str, value: &str, skin: Skin, height: u32) -> Node {
    el(
        key,
        "span",
        &[("class", class.into())],
        vec![label(&format!("{key}:label"), value, skin, height)],
    )
}
fn polyline(key: &str, class: &str, color: &str, points: &str) -> Node {
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
fn shape(key: &str, tag: &str, attrs: &[(&str, &str)]) -> Node {
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
fn frame_width(widget: &Widget) -> u32 {
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

fn motion_value(from: f64, to: f64, period: f64, phase: f64, ramp: bool) -> Option<f64> {
    MOTION.with(|motion| {
        let (native, milliseconds) = motion.get();
        if !native {
            return None;
        }
        let seconds = std::time::Duration::from_millis(milliseconds).as_secs_f64();
        let phase = (seconds / period + phase).rem_euclid(1.0);
        let blend = if ramp {
            phase
        } else {
            0.5 - 0.5 * (phase * std::f64::consts::TAU).cos()
        };
        Some(from + (to - from) * blend)
    })
}

fn background_motion_style(mode: &str) -> String {
    if mode != "animated" {
        return String::new();
    }
    motion_value(0.3, 0.85, 16.0, 0.0, false)
        .map_or_else(String::new, |value| format!("opacity:{value:.4}"))
}

fn energy_motion_style() -> String {
    let Some(opacity) = motion_value(0.06, 0.16, 7.0, 0.0, false) else {
        return String::new();
    };
    let left = motion_value(-5.0, 1.0, 11.0, 0.0, false).unwrap_or(-5.0);
    format!("opacity:{opacity:.4};left:{left:.4}%")
}

fn glint_motion_style(kind: &str) -> String {
    let phase = match kind {
        "selection" => 0.14,
        "score" => 0.28,
        "history-list" => 0.42,
        "history-graph" => 0.56,
        _ => 0.0,
    };
    motion_value(-20.0, 100.0, 6.0, phase, true)
        .map_or_else(String::new, |value| format!("left:{value:.4}%"))
}

fn clear_motion_style(clear: &str) -> String {
    let track = match clear_role(clear) {
        "full-combo" => Some((0.72, 0.82)),
        "ex-hard" => Some((1.1, 0.82)),
        _ => None,
    };
    track
        .and_then(|(period, from)| motion_value(from, 1.0, period, 0.0, false))
        .map_or_else(String::new, |value| format!("opacity:{value:.4}"))
}

fn lamp_motion_style(state: &str) -> String {
    let track = match state {
        "active" | "persisted" => Some((1.8, 0.68)),
        "processing" => Some((0.8, 0.4)),
        _ => None,
    };
    track
        .and_then(|(period, from)| motion_value(from, 1.0, period, 0.0, false))
        .map_or_else(String::new, |value| format!("opacity:{value:.4}"))
}
fn dot_style(value: &Value, ratio: f64, start: i64, end: i64) -> String {
    format!(
        "left:{:.2}%;top:{:.2}%",
        time_ratio(integer(value, "received_unix_ms"), start, end) * 100.0,
        100.0 - ratio.clamp(0.0, 1.0) * 100.0
    )
}
fn time_ratio(time: i64, start: i64, end: i64) -> f64 {
    let elapsed = time.saturating_sub(start).max(0).cast_unsigned();
    let span = end.saturating_sub(start).max(1).cast_unsigned();
    std::time::Duration::from_millis(elapsed).as_secs_f64()
        / std::time::Duration::from_millis(span).as_secs_f64()
}
fn clear_role(value: &str) -> &'static str {
    match value {
        "FULL COMBO" | "FULL COMBO CLEAR" | "FC" => "full-combo",
        "EX HARD CLEAR" | "EX HARD" | "EXH" => "ex-hard",
        "HARD CLEAR" | "HARD" => "hard",
        "CLEAR" => "clear",
        "EASY CLEAR" | "EASY" => "easy",
        "ASSIST CLEAR" | "ASSIST" => "assist",
        "FAILED" => "failed",
        _ => "unknown",
    }
}
fn path_text<'a>(value: &'a Value, path: &str) -> &'a str {
    value.pointer(path).and_then(Value::as_str).unwrap_or("")
}
fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}
fn integer(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}
fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}
fn value_text(value: Option<&Value>) -> String {
    value.map_or_else(String::new, |v| {
        v.as_str().map_or_else(
            || {
                if v.is_null() {
                    String::new()
                } else {
                    v.to_string()
                }
            },
            str::to_owned,
        )
    })
}
fn shown(value: impl AsRef<str>) -> String {
    if value.as_ref().is_empty() {
        "—".into()
    } else {
        value.as_ref().into()
    }
}
fn el(key: &str, tag: &str, attrs: &[(&str, String)], children: Vec<Node>) -> Node {
    Node::element(
        key,
        tag,
        attrs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect::<BTreeMap<_, _>>(),
        children,
    )
}
fn text(key: &str, value: impl Into<String>) -> Node {
    Node::text(key, value)
}
fn node_text(key: &str, tag: &str, attrs: &[(&str, String)], value: &str) -> Node {
    el(key, tag, attrs, vec![text(&format!("{key}:text"), value)])
}
fn error_tree(error: String) -> Output {
    Output {
        schedule: Schedule::Idle,
        tree: el(
            "canvas",
            "main",
            &[("class", "overlay-canvas skin-error".into())],
            vec![text("error", error)],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scorepeek_skin_sdk::Canvas;
    use std::collections::BTreeSet;

    static TEST_THEME: Theme = Theme {
        graph_score: "#55e9ff",
        graph_miss: "#ffbd44",
        frame_source: [340.0, 200.0],
        frame_factor: 0.16,
        selection_illumination: false,
        status_edge: "#13dcef",
        lamp_accent: "#10dcfa",
        rail_edge: "#13dcef",
        rail_inner: "#086f83",
        rank_material: "rank-type.png",
        label_font: "Oxanium",
        label_descent: 6.2,
    };

    #[test]
    fn arbitrary_widget_ids_make_valid_distinct_svg_fragment_ids() {
        assert_eq!(fragment_id("lamp / 一"), "6c616d70202f20e4b880");
        assert_ne!(fragment_id("a:"), fragment_id("a/"));
    }

    #[test]
    fn status_retains_the_original_expanded_rendering_surface() {
        let output = tree(
            Input {
                schema: "scorepeek-skin-input-v2".into(),
                backend: "native".into(),
                monotonic_ms: 0,
                canvas: Canvas {
                    id: "test".into(),
                    skin: "dev.example.skin".into(),
                    width: 560,
                    height: 60,
                    properties: BTreeMap::new(),
                },
                widgets: vec![Widget {
                    id: "status".into(),
                    kind: "status".into(),
                    x: 8,
                    y: 8,
                    width: 544,
                    height: 44,
                    settings: Value::Null,
                    properties: BTreeMap::new(),
                }],
                state: serde_json::json!({}),
            },
            &TEST_THEME,
        );
        let slot = element_with_class(&output.tree, "widget-slot").unwrap();
        assert_eq!(
            slot.get("style").unwrap(),
            "left:8px;top:8px;width:544px;height:44px"
        );
        let frame = element_with_class(&output.tree, "material-frame").unwrap();
        assert_eq!(
            frame.get("style").unwrap(),
            "left:0px;top:0px;width:560px;height:60px"
        );
    }

    #[test]
    fn original_widget_surface_is_retained_in_the_rendered_tree() {
        let widget = |kind: &str, y| Widget {
            id: kind.into(),
            kind: kind.into(),
            x: 0,
            y,
            width: 560,
            height: 224,
            settings: serde_json::json!({"history_count":5,"graph_months":6}),
            properties: BTreeMap::from([
                ("frame-width".into(), Value::String("m".into())),
                ("fill-opacity-percent".into(), Value::from(0)),
            ]),
        };
        let output = tree(
            Input {
                schema: "scorepeek-skin-input-v2".into(),
                backend: "native".into(),
                monotonic_ms: 0,
                canvas: Canvas {
                    id: "test".into(),
                    skin: "dev.example.skin".into(),
                    width: 560,
                    height: 1120,
                    properties: BTreeMap::from([(
                        "background".into(),
                        Value::String("animated".into()),
                    )]),
                },
                widgets: [
                    "status",
                    "selection",
                    "score",
                    "history-list",
                    "history-graph",
                    "empty",
                ]
                .iter()
                .enumerate()
                .map(|(index, kind)| widget(kind, i32::try_from(index * 224).unwrap()))
                .collect(),
                state: serde_json::json!({
                    "chart":{"play_type":"single","difficulty":"another","title":"TITLE","artist":"ARTIST","level":12,"notes":1000},
                    "best":{"score":"1778","dj_level":"AAA","miss":"0","clear":"FULL COMBO"},
                    "detail":{},
                    "history":{"recorded":true,"plays":[],"graph":[],"graph_ticks":[],"graph_start_unix_ms":[0,0,0,0],"graph_end_unix_ms":1}
                }),
            },
            &TEST_THEME,
        );
        let mut classes = BTreeSet::new();
        let mut keys = BTreeSet::new();
        collect(&output.tree, &mut classes, &mut keys);
        for class in [
            "status-widget",
            "selection-widget",
            "score-widget",
            "history-list-widget",
            "history-graph-widget",
            "material-frame",
            "metal-type",
            "material-label",
            "chart-rail",
            "plot-area",
            "panel-frame",
            "rail-frame",
            "empty-frame",
            "empty-fill",
            "canvas-background",
        ] {
            assert!(classes.contains(class), "missing {class}");
        }
        let encoded = serde_json::to_string(&output).unwrap();
        assert!(encoded.contains("mask-image:"));
        assert!(encoded.contains("width=%27560%27%20height=%27224%27"));
        assert!(encoded.contains("radialGradient"));
        assert!(encoded.contains("stroke-width"));
    }

    fn collect(node: &Node, classes: &mut BTreeSet<String>, keys: &mut BTreeSet<String>) {
        match node {
            Node::Text { key, .. } => assert!(keys.insert(key.clone()), "duplicate {key}"),
            Node::Element {
                key,
                attributes,
                children,
                ..
            } => {
                assert!(keys.insert(key.clone()), "duplicate {key}");
                if let Some(value) = attributes.get("class") {
                    classes.extend(value.split_ascii_whitespace().map(str::to_owned));
                }
                for child in children {
                    collect(child, classes, keys);
                }
            }
        }
    }

    fn element_with_class<'a>(
        node: &'a Node,
        expected: &str,
    ) -> Option<&'a BTreeMap<String, String>> {
        let Node::Element {
            attributes,
            children,
            ..
        } = node
        else {
            return None;
        };
        if attributes.get("class").is_some_and(|classes| {
            classes
                .split_ascii_whitespace()
                .any(|class| class == expected)
        }) {
            return Some(attributes);
        }
        children
            .iter()
            .find_map(|child| element_with_class(child, expected))
    }
}
