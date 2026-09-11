//! Shared canvas widgets for native and browser renderers.
use dioxus::prelude::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

mod appearance;
mod assets;
pub mod composition;
pub mod editor;
pub mod editor_model;
pub mod editor_surface;
mod frame;
pub use composition::{AspectRatio, Background, FrameWidth};
pub mod motion;
pub mod typography;
pub use appearance::{Appearance, Skin};
pub use assets::{FONT_ASSETS, FONT_CSS, FONT_LICENSES, SKIN_ASSETS, skin_asset};
pub const OXANIUM: &[u8] = include_bytes!("../assets/fonts/Oxanium.ttf");
pub const BASE_CSS: &str = include_str!("../styles/base.css");
pub const EDITOR_CSS: &str = concat!(
    include_str!("../styles/editor.css"),
    include_str!("../styles/editor-button.css")
);
pub const SKIN_CSS: &str = concat!(
    include_str!("../styles/cyan-system.css"),
    include_str!("../styles/result-aurora.css"),
    include_str!("../styles/dj-blackbox.css"),
    include_str!("../styles/rich.css"),
    include_str!("../styles/composition.css")
);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WaylandRefreshRate {
    #[default]
    Auto,
    Capped(u16),
}

impl WaylandRefreshRate {
    pub const MAX_HZ: u16 = 1_000;

    /// Builds an explicitly capped Wayland paint rate.
    ///
    /// # Errors
    /// Returns an error when `hz` is outside the supported 1 through 1000 Hz range.
    pub fn capped(hz: u16) -> Result<Self, &'static str> {
        if (1..=Self::MAX_HZ).contains(&hz) {
            Ok(Self::Capped(hz))
        } else {
            Err("refresh rate must be auto or an integer from 1 through 1000 Hz")
        }
    }

    #[must_use]
    pub const fn hz(self) -> Option<u16> {
        match self {
            Self::Auto => None,
            Self::Capped(hz) => Some(hz),
        }
    }
}

impl Serialize for WaylandRefreshRate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Auto => serializer.serialize_str("auto"),
            Self::Capped(hz) => serializer.serialize_u16(*hz),
        }
    }
}

impl<'de> Deserialize<'de> for WaylandRefreshRate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;
        impl de::Visitor<'_> for Visitor {
            type Value = WaylandRefreshRate;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("\"auto\" or an integer from 1 through 1000")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value == "auto" {
                    Ok(WaylandRefreshRate::Auto)
                } else {
                    Err(E::custom("refresh rate string must be \"auto\""))
                }
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                u16::try_from(value)
                    .ok()
                    .and_then(|hz| WaylandRefreshRate::capped(hz).ok())
                    .ok_or_else(|| E::custom("refresh rate must be from 1 through 1000 Hz"))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                u16::try_from(value)
                    .ok()
                    .and_then(|hz| WaylandRefreshRate::capped(hz).ok())
                    .ok_or_else(|| E::custom("refresh rate must be from 1 through 1000 Hz"))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

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
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GraphTick {
    pub unix_ms: i64,
    pub label: String,
}
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct History {
    pub recorded: bool,
    pub plays: Vec<HistoryPlay>,
    pub graph: Vec<GraphPlay>,
    #[serde(default)]
    pub graph_ticks: Vec<GraphTick>,
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
    pub result_signal: LampState,
    #[serde(default)]
    pub best: BestView,
    #[serde(default)]
    pub detail: ResultDetail,
    #[serde(default)]
    pub history: History,
    #[serde(default)]
    pub screen: ScreenView,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScreenKind {
    Unknown,
    MusicSelect,
    ModeSelect,
    DecideTransition,
    Play,
    Result,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScreenView {
    #[serde(default)]
    pub kind: Option<ScreenKind>,
    #[serde(default)]
    pub suspended_since_unix_ms: Option<i64>,
    #[serde(default)]
    pub revision: u64,
}

#[must_use]
pub fn canvas_visible(show_on: Option<&[ScreenKind]>, screen: ScreenView) -> bool {
    show_on.is_none_or(|screens| screens.contains(&screen.kind.unwrap_or(ScreenKind::Unknown)))
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WidgetKind {
    Status,
    Selection,
    Score,
    HistoryList,
    HistoryGraph,
    Empty,
}

impl WidgetKind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Selection => "selection",
            Self::Score => "score",
            Self::HistoryList => "history-list",
            Self::HistoryGraph => "history-graph",
            Self::Empty => "empty",
        }
    }
}

