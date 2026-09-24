//! Native overlay document, validation, migration, and storage.

mod document;
pub mod layout;
pub mod validation;
pub use document::{
    OverlayConfig, ProjectionGenerations, SCHEMA_VERSION, migrate_v8, visual_debug_config,
};
pub use layout::{Canvas, Widget};
use layout::{default_height, default_width};
use scorepeek_overlay::{Backend, Skin};
use std::collections::BTreeMap;
pub use validation::{ConfigIssue, validate_canvases};
pub const OBS_OUTPUT_ID: &str = "obs-output";
pub const PENDING_WAYLAND_OUTPUT_ID: &str = "__pending-wayland-output__";

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
use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

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

/// Loads the overlay document, creating the current default when absent.
///
/// # Errors
/// Returns an error for invalid configuration, unavailable skins, or filesystem failures.
pub fn load_or_create(path: &Path) -> Result<(OverlayConfig, Vec<ConfigIssue>), String> {
    if !path.exists() {
        let config = OverlayConfig::initial();
        save_atomic(path, &config)?;
        return Ok((config, Vec::new()));
    }
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| format!("overlay TOML is not UTF-8: {error}"))?;
    let (migrated, migrated_text) = migrate_v8(text)?;
    let parsed: OverlayConfig =
        toml::from_str(&migrated_text).map_err(|error| format!("overlay TOML: {error}"))?;
    let (canvases, mut issues) = parsed.validated()?;
    let store = crate::skin::StoreRoot::discover();
    let mut installed = Vec::new();
    for canvas in canvases {
        if store.is_installed(canvas.skin.name())? {
            installed.push(canvas);
        } else {
            issues.push(ConfigIssue {
                canvas_id: canvas.id.clone(),
                message: format!("skin {} is not installed", canvas.skin.name()),
            });
        }
    }
    let mut config = parsed;
    config.canvases = installed;
    if migrated {
        write_atomic(path, migrated_text.as_bytes())?;
    }
    Ok((config, issues))
}

/// Validates and atomically saves the overlay document.
///
/// # Errors
/// Returns an error for invalid configuration, unavailable skins, or filesystem failures.
pub fn save_atomic(path: &Path, config: &OverlayConfig) -> Result<(), String> {
    save_atomic_in_store(path, config, &crate::skin::StoreRoot::discover())
}

/// Validates and atomically saves the overlay document using the supplied skin store.
///
/// # Errors
/// Returns an error for invalid configuration, unavailable skins, or filesystem failures.
pub fn save_atomic_in_store(
    path: &Path,
    config: &OverlayConfig,
    store: &crate::skin::StoreRoot,
) -> Result<(), String> {
    let (canvases, issues) = config.validated()?;
    if let Some(issue) = issues.first() {
        return Err(format!(
            "overlay canvas {}: {}",
            issue.canvas_id, issue.message
        ));
    }
    for canvas in canvases {
        if !store.is_installed(canvas.skin.name())? {
            return Err(format!(
                "overlay canvas {}: skin {} is not installed",
                canvas.id,
                canvas.skin.name()
            ));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_document_with_invalid_obs_listener_loads_and_saves() {
        let root = std::env::temp_dir().join(format!(
            "scorepeek-overlay-config-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir(&root).unwrap();
        let path = root.join("overlay.toml");
        let mut document = OverlayConfig::initial();
        document.obs_listen = "invalid".into();
        fs::write(&path, toml::to_string_pretty(&document).unwrap()).unwrap();
        let (loaded, issues) = load_or_create(&path).unwrap();
        assert!(issues.is_empty());
        assert_eq!(loaded.obs_listen, "invalid");
        let store = crate::skin::StoreRoot::new(root.join("skins"));
        save_atomic_in_store(&path, &loaded, &store).unwrap();
        let restored: OverlayConfig = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(restored.obs_listen, "invalid");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn obs_and_wayland_offscreen_geometry_survives_isolated_save_and_reopen() {
        let root = std::env::temp_dir().join(format!(
            "scorepeek-overlay-geometry-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let skins = root.join("skins");
        fs::create_dir_all(&skins).unwrap();
        let skin: Skin = "dev.example.skin".parse().unwrap();
        fs::write(skins.join("dev.example.skin.zip"), []).unwrap();
        let store = crate::skin::StoreRoot::new(skins);
        let mut document = OverlayConfig::initial();
        for backend in [Backend::Obs, Backend::Wayland] {
            let mut canvas = empty_canvas(format!("{backend:?}"), backend, skin);
            canvas.output = if backend == Backend::Obs {
                OBS_OUTPUT_ID.into()
            } else {
                "temporarily-disconnected".into()
            };
            canvas.x = i32::MIN;
            canvas.y = 20_001;
            canvas.width = 701;
            canvas.height = 33;
            canvas.widgets.push(Widget {
                id: "offscreen-widget".into(),
                kind: scorepeek_overlay::WidgetKind::Empty,
                x: -19,
                y: 9_999,
                width: 17,
                height: 16,
                settings: scorepeek_overlay::WidgetSettings::default(),
                skin_properties: BTreeMap::default(),
            });
            document.canvases.push(canvas);
        }
        let path = root.join("overlay.toml");
        save_atomic_in_store(&path, &document, &store).unwrap();
        let reopened: OverlayConfig = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(reopened.validated().unwrap().1.is_empty());
        assert_eq!(reopened.canvases, document.canvases);
        for canvas in &reopened.canvases {
            let presentation = canvas.presentation();
            assert_eq!(presentation.x, i32::MIN);
            assert_eq!(presentation.widgets[0].x, -19);
        }
        fs::remove_dir_all(&root).unwrap();
    }
}
