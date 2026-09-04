//! Presentation shared by the native DOM and browser DOM.
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Chart {
    pub song_id: String,
    pub play_type: String,
    pub difficulty: String,
    pub title: String,
    pub artist: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Field {
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResultView {
    pub title: String,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Confirmation {
    pub title: String,
    pub confirmed: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct History {
    pub status: String,
    pub best: Vec<Field>,
    pub plays: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OverlayState {
    pub connected: bool,
    pub selecting: bool,
    pub chart: Option<Chart>,
    pub result: Option<ResultView>,
    pub confirmation: Option<Confirmation>,
    pub best_received: [bool; 3],
    pub history: History,
}

const CSS: &str = r"
html,body { margin:0; background:transparent; color:#edf1fa; font-family:sans-serif; }
.overlay { position:absolute; right:24px; top:24px; width:480px; max-width:90vw; }
.panel { background:rgba(14,20,31,0.88); border-radius:12px; padding:18px; margin-bottom:12px; }
.heading { color:#8ea6bf; font-size:13px; margin-bottom:10px; }
.title { font-size:24px; font-weight:700; overflow-wrap:anywhere; }
.muted { color:#a0adbf; font-size:14px; }
.stale { opacity:0.55; }
.row { display:flex; justify-content:space-between; gap:12px; padding:4px 0; font-size:15px; }
.value { font-weight:600; }
.badge { color:#7fdab3; font-size:15px; margin-top:10px; }
.waiting { color:#edca87; }
.result { animation:entry 600ms ease-out; }
@keyframes entry { from { opacity:0.3; } to { opacity:1; } }
";

/// Renders only display data; transport and database ownership stay with the host.
/// # Errors
/// Propagates Dioxus rendering errors.
pub fn overlay_panel(state: &OverlayState) -> Element {
    let dimmed = !state.connected || state.selecting;
    let live_class = if dimmed { "panel stale" } else { "panel" };
    let title = state
        .chart
        .as_ref()
        .map_or("選曲待ち", |chart| chart.title.as_str());
    let chart_label = state.chart.as_ref().map_or_else(String::new, |chart| {
        format!("{} / {}", chart.play_type, chart.difficulty)
    });
    let checks = ["EX", "MISS", "CLEAR"]
        .into_iter()
        .zip(state.best_received)
        .map(|(label, received)| format!("{label} {}", if received { "✓" } else { "…" }))
        .collect::<Vec<_>>()
        .join(" / ");
    rsx! {
        style { "{CSS}" }
        div { class: "overlay",
            div { class: "{live_class}",
                div { class: "heading", "LIVE" }
                if !state.connected { div { class: "waiting", "接続待ち" } }
                else if state.selecting { div { class: "waiting", "選曲確認中" } }
                if let Some(result) = &state.result {
                    div { class: "result",
                        div { class: "title", "{result.title}" }
                        for field in &result.fields {
                            div { class: "row", span { "{field.label}" } span { class: "value", "{field.value}" } }
                        }
                    }
                } else {
                    div { class: "title", "{title}" }
                    div { class: "muted", "{chart_label}" }
                    if let Some(chart) = &state.chart { div { class: "muted", "{chart.artist}" } }
                    div { class: "badge", "SELECT best: {checks}" }
                }
            }
            div { class: "panel",
                div { class: "heading", "直近プレイ" }
                if let Some(confirmation) = &state.confirmation {
                    div { "{confirmation.title}" }
                    if confirmation.confirmed { div { class: "badge", "確定 ✓" } }
                    else { div { class: "waiting", "確定待ち" } }
                } else { div { class: "muted", "結果待ち" } }
            }
            div { class: if state.selecting { "panel stale" } else { "panel" },
                div { class: "heading", "選曲譜面の BEST / HISTORY" }
                div { class: "muted", "{title} / {chart_label}" }
                div { class: "muted", "{state.history.status}" }
                for field in &state.history.best {
                    div { class: "row", span { "{field.label}" } span { class: "value", "{field.value}" } }
                }
                div { class: "heading", "直近5件 · 通知日時" }
                for play in &state.history.plays { div { class: "row", "{play}" } }
            }
        }
    }
}
