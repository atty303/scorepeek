use crate::runtime::Backend;
use scorepeek_overlay_ui::Skin;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayConfig {
    pub schema_version: u32,
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
    pub id: String,
    pub backend: Backend,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub skin: Skin,
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
    pub z: u32,
    #[serde(default)]
    pub settings: WidgetSettings,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WidgetKind {
    Status,
    Selection,
    Score,
    HistoryList,
    HistoryGraph,
}

impl WidgetKind {
    #[must_use]
    pub const fn class(self) -> &'static str {
        match self {
            Self::Status => "status-widget",
            Self::Selection => "selection-widget",
            Self::Score => "score-widget",
            Self::HistoryList => "history-list-widget",
            Self::HistoryGraph => "history-graph-widget",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetSettings {
    #[serde(default = "default_history_count")]
    pub history_count: u32,
    #[serde(default = "default_graph_months")]
    pub graph_months: u32,
}

impl Default for WidgetSettings {
    fn default() -> Self {
        Self {
            history_count: default_history_count(),
            graph_months: default_graph_months(),
        }
    }
}

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
            backend_revisions: BackendRevisions::default(),
            obs_listen: default_listen(),
            canvases: vec![
                initial_canvas("wayland-main", Backend::Wayland),
                initial_canvas("obs-main", Backend::Obs),
            ],
        }
    }

    /// Validates global invariants and returns individually valid canvases.
    /// # Errors
    /// Returns an unsupported schema or a backend without an enabled valid canvas.
    pub fn validated(&self) -> Result<(Vec<Canvas>, Vec<ConfigIssue>), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!("overlay schema_version must be {SCHEMA_VERSION}"));
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
            if !valid
                .iter()
                .any(|canvas| canvas.backend == backend && canvas.enabled)
            {
                return Err(format!(
                    "overlay {backend:?} must retain at least one enabled canvas"
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
            id: self.id.clone(),
            skin: self.skin,
            revision: self.revision,
            widgets: self
                .widgets
                .iter()
                .map(|widget| scorepeek_overlay_ui::WidgetLayout {
                    id: widget.id.clone(),
                    kind: match widget.kind {
                        WidgetKind::Status => scorepeek_overlay_ui::WidgetKind::Status,
                        WidgetKind::Selection => scorepeek_overlay_ui::WidgetKind::Selection,
                        WidgetKind::Score => scorepeek_overlay_ui::WidgetKind::Score,
                        WidgetKind::HistoryList => scorepeek_overlay_ui::WidgetKind::HistoryList,
                        WidgetKind::HistoryGraph => scorepeek_overlay_ui::WidgetKind::HistoryGraph,
                    },
                    x: widget.x,
                    y: widget.y,
                    width: widget.width,
                    height: widget.height,
                    z: widget.z,
                    settings: scorepeek_overlay_ui::WidgetSettings {
                        history_count: widget.settings.history_count,
                        graph_months: widget.settings.graph_months,
                    },
                })
                .collect(),
        }
    }

    pub fn apply_presentation(&mut self, presentation: &scorepeek_overlay_ui::CanvasPresentation) {
        self.skin = presentation.skin;
        self.revision = presentation.revision;
        self.widgets = presentation
            .widgets
            .iter()
            .map(|widget| Widget {
                id: widget.id.clone(),
                kind: match widget.kind {
                    scorepeek_overlay_ui::WidgetKind::Status => WidgetKind::Status,
                    scorepeek_overlay_ui::WidgetKind::Selection => WidgetKind::Selection,
                    scorepeek_overlay_ui::WidgetKind::Score => WidgetKind::Score,
                    scorepeek_overlay_ui::WidgetKind::HistoryList => WidgetKind::HistoryList,
                    scorepeek_overlay_ui::WidgetKind::HistoryGraph => WidgetKind::HistoryGraph,
                },
                x: widget.x,
                y: widget.y,
                width: widget.width,
                height: widget.height,
                z: widget.z,
                settings: WidgetSettings {
                    history_count: widget.settings.history_count,
                    graph_months: widget.settings.graph_months,
                },
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
    let mut config: OverlayConfig =
        toml::from_str(text).map_err(|error| format!("overlay TOML: {error}"))?;
    let (valid, issues) = config.validated()?;
    config.canvases = valid;
    Ok((config, issues))
}

/// Replaces a configuration durably in the same directory.
/// # Errors
/// Returns validation, serialization, or filesystem errors.
pub fn save_atomic(path: &Path, config: &OverlayConfig) -> Result<(), String> {
    config.validated()?;
    let parent = path
        .parent()
        .ok_or_else(|| "overlay config has no parent directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    let bytes = toml::to_string_pretty(config)
        .map_err(|error| format!("serialize overlay TOML: {error}"))?;
    let temporary = parent.join(format!(".overlay.toml.{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| format!("create {}: {error}", temporary.display()))?;
    let result = (|| {
        file.write_all(bytes.as_bytes())?;
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
        if widget.width < 32 || widget.height < 32 {
            return Err(format!("widget {} must be at least 32x32", widget.id));
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

fn initial_canvas(id: &str, backend: Backend) -> Canvas {
    let widget = |id: &str, kind, x, y, width, height, z| Widget {
        id: id.into(),
        kind,
        x,
        y,
        width,
        height,
        z,
        settings: WidgetSettings::default(),
    };
    Canvas {
        id: id.into(),
        backend,
        enabled: true,
        skin: Skin::CyanSystem,
        output: None,
        initial_placement: (backend == Backend::Wayland).then_some(InitialPlacement::UpperRight),
        x: 20,
        y: 20,
        width: default_width(),
        height: default_height(),
        revision: 0,
        widgets: vec![
            widget("status", WidgetKind::Status, 0, 0, 560, 72, 0),
            widget("selection", WidgetKind::Selection, 0, 80, 560, 120, 1),
            widget("score", WidgetKind::Score, 0, 208, 560, 300, 2),
            widget("history-list", WidgetKind::HistoryList, 0, 516, 560, 236, 3),
            widget(
                "history-graph",
                WidgetKind::HistoryGraph,
                0,
                760,
                560,
                280,
                4,
            ),
        ],
    }
}

#[must_use]
pub fn empty_canvas(id: String, backend: Backend) -> Canvas {
    Canvas {
        id,
        backend,
        enabled: true,
        skin: Skin::CyanSystem,
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

const fn enabled() -> bool {
    true
}
const fn default_width() -> u32 {
    560
}
const fn default_height() -> u32 {
    1040
}
const fn default_history_count() -> u32 {
    5
}
const fn default_graph_months() -> u32 {
    6
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
    }

    #[test]
    fn invalid_canvas_is_isolated() {
        let mut config = OverlayConfig::initial();
        config.canvases.push(Canvas {
            id: "bad id".into(),
            ..config.canvases[0].clone()
        });
        let (valid, issues) = config.validated().unwrap();
        assert_eq!(valid.len(), 2);
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
        assert_eq!(loaded.canvases.len(), 2);
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
        assert_eq!(valid.len(), 2);
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
}
