use crate::{RANKS, element, group, letters, text};
use scorepeek_skin_sdk::{Node, Widget};
use serde_json::Value;
fn line(key: &str, x1: f64, y1: f64, x2: f64, y2: f64, color: &str, width: &str) -> Node {
    element(
        key,
        "path",
        &[
            ("d", &format!("M{x1} {y1} L{x2} {y2}")),
            ("stroke", color),
            ("stroke-width", width),
            ("fill", "none"),
        ],
        vec![],
    )
}
pub fn render(widget: &Widget, state: &Value) -> Node {
    let id = &widget.id;
    let history = &state["history"];
    let months = widget.settings["graph_months"].as_u64().unwrap_or(6);
    let range = [1, 3, 6, 12].iter().position(|m| *m == months).unwrap_or(2);
    let start = history["graph_start_unix_ms"][range]
        .as_f64()
        .unwrap_or(0.0);
    let end = history["graph_end_unix_ms"].as_f64().unwrap_or(0.0);
    let plot_width = f64::from(widget.width.saturating_sub(100));
    let plot_height = f64::from(widget.height.saturating_sub(76));
    let mut paths = vec![];
    let mut annotations = vec![];
    axes(id, plot_width, plot_height, &mut paths, &mut annotations);
    if end > start {
        for (i, tick) in history["graph_ticks"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let Some(time) = tick["unix_ms"]
                .as_f64()
                .filter(|t| *t >= start && *t <= end)
            else {
                continue;
            };
            let x = (time - start) / (end - start) * plot_width;
            paths.push(line(
                &format!("{id}-time-line-{i}"),
                x,
                0.0,
                x,
                plot_height,
                "#55748a66",
                "0.7",
            ));
            annotations.push(group(
                &format!("{id}-time-{i}"),
                "graph-time",
                &format!("left:{x}px;top:{}px;", plot_height + 8.0),
                vec![letters(
                    &format!("{id}-time-label-{i}"),
                    text(&tick["label"]),
                    13.0,
                    "label",
                )],
            ));
        }
        paths.extend(data_lines(id, history, start, end, plot_width, plot_height));
    }
    annotations.insert(
        0,
        element(
            &format!("{id}-plot-svg"),
            "svg",
            &[
                ("class", "graph-svg"),
                ("width", &(plot_width + 2.0).to_string()),
                ("height", &(plot_height + 3.0).to_string()),
                (
                    "viewBox",
                    &format!("-1 -1 {} {}", plot_width + 2.0, plot_height + 3.0),
                ),
                ("aria-hidden", "true"),
            ],
            paths,
        ),
    );
    group(
        &format!("{id}-content"),
        "graph-content",
        "",
        vec![
            group(
                &format!("{id}-heading"),
                "graph-heading",
                "",
                vec![
                    letters(&format!("{id}-title"), "HISTORY GRAPH", 18.0, "silver"),
                    group(
                        &format!("{id}-legends"),
                        "graph-legends",
                        "",
                        vec![
                            letters(&format!("{id}-score-legend"), "DJ LEVEL", 13.0, "blue"),
                            letters(&format!("{id}-miss-legend"), "MISS RATE", 13.0, "red"),
                        ],
                    ),
                ],
            ),
            group(
                &format!("{id}-plot"),
                "graph-plot",
                &format!("width:{plot_width}px;height:{plot_height}px;"),
                annotations,
            ),
        ],
    )
}

fn data_lines(
    id: &str,
    history: &Value,
    start: f64,
    end: f64,
    plot_width: f64,
    plot_height: f64,
) -> Vec<Node> {
    let mut paths = vec![];
    for (series, color) in [("score_ratio", "#4ce1ff"), ("miss_ratio", "#ff627c")] {
        let mut previous = None;
        for (i, point) in history["graph"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let sample = point["received_unix_ms"]
                .as_f64()
                .zip(point[series].as_f64())
                .filter(|(time, value)| {
                    *time >= start && *time <= end && (0.0..=1.0).contains(value)
                });
            let Some((time, value)) = sample else {
                previous = None;
                continue;
            };
            let (x, y) = (
                (time - start) / (end - start) * plot_width,
                (1.0 - value) * plot_height,
            );
            if let Some((px, py)) = previous {
                paths.push(line(
                    &format!("{id}-{series}-line-{i}"),
                    px,
                    py,
                    x,
                    y,
                    color,
                    "1.8",
                ));
            }
            paths.push(element(
                &format!("{id}-{series}-point-{i}"),
                "circle",
                &[
                    ("cx", &x.to_string()),
                    ("cy", &y.to_string()),
                    ("r", "2.2"),
                    ("fill", color),
                ],
                vec![],
            ));
            previous = Some((x, y));
        }
    }
    paths
}

fn axes(
    id: &str,
    plot_width: f64,
    plot_height: f64,
    paths: &mut Vec<Node>,
    annotations: &mut Vec<Node>,
) {
    for (i, label) in RANKS[..8].iter().enumerate() {
        let fraction = if i == 0 {
            0.0
        } else {
            f64::from(u32::try_from(i + 1).unwrap_or(0)) / 9.0
        };
        let y = (1.0 - fraction) * plot_height;
        paths.push(line(
            &format!("{id}-rank-line-{i}"),
            0.0,
            y,
            plot_width,
            y,
            "#55748a66",
            "0.7",
        ));
        annotations.push(group(
            &format!("{id}-rank-{i}"),
            "graph-rank",
            &format!("top:{}px;", y - 6.0),
            vec![letters(
                &format!("{id}-rank-text-{i}"),
                label,
                14.0,
                "label",
            )],
        ));
    }
    for i in 0..=4 {
        let y = f64::from(4 - i) / 4.0 * plot_height;
        annotations.push(group(
            &format!("{id}-percent-{i}"),
            "graph-percent",
            &format!("top:{}px;left:{}px;", y - 6.0, plot_width + 6.0),
            vec![letters(
                &format!("{id}-percent-text-{i}"),
                &format!("{}%", i * 25),
                13.0,
                "label",
            )],
        ));
    }
}
