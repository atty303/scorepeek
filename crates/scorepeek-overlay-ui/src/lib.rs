//! Shared canvas widgets for native and browser renderers.
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

mod appearance;
mod assets;
pub use appearance::{Appearance, Skin};
pub use assets::{SKIN_ASSETS, skin_asset};
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
    #[serde(default)]
    pub level: Option<u32>,
    #[serde(default)]
    pub notes: Option<u32>,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LampState {
    #[default]
    Inactive,
    Active,
    Error,
    Processing,
    Persisted,
    Failed,
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BestView {
    pub score: String,
    pub dj_level: String,
    pub miss: String,
    pub clear: String,
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResultDetail {
    pub pgreat: String,
    pub great: String,
    pub good: String,
    pub bad: String,
    pub poor: String,
    pub fast: String,
    pub slow: String,
    pub combo_break: String,
    pub play_options: String,
}
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct GraphPlay {
    pub received_unix_ms: i64,
    pub score_ratio: f64,
    pub miss_ratio: Option<f64>,
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HistoryPlay {
    pub notified_at: String,
    pub score: String,
    pub dj_level: String,
    pub miss: String,
    pub clear: String,
}
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct History {
    pub recorded: bool,
    pub plays: Vec<HistoryPlay>,
    pub graph: Vec<GraphPlay>,
    #[serde(default)]
    pub graph_start_unix_ms: [i64; 4],
    #[serde(default)]
    pub graph_end_unix_ms: i64,
}
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct OverlayState {
    pub connected: bool,
    pub chart: Option<Chart>,
    #[serde(default)]
    pub system: LampState,
    #[serde(default)]
    pub result_ingest: LampState,
    #[serde(default)]
    pub best: BestView,
    #[serde(default)]
    pub detail: ResultDetail,
    #[serde(default)]
    pub history: History,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WidgetKind {
    Status,
    Selection,
    Score,
    HistoryList,
    HistoryGraph,
}

#[must_use]
pub const fn default_widget_size(kind: WidgetKind) -> (u32, u32) {
    match kind {
        WidgetKind::Status => (560, 72),
        WidgetKind::Selection => (560, 120),
        WidgetKind::Score => (560, 300),
        WidgetKind::HistoryList => (560, 236),
        WidgetKind::HistoryGraph => (560, 280),
    }
}

#[must_use]
pub fn next_widget_id(kind: WidgetKind, widgets: &[WidgetLayout]) -> String {
    let stem = match kind {
        WidgetKind::Status => "status",
        WidgetKind::Selection => "selection",
        WidgetKind::Score => "score",
        WidgetKind::HistoryList => "history-list",
        WidgetKind::HistoryGraph => "history-graph",
    };
    (1..=widgets.len().saturating_add(1))
        .map(|number| format!("{stem}-{number}"))
        .find(|candidate| widgets.iter().all(|widget| widget.id != *candidate))
        .unwrap_or_else(|| format!("{stem}-{}", widgets.len().saturating_add(1)))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetSettings {
    #[serde(default = "default_history_count")]
    pub history_count: u32,
    #[serde(default = "default_graph_months")]
    pub graph_months: u32,
}
impl Default for WidgetSettings {
    fn default() -> Self {
        Self {
            history_count: 5,
            graph_months: 6,
        }
    }
}
const fn default_history_count() -> u32 {
    5
}
const fn default_graph_months() -> u32 {
    6
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetLayout {
    pub id: String,
    pub kind: WidgetKind,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub z: u32,
    #[serde(default)]
    pub settings: WidgetSettings,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanvasPresentation {
    pub id: String,
    pub skin: Skin,
    pub revision: u64,
    pub widgets: Vec<WidgetLayout>,
}

fn mode_label(mode: &str) -> &str {
    match mode {
        "single" => "SP",
        "double" => "DP",
        other => other,
    }
}
fn shown(value: &str) -> &str {
    if value.is_empty() { "—" } else { value }
}
fn lamp(state: LampState, label: Option<&str>, class: &str) -> Element {
    let state = format!("{state:?}").to_ascii_lowercase();
    rsx! { div { class: "lamp-group {class}", if let Some(label) = label { span { class: "lamp-label", "{label}" } } span { class: "lamp", "data-state": state, aria_hidden: "true" } } }
}
fn chrome() -> Element {
    rsx! { div { class: "skin-frame", aria_hidden: "true", div { class: "skin-surface" } div { class: "skin-corners" } div { class: "skin-light" } div { class: "skin-hardware" } } }
}

/// Renders the approved five-widget master composition.
/// # Errors
/// Returns a Dioxus render error if element construction fails.
pub fn overlay_panel(state: &OverlayState, appearance: Appearance) -> Element {
    overlay_canvas(state, appearance, &default_widgets(), false, None)
}

/// Renders independently positioned widgets inside one canvas.
/// # Errors
/// Returns a Dioxus render error if element construction fails.
pub fn overlay_canvas(
    state: &OverlayState,
    appearance: Appearance,
    widgets: &[WidgetLayout],
    editing: bool,
    selected: Option<&str>,
) -> Element {
    let skin = appearance.skin.name();
    let chart = state.chart.as_ref();
    let title = chart.map_or("", |v| v.title.as_str());
    let artist = chart.map_or("", |v| v.artist.as_str());
    let play_type = chart.map_or("", |v| mode_label(&v.play_type));
    let difficulty = chart
        .map(|v| v.difficulty.to_ascii_uppercase())
        .unwrap_or_default();
    let level = chart
        .and_then(|v| v.level)
        .map_or_else(String::new, |v| v.to_string());
    let notes = chart
        .and_then(|v| v.notes)
        .map_or_else(String::new, |v| v.to_string());
    let selected_label = selected.unwrap_or("NO SELECTION");
    let selected_widget = selected.and_then(|id| widgets.iter().find(|widget| widget.id == id));
    rsx! {
        style { "{BASE_CSS}{SKIN_CSS}" }
        main { class: if editing { "overlay-canvas editing" } else { "overlay-canvas" }, "data-skin": skin,
            for widget in widgets {
                div {
                    key: "{widget.id}",
                    class: if selected == Some(widget.id.as_str()) { "widget-slot selected" } else { "widget-slot" },
                    "data-widget-id": "{widget.id}",
                    style: format!("left:{}px;top:{}px;width:{}px;height:{}px;z-index:{}", widget.x, widget.y, widget.width, widget.height, widget.z),
                    {render_widget(widget, state, title, artist, play_type, &difficulty, &level, &notes)}
                    if editing { div { class: "resize-handle", aria_hidden: "true" } }
                }
            }
            if editing {
                div { class: "native-editor-shell", strong { "CANVAS EDIT" } span { class:"native-skin-options", b { "CYAN" } b { "AURORA" } b { "BLACKBOX" } } b { class:"native-done", "DONE" } }
                div { class: "native-widget-palette", span { "STATUS" } span { "SELECTION" } span { "SCORE" } span { "HISTORY LIST" } span { "HISTORY GRAPH" } }
                div { class: "native-editor-inspector", strong { "INSPECTOR" } span { "{selected_label}" } b { class:"native-remove", "REMOVE" }
                    if let Some(widget)=selected_widget {
                        if widget.kind == WidgetKind::HistoryList { div { class:"native-setting", for value in [5,10,20,50] { b { "{value}" } } } }
                        if widget.kind == WidgetKind::HistoryGraph { div { class:"native-setting", for value in [1,3,6,12] { b { "{value}M" } } } }
                        b { class:"native-return", "キャンバス内へ戻す" }
                    }
                }
                i { class: "native-canvas-resize" }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_widget(
    widget: &WidgetLayout,
    state: &OverlayState,
    title: &str,
    artist: &str,
    play_type: &str,
    difficulty: &str,
    level: &str,
    notes: &str,
) -> Element {
    match widget.kind {
        WidgetKind::Status => {
            rsx! { section { class: "widget status-widget", {chrome()} div { class: "widget-content status-content", span { class: "wordmark", "score" span { "peek" } } div { class: "status-lamps", {lamp(state.system, Some("SYSTEM"), "system-lamp")} {lamp(state.result_ingest, Some("RESULT"), "result-lamp")} } } } }
        }
        WidgetKind::Selection => {
            rsx! { section { class: "widget selection-widget", {chrome()} div { class: "widget-content selection-content",
                {lamp(if state.history.recorded { LampState::Active } else { LampState::Inactive }, None, "recorded-lamp")}
                div { class: "song-copy", h1 { title: title, "{title}" } p { title: artist, "{artist}" } }
                div { class: "chart-rail", span { class: "play-type", "{play_type}" } span { class: "difficulty", "{difficulty}" } span { "LV {level}" } span { "NOTES {notes}" } }
            } } }
        }
        WidgetKind::Score => score_widget(state),
        WidgetKind::HistoryList => history_list(&state.history, widget.settings.history_count),
        WidgetKind::HistoryGraph => history_graph(&state.history, widget.settings.graph_months),
    }
}

#[must_use]
pub fn default_widgets() -> Vec<WidgetLayout> {
    let widget = |id: &str, kind, y, height, z| WidgetLayout {
        id: id.into(),
        kind,
        x: 0,
        y,
        width: 560,
        height,
        z,
        settings: WidgetSettings::default(),
    };
    vec![
        widget("status", WidgetKind::Status, 0, 74, 0),
        widget("selection", WidgetKind::Selection, 82, 120, 1),
        widget("score", WidgetKind::Score, 210, 300, 2),
        widget("history-list", WidgetKind::HistoryList, 518, 236, 3),
        widget("history-graph", WidgetKind::HistoryGraph, 762, 278, 4),
    ]
}

fn score_widget(state: &OverlayState) -> Element {
    let fields = [
        ("PGREAT", &state.detail.pgreat, "pgreat"),
        ("GREAT", &state.detail.great, "great"),
        ("GOOD", &state.detail.good, "good"),
        ("BAD", &state.detail.bad, "bad"),
        ("POOR", &state.detail.poor, "poor"),
        ("FAST", &state.detail.fast, "fast"),
        ("SLOW", &state.detail.slow, "slow"),
        ("COMBO BREAK", &state.detail.combo_break, "combo"),
        ("PLAY OPTIONS", &state.detail.play_options, "options"),
    ];
    rsx! { section { class: "widget score-widget", {chrome()} div { class: "widget-content score-content",
        div { class: "best-section", h2 { "BEST" } div { class: "best-grid", div { class: "score-main", label { "EX SCORE" } strong { "{shown(&state.best.score)}" } span { class: "clear-value", "{shown(&state.best.clear)}" } } div { class: "best-side", label { "DJ LEVEL" } strong { class: "dj-level", "{shown(&state.best.dj_level)}" } label { "MISS COUNT" } b { "{shown(&state.best.miss)}" } } } }
        div { class: "detail-section", h2 { "RESULT DETAIL" } for (label, value, class) in fields { div { class: "detail-row {class}", span { "{label}" } b { "{shown(value)}" } } } }
    } } }
}
fn history_list(history: &History, count: u32) -> Element {
    rsx! { section { class: "widget history-list-widget", {chrome()} div { class: "widget-content history-content", h2 { "HISTORY" } div { class: "history-row history-head", span { "DATE" } span { "EX SCORE" } span { "DJ LEVEL" } span { "MISS" } span { "CLEAR" } } for play in history.plays.iter().take(count as usize) { div { class: "history-row", time { "{play.notified_at}" } b { "{play.score}" } span { "{play.dj_level}" } span { "{play.miss}" } span { "{play.clear}" } } } } } }
}
fn history_graph(history: &History, months: u32) -> Element {
    let start = match months {
        1 => history.graph_start_unix_ms[0],
        3 => history.graph_start_unix_ms[1],
        12 => history.graph_start_unix_ms[3],
        _ => history.graph_start_unix_ms[2],
    };
    let values = history
        .graph
        .iter()
        .filter(|play| play.received_unix_ms >= start)
        .cloned()
        .collect::<Vec<_>>();
    let points = graph_points(&values, start, history.graph_end_unix_ms);
    let score_dots: Vec<_> = values
        .iter()
        .map(|play| {
            dot_style(
                play.received_unix_ms,
                play.score_ratio,
                start,
                history.graph_end_unix_ms,
            )
        })
        .collect();
    let miss_dots: Vec<_> = values
        .iter()
        .filter_map(|play| {
            play.miss_ratio.map(|ratio| {
                dot_style(
                    play.received_unix_ms,
                    ratio,
                    start,
                    history.graph_end_unix_ms,
                )
            })
        })
        .collect();
    let levels = [
        ("AAA", 88.889),
        ("AA", 77.778),
        ("A", 66.667),
        ("B", 55.556),
        ("C", 44.444),
        ("D", 33.333),
        ("E", 22.222),
    ];
    rsx! { section { class: "widget history-graph-widget", {chrome()} div { class: "widget-content graph-content", h2 { "HISTORY GRAPH" } div { class: "graph-legend", span { class: "score-key", "DJ LEVEL" } span { class: "miss-key", "MISS RATE" } } div { class: "plot", div { class: "level-axis", for (level,threshold) in levels { span { style: format!("top:{:.3}%",100.0-threshold), "{level}" } } } div { class: "plot-area", for (level,threshold) in levels { i { class: "threshold", "data-level": level, style: format!("top:{:.3}%",100.0-threshold) } } for style in score_dots { i { class:"graph-dot score-dot",style } } for style in miss_dots { i { class:"graph-dot miss-dot",style } } svg { view_box: "0 0 1000 100", preserve_aspect_ratio: "none", polyline { class: "score-line", points: "{points.0}" } for segment in points.1 { polyline { class: "miss-line", points: "{segment}" } } } } div { class: "miss-axis", for value in ["100%","75%","50%","25%","0%"] { span { "{value}" } } } } } } }
}
fn dot_style(time: i64, ratio: f64, start: i64, end: i64) -> String {
    let x = (time_ratio(time, start, end) * 100.0).clamp(0.0, 100.0);
    let y = 100.0 - ratio.clamp(0.0, 1.0) * 100.0;
    format!("left:{x:.2}%;top:{y:.2}%")
}
fn graph_points(values: &[GraphPlay], start: i64, end: i64) -> (String, Vec<String>) {
    let point = |time: i64, ratio: f64| {
        format!(
            "{:.2},{:.2}",
            time_ratio(time, start, end) * 1000.0,
            100.0 - ratio.clamp(0.0, 1.0) * 100.0
        )
    };
    let score = values
        .iter()
        .map(|v| point(v.received_unix_ms, v.score_ratio))
        .collect::<Vec<_>>()
        .join(" ");
    let mut miss = Vec::new();
    let mut segment = Vec::new();
    for value in values {
        if let Some(ratio) = value.miss_ratio {
            segment.push(point(value.received_unix_ms, ratio));
        } else if !segment.is_empty() {
            miss.push(std::mem::take(&mut segment).join(" "));
        }
    }
    if !segment.is_empty() {
        miss.push(segment.join(" "));
    }
    (score, miss)
}

fn time_ratio(time: i64, start: i64, end: i64) -> f64 {
    let elapsed = time.saturating_sub(start).max(0).cast_unsigned();
    let span = end.saturating_sub(start).max(1).cast_unsigned();
    std::time::Duration::from_millis(elapsed).as_secs_f64()
        / std::time::Duration::from_millis(span).as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::{
        GraphPlay, WidgetKind, WidgetLayout, WidgetSettings, default_widget_size, graph_points,
        next_widget_id, time_ratio,
    };

    #[test]
    fn unknown_miss_breaks_the_graph_line() {
        let plays = [
            GraphPlay {
                received_unix_ms: 0,
                score_ratio: 0.5,
                miss_ratio: Some(0.1),
            },
            GraphPlay {
                received_unix_ms: 1_000,
                score_ratio: 0.6,
                miss_ratio: None,
            },
            GraphPlay {
                received_unix_ms: 2_000,
                score_ratio: 0.7,
                miss_ratio: Some(0.2),
            },
        ];
        let (_, miss) = graph_points(&plays, 0, 2_000);
        assert_eq!(miss.len(), 2);
        assert!(miss.iter().all(|segment| !segment.contains(' ')));
    }

    #[test]
    fn widget_defaults_are_on_grid_and_ids_fill_deleted_holes() {
        for kind in [
            WidgetKind::Status,
            WidgetKind::Selection,
            WidgetKind::Score,
            WidgetKind::HistoryList,
            WidgetKind::HistoryGraph,
        ] {
            let (width, height) = default_widget_size(kind);
            assert!(width.is_multiple_of(4));
            assert!(height.is_multiple_of(4));
        }
        let widget = |id: &str| WidgetLayout {
            id: id.into(),
            kind: WidgetKind::Status,
            x: 0,
            y: 0,
            width: 560,
            height: 72,
            z: 0,
            settings: WidgetSettings::default(),
        };
        let widgets = [widget("status"), widget("status-1"), widget("status-3")];
        assert_eq!(next_widget_id(WidgetKind::Status, &widgets), "status-2");
    }

    #[test]
    fn graph_positions_retain_millisecond_differences() {
        assert!((time_ratio(1, 0, 1_000) - 0.001).abs() < f64::EPSILON);
        assert!((time_ratio(999, 0, 1_000) - 0.999).abs() < f64::EPSILON);
    }
}
