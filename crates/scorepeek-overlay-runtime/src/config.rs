//! Filesystem and installed-skin adapter for the backend-neutral config document.

pub use scorepeek_overlay::config::{
    Canvas, ConfigIssue, OBS_OUTPUT_ID, OverlayConfig, PENDING_WAYLAND_OUTPUT_ID,
    ProjectionGenerations, SCHEMA_VERSION, Widget, empty_canvas, visual_debug_config,
};
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
    let (migrated, migrated_text) = scorepeek_overlay::config::migrate_v8(text)?;
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
