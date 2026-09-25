use scorepeek_skin_sdk::{
    BoundaryDirection, Input, Node, Output, Schedule, Widget, allocate, deallocate, decode, encode,
    rank_thresholds, score_progress,
};
use serde_json::Value;
use std::collections::BTreeMap;

const DJ_LEVELS: [&str; 8] = ["F", "E", "D", "C", "B", "A", "AA", "AAA"];

fn element(
    key: impl Into<String>,
    tag: &str,
    class: &str,
    style: Option<String>,
    children: Vec<Node>,
) -> Node {
    let mut attributes = BTreeMap::from([("class".to_owned(), class.to_owned())]);
    if let Some(style) = style {
        attributes.insert("style".to_owned(), style);
    }
    Node::element(key, tag, attributes, children)
}

fn label(key: &str, class: &str, value: &str) -> Node {
    element(
        format!("{key}-el"),
        "span",
        class,
        None,
        vec![Node::text(format!("{key}-text"), value)],
    )
}

fn field<'a>(state: &'a Value, path: &[&str]) -> &'a str {
    path.iter()
        .try_fold(state, |value, key| value.get(*key))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn visible_field<'a>(state: &'a Value, path: &[&str]) -> &'a str {
    let value = field(state, path);
    if value.trim().is_empty() {
        "—"
    } else {
        value
    }
}

fn clear_class(clear: &str) -> &'static str {
    match clear {
        "NO PLAY" => "clear-no-play",
        "FAILED" => "clear-failed",
        "ASSIST CLEAR" | "ASSIST" => "clear-assist",
        "EASY CLEAR" | "EASY" => "clear-easy",
        "CLEAR" => "clear-normal",
        "HARD CLEAR" | "HARD" => "clear-hard",
        "EX HARD CLEAR" | "EX HARD" => "clear-ex-hard",
        "FULL COMBO" => "clear-full-combo",
        _ => "clear-unknown",
    }
}

fn widget_shell(widget: &Widget, class: &str, body: Vec<Node>) -> Node {
    let id = &widget.id;
    let mut children = vec![
        element(
            format!("{id}-surface"),
            "div",
            "abyssal-surface",
            None,
            vec![],
        ),
        element(format!("{id}-scan"), "div", "abyssal-scan", None, vec![]),
        element(format!("{id}-sweep"), "div", "abyssal-sweep", None, vec![]),
    ];
    for (corner, name) in [
        ("tl", "corner-tl"),
        ("tr", "corner-tr"),
        ("bl", "corner-bl"),
        ("br", "corner-br"),
    ] {
        children.push(element(
            format!("{id}-{name}"),
            "div",
            &format!("abyssal-corner corner-{corner}"),
            None,
            vec![],
        ));
    }
    children.push(element(
        format!("{id}-content"),
        "div",
        "abyssal-content",
        None,
        body,
    ));
    element(
        format!("{id}-slot"),
        "section",
        &format!("abyssal-widget {class}"),
        Some(format!(
            "left:{}px;top:{}px;width:{}px;height:{}px",
            widget.x, widget.y, widget.width, widget.height
        )),
        children,
    )
}

