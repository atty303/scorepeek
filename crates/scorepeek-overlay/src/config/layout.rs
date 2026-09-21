//! Persisted canvas and widget layout types and presentation conversion.

use serde::{Deserialize, Serialize};

use crate::{Backend, Skin};

pub use crate::{AspectRatio, CanvasPresentation, WidgetKind, WidgetLayout, WidgetSettings};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Canvas {
    pub id: String,
    pub name: String,
    pub backend: Backend,
    pub skin: Skin,
    #[serde(default)]
    pub skin_properties: std::collections::BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub show_on: Option<Vec<crate::ScreenKind>>,
    #[serde(default = "default_opacity_percent")]
    pub opacity_percent: u8,
    pub output: String,
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
    #[serde(default)]
    pub widgets: Vec<Widget>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Widget {
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

impl Canvas {
    #[must_use]
    pub fn presentation(&self) -> CanvasPresentation {
        CanvasPresentation {
            id: self.id.clone(),
            name: self.name.clone(),
            skin: self.skin,
            skin_properties: self.skin_properties.clone(),
            show_on: self.show_on.clone(),
            opacity_percent: self.opacity_percent,
            output: Some(self.output.clone()),
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            widgets: self
                .widgets
                .iter()
                .map(|widget| WidgetLayout {
                    id: widget.id.clone(),
                    kind: widget.kind,
                    x: widget.x,
                    y: widget.y,
                    width: widget.width,
                    height: widget.height,
                    settings: widget.settings.clone(),
                    skin_properties: widget.skin_properties.clone(),
                })
                .collect(),
        }
    }

    pub fn apply_presentation(&mut self, presentation: &CanvasPresentation) {
        self.name.clone_from(&presentation.name);
        self.skin = presentation.skin;
        self.skin_properties
            .clone_from(&presentation.skin_properties);
        self.show_on.clone_from(&presentation.show_on);
        self.opacity_percent = presentation.opacity_percent;
        if let Some(output) = &presentation.output {
            self.output.clone_from(output);
        }
        self.x = presentation.x;
        self.y = presentation.y;
        self.width = presentation.width;
        self.height = presentation.height;
        self.widgets = presentation
            .widgets
            .iter()
            .map(|widget| Widget {
                id: widget.id.clone(),
                kind: widget.kind,
                x: widget.x,
                y: widget.y,
                width: widget.width,
                height: widget.height,
                settings: widget.settings.clone(),
                skin_properties: widget.skin_properties.clone(),
            })
            .collect();
    }
}

pub(super) const fn default_width() -> u32 {
    560
}

pub(super) const fn default_height() -> u32 {
    1040
}

const fn default_opacity_percent() -> u8 {
    100
}
