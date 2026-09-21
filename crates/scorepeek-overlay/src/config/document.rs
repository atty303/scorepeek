use crate::{Backend, Skin, WidgetKind, WidgetSettings};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::layout::{Canvas, Widget, default_height, default_width};

pub const SCHEMA_VERSION: u32 = 9;
pub const OBS_OUTPUT_ID: &str = "obs-output";
pub const PENDING_WAYLAND_OUTPUT_ID: &str = "__pending-wayland-output__";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayConfig {
    pub schema_version: u32,
    #[doc(hidden)]
    #[serde(default, rename = "wayland_refresh_hz", skip_serializing)]
    pub legacy_wayland_refresh_hz: Option<serde_json::Value>,
    #[serde(default = "default_unknown_grace_ms")]
    pub unknown_grace_ms: u32,
    /// Run-local projection generations; omitted from the persisted v7 document.
    #[serde(skip)]
    pub projection_generations: ProjectionGenerations,
    #[serde(default = "default_listen")]
    pub obs_listen: String,
    pub canvases: Vec<Canvas>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectionGenerations {
    pub wayland: u64,
    pub obs: u64,
}

impl ProjectionGenerations {
    #[must_use]
    pub fn get(&self, backend: Backend) -> u64 {
        match backend {
            Backend::Wayland => self.wayland,
            Backend::Obs => self.obs,
        }
    }
    /// Advances the run-local renderer projection generation.
    ///
    /// # Errors
    /// Returns an error if the process-lifetime counter is exhausted.
    pub fn increment(&mut self, backend: Backend) -> Result<u64, String> {
        let generation = match backend {
            Backend::Wayland => &mut self.wayland,
            Backend::Obs => &mut self.obs,
        };
        *generation = generation
            .checked_add(1)
            .ok_or("projection generation exhausted")?;
        Ok(*generation)
    }
}

impl OverlayConfig {
    #[must_use]
    pub fn initial() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            legacy_wayland_refresh_hz: None,
            unknown_grace_ms: default_unknown_grace_ms(),
            projection_generations: ProjectionGenerations::default(),
            obs_listen: default_listen(),
            canvases: Vec::new(),
        }
    }
}

/// Migrates a version-eight overlay document to the current schema.
///
/// # Errors
/// Returns an error when the source document is malformed or cannot be serialized.
pub fn migrate_v8(text: &str) -> Result<(bool, String), String> {
    let mut document: toml::Value =
        toml::from_str(text).map_err(|error| format!("overlay TOML: {error}"))?;
    let Some(root) = document.as_table_mut() else {
        return Err("overlay TOML root must be a table".into());
    };
    if root.get("schema_version").and_then(toml::Value::as_integer) != Some(8) {
        return Ok((false, text.to_owned()));
    }
    if let Some(canvases) = root.get_mut("canvases").and_then(toml::Value::as_array_mut) {
        for canvas in canvases {
            let Some(canvas) = canvas.as_table_mut() else {
                continue;
            };
            if let Some(background) = canvas.remove("background") {
                let properties = canvas
                    .entry("skin_properties")
                    .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
                    .as_table_mut()
                    .ok_or("canvas skin_properties must be a table")?;
                properties.entry("background").or_insert(background);
            }
            if let Some(widgets) = canvas
                .get_mut("widgets")
                .and_then(toml::Value::as_array_mut)
            {
                for widget in widgets {
                    let Some(widget) = widget.as_table_mut() else {
                        continue;
                    };
                    let migrated = {
                        let Some(settings) = widget
                            .get_mut("settings")
                            .and_then(toml::Value::as_table_mut)
                        else {
                            continue;
                        };
                        [
                            ("frame_width", "frame-width"),
                            ("fill_opacity_percent", "fill-opacity-percent"),
                        ]
                        .into_iter()
                        .filter_map(|(old, new)| settings.remove(old).map(|value| (new, value)))
                        .collect::<Vec<_>>()
                    };
                    let properties = widget
                        .entry("skin_properties")
                        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
                        .as_table_mut()
                        .ok_or("widget skin_properties must be a table")?;
                    for (key, value) in migrated {
                        properties.entry(key).or_insert(value);
                    }
                }
            }
        }
    }
    root.insert("schema_version".into(), toml::Value::Integer(9));
    let text = toml::to_string_pretty(&document)
        .map_err(|error| format!("serialize migrated overlay TOML: {error}"))?;
    Ok((true, text))
}

