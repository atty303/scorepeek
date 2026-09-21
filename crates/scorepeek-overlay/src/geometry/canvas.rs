//! Canvas geometry and visibility.

use serde::{Deserialize, Serialize};

use super::widget::WidgetLayout;
use crate::{ScreenKind, ScreenView, Skin};

#[must_use]
pub fn canvas_visible(show_on: Option<&[ScreenKind]>, screen: ScreenView) -> bool {
    show_on.is_none_or(|screens| screens.contains(&screen.kind.unwrap_or(ScreenKind::Unknown)))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanvasPresentation {
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
