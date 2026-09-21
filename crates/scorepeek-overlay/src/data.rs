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

pub use crate::style::{EDITOR_CSS, HOST_CSS};

pub(crate) fn validate_skin_id(value: &str) -> Result<(), String> {
    let segments = value.split('.').collect::<Vec<_>>();
    if segments.len() < 2
        || segments.iter().any(|segment| {
            segment.is_empty()
                || (!segment.as_bytes()[0].is_ascii_lowercase()
                    && !segment.as_bytes()[0].is_ascii_digit())
                || !segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
    {
        return Err("skin id must be a lowercase ASCII reverse-domain name".into());
    }
    Ok(())
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

/// Stable editor-only data for arranging widgets while recognition is inactive.
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
