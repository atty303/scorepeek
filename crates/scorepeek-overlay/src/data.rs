//! Backend-neutral overlay display DTOs and layout data.
pub use crate::geometry::canvas::{CanvasPresentation, canvas_visible};
pub use crate::geometry::widget::{
    AspectRatio, WidgetKind, WidgetLayout, WidgetSettings, next_widget_id,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Wayland,
    Obs,
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

#[cfg(test)]
mod tests {
    use super::{
        ScreenKind, ScreenView, WidgetKind, WidgetLayout, WidgetSettings, canvas_visible,
        next_widget_id,
    };

    #[test]
    fn ids_fill_deleted_holes() {
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
    fn canvas_screen_filter_treats_omission_as_always_visible() {
        let screen = ScreenView {
            kind: Some(ScreenKind::Play),
            ..ScreenView::default()
        };
        assert!(canvas_visible(None, screen));
        assert!(canvas_visible(Some(&[ScreenKind::Play]), screen));
        assert!(!canvas_visible(Some(&[ScreenKind::Result]), screen));
        assert!(canvas_visible(
            Some(&[ScreenKind::Unknown]),
            ScreenView::default()
        ));
    }
}
