use scorepeek_skin_sdk::{Input, Node, Output, Schedule, Widget};
use serde_json::Value;

use crate::theme::{Skin, Theme};

use crate::primitive::{el, error_tree};
#[cfg(test)]
use crate::render::fragment_id;
use crate::render::{
    canvas_background, empty_widget, history_graph, history_list, score_widget, selection, status,
};

pub(crate) const GLYPHS: &str = "0123456789ABCDEFG-";
pub(crate) const LABELS: &[&str] = &[
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
        schedule: Schedule::Idle,
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

pub(crate) fn expanded_widget(widget: &Widget) -> Widget {
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

#[cfg(test)]
mod tests {
    use super::*;
    use scorepeek_skin_sdk::Canvas;
    use std::collections::{BTreeMap, BTreeSet};

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
    fn bundled_native_motion_uses_css_without_time_dependent_tree_updates() {
        let input = |monotonic_ms| Input {
            schema: "scorepeek-skin-input-v2".into(),
            backend: "native".into(),
            monotonic_ms,
            canvas: Canvas {
                id: "test".into(),
                skin: "dev.example.skin".into(),
                width: 560,
                height: 60,
                properties: BTreeMap::from([(
                    "background".into(),
                    Value::String("animated".into()),
                )]),
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
            state: serde_json::json!({"system":"active"}),
        };
        let first = tree(input(0), &TEST_THEME);
        let later = tree(input(4500), &TEST_THEME);
        assert_eq!(first.schedule, Schedule::Idle);
        assert_eq!(first, later);
        for class in [
            "canvas-background-light",
            "skin-energy",
            "skin-glint",
            "lamp",
        ] {
            let attributes = element_with_class(&first.tree, class).unwrap();
            assert!(!attributes.contains_key("style"), "{class}");
        }
    }

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
