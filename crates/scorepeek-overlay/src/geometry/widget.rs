//! Widget geometry and settings.

use serde::{Deserialize, Serialize};

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
pub fn next_widget_id(kind: WidgetKind, widgets: &[WidgetLayout]) -> String {
    let stem = kind.name();
    (1..=widgets.len().saturating_add(1))
        .map(|number| format!("{stem}-{number}"))
        .find(|candidate| widgets.iter().all(|widget| widget.id != *candidate))
        .unwrap_or_else(|| format!("{stem}-{}", widgets.len().saturating_add(1)))
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AspectRatio {
    #[default]
    Free,
    Wide,
    Standard,
    Current([u32; 2]),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetSettings {
    #[serde(default)]
    pub title: String,
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
            title: String::new(),
            aspect_ratio: AspectRatio::default(),
            history_count: default_history_count(),
            graph_months: default_graph_months(),
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
