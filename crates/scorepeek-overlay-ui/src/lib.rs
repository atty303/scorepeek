//! Presentation shared by the native DOM and browser DOM.
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

mod appearance;
pub use appearance::{Appearance, Layout, Skin};
pub const OXANIUM: &[u8] = include_bytes!("../assets/fonts/Oxanium.ttf");
pub const BASE_CSS: &str = include_str!("../styles/base.css");
pub const SKIN_CSS: &str = concat!(
    include_str!("../styles/cyan-system.css"),
    include_str!("../styles/result-aurora.css"),
    include_str!("../styles/dj-blackbox.css")
);

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Chart {
    pub song_id: String,
    pub play_type: String,
    pub difficulty: String,
    pub title: String,
    pub artist: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldRole {
    #[default]
    Detail,
    Score,
    Rank,
    Miss,
    Clear,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Field {
    pub role: FieldRole,
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResultView {
    pub title: String,
    pub artist: String,
    pub play_type: String,
    pub difficulty: String,
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
    pub plays: Vec<HistoryPlay>,
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

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HistoryPlay {
    pub notified_at: String,
    pub score: String,
    pub miss: String,
    pub clear: String,
}

fn metric(fields: &[Field], role: FieldRole) -> &str {
    fields
        .iter()
        .find(|field| field.role == role)
        .map_or("—", |field| field.value.as_str())
}

fn mode_label(mode: &str) -> &str {
    match mode {
        "single" => "SP",
        "double" => "DP",
        other => other,
    }
}

/// Renders display data with the same semantic structure on native and browser DOMs.
/// # Errors
/// Propagates Dioxus rendering errors.
pub fn overlay_panel(state: &OverlayState, appearance: Appearance) -> Element {
    let skin = appearance.skin.name();
    let layout = appearance.layout.name();
    let dimmed = !state.connected || state.selecting;
    let title = state
        .result
        .as_ref()
        .map(|result| result.title.as_str())
        .or_else(|| state.chart.as_ref().map(|chart| chart.title.as_str()))
        .unwrap_or("選曲待ち");
    let (artist, mode, difficulty) = if let Some(result) = &state.result {
        (
            result.artist.as_str(),
            result.play_type.as_str(),
            result.difficulty.as_str(),
        )
    } else {
        state.chart.as_ref().map_or(("", "", ""), |chart| {
            (
                chart.artist.as_str(),
                chart.play_type.as_str(),
                chart.difficulty.as_str(),
            )
        })
    };
    let mode = mode_label(mode);
    let confirmed = state
        .confirmation
        .as_ref()
        .is_some_and(|confirmation| confirmation.confirmed);
    let status = if !state.connected {
        "接続待ち"
    } else if state.selecting {
        "選曲確認中"
    } else if state.result.is_some() {
        if confirmed {
            "RESULT / CONFIRMED"
        } else {
            "RESULT / PENDING"
        }
    } else {
        "SELECTED CHART"
    };
    rsx! {
        style { "{BASE_CSS}{SKIN_CSS}" }
        div { class: "overlay", "data-skin": skin, "data-layout": layout,
            section { class: if dimmed { "panel live stale" } else { "panel live" },
                div { class: "frame-accent", aria_hidden: "true" }
                header { class: "panel-bar", span { class: "wordmark", "score" span { "peek" } } span { class: "state-label", "{status}" } }
                div { class: "chart-header",
                    if !mode.is_empty() || !difficulty.is_empty() {
                        div { class: "chart-badge", "data-difficulty": difficulty,
                            strong { "{mode}" } span { "{difficulty}" }
                        }
                    }
                    div { class: "chart-copy", h1 { class: "song-title", "{title}" } p { class: "artist", "{artist}" } }
                }
                if let Some(result) = &state.result {
                    div { class: "performance",
                        div { class: "score-block", span { class: "metric-label", "EX SCORE" } strong { class: "score-value", "{metric(&result.fields, FieldRole::Score)}" } }
                        div { class: "side-metrics",
                            div { class: "rank-block", span { class: "metric-label", "DJ RANK" } strong { class: "rank-value", "{metric(&result.fields, FieldRole::Rank)}" } }
                            div { class: "miss-block", span { class: "metric-label", "MISS COUNT" } strong { class: "miss-value", "{metric(&result.fields, FieldRole::Miss)}" } }
                        }
                    }
                    div { class: "clear-banner", "{metric(&result.fields, FieldRole::Clear)}" }
                    div { class: "details",
                        for field in result.fields.iter().filter(|field| field.role == FieldRole::Detail) {
                            div { class: "detail-row", span { class: "metric-label", "{field.label}" } span { "{field.value}" } }
                        }
                    }
                } else {
                    div { class: "selection-receipts",
                        span { class: "metric-label", "SELECT BEST 取込" }
                        for (label, received) in ["EX", "MISS", "CLEAR"].into_iter().zip(state.best_received) {
                            span { class: if received { "receipt received" } else { "receipt" }, "{label}" span { if received { " ✓" } else { " …" } } }
                        }
                    }
                }
            }
            section { class: "panel confirmation", "data-confirmed": if confirmed { "true" } else { "false" },
                div { class: "section-heading", "LAST PLAY" span { class: "status-light", aria_hidden: "true" } }
                if let Some(confirmation) = &state.confirmation {
                    div { class: "confirmation-row", span { class: "confirmation-title", "{confirmation.title}" }
                        span { class: "confirmation-state", if confirmed { "確定 ✓" } else { "確定待ち" } }
                    }
                } else { p { class: "empty", "結果待ち" } }
            }
            {history_panel(state)}
            footer { class: "overlay-footer", span { "SCOREPEEK" } span { "{skin}" } }
        }
    }
}

fn history_panel(state: &OverlayState) -> Element {
    rsx! {
            section { class: if state.selecting { "panel history stale" } else { "panel history" },
                div { class: "section-heading", "CHART BEST" }
                if let Some(chart) = &state.chart {
                    p { class: "history-chart", "{chart.title} / {mode_label(&chart.play_type)} {chart.difficulty}" }
                }
                p { class: "history-status", "{state.history.status}" }
                div { class: "best-grid",
                    for field in &state.history.best {
                        div { class: "best-metric", span { class: "metric-label", "{field.label}" } strong { "{field.value}" } }
                    }
                }
                div { class: "history-heading", span { "RECENT PLAYS" } span { "直近5件 · 通知日時 UTC" } }
                if state.history.plays.is_empty() { p { class: "empty", "履歴の表示はありません" } }
                else {
                    div { class: "history-table",
                        div { class: "history-row history-columns", span { "NOTIFIED" } span { "EX" } span { "MISS" } span { "CLEAR" } }
                        for play in &state.history.plays {
                            div { class: "history-row", span { class: "history-date", "{play.notified_at}" } strong { "{play.score}" } span { "{play.miss}" } span { "{play.clear}" } }
                        }
                    }
                }
            }
    }
}
