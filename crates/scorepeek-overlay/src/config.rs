use crate::runtime::Backend;
use scorepeek_overlay_ui::Skin;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

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
    pub show_on: Option<Vec<scorepeek_overlay_ui::ScreenKind>>,
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

pub use scorepeek_overlay_ui::{WidgetKind, WidgetSettings};

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
        self.validated_with_store(&crate::skin::StoreRoot::discover())
    }

    fn validated_with_store(
        &self,
        store: &crate::skin::StoreRoot,
    ) -> Result<(Vec<Canvas>, Vec<ConfigIssue>), String> {
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
            match validate_canvas(canvas, &mut canvas_ids, &mut canvas_names).and_then(|()| {
                crate::skin::validate_id(canvas.skin.name())?;
                store
                    .is_installed(canvas.skin.name())?
                    .then_some(())
                    .ok_or_else(|| format!("skin {} is not installed", canvas.skin.name()))
            }) {
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
    pub fn presentation(&self) -> scorepeek_overlay_ui::CanvasPresentation {
        scorepeek_overlay_ui::CanvasPresentation {
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
                .map(|widget| scorepeek_overlay_ui::WidgetLayout {
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

    pub fn apply_presentation(&mut self, presentation: &scorepeek_overlay_ui::CanvasPresentation) {
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

#[must_use]
pub fn default_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map_or_else(
        || {
            std::env::var_os("HOME").map_or_else(
                || PathBuf::from(".config/scorepeek/overlay.toml"),
                |home| PathBuf::from(home).join(".config/scorepeek/overlay.toml"),
            )
        },
        |root| PathBuf::from(root).join("scorepeek/overlay.toml"),
    )
}

/// Loads a strict configuration, creating the initial document when absent.
/// # Errors
/// Returns filesystem, TOML, or global validation errors.
pub fn load_or_create(path: &Path) -> Result<(OverlayConfig, Vec<ConfigIssue>), String> {
    load_or_create_with_store(path, &crate::skin::StoreRoot::discover())
}

fn load_or_create_with_store(
    path: &Path,
    store: &crate::skin::StoreRoot,
) -> Result<(OverlayConfig, Vec<ConfigIssue>), String> {
    if !path.exists() {
        let config = OverlayConfig::initial();
        save_atomic_with_store(path, &config, store)?;
        return Ok((config, Vec::new()));
    }
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| format!("overlay TOML is not UTF-8: {error}"))?;
    let (migrated, migrated_text) = migrate_v8(text)?;
    let mut config: OverlayConfig =
        toml::from_str(&migrated_text).map_err(|error| format!("overlay TOML: {error}"))?;
    let (valid, issues) = config.validated_with_store(store)?;
    if migrated {
        write_atomic(path, migrated_text.as_bytes())?;
    }
    config.canvases = valid;
    Ok((config, issues))
}

fn migrate_v8(text: &str) -> Result<(bool, String), String> {
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

/// Replaces a configuration durably in the same directory.
/// # Errors
/// Returns validation, serialization, or filesystem errors.
pub fn save_atomic(path: &Path, config: &OverlayConfig) -> Result<(), String> {
    save_atomic_with_store(path, config, &crate::skin::StoreRoot::discover())
}

fn save_atomic_with_store(
    path: &Path,
    config: &OverlayConfig,
    store: &crate::skin::StoreRoot,
) -> Result<(), String> {
    let (_, issues) = config.validated_with_store(store)?;
    if let Some(issue) = issues.first() {
        return Err(format!(
            "overlay canvas {}: {}",
            issue.canvas_id, issue.message
        ));
    }
    let bytes = toml::to_string_pretty(config)
        .map_err(|error| format!("serialize overlay TOML: {error}"))?;
    write_atomic(path, bytes.as_bytes())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "overlay config has no parent directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    let temporary = parent.join(format!(".overlay.toml.{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| format!("create {}: {error}", temporary.display()))?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        fs::File::open(parent)?.sync_all()
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(format!("persist {}: {error}", path.display()));
    }
    Ok(())
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
        if let scorepeek_overlay_ui::AspectRatio::Current([width, height]) =
            widget.settings.aspect_ratio
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
pub fn visual_debug_config(skin: scorepeek_overlay_ui::Skin) -> OverlayConfig {
    use scorepeek_overlay_ui::ScreenKind;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_skin() -> scorepeek_overlay_ui::Skin {
        "dev.atty303.scorepeek.skin.cyan-system".parse().unwrap()
    }

    fn test_widget() -> Widget {
        Widget {
            id: "status".into(),
            kind: WidgetKind::Status,
            x: 0,
            y: 0,
            width: 560,
            height: 72,
            settings: WidgetSettings::default(),
            skin_properties: std::collections::BTreeMap::new(),
        }
    }

    fn temporary(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "scorepeek-overlay-{name}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ))
    }

    #[test]
    fn initial_empty_workspace_round_trips_without_revisions() {
        let config = OverlayConfig::initial();
        let text = toml::to_string(&config).unwrap();
        let decoded: OverlayConfig = toml::from_str(&text).unwrap();
        assert_eq!(decoded, config);
        assert!(config.validated().unwrap().1.is_empty());
        assert!(!text.contains("wayland_refresh_hz"));
        assert!(config.canvases.is_empty());
        assert!(!text.contains("revision"));
    }

    #[test]
    fn legacy_wayland_refresh_rate_is_accepted_but_not_serialized() {
        let text = format!(
            "wayland_refresh_hz = 30\n{}",
            toml::to_string(&OverlayConfig::initial()).unwrap()
        );
        let config = toml::from_str::<OverlayConfig>(&text).unwrap();
        assert_eq!(
            config.legacy_wayland_refresh_hz,
            Some(serde_json::json!(30))
        );
        assert!(
            !toml::to_string(&config)
                .unwrap()
                .contains("wayland_refresh_hz")
        );
    }

    #[test]
    fn invalid_canvas_is_isolated() {
        let mut config = visual_debug_config(test_skin());
        config.canvases.push(Canvas {
            id: "bad id".into(),
            ..config.canvases[0].clone()
        });
        let (valid, issues) = config.validated().unwrap();
        assert_eq!(valid.len(), 8);
        assert_eq!(issues.len(), 1);
    }

    #[test]
    fn loading_omits_invalid_canvases_and_reports_them() {
        let root = temporary("invalid-canvas");
        let path = root.join("overlay.toml");
        let mut config = visual_debug_config(test_skin());
        config.canvases.push(Canvas {
            id: "bad id".into(),
            ..config.canvases[0].clone()
        });
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
        let (loaded, issues) = load_or_create(&path).unwrap();
        assert_eq!(loaded.canvases.len(), 8);
        assert_eq!(issues.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn obs_listener_must_be_loopback() {
        let mut config = OverlayConfig::initial();
        config.obs_listen = "0.0.0.0:3939".into();
        assert_eq!(
            config.validated().unwrap_err(),
            "overlay obs_listen must use a loopback address"
        );
    }

    #[test]
    fn canvas_with_off_grid_widget_is_isolated() {
        let mut config = visual_debug_config(test_skin());
        let mut invalid = config.canvases[0].clone();
        invalid.id = "wayland-off-grid".into();
        invalid.name = "Off grid".into();
        invalid.widgets.push(test_widget());
        invalid.widgets[0].x = 1;
        config.canvases.push(invalid);
        let (valid, issues) = config.validated().unwrap();
        assert_eq!(valid.len(), 8);
        assert_eq!(issues[0].canvas_id, "wayland-off-grid");
        assert!(issues[0].message.contains("4px grid"));
    }

    #[test]
    fn unknown_toml_fields_are_rejected() {
        let mut text = toml::to_string(&OverlayConfig::initial()).unwrap();
        text.insert_str(0, "unknown = true\n");
        assert!(toml::from_str::<OverlayConfig>(&text).is_err());
    }

    #[test]
    fn missing_config_creates_an_empty_workspace_without_a_skin() {
        let root = temporary("create");
        let path = root.join("overlay.toml");
        let store = crate::skin::StoreRoot::new(root.join("skins"));
        let (config, issues) = load_or_create_with_store(&path, &store).unwrap();
        assert!(config.canvases.is_empty());
        assert!(issues.is_empty());
        assert!(path.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn injected_skin_store_is_used_for_existing_config_and_save() {
        let root = temporary("injected-store");
        let path = root.join("overlay.toml");
        let store = crate::skin::StoreRoot::new(root.join("skins"));
        std::fs::create_dir_all(store.path()).unwrap();
        std::fs::write(store.path().join("dev.example.custom.zip"), []).unwrap();
        let mut config = visual_debug_config(test_skin());
        for canvas in &mut config.canvases {
            canvas.skin = "dev.example.custom".parse().unwrap();
        }
        std::fs::write(&path, toml::to_string_pretty(&config).unwrap()).unwrap();
        let (loaded, issues) = load_or_create_with_store(&path, &store).unwrap();
        assert!(issues.is_empty());
        assert!(
            loaded
                .canvases
                .iter()
                .all(|canvas| canvas.skin.name() == "dev.example.custom")
        );
        save_atomic_with_store(&path, &loaded, &store).unwrap();
    }

    #[test]
    fn noncurrent_schema_is_rejected() {
        let mut config = OverlayConfig::initial();
        config.schema_version = 1;
        assert_eq!(
            config.validated().unwrap_err(),
            "overlay schema_version must be 9"
        );
    }

    #[test]
    fn v8_skin_fields_migrate_once_without_overwriting_generic_properties() {
        let source = r#"
schema_version = 8
unknown_grace_ms = 1000
obs_listen = "127.0.0.1:3939"

[[canvases]]
id = "main"
name = "Main"
backend = "obs"
skin = "dev.example.test-skin"
background = "animated"
opacity_percent = 100
output = "obs-output"
x = 0
y = 0
width = 1920
height = 1080

[canvases.skin_properties]
background = "static"

[[canvases.widgets]]
id = "score"
kind = "score"
x = 0
y = 0
width = 544
height = 200

[canvases.widgets.skin_properties]
frame-width = "s"

[canvases.widgets.settings]
frame_width = "l"
fill_opacity_percent = 37
history_count = 5
graph_months = 6
"#;

        let (changed, migrated) = migrate_v8(source).unwrap();
        assert!(changed);
        let config: OverlayConfig = toml::from_str(&migrated).unwrap();
        assert_eq!(config.schema_version, 9);
        assert_eq!(config.canvases[0].skin_properties["background"], "static");
        assert_eq!(
            config.canvases[0].widgets[0].skin_properties["frame-width"],
            "s"
        );
        assert_eq!(
            config.canvases[0].widgets[0].skin_properties["fill-opacity-percent"],
            37
        );
        assert!(!migrated.contains("frame_width"));
        assert!(!migrated.contains("fill_opacity_percent"));
    }

    #[test]
    fn canvas_names_are_non_empty_and_unique_per_backend() {
        let mut config = visual_debug_config(test_skin());
        config.canvases[0].name.clear();
        let (_, issues) = config.validated().unwrap();
        assert_eq!(issues[0].message, "canvas name must be non-empty");

        let mut config = visual_debug_config(test_skin());
        let duplicate = config.canvases[0].name.clone();
        config.canvases[1].name = duplicate;
        let (_, issues) = config.validated().unwrap();
        assert_eq!(
            issues[0].message,
            "duplicate canvas name within backend workspace"
        );
    }

    #[test]
    fn initial_config_has_no_canvases() {
        let config = OverlayConfig::initial();
        assert!(config.canvases.is_empty());
        assert!(config.validated().unwrap().1.is_empty());
    }

    #[test]
    fn visibility_and_opacity_are_strict() {
        let mut empty = visual_debug_config(test_skin());
        empty.canvases[0].show_on = Some(Vec::new());
        assert!(empty.validated().unwrap().1.is_empty());

        let mut transparent = visual_debug_config(test_skin());
        transparent.canvases[0].opacity_percent = 0;
        assert!(
            transparent
                .validated()
                .unwrap()
                .1
                .iter()
                .any(|issue| issue.message.contains("1 and 100"))
        );

        let mut obs = visual_debug_config(test_skin());
        let canvas = obs
            .canvases
            .iter_mut()
            .find(|canvas| canvas.backend == Backend::Obs)
            .unwrap();
        canvas.opacity_percent = 50;
        assert!(obs.validated().unwrap().1.is_empty());
    }
    #[test]
    fn composition_settings_survive_presentation_and_storage() {
        let root = temporary("composition");
        let path = root.join("overlay.toml");
        let mut config = OverlayConfig::initial();
        for canvas in &mut config.canvases {
            canvas.widgets.push(test_widget());
            let mut view = canvas.presentation();
            view.skin_properties
                .insert("background".into(), serde_json::json!("animated"));
            view.widgets[0].kind = WidgetKind::Empty;
            view.widgets[0].settings.title = "手元 CAMERA / DP".into();
            view.widgets[0]
                .skin_properties
                .insert("frame-width".into(), serde_json::json!("l"));
            view.widgets[0]
                .skin_properties
                .insert("fill-opacity-percent".into(), serde_json::json!(37));
            view.widgets[0].settings.aspect_ratio =
                scorepeek_overlay_ui::AspectRatio::Current([640, 360]);
            canvas.apply_presentation(&view);
            assert_eq!(canvas.presentation(), view);
        }
        save_atomic(&path, &config).unwrap();
        let (loaded, issues) = load_or_create(&path).unwrap();
        assert!(issues.is_empty());
        assert_eq!(loaded, config);
        std::fs::remove_dir_all(root).unwrap();
    }
}