fn status(widget: &Widget, state: &Value) -> Node {
    let id = &widget.id;
    let recorded = state
        .get("history")
        .and_then(|history| history.get("recorded"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let signals = [
        ("system", field(state, &["system"]), "SYSTEM"),
        ("result", field(state, &["result_signal"]), "RESULT"),
        (
            "select",
            if recorded { "active" } else { "inactive" },
            "SELECT",
        ),
    ];
    let lamps = signals
        .into_iter()
        .map(|(key, value, name)| {
            element(
                format!("{id}-{key}-signal"),
                "div",
                "status-signal",
                None,
                vec![
                    element(
                        format!("{id}-{key}-lamp"),
                        "span",
                        &format!("signal-lamp lamp-{value}"),
                        None,
                        vec![],
                    ),
                    label(
                        &format!("{id}-{key}-name"),
                        &format!("signal-label signal-label-{key}"),
                        name,
                    ),
                ],
            )
        })
        .collect();
    widget_shell(
        widget,
        if widget.width <= 600 {
            "status status-compact"
        } else {
            "status"
        },
        vec![
            label(&format!("{id}-identity"), "status-logo", "scorepeek"),
            element(
                format!("{id}-signals"),
                "div",
                "status-signals",
                None,
                lamps,
            ),
        ],
    )
}

fn selection(widget: &Widget, state: &Value) -> Node {
    let id = &widget.id;
    let title = visible_field(state, &["chart", "title"]);
    let artist = visible_field(state, &["chart", "artist"]);
    let play_type = match visible_field(state, &["chart", "play_type"]) {
        "single" | "SP" => "SP",
        "double" | "DP" => "DP",
        other => other,
    };
    let difficulty = visible_field(state, &["chart", "difficulty"]).to_ascii_uppercase();
    let level = state
        .get("chart")
        .and_then(|chart| chart.get("level"))
        .and_then(Value::as_u64)
        .map_or_else(|| "—".to_owned(), |value| value.to_string());
    let notes = state
        .get("chart")
        .and_then(|chart| chart.get("notes"))
        .and_then(Value::as_u64)
        .map_or_else(|| "—".to_owned(), |value| value.to_string());
    let title_width = title
        .chars()
        .map(|glyph| if glyph.is_ascii() { 0.55 } else { 1.0 })
        .sum::<f64>()
        * 21.0;
    let long_title =
        widget.width <= 460 && title_width > f64::from(widget.width.saturating_sub(30));
    let rail = element(
        format!("{id}-rail"),
        "div",
        "selection-rail",
        None,
        vec![
            label(&format!("{id}-play"), "selection-play", play_type),
            label(
                &format!("{id}-difficulty"),
                &format!(
                    "selection-difficulty difficulty-{}",
                    difficulty.to_ascii_lowercase()
                ),
                &difficulty,
            ),
            label(
                &format!("{id}-level"),
                "selection-level",
                &format!("LV {level}"),
            ),
            label(
                &format!("{id}-notes"),
                "selection-notes",
                &format!("NOTES {notes}"),
            ),
        ],
    );
    widget_shell(
        widget,
        if long_title {
            "selection selection-compact selection-long"
        } else if widget.width <= 460 {
            "selection selection-compact"
        } else {
            "selection"
        },
        vec![
            label(&format!("{id}-caption"), "micro-caption", "SELECT / CHART"),
            label(&format!("{id}-title"), "selection-title", title),
            label(&format!("{id}-artist"), "selection-artist", artist),
            rail,
        ],
    )
}

fn digits(id: &str, value: &str, class: &str) -> Node {
    let content = if value.is_empty() { "—" } else { value };
    let children = content
        .chars()
        .enumerate()
        .map(|(index, digit)| {
            let class = if digit.is_ascii_digit() {
                format!("atlas-digit digit-{digit}")
            } else {
                "atlas-unknown".to_owned()
            };
            label(&format!("{id}-{index}"), &class, &digit.to_string())
        })
        .collect();
    element(id, "span", class, None, children)
}

fn score_ticks(id: &str, notes: Option<u64>) -> Vec<Node> {
    notes
        .and_then(rank_thresholds)
        .map(|thresholds| {
            DJ_LEVELS
                .iter()
                .enumerate()
                .map(|(index, rank)| {
                    let hundredths =
                        u128::from(thresholds[index]) * 10_000 / u128::from(thresholds[8]);
                    let percent = f64::from(u32::try_from(hundredths).unwrap_or(0)) / 100.0;
                    element(
                        format!("{id}-tick-{index}"),
                        "span",
                        "score-tick",
                        Some(format!("left:{percent:.4}%")),
                        vec![Node::text(format!("{id}-tick-{index}-text"), *rank)],
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn score_rank_node(id: &str, rank: &str) -> Node {
    let class = if rank == "AAA" {
        "rank-value rank-aaa"
    } else {
        "rank-value"
    };
    let mut children = vec![Node::text(format!("{id}-rank-text"), rank)];
    if rank == "AAA" {
        children.push(element(
            format!("{id}-rank-glow"),
            "span",
            "rank-glow",
            None,
            vec![],
        ));
    }
    element(format!("{id}-rank-el"), "span", class, None, children)
}

fn score(widget: &Widget, state: &Value) -> Node {
    let id = &widget.id;
    let score = field(state, &["best", "score"]);
    let rank = visible_field(state, &["best", "dj_level"]);
    let miss = field(state, &["best", "miss"]);
    let clear = visible_field(state, &["best", "clear"]);
    let notes = state
        .get("chart")
        .and_then(|chart| chart.get("notes"))
        .and_then(Value::as_u64);
    let progress = score
        .parse::<u64>()
        .ok()
        .zip(notes)
        .and_then(|(score, notes)| score_progress(score, notes));
    let rate = progress.map_or(0.0, |value| {
        f64::from(u32::try_from(value.rate_hundredths).unwrap_or(0)) / 100.0
    });
    let distance = progress
        .and_then(|value| {
            DJ_LEVELS
                .iter()
                .position(|label| *label == rank)
                .and_then(|index| value.nearest_boundary(index))
        })
        .map_or_else(
            || "—".to_owned(),
            |distance| match distance.direction {
                BoundaryDirection::Above => format!("{rank}+{}", distance.difference),
                BoundaryDirection::Below => {
                    let boundary = DJ_LEVELS
                        .get(distance.boundary_index)
                        .copied()
                        .unwrap_or("MAX");
                    format!("{boundary}-{}", distance.difference)
                }
                BoundaryDirection::ExactMax => "MAX".to_owned(),
            },
        );
    let tick_nodes = score_ticks(id, notes);
    let clear_class = format!("clear-value {}", clear_class(clear));
    let main = element(
        format!("{id}-main"),
        "div",
        "score-main",
        None,
        vec![
            label(&format!("{id}-score-label"), "score-label", "SCORE"),
            label(&format!("{id}-rank-label"), "rank-label", "DJ LEVEL"),
            digits(&format!("{id}-digits"), score, "score-digits"),
            score_rank_node(id, rank),
            label(&format!("{id}-miss-label"), "miss-label", "MISS COUNT"),
            digits(&format!("{id}-miss"), miss, "miss-digits"),
            label(&format!("{id}-clear-label"), "clear-label", "CLEAR"),
            element(format!("{id}-clear-el"), "span", &clear_class, None, {
                let mut children = vec![Node::text(format!("{id}-clear-text"), clear)];
                if clear == "FULL COMBO" {
                    children.push(element(
                        format!("{id}-clear-glow"),
                        "span",
                        "clear-glow",
                        None,
                        vec![],
                    ));
                }
                children
            }),
            label(&format!("{id}-distance"), "distance", &distance),
            element(
                format!("{id}-bar"),
                "div",
                "score-bar",
                None,
                vec![element(
                    format!("{id}-bar-fill"),
                    "div",
                    "score-bar-fill",
                    Some(format!("width:{rate:.2}%")),
                    vec![],
                )],
            ),
            element(
                format!("{id}-ticks"),
                "div",
                "score-ticks",
                None,
                tick_nodes,
            ),
        ],
    );
    widget_shell(
        widget,
        if widget.width <= 460 {
            "score score-compact"
        } else {
            "score"
        },
        vec![main, score_details(id, state)],
    )
}

fn score_details(id: &str, state: &Value) -> Node {
    let details = [
        ("PGREAT", "pgreat", "positive-strong"),
        ("GREAT", "great", "positive"),
        ("GOOD", "good", "positive"),
        ("BAD", "bad", "negative"),
        ("POOR", "poor", "negative"),
        ("FAST", "fast", "timing-fast"),
        ("SLOW", "slow", "timing-slow"),
        ("COMBO BREAK", "combo_break", "negative"),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (name, key, class))| {
        element(
            format!("{id}-detail-{index}"),
            "div",
            "detail-row",
            None,
            vec![
                label(
                    &format!("{id}-detail-{index}-name"),
                    &format!("detail-name detail-label-{key}"),
                    name,
                ),
                label(
                    &format!("{id}-detail-{index}-value"),
                    &format!("detail-value {class}"),
                    visible_field(state, &["detail", key]),
                ),
            ],
        )
    })
    .collect::<Vec<_>>();
    let mut detail_children = details;
    detail_children.push(element(
        format!("{id}-options"),
        "div",
        "options-row",
        None,
        vec![
            label(
                &format!("{id}-options-label"),
                "detail-name detail-label-options",
                "PLAY OPTIONS",
            ),
            label(
                &format!("{id}-options-value"),
                "options-value",
                visible_field(state, &["detail", "play_options"]),
            ),
        ],
    ));
    element(
        format!("{id}-detail"),
        "div",
        "score-detail",
        None,
        detail_children,
    )
}

fn history_list(widget: &Widget, state: &Value) -> Node {
    let id = &widget.id;
    let row_count = match widget.settings.get("history_count").and_then(Value::as_u64) {
        Some(10) => 10,
        Some(20) => 20,
        Some(50) => 50,
        _ => 5,
    };
    let headings = [
        ("date", "DATE"),
        ("score", "SCORE"),
        ("level", "DJ LEVEL"),
        ("miss", "MISS"),
        ("clear", "CLEAR"),
    ];
    let header = element(
        format!("{id}-header"),
        "div",
        "history-header",
        None,
        headings
            .into_iter()
            .map(|(key, name)| {
                label(
                    &format!("{id}-head-{key}"),
                    &format!("history-head history-head-{key}"),
                    name,
                )
            })
            .collect(),
    );
    let rows = state
        .get("history")
        .and_then(|history| history.get("plays"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(row_count)
        .enumerate()
        .map(|(index, row)| {
            let rank = visible_field(row, &["dj_level"]);
            let clear = visible_field(row, &["clear"]);
            element(
                format!("{id}-row-{index}"),
                "div",
                "history-row",
                None,
                vec![
                    label(
                        &format!("{id}-{index}-date"),
                        "history-date",
                        visible_field(row, &["notified_at"]),
                    ),
                    label(
                        &format!("{id}-{index}-score"),
                        "history-score",
                        visible_field(row, &["score"]),
                    ),
                    label(
                        &format!("{id}-{index}-level"),
                        &format!("history-level level-{}", rank.to_ascii_lowercase()),
                        rank,
                    ),
                    label(
                        &format!("{id}-{index}-miss"),
                        "history-miss",
                        visible_field(row, &["miss"]),
                    ),
                    label(
                        &format!("{id}-{index}-clear"),
                        &format!("history-clear {}", clear_class(clear)),
                        clear,
                    ),
                ],
            )
        })
        .collect();
    widget_shell(
        widget,
        "history-list",
        vec![
            label(
                &format!("{id}-caption"),
                "micro-caption history-caption",
                "HISTORY",
            ),
            header,
            element(format!("{id}-rows"), "div", "history-rows", None, rows),
        ],
    )
}

fn empty(widget: &Widget) -> Node {
    let id = &widget.id;
    let title = widget
        .settings
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("");
    let opacity = widget
        .properties
        .get("interior-opacity")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let mut body = vec![element(
        format!("{id}-interior"),
        "div",
        "empty-interior",
        Some(format!("background:rgba(4,20,27,{opacity:.3})")),
        vec![],
    )];
    if !title.is_empty() {
        body.push(label(&format!("{id}-title"), "empty-title", title));
    }
    widget_shell(widget, "empty", body)
}

fn graph(widget: &Widget, state: &Value) -> Node {
    let id = &widget.id;
    let history = state.get("history");
    let months_index = match widget.settings.get("graph_months").and_then(Value::as_u64) {
        Some(1) => 0,
        Some(3) => 1,
        Some(12) => 3,
        _ => 2,
    };
    let bounds = history.and_then(|history| {
        let start = history
            .get("graph_start_unix_ms")?
            .as_array()?
            .get(months_index)?
            .as_i64()?;
        let end = history.get("graph_end_unix_ms")?.as_i64()?;
        (start < end).then_some((start, end))
    });
    let points = history
        .and_then(|history| history.get("graph"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|point| {
            bounds.is_some_and(|(start, end)| {
                point
                    .get("received_unix_ms")
                    .and_then(Value::as_i64)
                    .is_some_and(|time| time >= start && time <= end)
            })
        })
        .collect::<Vec<_>>();
    let plot_width = widget.width.saturating_sub(95).max(220);
    let plot_span = f64::from(plot_width.saturating_sub(24));
    let (first, last) = bounds.unwrap_or((0, 1));
    let x_for = |time: i64| {
        let elapsed = u32::try_from((time - first).max(0) / 1000).unwrap_or(u32::MAX);
        let span = u32::try_from((last - first) / 1000)
            .unwrap_or(u32::MAX)
            .max(1);
        12.0 + (f64::from(elapsed) / f64::from(span)) * plot_span
    };
    let (plot, gaps) = graph_plot(id, &points, plot_width, x_for);
    let axis = graph_axes(id);
    let ticks = history
        .and_then(|history| history.get("graph_ticks"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, tick)| {
            let time = tick.get("unix_ms")?.as_i64()?;
            let value = tick.get("label")?.as_str()?;
            let (start, end) = bounds?;
            if time < start || time > end {
                return None;
            }
            let x = 47.0 + x_for(time);
            Some(element(
                format!("{id}-tick-label-{index}"),
                "span",
                "time-tick",
                Some(format!("left:{x:.1}px")),
                vec![Node::text(format!("{id}-tick-text-{index}"), value)],
            ))
        });
    let mut body = vec![
        label(&format!("{id}-caption"), "micro-caption", "HISTORY GRAPH"),
        label(
            &format!("{id}-legend-score"),
            "graph-legend legend-score",
            "DJ LEVEL",
        ),
        label(
            &format!("{id}-legend-miss"),
            "graph-legend legend-miss",
            "MISS RATE",
        ),
    ];
    body.extend(axis);
    body.push(plot);
    body.extend(gaps);
    body.extend(ticks);
    widget_shell(widget, "history-graph", body)
}

fn graph_axes(id: &str) -> Vec<Node> {
    let mut axis = Vec::new();
    for (index, rank) in DJ_LEVELS.iter().enumerate() {
        let ratio = if index == 0 {
            0.0
        } else {
            f64::from(u32::try_from(index + 1).unwrap_or(0)) / 9.0
        };
        let top = 41.0 + (1.0 - ratio) * 120.0 - 5.0;
        axis.push(element(
            format!("{id}-rank-axis-{index}"),
            "span",
            "axis-rank",
            Some(format!("top:{top:.1}px")),
            vec![Node::text(format!("{id}-rank-axis-text-{index}"), *rank)],
        ));
    }
    for (index, percent) in [0, 25, 50, 75, 100].into_iter().enumerate() {
        let top = 36.0 + f64::from(u32::try_from(4 - index).unwrap_or(0)) * 30.0;
        axis.push(element(
            format!("{id}-miss-axis-{index}"),
            "span",
            "axis-miss",
            Some(format!("top:{top:.1}px")),
            vec![Node::text(
                format!("{id}-miss-axis-text-{index}"),
                format!("{percent}%"),
            )],
        ));
    }
    axis
}

fn graph_plot(
    id: &str,
    points: &[&Value],
    plot_width: u32,
    x_for: impl Fn(i64) -> f64 + Copy,
) -> (Node, Vec<Node>) {
    let mut paths = Vec::new();
    let mut gaps = Vec::new();
    for pair in points.windows(2) {
        let before = pair[0].get("received_unix_ms").and_then(Value::as_i64);
        let after = pair[1].get("received_unix_ms").and_then(Value::as_i64);
        if let (Some(before), Some(after)) = (before, after)
            && after - before > 35 * 86_400_000
        {
            let center = x_for(before).midpoint(x_for(after));
            gaps.push(element(
                format!("{id}-gap-{}", gaps.len()),
                "span",
                "gap-marker",
                Some(format!("left:{:.1}px", 47.0 + center - 15.0)),
                vec![Node::text(format!("{id}-gap-text-{}", gaps.len()), "GAP")],
            ));
        }
    }
    for (field, class) in [("score_ratio", "score-path"), ("miss_ratio", "miss-path")] {
        let mut segment = Vec::new();
        let mut previous_time = None;
        for point in points {
            let time = point.get("received_unix_ms").and_then(Value::as_i64);
            if let (Some(previous), Some(current)) = (previous_time, time)
                && current - previous > 35 * 86_400_000
                && !segment.is_empty()
            {
                paths.push(svg_polyline(id, field, class, paths.len(), &segment));
                segment.clear();
            }
            previous_time = time;
            if let Some(ratio) = point.get(field).and_then(Value::as_f64) {
                let x = time.map_or(12.0, x_for);
                let y = (1.0 - ratio.clamp(0.0, 1.0)) * 120.0;
                segment.push(format!("{x:.1},{y:.1}"));
            } else if !segment.is_empty() {
                paths.push(svg_polyline(id, field, class, paths.len(), &segment));
                segment.clear();
            }
        }
        if !segment.is_empty() {
            paths.push(svg_polyline(id, field, class, paths.len(), &segment));
        }
    }
    let plot = Node::element(
        format!("{id}-plot"),
        "svg",
        BTreeMap::from([
            ("class".to_owned(), "history-plot".to_owned()),
            ("viewBox".to_owned(), format!("0 0 {plot_width} 120")),
            ("width".to_owned(), plot_width.to_string()),
            ("height".to_owned(), "120".to_owned()),
        ]),
        {
            let mut nodes = vec![];
            for (index, y) in [0, 30, 60, 90, 120].into_iter().enumerate() {
                let attrs = BTreeMap::from([
                    ("class".to_owned(), "plot-grid".to_owned()),
                    ("x1".to_owned(), "0".to_owned()),
                    ("x2".to_owned(), plot_width.to_string()),
                    ("y1".to_owned(), y.to_string()),
                    ("y2".to_owned(), y.to_string()),
                    ("stroke".to_owned(), "#376d79".to_owned()),
                    ("stroke-width".to_owned(), "1".to_owned()),
                ]);
                nodes.push(Node::element(
                    format!("{id}-grid-{index}"),
                    "line",
                    attrs,
                    vec![],
                ));
            }
            nodes.extend(paths);
            nodes
        },
    );
    (plot, gaps)
}

fn svg_polyline(id: &str, field: &str, class: &str, index: usize, points: &[String]) -> Node {
    Node::element(
        format!("{id}-{field}-{index}"),
        "polyline",
        BTreeMap::from([
            ("class".to_owned(), class.to_owned()),
            ("points".to_owned(), points.join(" ")),
            ("fill".to_owned(), "none".to_owned()),
            (
                "stroke".to_owned(),
                if field == "score_ratio" {
                    "#8beef4"
                } else {
                    "#ef929c"
                }
                .to_owned(),
            ),
            (
                "stroke-width".to_owned(),
                if field == "score_ratio" { "3" } else { "2" }.to_owned(),
            ),
        ]),
        vec![],
    )
}

#[derive(Clone, Copy)]
struct Tile {
    x: i64,
    y: i64,
    width: i64,
    height: i64,
}

impl Tile {
    fn without(self, hole: Self) -> Vec<Self> {
        let left = self.x.max(hole.x);
        let top = self.y.max(hole.y);
        let right = (self.x + self.width).min(hole.x + hole.width);
        let bottom = (self.y + self.height).min(hole.y + hole.height);
        if left >= right || top >= bottom {
            return vec![self];
        }
        [
            Self {
                x: self.x,
                y: self.y,
                width: self.width,
                height: top - self.y,
            },
            Self {
                x: self.x,
                y: bottom,
                width: self.width,
                height: self.y + self.height - bottom,
            },
            Self {
                x: self.x,
                y: top,
                width: left - self.x,
                height: bottom - top,
            },
            Self {
                x: right,
                y: top,
                width: self.x + self.width - right,
                height: bottom - top,
            },
        ]
        .into_iter()
        .filter(|tile| tile.width > 0 && tile.height > 0)
        .collect()
    }
}

fn background_tiles(input: &Input, mode: &str) -> Vec<Node> {
    if mode == "off" {
        return vec![];
    }
    let canvas_width = i64::from(input.canvas.width);
    let canvas_height = i64::from(input.canvas.height);
    let mut tiles = vec![Tile {
        x: 0,
        y: 0,
        width: canvas_width,
        height: canvas_height,
    }];
    for widget in input.widgets.iter().filter(|widget| widget.kind == "empty") {
        let hole = Tile {
            x: i64::from(widget.x),
            y: i64::from(widget.y),
            width: i64::from(widget.width),
            height: i64::from(widget.height),
        };
        tiles = tiles
            .into_iter()
            .flat_map(|tile| tile.without(hole))
            .collect();
    }
    let image_size = canvas_width.max(canvas_height);
    let crop_x = (image_size - canvas_width) / 2;
    let crop_y = (image_size - canvas_height) / 2;
    tiles
        .into_iter()
        .enumerate()
        .map(|(index, tile)| {
            element(
                format!("background-tile-{index}"),
                "div",
                "abyssal-background-tile",
                Some(format!(
                    "left:{}px;top:{}px;width:{}px;height:{}px;background-size:{image_size}px {image_size}px;background-position:{}px {}px",
                    tile.x,
                    tile.y,
                    tile.width,
                    tile.height,
                    -tile.x - crop_x,
                    -tile.y - crop_y,
                )),
                vec![],
            )
        })
        .collect()
}

fn render(input: &Input) -> Output {
    let state = &input.state;
    let background = input
        .canvas
        .properties
        .get("background")
        .and_then(Value::as_str)
        .filter(|value| matches!(*value, "off" | "static" | "animated"))
        .unwrap_or("animated");
    let mut children = background_tiles(input, background);
    children.extend(
        input
            .widgets
            .iter()
            .filter_map(|widget| match widget.kind.as_str() {
                "status" => Some(status(widget, state)),
                "selection" => Some(selection(widget, state)),
                "score" => Some(score(widget, state)),
                "history-list" => Some(history_list(widget, state)),
                "history-graph" => Some(graph(widget, state)),
                "empty" => Some(empty(widget)),
                _ => None,
            }),
    );
    Output {
        schedule: Schedule::Idle,
        tree: element(
            "abyssal-canvas",
            "main",
            &format!("abyssal-canvas background-{background}"),
            Some(format!(
                "width:{}px;height:{}px",
                input.canvas.width, input.canvas.height
            )),
            children,
        ),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn scorepeek_alloc(length: i32) -> i32 {
    allocate(length)
}

#[unsafe(no_mangle)]
pub extern "C" fn scorepeek_dealloc(pointer: i32, length: i32) {
    deallocate(pointer, length);
}

#[unsafe(no_mangle)]
pub extern "C" fn scorepeek_init(pointer: i32, length: i32) -> i64 {
    scorepeek_render(pointer, length)
}

#[unsafe(no_mangle)]
pub extern "C" fn scorepeek_render(pointer: i32, length: i32) -> i64 {
    let output = decode(pointer, length).map_or_else(
        |_| Output {
            schedule: Schedule::Idle,
            tree: element("invalid-input", "main", "abyssal-canvas", None, vec![]),
        },
        |input| render(&input),
    );
    encode(&output)
}
