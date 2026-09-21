use crate::{Backend, Skin};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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

pub use crate::{WidgetKind, WidgetSettings};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigIssue {
    pub canvas_id: String,
    pub message: String,
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

    /// Validates global invariants and returns individually valid canvases.
    /// # Errors
    /// Returns an unsupported schema or a backend without a valid canvas.
    pub fn validated(&self) -> Result<(Vec<Canvas>, Vec<ConfigIssue>), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!("overlay schema_version must be {SCHEMA_VERSION}"));
        }
        if self.unknown_grace_ms > 10_000 {
            return Err("overlay unknown_grace_ms must be at most 10000".into());
        }
        let listen = self
            .obs_listen
            .parse::<std::net::SocketAddr>()
            .map_err(|error| format!("overlay obs_listen: {error}"))?;
        if !listen.ip().is_loopback() {
            return Err("overlay obs_listen must use a loopback address".into());
        }
        let mut canvas_ids = BTreeSet::new();
        let mut canvas_names = BTreeSet::new();
        let mut valid = Vec::new();
        let mut issues = Vec::new();
        for canvas in &self.canvases {
            match validate_canvas(canvas, &mut canvas_ids, &mut canvas_names)
                .and_then(|()| crate::validate_skin_id(canvas.skin.name()))
            {
                Ok(()) => valid.push(canvas.clone()),
                Err(message) => issues.push(ConfigIssue {
                    canvas_id: canvas.id.clone(),
                    message,
                }),
            }
        }
        Ok((valid, issues))
    }
}

impl Canvas {
    #[must_use]
    pub fn presentation(&self) -> crate::CanvasPresentation {
        crate::CanvasPresentation {
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
                .map(|widget| crate::WidgetLayout {
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

    pub fn apply_presentation(&mut self, presentation: &crate::CanvasPresentation) {
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

fn validate_canvas(
    canvas: &Canvas,
    canvas_ids: &mut BTreeSet<(Backend, String)>,
    canvas_names: &mut BTreeSet<(Backend, String)>,
) -> Result<(), String> {
    if canvas.id.is_empty()
        || !canvas
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("id must use ASCII letters, digits, '-' or '_'".into());
    }
    if !canvas_ids.insert((canvas.backend, canvas.id.clone())) {
        return Err("duplicate canvas id within backend workspace".into());
    }
    if canvas.name.trim().is_empty() {
        return Err("canvas name must be non-empty".into());
    }
    if !canvas_names.insert((canvas.backend, canvas.name.clone())) {
        return Err("duplicate canvas name within backend workspace".into());
    }
    if canvas.output.is_empty() || canvas.output == PENDING_WAYLAND_OUTPUT_ID {
        return Err("output must be assigned".into());
    }
    if canvas.width < 32 || canvas.height < 32 {
        return Err("canvas dimensions must be at least 32x32".into());
    }
    if canvas.opacity_percent == 0 || canvas.opacity_percent > 100 {
        return Err("canvas opacity_percent must be between 1 and 100".into());
    }
    let mut widget_ids = BTreeSet::new();
    for widget in &canvas.widgets {
        if widget.id.is_empty() || !widget_ids.insert(widget.id.clone()) {
            return Err("widget ids must be non-empty and unique per canvas".into());
        }
        if let crate::AspectRatio::Current([width, height]) = widget.settings.aspect_ratio
            && (width == 0 || height == 0)
        {
            return Err(format!(
                "widget {} locked aspect ratio dimensions must be positive",
                widget.id
            ));
        }
        if widget.width < 16 || widget.height < 16 {
            return Err(format!("widget {} must be at least 16x16", widget.id));
        }
        if widget.x % 4 != 0
            || widget.y % 4 != 0
            || !widget.width.is_multiple_of(4)
            || !widget.height.is_multiple_of(4)
        {
            return Err(format!(
                "widget {} position and dimensions must align to the 4px grid",
                widget.id
            ));
        }
        if !matches!(widget.settings.history_count, 5 | 10 | 20 | 50) {
            return Err(format!(
                "widget {} history_count must be 5, 10, 20 or 50",
                widget.id
            ));
        }
        if !matches!(widget.settings.graph_months, 1 | 3 | 6 | 12) {
            return Err(format!(
                "widget {} graph_months must be 1, 3, 6 or 12",
                widget.id
            ));
        }
    }
    Ok(())
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

const fn default_width() -> u32 {
    560
}
const fn default_height() -> u32 {
    1040
}

const fn default_unknown_grace_ms() -> u32 {
    1_000
}
const fn default_opacity_percent() -> u8 {
    100
}
fn default_listen() -> String {
    "127.0.0.1:3939".into()
}
