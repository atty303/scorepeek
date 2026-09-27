//! Independent INFINITAS-inspired overlay; all game meaning comes from the host.
mod glyphs;
mod history;
mod score;
mod selection;
mod status;
mod typography;
use typography::letters;
mod canvas;
mod graph;
mod palette;
mod surface;

use scorepeek_skin_sdk::{Input, Node, Output, Schedule, Widget};
use serde_json::Value;
use std::collections::BTreeMap;

const CHARS: &str = " 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ+-/%.:?";
const RANKS: [&str; 9] = ["F", "E", "D", "C", "B", "A", "AA", "AAA", "MAX"];

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
    render_input(pointer, length)
}
#[unsafe(no_mangle)]
pub extern "C" fn scorepeek_render(pointer: i32, length: i32) -> i64 {
    render_input(pointer, length)
}
fn render_input(pointer: i32, length: i32) -> i64 {
    let output = match scorepeek_skin_sdk::decode(pointer, length) {
        Ok(input) => render(&input),
        Err(_) => Output {
            schedule: Schedule::Idle,
            tree: element("error", "main", &[], vec![]),
        },
    };
    scorepeek_skin_sdk::encode(&output)
}
fn element(key: &str, tag: &str, attrs: &[(&str, &str)], children: Vec<Node>) -> Node {
    Node::element(
        key,
        tag,
        attrs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect::<BTreeMap<_, _>>(),
        children,
    )
}
fn group(key: &str, class: &str, style: &str, children: Vec<Node>) -> Node {
    element(key, "div", &[("class", class), ("style", style)], children)
}
fn live(key: &str, class: &str, value: &str) -> Node {
    element(
        key,
        "span",
        &[("class", class)],
        vec![Node::text(format!("{key}-text"), value)],
    )
}
fn text(value: &Value) -> &str {
    value.as_str().filter(|s| !s.is_empty()).unwrap_or("?")
}
fn number(value: &Value) -> String {
    value.as_u64().map_or_else(|| "?".into(), |n| n.to_string())
}
fn rank_tone(rank: &str) -> &str {
    match rank {
        "AAA" => "gold",
        "AA" => "ice",
        "A" => "silver",
        _ => "neutral",
    }
}
fn clear_tone(clear: &str) -> &str {
    match clear {
        "FULL COMBO" => "prism",
        "EX HARD" | "FAILED" => "red",
        "HARD" => "gold",
        "CLEAR" => "blue",
        "EASY" => "green",
        "ASSIST" => "purple",
        _ => "neutral",
    }
}
fn state_letters(key: &str, value: &str, size: f64, rank: bool, phase: f64) -> Node {
    // The editor sample still supplies the expanded clear label.
    let meaning = if rank {
        value
    } else {
        value.strip_suffix(" CLEAR").unwrap_or(value)
    };
    let tone = if rank {
        rank_tone(meaning)
    } else {
        clear_tone(meaning)
    };
    let class = match meaning {
        "AAA" => "result jewel",
        "AA" | "HARD" => "result positive",
        "FULL COMBO" => "result prism",
        "EX HARD" => "result strong",
        _ => "result",
    };
    let mut content = vec![letters(&format!("{key}-value"), value, size, tone)];
    if matches!(value, "AAA" | "FULL COMBO") {
        content.push(group(
            &format!("{key}-glint"),
            "state-glint",
            &format!(
                "left:{}%;opacity:{};",
                5.0 + 70.0 * phase,
                0.3 + phase * 0.7
            ),
            vec![],
        ));
    }
    group(key, class, "", content)
}
fn cell(key: &str, label: &str, value: &str, size: f64, tone: &str) -> Node {
    group(
        key,
        "metric",
        "",
        vec![
            letters(
                &format!("{key}-label"),
                label,
                if label == "MISS COUNT" { 14.0 } else { 15.0 },
                "label",
            ),
            letters(&format!("{key}-value"), value, size, tone),
        ],
    )
}
fn empty(w: &Widget, palette: palette::Palette) -> Node {
    let opacity = w
        .properties
        .get("opacity")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let mut parts = vec![group(
        &format!("{}-fill", w.id),
        "empty-fill",
        &format!("background:{};opacity:{opacity};", palette.dark),
        vec![],
    )];
    if let Some(title) = w.settings["title"].as_str().filter(|v| !v.is_empty()) {
        parts.push(live(&format!("{}-title", w.id), "empty-title", title));
    }
    group(&format!("{}-content", w.id), "empty-content", "", parts)
}
fn render(input: &Input) -> Output {
    let palette = palette::get(
        input
            .canvas
            .properties
            .get("series")
            .and_then(Value::as_str)
            .unwrap_or("INFINITAS"),
    );
    let background = input
        .canvas
        .properties
        .get("background")
        .and_then(Value::as_str)
        .unwrap_or("animated");
    let fraction = f64::from(u32::try_from(input.monotonic_ms % 6000).unwrap_or(0)) / 6000.0;
    let phase = (1.0 - (fraction * std::f64::consts::TAU).cos()) / 2.0;
    let mut widgets = if background == "none" {
        vec![]
    } else {
        canvas::background(
            input,
            palette,
            if background == "animated" {
                (fraction * std::f64::consts::TAU).sin() * 16.0
            } else {
                0.0
            },
        )
    };
    widgets.extend(
        input
            .widgets
            .iter()
            .map(|w| render_widget(w, input, palette, phase)),
    );
    let special = input.widgets.iter().any(|w| match w.kind.as_str() {
        "score" => {
            text(&input.state["best"]["dj_level"]) == "AAA"
                || text(&input.state["best"]["clear"]) == "FULL COMBO"
        }
        "history-list" => input.state["history"]["plays"]
            .as_array()
            .into_iter()
            .flatten()
            .take(usize::try_from(w.settings["history_count"].as_u64().unwrap_or(5)).unwrap_or(5))
            .any(|p| text(&p["dj_level"]) == "AAA" || text(&p["clear"]) == "FULL COMBO"),
        _ => false,
    });
    Output {
        schedule: if background == "animated" || special {
            Schedule::AfterMs { milliseconds: 50 }
        } else {
            Schedule::Idle
        },
        tree: group(
            "infinitas",
            "infinitas",
            &format!(
                "width:{}px;height:{}px;",
                input.canvas.width, input.canvas.height
            ),
            widgets,
        ),
    }
}