#[must_use]
pub const fn default_widget_size(kind: WidgetKind) -> (u32, u32) {
    match kind {
        WidgetKind::Status => (544, 44),
        WidgetKind::Selection => (544, 124),
        WidgetKind::Score => (544, 200),
        WidgetKind::HistoryList => (544, 156),
        WidgetKind::HistoryGraph => (544, 208),
        WidgetKind::Empty => (640, 360),
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
        WidgetKind::Empty => "empty",
    };
    (1..=widgets.len().saturating_add(1))
        .map(|number| format!("{stem}-{number}"))
        .find(|candidate| widgets.iter().all(|widget| widget.id != *candidate))
        .unwrap_or_else(|| format!("{stem}-{}", widgets.len().saturating_add(1)))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetSettings {
    #[serde(default)]
    pub frame_width: FrameWidth,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub fill_opacity_percent: u8,
    #[serde(default)]
    pub aspect_ratio: AspectRatio,
    #[serde(default = "default_history_count")]
    pub history_count: u32,
    #[serde(default = "default_graph_months")]
    pub graph_months: u32,
}
impl Default for WidgetSettings {
    fn default() -> Self {
        Self {
            frame_width: FrameWidth::default(),
            title: String::new(),
            fill_opacity_percent: 0,
            aspect_ratio: AspectRatio::default(),
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
    #[serde(default)]
    pub settings: WidgetSettings,
    #[serde(default)]
    pub skin_properties: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanvasPresentation {
    #[serde(default)]
    pub background: Background,
    pub id: String,
    pub name: String,
    pub skin: Skin,
    #[serde(default)]
    pub skin_properties: std::collections::BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub show_on: Option<Vec<ScreenKind>>,
    #[serde(default = "default_opacity_percent")]
    pub opacity_percent: u8,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    #[serde(default = "default_canvas_width")]
    pub width: u32,
    #[serde(default = "default_canvas_height")]
    pub height: u32,
    pub widgets: Vec<WidgetLayout>,
}

const fn default_opacity_percent() -> u8 {
    100
}
const fn default_canvas_width() -> u32 {
    560
}
const fn default_canvas_height() -> u32 {
    1040
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
fn lamp(state: LampState, label: Option<&str>, class: &str, skin: Skin) -> Element {
    let artwork = frame::lamp(state, label.is_none(), skin, class);
    let state = format!("{state:?}").to_ascii_lowercase();
    rsx! { div { class: "lamp-group {class}", span { class: "lamp", "data-state": state, aria_hidden: "true", {artwork} } if let Some(label) = label { span { class: "lamp-label", {typography::label(label, skin, 15)} } } } }
}
fn chrome(widget: &WidgetLayout, skin: Skin) -> Element {
    rsx! { div { class: "skin-frame", aria_hidden: "true",
        {frame::render(widget, skin)}
        div { class: "skin-energy" }
        div { class: "skin-glint" }
    } }
}

#[component]
pub fn OverlayStyles() -> Element {
    rsx! {style {"{BASE_CSS}{SKIN_CSS}{EDITOR_CSS}"}}
}

/// Renders the approved five-widget master composition.
/// # Errors
/// Returns a Dioxus render error if element construction fails.
pub fn overlay_panel(state: &OverlayState, appearance: Appearance) -> Element {
    overlay_canvas(state, appearance, &default_widgets(), Background::None)
}

/// Renders independently positioned widgets inside one canvas.
/// # Errors
/// Returns a Dioxus render error if element construction fails.
pub fn overlay_canvas(
    state: &OverlayState,
    appearance: Appearance,
    widgets: &[WidgetLayout],
    background: Background,
) -> Element {
    let skin = appearance.skin.name();
    let graph_colors = appearance.skin.graph_colors();
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
    rsx! {
        OverlayStyles {}
        main { class: "overlay-canvas", "data-skin": skin, style: format!("--graph-score:{};--graph-miss:{}", graph_colors.score, graph_colors.miss),
            {composition::background(background, widgets, appearance.skin)}
            for widget in widgets {
                div {
                    key: "{widget.id}",
                    class: "widget-slot",
                    "data-widget-id": "{widget.id}",
                    style: format!("left:{}px;top:{}px;width:{}px;height:{}px", widget.x, widget.y, widget.width, widget.height),
                    {render_inner_widget(widget, state, title, artist, play_type, &difficulty, &level, &notes, appearance.skin)}
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_inner_widget(
    widget: &WidgetLayout,
    state: &OverlayState,
    title: &str,
    artist: &str,
    play_type: &str,
    difficulty: &str,
    level: &str,
    notes: &str,
    skin: Skin,
) -> Element {
    if widget.kind == WidgetKind::Empty {
        return composition::empty_widget(widget, skin);
    }
    let mut legacy = widget.clone();
    legacy.width += 16;
    legacy.height += 16;
    rsx! { div { class: "widget-content-origin", style:format!("position:absolute;left:-8px;top:-8px;width:{}px;height:{}px",legacy.width,legacy.height),
        {render_widget(&legacy,state,title,artist,play_type,difficulty,level,notes,skin)}
    } }
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
    skin: Skin,
) -> Element {
    match widget.kind {
        WidgetKind::Status => {
            rsx! { section { class: "widget status-widget", {chrome(widget, skin)} div { class: "widget-content status-content", span { class: "wordmark", "score" span { "peek" } } div { class: "status-lamps", {lamp(state.system, Some("SYSTEM"), "system-lamp", skin)} {lamp(state.result_signal, Some("RESULT"), "result-lamp", skin)} } } } }
        }
        WidgetKind::Selection => {
            rsx! { section { class: "widget selection-widget", {chrome(widget, skin)} div { class: "widget-content selection-content",
                {lamp(if state.history.recorded { LampState::Active } else { LampState::Inactive }, None, "recorded-lamp", skin)}
                div { class: "song-copy", h1 { title: title, "{title}" } p { title: artist, "{artist}" } }
                div { class: "chart-rail", {frame::chart_rail(widget.width.saturating_sub(56), skin)} span { class: "play-type", {typography::label(play_type, skin, 24)} } span { class: "difficulty", "data-difficulty": difficulty, {typography::label(difficulty, skin, 20)} } span { span { class: "field-label", {typography::label("LV", skin, 18)} } "{level}" } span { span { class: "field-label", {typography::label("NOTES", skin, 18)} } "{notes}" } }
            } } }
        }
        WidgetKind::Score => score_widget(state, widget, skin),
        WidgetKind::HistoryList => history_list(&state.history, widget, skin),
        WidgetKind::HistoryGraph => history_graph(&state.history, widget, skin),
        WidgetKind::Empty => composition::empty_widget(widget, skin),
    }
}

#[must_use]
pub fn default_widgets() -> Vec<WidgetLayout> {
    let widget = |id: &str, kind, y: i32, height: u32| WidgetLayout {
        id: id.into(),
        kind,
        x: 8,
        y: y + 8,
        width: 544,
        height: height - 16,
        settings: WidgetSettings::default(),
        skin_properties: std::collections::BTreeMap::new(),
    };
    vec![
        widget("status", WidgetKind::Status, 0, 60),
        widget("selection", WidgetKind::Selection, 68, 140),
        widget("score", WidgetKind::Score, 216, 216),
        widget("history-list", WidgetKind::HistoryList, 440, 172),
        widget("history-graph", WidgetKind::HistoryGraph, 620, 224),
    ]
}

/// Stable editor-only data for arranging widgets while the recognition session is inactive.
#[must_use]
pub fn editor_sample_state() -> OverlayState {
    let day = 86_400_000_i64;
    let end = 1_788_134_400_000_i64;
    let scores = [
        0.57, 0.72, 0.67, 0.79, 0.69, 0.61, 0.65, 0.88, 0.80, 0.84, 0.86, 0.64, 0.68, 0.62, 0.83,
        0.70, 0.66, 0.74,
    ];
    let misses = [
        0.12, 0.15, 0.09, 0.22, 0.30, 0.14, 0.18, 0.24, 0.46, 0.34, 0.39, 0.58, 0.21, 0.18, 0.16,
        0.22, 0.24, 0.18,
    ];
    let graph = scores
        .into_iter()
        .zip(misses)
        .zip(0_i64..)
        .map(|((score_ratio, miss_ratio), index)| GraphPlay {
            received_unix_ms: end - day * (178 - index * 10),
            score_ratio,
            miss_ratio: Some(miss_ratio),
        })
        .collect();
    OverlayState {
        connected: false,
        chart: Some(Chart {
            song_id: "editor-sample".into(),
            play_type: "single".into(),
            difficulty: "hyper".into(),
            title: "NEON CIRCUIT".into(),
            artist: "SAMPLE ARTIST".into(),
            level: Some(12),
            notes: Some(1877),
        }),
        system: LampState::Inactive,
        result_signal: LampState::Active,
        best: BestView {
            score: "2846".into(),
            dj_level: "AA".into(),
            miss: "12".into(),
            clear: "HARD CLEAR".into(),
        },
        detail: ResultDetail {
            pgreat: "1324".into(),
            great: "198".into(),
            good: "21".into(),
            bad: "6".into(),
            poor: "12".into(),
            fast: "143".into(),
            slow: "137".into(),
            combo_break: "9".into(),
            play_options: "RANDOM".into(),
        },
        history: History {
            recorded: true,
            plays: (0..20)
                .map(|index| HistoryPlay {
                    notified_at: format!("2026.08.{:02} 21:{:02}", 28 - index, 10 + index),
                    score: (2846 - index * 13).to_string(),
                    dj_level: if index == 1 { "AAA" } else { "AA" }.into(),
                    miss: (12 + index).to_string(),
                    clear: if index % 5 == 4 {
                        "CLEAR"
                    } else {
                        "HARD CLEAR"
                    }
                    .into(),
                })
                .collect(),
            graph,
            graph_ticks: ["MAR", "APR", "MAY", "JUN", "JUL", "AUG"]
                .into_iter()
                .zip(0_i64..)
                .map(|(label, index)| GraphTick {
                    unix_ms: end - day * (178 - index * 30),
                    label: label.into(),
                })
                .collect(),
            graph_start_unix_ms: [
                end - day * 30,
                end - day * 90,
                end - day * 183,
                end - day * 365,
            ],
            graph_end_unix_ms: end,
        },
        screen: ScreenView {
            kind: Some(ScreenKind::MusicSelect),
            suspended_since_unix_ms: None,
            revision: 0,
        },
    }
}

fn score_widget(state: &OverlayState, widget: &WidgetLayout, skin: Skin) -> Element {
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
    rsx! { section { class: "widget score-widget", {chrome(widget, skin)} div { class: "widget-content score-content",
        div { class: "best-section", h2 { {typography::label("BEST", skin, 18)} } div { class: "best-grid", div { class: "score-main", label { {typography::label("EX SCORE", skin, 15)} } strong { {typography::metallic(shown(&state.best.score), skin, 66, false)} } span { class: "clear-value", "data-clear": motion::clear_role(&state.best.clear), {typography::label(shown(&state.best.clear), skin, 20)} } } div { class: "best-side", label { {typography::label("DJ LEVEL", skin, 15)} } strong { class: "dj-level", "data-rank": shown(&state.best.dj_level), {typography::metallic(shown(&state.best.dj_level), skin, 48, true)} } label { {typography::label("MISS COUNT", skin, 15)} } b { "{shown(&state.best.miss)}" } } } }
        div { class: "detail-section", h2 { {typography::label("RESULT DETAIL", skin, 18)} } for (label, value, class) in fields { div { class: "detail-row {class}", span { {typography::label(label, skin, 15)} } b { "{shown(value)}" } } } }
    } } }
}
fn history_list(history: &History, widget: &WidgetLayout, skin: Skin) -> Element {
    let count = widget.settings.history_count;
    rsx! { section { class: "widget history-list-widget", {chrome(widget, skin)} div { class: "widget-content history-content", h2 { {typography::label("HISTORY", skin, 18)} } div { class: "history-row history-head", span { {typography::label("DATE", skin, 12)} } span { {typography::label("EX SCORE", skin, 12)} } span { {typography::label("DJ LEVEL", skin, 12)} } span { {typography::label("MISS", skin, 12)} } span { {typography::label("CLEAR", skin, 12)} } } for play in history.plays.iter().take(count as usize) { div { class: "history-row", time { "{play.notified_at}" } b { "{play.score}" } span { "data-rank": &play.dj_level, "{play.dj_level}" } span { "{play.miss}" } span { "data-clear": motion::clear_role(&play.clear), {typography::label(&play.clear, skin, 16)} } } } } } }
}
fn history_graph(history: &History, widget: &WidgetLayout, skin: Skin) -> Element {
    let colors = skin.graph_colors();
    // Blitz paints inline SVG as a contained image. Share the plot viewport with CSS
    // so its image aspect ratio also follows widget resizing.
    let padding_x = 18;
    let padding_y = 12;
    let score_axis = 36;
    let miss_axis = 39;
    let header_space = 68;
    let plot_width = widget
        .width
        .saturating_sub(2 * padding_x + score_axis + miss_axis)
        .max(1);
    let plot_height = widget
        .height
        .saturating_sub(2 * padding_y + header_space)
        .max(1);
    let geometry = format!(
        "--graph-padding-x:{padding_x}px;--graph-padding-y:{padding_y}px;--score-axis:{score_axis}px;--miss-axis:{miss_axis}px;--plot-width:{plot_width}px;--plot-height:{plot_height}px"
    );
    let start = match widget.settings.graph_months {
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
    let time_ticks = history
        .graph_ticks
        .iter()
        .filter(|tick| tick.unix_ms >= start && tick.unix_ms <= history.graph_end_unix_ms)
        .map(|tick| {
            (
                time_ratio(tick.unix_ms, start, history.graph_end_unix_ms) * 100.0,
                &tick.label,
            )
        });
    let levels = [
        ("AAA", 88.889),
        ("AA", 77.778),
        ("A", 66.667),
        ("B", 55.556),
        ("C", 44.444),
        ("D", 33.333),
        ("E", 22.222),
    ];
    rsx! { section { class: "widget history-graph-widget", style: geometry, {chrome(widget, skin)} div { class: "widget-content graph-content", h2 { {typography::label("HISTORY GRAPH", skin, 18)} } div { class: "graph-legend", span { class: "score-key", {typography::label("DJ LEVEL", skin, 14)} } span { class: "miss-key", {typography::label("MISS RATE", skin, 14)} } } div { class: "plot", div { class: "level-axis", for (level,threshold) in levels { span { style: format!("top:{:.3}%",100.0-threshold), {typography::label(level, skin, 14)} } } } div { class: "plot-area", for (level,threshold) in levels { i { class: "threshold", "data-level": level, style: format!("top:{:.3}%",100.0-threshold) } } for style in score_dots { i { class:"graph-dot score-dot",style } } for style in miss_dots { i { class:"graph-dot miss-dot",style } } svg { width: "{plot_width}", height: "{plot_height}", view_box: "0 0 1000 100", preserve_aspect_ratio: "none", polyline { class: "score-line", fill: "none", stroke: colors.score, stroke_width: "1.25", vector_effect: "non-scaling-stroke", points: "{points.0}" } for segment in points.1 { polyline { class: "miss-line", fill: "none", stroke: colors.miss, stroke_width: "1.25", vector_effect: "non-scaling-stroke", points: "{segment}" } } } } div { class: "miss-axis", for value in ["100%","75%","50%","25%","0%"] { span { {typography::label(value, skin, 14)} } } } } div { class: "time-axis", for (position,label) in time_ticks { span { style: format!("left:{position:.3}%"), "{label}" } } } } } }
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
        GraphPlay, ScreenKind, ScreenView, WidgetKind, WidgetLayout, WidgetSettings,
        canvas_visible, default_widget_size, graph_points, next_widget_id, time_ratio,
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
            settings: WidgetSettings::default(),
            skin_properties: std::collections::BTreeMap::new(),
        };
        let widgets = [widget("status"), widget("status-1"), widget("status-3")];
        assert_eq!(next_widget_id(WidgetKind::Status, &widgets), "status-2");
    }

    #[test]
    fn graph_positions_retain_millisecond_differences() {
        assert!((time_ratio(1, 0, 1_000) - 0.001).abs() < f64::EPSILON);
        assert!((time_ratio(999, 0, 1_000) - 0.999).abs() < f64::EPSILON);
    }

    #[test]
    fn canvas_screen_filter_treats_omission_as_always_visible() {
        let screen = ScreenView {
            kind: Some(ScreenKind::Play),
            ..ScreenView::default()
        };
        assert!(canvas_visible(None, screen));
        assert!(canvas_visible(Some(&[ScreenKind::Play]), screen));
        assert!(!canvas_visible(Some(&[ScreenKind::Result]), screen));
        assert!(!canvas_visible(
            Some(&[ScreenKind::Play]),
            ScreenView::default()
        ));
        assert!(canvas_visible(
            Some(&[ScreenKind::Unknown]),
            ScreenView::default()
        ));
    }
}
