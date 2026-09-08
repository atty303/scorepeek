use scorepeek_skin_sdk::{Input, Node, Output, Schedule};
use std::collections::BTreeMap;

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
    render(pointer, length)
}

#[unsafe(no_mangle)]
pub extern "C" fn scorepeek_render(pointer: i32, length: i32) -> i64 {
    render(pointer, length)
}

fn render(pointer: i32, length: i32) -> i64 {
    let input = scorepeek_skin_sdk::decode(pointer, length);
    let output = input.map_or_else(error_tree, tree);
    scorepeek_skin_sdk::encode(&output)
}

fn tree(input: Input) -> Output {
    let mut root_attributes = BTreeMap::from([
        ("class".into(), "skin-canvas".into()),
        ("data-backend".into(), input.backend),
        ("data-canvas-id".into(), input.canvas.id.clone()),
        (
            "style".into(),
            format!(
                "width:{}px;height:{}px",
                input.canvas.width, input.canvas.height
            ),
        ),
    ]);
    for (key, value) in input.canvas.properties {
        root_attributes.insert(format!("data-property-{key}"), property(&value));
    }
    let children = input
        .widgets
        .into_iter()
        .map(|widget| {
            let mut attributes = BTreeMap::from([
                (
                    "class".into(),
                    format!("skin-widget skin-widget-{}", widget.kind),
                ),
                ("data-widget-kind".into(), widget.kind.clone()),
                (
                    "style".into(),
                    format!(
                        "left:{}px;top:{}px;width:{}px;height:{}px",
                        widget.x, widget.y, widget.width, widget.height
                    ),
                ),
            ]);
            for (key, value) in widget.properties {
                attributes.insert(format!("data-property-{key}"), property(&value));
            }
            let content = widget_content(&widget.kind, &input.state);
            Node::element(
                format!("widget:{}", widget.id),
                "section",
                attributes,
                vec![Node::element(
                    format!("widget:{}:content", widget.id),
                    "div",
                    BTreeMap::from([("class".into(), "skin-widget-content".into())]),
                    vec![Node::text(
                        format!("widget:{}:content:text", widget.id),
                        content,
                    )],
                )],
            )
        })
        .collect();
    Output {
        schedule: Schedule::Idle,
        tree: Node::element("canvas", "main", root_attributes, children),
    }
}

fn widget_content(kind: &str, state: &serde_json::Value) -> String {
    let text = |pointer: &str| {
        state
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("—")
    };
    match kind {
        "status" => {
            let connection = if state
                .get("connected")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                "CONNECTED"
            } else {
                "WAITING"
            };
            format!("SCOREPEEK  •  {connection}")
        }
        "selection" => format!(
            "{}\n{}  •  {} {}  •  LV {}",
            text("/chart/title"),
            text("/chart/artist"),
            text("/chart/play_type"),
            text("/chart/difficulty"),
            state
                .pointer("/chart/level")
                .map_or_else(|| "—".into(), serde_json::Value::to_string)
        ),
        "score" => format!(
            "SCORE {}  {}\nMISS {}  {}",
            text("/best/score"),
            text("/best/dj_level"),
            text("/best/miss"),
            text("/best/clear")
        ),
        "history-list" => format!(
            "HISTORY  •  {} PLAYS",
            state
                .pointer("/history/plays")
                .and_then(serde_json::Value::as_array)
                .map_or(0, Vec::len)
        ),
        "history-graph" => format!(
            "TREND  •  {} SAMPLES",
            state
                .pointer("/history/graph")
                .and_then(serde_json::Value::as_array)
                .map_or(0, Vec::len)
        ),
        "empty" => String::new(),
        other => other.to_uppercase(),
    }
}

fn error_tree(error: String) -> Output {
    Output {
        schedule: Schedule::Idle,
        tree: Node::element(
            "canvas",
            "main",
            BTreeMap::from([("class".into(), "skin-canvas skin-error".into())]),
            vec![Node::text("error", error)],
        ),
    }
}

fn property(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}
