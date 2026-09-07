use crate::runtime::Backend;
use scorepeek_overlay_ui::{Skin, WaylandRefreshRate};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

pub const SCHEMA_VERSION: u32 = 5;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayConfig {
    pub schema_version: u32,
    #[serde(default)]
    pub wayland_refresh_hz: WaylandRefreshRate,
    #[serde(default)]
    pub settings_revision: u64,
    #[serde(default = "default_unknown_grace_ms")]
    pub unknown_grace_ms: u32,
    #[serde(default)]
    pub backend_revisions: BackendRevisions,
    #[serde(default = "default_listen")]
    pub obs_listen: String,
    pub canvases: Vec<Canvas>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackendRevisions {
    #[serde(default)]
    pub wayland: u64,
    #[serde(default)]
    pub obs: u64,
}

impl BackendRevisions {
    #[must_use]
    pub fn get(&self, backend: Backend) -> u64 {
        match backend {
            Backend::Wayland => self.wayland,
            Backend::Obs => self.obs,
        }
    }
    /// Advances one backend's canvas-list revision.
    /// # Errors
    /// Returns an error if the revision counter is exhausted.
    pub fn increment(&mut self, backend: Backend) -> Result<u64, String> {
        let revision = match backend {
            Backend::Wayland => &mut self.wayland,
            Backend::Obs => &mut self.obs,
        };
        *revision = revision
            .checked_add(1)
            .ok_or("backend revision exhausted")?;
        Ok(*revision)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Canvas {
    #[serde(default)]
    pub background: scorepeek_overlay_ui::Background,
    pub id: String,
    pub backend: Backend,
    #[serde(default)]
    pub skin: Skin,
    #[serde(default)]
    pub show_on: Option<Vec<scorepeek_overlay_ui::ScreenKind>>,
    #[serde(default = "default_opacity_percent")]
    pub opacity_percent: u8,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub initial_placement: Option<InitialPlacement>,
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub widgets: Vec<Widget>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InitialPlacement {
    UpperRight,
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
            wayland_refresh_hz: WaylandRefreshRate::Auto,
            settings_revision: 0,
            unknown_grace_ms: default_unknown_grace_ms(),
            backend_revisions: BackendRevisions::default(),
            obs_listen: default_listen(),
            canvases: [Backend::Wayland, Backend::Obs]
                .into_iter()
                .flat_map(initial_canvases)
                .collect(),
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
        if let WaylandRefreshRate::Capped(hz) = self.wayland_refresh_hz
            && !(1..=WaylandRefreshRate::MAX_HZ).contains(&hz)
        {
            return Err("overlay wayland_refresh_hz must be auto or from 1 through 1000".into());
        }
        let listen = self
            .obs_listen
            .parse::<std::net::SocketAddr>()
            .map_err(|error| format!("overlay obs_listen: {error}"))?;
        if !listen.ip().is_loopback() {
            return Err("overlay obs_listen must use a loopback address".into());
        }
        let mut canvas_ids = BTreeSet::new();
        let mut valid = Vec::new();
        let mut issues = Vec::new();
        for canvas in &self.canvases {
            match validate_canvas(canvas, &mut canvas_ids) {
                Ok(()) => valid.push(canvas.clone()),
                Err(message) => issues.push(ConfigIssue {
                    canvas_id: canvas.id.clone(),
                    message,
                }),
            }
        }
        for backend in [Backend::Wayland, Backend::Obs] {
            if !valid.iter().any(|canvas| canvas.backend == backend) {
                return Err(format!(
                    "overlay {backend:?} must retain at least one valid canvas"
                ));
            }
        }
        Ok((valid, issues))
    }
}

impl Canvas {
    #[must_use]
    pub fn presentation(&self) -> scorepeek_overlay_ui::CanvasPresentation {
        scorepeek_overlay_ui::CanvasPresentation {
            background: self.background,
            id: self.id.clone(),
            skin: self.skin,
            revision: self.revision,
            show_on: self.show_on.clone(),
            opacity_percent: self.opacity_percent,
            output: self.output.clone(),
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
                })
                .collect(),
        }
    }

    pub fn apply_presentation(&mut self, presentation: &scorepeek_overlay_ui::CanvasPresentation) {
        self.background = presentation.background;
        self.skin = presentation.skin;
        self.revision = presentation.revision;
        self.show_on.clone_from(&presentation.show_on);
        self.opacity_percent = presentation.opacity_percent;
        self.output.clone_from(&presentation.output);
        self.initial_placement = None;
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
    if !path.exists() {
        let config = OverlayConfig::initial();
        save_atomic(path, &config)?;
        return Ok((config, Vec::new()));
    }
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| format!("overlay TOML is not UTF-8: {error}"))?;
    let mut document: toml::Value =
        toml::from_str(text).map_err(|error| format!("overlay TOML: {error}"))?;
    let schema = document
        .get("schema_version")
        .and_then(toml::Value::as_integer)
        .ok_or("overlay schema_version is required")?;
    let mut migrated = matches!(schema, 2..=4);
    if schema == 2 {
        migrate_v2_document(&mut document)?;
        migrate_v3_document(&mut document)?;
    } else if schema == 3 {
        migrate_v3_document(&mut document)?;
    } else if schema != 4 && schema != i64::from(SCHEMA_VERSION) {
        return Err(format!("overlay schema_version must be {SCHEMA_VERSION}"));
    }
    if migrated {
        migrate_v4_document(&mut document)?;
    }
    if document.get("wayland_refresh_hz").is_none() {
        document
            .as_table_mut()
            .ok_or("overlay document must be a table")?
            .insert(
                "wayland_refresh_hz".into(),
                toml::Value::String("auto".into()),
            );
        migrated = true;
    }
    let mut config: OverlayConfig = document
        .clone()
        .try_into()
        .map_err(|error| format!("overlay TOML: {error}"))?;
    let (valid, issues) = config.validated()?;
    if migrated {
        let migrated = toml::to_string_pretty(&document)
            .map_err(|error| format!("serialize migrated overlay TOML: {error}"))?;
        write_atomic(path, migrated.as_bytes())?;
    }
    config.canvases = valid;
    Ok((config, issues))
}

fn migrate_v4_document(document: &mut toml::Value) -> Result<(), String> {
    let canvases = document
        .get_mut("canvases")
        .and_then(toml::Value::as_array_mut)
        .ok_or("overlay canvases must be an array")?;
    for canvas in canvases {
        if let Some(widgets) = canvas
            .get_mut("widgets")
            .and_then(toml::Value::as_array_mut)
        {
            for widget in widgets {
                for (field, delta) in [("x", 8), ("y", 8), ("width", -16), ("height", -16)] {
                    if let Some(value) = widget.get_mut(field) {
                        let old = value
                            .as_integer()
                            .ok_or("widget geometry must be an integer")?;
                        *value = toml::Value::Integer(
                            old.checked_add(delta).ok_or("widget geometry overflow")?,
                        );
                    }
                }
            }
        }
    }
    document["schema_version"] = toml::Value::Integer(i64::from(SCHEMA_VERSION));
    Ok(())
}

fn migrate_v2_document(document: &mut toml::Value) -> Result<(), String> {
    let root = document
        .as_table_mut()
        .ok_or("overlay TOML root must be a table")?;
    root.insert("schema_version".into(), toml::Value::Integer(3));
    let canvases = root
        .get_mut("canvases")
        .and_then(toml::Value::as_array_mut)
        .ok_or("overlay canvases must be an array")?;
    for canvas in canvases {
        let table = canvas
            .as_table_mut()
            .ok_or("overlay canvas must be a table")?;
        table.remove("z");
        if let Some(widgets) = table.get_mut("widgets").and_then(toml::Value::as_array_mut) {
            for widget in widgets {
                widget
                    .as_table_mut()
                    .ok_or("overlay widget must be a table")?
                    .remove("z");
            }
        }
    }
    Ok(())
}

fn migrate_v3_document(document: &mut toml::Value) -> Result<(), String> {
    let root = document
        .as_table_mut()
        .ok_or("overlay TOML root must be a table")?;
    root.insert("schema_version".into(), toml::Value::Integer(4));
    let canvases = root
        .get_mut("canvases")
        .and_then(toml::Value::as_array_mut)
        .ok_or("overlay canvases must be an array")?;
    for canvas in canvases {
        let table = canvas
            .as_table_mut()
            .ok_or("overlay canvas must be a table")?;
        match table.remove("enabled") {
            Some(toml::Value::Boolean(false)) => {
                table.insert("show_on".into(), toml::Value::Array(Vec::new()));
            }
            Some(toml::Value::Boolean(true)) | None => {}
            Some(_) => return Err("overlay canvas enabled must be a boolean".into()),
        }
    }
    Ok(())
}

/// Replaces a configuration durably in the same directory.
/// # Errors
/// Returns validation, serialization, or filesystem errors.
pub fn save_atomic(path: &Path, config: &OverlayConfig) -> Result<(), String> {
    let (_, issues) = config.validated()?;
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

fn validate_canvas(canvas: &Canvas, canvas_ids: &mut BTreeSet<String>) -> Result<(), String> {
    if canvas.id.is_empty()
        || !canvas
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("id must use ASCII letters, digits, '-' or '_'".into());
    }
    if !canvas_ids.insert(canvas.id.clone()) {
        return Err("duplicate canvas id".into());
    }
    if canvas.width < 32 || canvas.height < 32 {
        return Err("canvas dimensions must be at least 32x32".into());
    }
    if canvas.opacity_percent == 0 || canvas.opacity_percent > 100 {
        return Err("canvas opacity_percent must be between 1 and 100".into());
    }
    if canvas.backend == Backend::Obs && canvas.opacity_percent != 100 {
        return Err("OBS canvas opacity_percent must be 100".into());
    }
    if canvas.x % 4 != 0
        || canvas.y % 4 != 0
        || !canvas.width.is_multiple_of(4)
        || !canvas.height.is_multiple_of(4)
    {
        return Err("canvas position and dimensions must align to the 4px grid".into());
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
        if widget.settings.fill_opacity_percent > 100 {
            return Err(format!("widget {} fill opacity must be 0..100", widget.id));
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

fn initial_canvases(backend: Backend) -> Vec<Canvas> {
    use scorepeek_overlay_ui::ScreenKind;
    let prefix = match backend {
        Backend::Wayland => "wayland",
        Backend::Obs => "obs",
    };
    let x = if backend == Backend::Obs { 1340 } else { 20 };
    vec![
        initial_canvas(
            format!("{prefix}-status"),
            backend,
            x,
            20,
            560,
            72,
            None,
            vec![("status", WidgetKind::Status, 0, 0)],
        ),
        initial_canvas(
            format!("{prefix}-selection"),
            backend,
            x,
            100,
            560,
            960,
            Some(vec![ScreenKind::MusicSelect]),
            dashboard_widgets(),
        ),
        initial_canvas(
            format!("{prefix}-play"),
            backend,
            x,
            100,
            560,
            140,
            Some(vec![ScreenKind::DecideTransition, ScreenKind::Play]),
            vec![("selection", WidgetKind::Selection, 0, 0)],
        ),
        initial_canvas(
            format!("{prefix}-result"),
            backend,
            x,
            100,
            560,
            960,
            Some(vec![ScreenKind::Result]),
            dashboard_widgets(),
        ),
    ]
}

fn dashboard_widgets() -> Vec<(&'static str, WidgetKind, i32, i32)> {
    vec![
        ("selection", WidgetKind::Selection, 0, 0),
        ("score", WidgetKind::Score, 0, 148),
        ("history-list", WidgetKind::HistoryList, 0, 372),
        ("history-graph", WidgetKind::HistoryGraph, 0, 552),
    ]
}

#[allow(clippy::too_many_arguments)]
fn initial_canvas(
    id: String,
    backend: Backend,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    show_on: Option<Vec<scorepeek_overlay_ui::ScreenKind>>,
    widgets: Vec<(&str, WidgetKind, i32, i32)>,
) -> Canvas {
    let widget = |id: &str, kind, x, y, width, height| Widget {
        id: id.into(),
        kind,
        x,
        y,
        width,
        height,
        settings: WidgetSettings::default(),
    };
    Canvas {
        id,
        backend,
        background: scorepeek_overlay_ui::Background::None,
        skin: Skin::CyanSystem,
        show_on,
        opacity_percent: 100,
        output: None,
        initial_placement: (backend == Backend::Wayland).then_some(InitialPlacement::UpperRight),
        x,
        y,
        width,
        height,
        revision: 0,
        widgets: widgets
            .into_iter()
            .map(|(id, kind, x, y)| {
                let (width, height) = scorepeek_overlay_ui::default_widget_size(kind);
                widget(id, kind, x + 8, y + 8, width, height)
            })
            .collect(),
    }
}

#[must_use]
pub fn empty_canvas(id: String, backend: Backend) -> Canvas {
    Canvas {
        id,
        backend,
        background: scorepeek_overlay_ui::Background::None,
        skin: Skin::CyanSystem,
        show_on: Some(Vec::new()),
        opacity_percent: 100,
        output: None,
        initial_placement: None,
        x: 20,
        y: 20,
        width: default_width(),
        height: default_height(),
        revision: 0,
        widgets: Vec::new(),
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

    fn temporary(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "scorepeek-overlay-{name}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ))
    }

    #[test]
    fn initial_config_round_trips_and_has_both_backends() {
        let config = OverlayConfig::initial();
        let text = toml::to_string(&config).unwrap();
        let decoded: OverlayConfig = toml::from_str(&text).unwrap();
        assert_eq!(decoded, config);
        assert!(config.validated().unwrap().1.is_empty());
        assert!(text.contains("wayland_refresh_hz = \"auto\""));
    }

    #[test]
    fn wayland_refresh_rate_round_trips_as_auto_or_integer() {
        let mut config = OverlayConfig::initial();
        config.wayland_refresh_hz = WaylandRefreshRate::capped(30).unwrap();
        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("wayland_refresh_hz = 30"));
        assert_eq!(toml::from_str::<OverlayConfig>(&text).unwrap(), config);

        for invalid in ["0", "1001", "\"AUTO\"", "\"30\""] {
            let text = toml::to_string(&OverlayConfig::initial()).unwrap().replace(
                "wayland_refresh_hz = \"auto\"",
                &format!("wayland_refresh_hz = {invalid}"),
            );
            assert!(toml::from_str::<OverlayConfig>(&text).is_err(), "{invalid}");
        }
    }

    #[test]
    fn existing_v5_without_refresh_rate_is_rewritten_with_auto() {
        let root = temporary("migrate-v5-refresh");
        let path = root.join("overlay.toml");
        std::fs::create_dir_all(&root).unwrap();
        let text = toml::to_string_pretty(&OverlayConfig::initial())
            .unwrap()
            .lines()
            .filter(|line| !line.starts_with("wayland_refresh_hz ="))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, text).unwrap();

        let (loaded, issues) = load_or_create(&path).unwrap();
        assert!(issues.is_empty());
        assert_eq!(loaded.wayland_refresh_hz, WaylandRefreshRate::Auto);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("wayland_refresh_hz = \"auto\"")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_canvas_is_isolated() {
        let mut config = OverlayConfig::initial();
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
        let mut config = OverlayConfig::initial();
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
        let mut config = OverlayConfig::initial();
        let mut invalid = config.canvases[0].clone();
        invalid.id = "wayland-off-grid".into();
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
        text.push_str("\nunknown = true\n");
        assert!(toml::from_str::<OverlayConfig>(&text).is_err());
    }

    #[test]
    fn missing_config_is_created_and_read_back() {
        let root = temporary("create");
        let path = root.join("overlay.toml");
        let (created, issues) = load_or_create(&path).unwrap();
        assert!(issues.is_empty());
        let (loaded, _) = load_or_create(&path).unwrap();
        assert_eq!(loaded, created);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_v1_is_rejected_without_migration() {
        let mut config = OverlayConfig::initial();
        config.schema_version = 1;
        assert_eq!(
            config.validated().unwrap_err(),
            "overlay schema_version must be 5"
        );
    }

    #[test]
    fn schema_v2_is_migrated_atomically_through_v5() {
        let root = temporary("migrate-v2");
        let path = root.join("overlay.toml");
        std::fs::create_dir_all(&root).unwrap();
        let mut value = toml::Value::try_from(OverlayConfig::initial()).unwrap();
        value["schema_version"] = toml::Value::Integer(2);
        let canvases = value["canvases"].as_array_mut().unwrap();
        canvases[0]
            .as_table_mut()
            .unwrap()
            .insert("z".into(), 7.into());
        canvases[0]["widgets"].as_array_mut().unwrap()[0]
            .as_table_mut()
            .unwrap()
            .insert("z".into(), 9.into());
        std::fs::write(&path, toml::to_string_pretty(&value).unwrap()).unwrap();

        let (loaded, issues) = load_or_create(&path).unwrap();
        assert!(issues.is_empty());
        assert_eq!(loaded.schema_version, SCHEMA_VERSION);
        let persisted = std::fs::read_to_string(&path).unwrap();
        assert!(
            !persisted
                .lines()
                .any(|line| line.trim_start().starts_with("z ="))
        );
        assert!(persisted.contains("schema_version = 5"));
        assert!(
            !persisted
                .lines()
                .any(|line| line.trim_start().starts_with("enabled ="))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_v3_disabled_canvas_migrates_to_empty_show_on() {
        let root = temporary("migrate-v3-disabled");
        let path = root.join("overlay.toml");
        std::fs::create_dir_all(&root).unwrap();
        let mut value = toml::Value::try_from(OverlayConfig::initial()).unwrap();
        value["schema_version"] = toml::Value::Integer(3);
        value["canvases"].as_array_mut().unwrap()[0]
            .as_table_mut()
            .unwrap()
            .insert("enabled".into(), false.into());
        std::fs::write(&path, toml::to_string_pretty(&value).unwrap()).unwrap();

        let (loaded, issues) = load_or_create(&path).unwrap();
        assert!(issues.is_empty());
        assert_eq!(loaded.schema_version, SCHEMA_VERSION);
        assert_eq!(loaded.canvases[0].show_on, Some(Vec::new()));
        let persisted = std::fs::read_to_string(&path).unwrap();
        assert!(
            !persisted
                .lines()
                .any(|line| line.trim_start().starts_with("enabled ="))
        );
        assert!(persisted.contains("show_on = []"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_v3_enabled_canvas_preserves_show_on() {
        let root = temporary("migrate-v3-enabled");
        let path = root.join("overlay.toml");
        std::fs::create_dir_all(&root).unwrap();
        let mut value = toml::Value::try_from(OverlayConfig::initial()).unwrap();
        value["schema_version"] = toml::Value::Integer(3);
        let canvas = value["canvases"]
            .as_array_mut()
            .unwrap()
            .first_mut()
            .unwrap();
        let canvas = canvas.as_table_mut().unwrap();
        canvas.insert("enabled".into(), toml::Value::Boolean(true));
        canvas.insert(
            "show_on".into(),
            toml::Value::Array(vec![toml::Value::String("result".into())]),
        );
        std::fs::write(&path, toml::to_string_pretty(&value).unwrap()).unwrap();

        let (loaded, issues) = load_or_create(&path).unwrap();
        assert!(issues.is_empty());
        assert_eq!(
            loaded.canvases[0].show_on,
            Some(vec![scorepeek_overlay_ui::ScreenKind::Result])
        );
        let persisted = std::fs::read_to_string(&path).unwrap();
        assert!(!persisted.contains("enabled ="));
        assert!(persisted.contains("show_on = [\"result\"]"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn initial_config_has_four_screen_layouts_per_backend() {
        let config = OverlayConfig::initial();
        for backend in [Backend::Wayland, Backend::Obs] {
            let canvases = config
                .canvases
                .iter()
                .filter(|canvas| canvas.backend == backend)
                .collect::<Vec<_>>();
            assert_eq!(canvases.len(), 4);
            assert_eq!(
                canvases
                    .iter()
                    .filter(|canvas| canvas.show_on.is_none())
                    .count(),
                1
            );
            assert!(canvases.iter().any(|canvas| {
                canvas.show_on.as_deref() == Some(&[scorepeek_overlay_ui::ScreenKind::MusicSelect])
            }));
        }
    }

    #[test]
    fn visibility_and_opacity_are_strict() {
        let mut empty = OverlayConfig::initial();
        empty.canvases[0].show_on = Some(Vec::new());
        assert!(empty.validated().unwrap().1.is_empty());

        let mut transparent = OverlayConfig::initial();
        transparent.canvases[0].opacity_percent = 0;
        assert!(
            transparent.validated().unwrap().1[0]
                .message
                .contains("opacity")
        );

        let mut obs = OverlayConfig::initial();
        let canvas = obs
            .canvases
            .iter_mut()
            .find(|canvas| canvas.backend == Backend::Obs)
            .unwrap();
        canvas.opacity_percent = 50;
        assert!(obs.validated().unwrap().1[0].message.contains("OBS"));
    }
    #[test]
    fn schema_v4_migration_preserves_outer_geometry_and_round_trips_once() {
        let root = temporary("migrate-inner");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("overlay.toml");
        let expected = OverlayConfig::initial();
        let mut old = expected.clone();
        old.schema_version = 4;
        for canvas in &mut old.canvases {
            for widget in &mut canvas.widgets {
                widget.x -= 8;
                widget.y -= 8;
                widget.width += 16;
                widget.height += 16;
            }
        }
        std::fs::write(&path, toml::to_string_pretty(&old).unwrap()).unwrap();
        let (loaded, issues) = load_or_create(&path).unwrap();
        assert!(issues.is_empty());
        assert_eq!(loaded, expected);
        let (reloaded, issues) = load_or_create(&path).unwrap();
        assert!(issues.is_empty());
        assert_eq!(reloaded, expected);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn composition_settings_survive_presentation_and_storage() {
        let root = temporary("composition");
        let path = root.join("overlay.toml");
        let mut config = OverlayConfig::initial();
        for canvas in &mut config.canvases {
            let mut view = canvas.presentation();
            view.background = scorepeek_overlay_ui::Background::Animated;
            view.widgets[0].kind = WidgetKind::Empty;
            view.widgets[0].settings.title = "手元 CAMERA / DP".into();
            view.widgets[0].settings.frame_width = scorepeek_overlay_ui::FrameWidth::L;
            view.widgets[0].settings.fill_opacity_percent = 37;
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