fn render_widget(w: &Widget, input: &Input, palette: palette::Palette, phase: f64) -> Node {
    let body = match w.kind.as_str() {
        "selection" => selection::render(w, &input.state),
        "score" => score::render(w, &input.state, phase),
        "history-list" => history::render(w, &input.state, phase),
        "status" => status::render(w, &input.state),
        "history-graph" => graph::render(w, &input.state),
        "empty" => empty(w, palette),
        _ => group(&format!("{}-unknown", w.id), "", "", vec![]),
    };
    let mut parts = vec![surface::frame(
        &w.id,
        w.width,
        w.height,
        palette,
        w.kind == "empty",
    )];
    if w.kind != "empty" {
        parts.push(group(
            &format!("{}-glass-face", w.id),
            "glass-face",
            &format!("background:{};", palette.dark),
            vec![
                element(
                    &format!("{}-glass-image", w.id),
                    "img",
                    &[
                        ("src", "optical-glass.png"),
                        ("class", "glass-image"),
                        ("alt", ""),
                    ],
                    vec![],
                ),
                group(
                    &format!("{}-glass-tint", w.id),
                    "glass-tint",
                    &format!("background:{};", palette.middle),
                    vec![],
                ),
            ],
        ));
    }
    parts.push(body);
    group(
        &w.id,
        &format!("panel {}", w.kind),
        &format!(
            "left:{}px;top:{}px;width:{}px;height:{}px;",
            w.x, w.y, w.width, w.height
        ),
        parts,
    )
}

#[cfg(test)]
mod tests;