#[doc(hidden)]
#[must_use]
pub fn visual_debug_config(skin: crate::Skin) -> OverlayConfig {
    use crate::ScreenKind;

    let mut config = OverlayConfig::initial();
    config.canvases = [Backend::Wayland, Backend::Obs]
        .into_iter()
        .flat_map(|backend| {
            let prefix = match backend {
                Backend::Wayland => "wayland",
                Backend::Obs => "obs",
            };
            let x = if backend == Backend::Obs { 1340 } else { 20 };
            [
                (
                    "status",
                    20,
                    560,
                    72,
                    None,
                    vec![("status", WidgetKind::Status, 0, 0)],
                ),
                (
                    "selection",
                    100,
                    560,
                    960,
                    Some(vec![ScreenKind::MusicSelect]),
                    dashboard_test_widgets(),
                ),
                (
                    "play",
                    100,
                    560,
                    140,
                    Some(vec![ScreenKind::DecideTransition, ScreenKind::Play]),
                    vec![("selection", WidgetKind::Selection, 0, 0)],
                ),
                (
                    "result",
                    100,
                    560,
                    960,
                    Some(vec![ScreenKind::Result]),
                    dashboard_test_widgets(),
                ),
            ]
            .into_iter()
            .map(move |(suffix, y, width, height, show_on, widgets)| Canvas {
                id: format!("{prefix}-{suffix}"),
                name: suffix.replace('-', " "),
                backend,
                skin,
                skin_properties: BTreeMap::new(),
                show_on,
                opacity_percent: 100,
                output: if backend == Backend::Obs {
                    OBS_OUTPUT_ID.into()
                } else {
                    "DP-1".into()
                },
                x,
                y,
                width,
                height,
                widgets: widgets
                    .into_iter()
                    .map(|(id, kind, x, y)| {
                        let (width, height) = visual_debug_widget_size(kind);
                        Widget {
                            id: id.into(),
                            kind,
                            x: x + 8,
                            y: y + 8,
                            width,
                            height,
                            settings: WidgetSettings::default(),
                            skin_properties: BTreeMap::new(),
                        }
                    })
                    .collect(),
            })
        })
        .collect();
    config
}

fn dashboard_test_widgets() -> Vec<(&'static str, WidgetKind, i32, i32)> {
    vec![
        ("selection", WidgetKind::Selection, 0, 0),
        ("score", WidgetKind::Score, 0, 148),
        ("history-list", WidgetKind::HistoryList, 0, 372),
        ("history-graph", WidgetKind::HistoryGraph, 0, 552),
    ]
}

#[must_use]
pub fn empty_canvas(id: String, backend: Backend, skin: Skin) -> Canvas {
    Canvas {
        name: id.clone(),
        id,
        backend,
        skin,
        skin_properties: BTreeMap::new(),
        show_on: Some(Vec::new()),
        opacity_percent: 100,
        output: if backend == Backend::Obs {
            OBS_OUTPUT_ID.into()
        } else {
            PENDING_WAYLAND_OUTPUT_ID.into()
        },
        x: 20,
        y: 20,
        width: default_width(),
        height: default_height(),
        widgets: Vec::new(),
    }
}

const fn visual_debug_widget_size(kind: WidgetKind) -> (u32, u32) {
    match kind {
        WidgetKind::Status => (544, 56),
        WidgetKind::Selection => (544, 132),
        WidgetKind::Score | WidgetKind::HistoryGraph => (544, 208),
        WidgetKind::HistoryList => (544, 164),
        WidgetKind::Empty => (320, 180),
    }
}

const fn default_unknown_grace_ms() -> u32 {
    1_000
}
fn default_listen() -> String {
    "127.0.0.1:3939".into()
}
