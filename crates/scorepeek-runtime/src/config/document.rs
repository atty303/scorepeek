//! Persisted runtime configuration document and validation.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

const MAX_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CaptureKind {
    Pipewire,
    VulkanLayer,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ConfigFile {
    pub(crate) capture: Option<CaptureConfig>,
    pub(crate) crop: CropConfig,
    pub(crate) scores: ScoresConfig,
    pub(crate) overlay: OverlayConfig,
    pub(crate) recording: RecordingConfig,
    pub(crate) catalog: Option<CatalogConfig>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureConfig {
    pub(crate) backend: CaptureKind,
    pub(crate) node_name: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct CropConfig {
    pub(crate) left: Option<u32>,
    pub(crate) top: Option<u32>,
    pub(crate) right: Option<u32>,
    pub(crate) bottom: Option<u32>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ScoresConfig {
    pub(crate) enabled: Option<bool>,
    pub(crate) database: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct OverlayConfig {
    pub(crate) wayland: Option<bool>,
    pub(crate) wayland_edit: Option<bool>,
    pub(crate) obs: Option<bool>,
    pub(crate) config: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct RecordingConfig {
    pub(crate) enabled: Option<bool>,
    pub(crate) memory_mib: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogConfig {
    pub(crate) url: String,
}

pub(crate) fn read(path: &Path) -> Result<Option<(String, ConfigFile)>, String> {
    let Some(text) = read_text(path)? else {
        return Ok(None);
    };
    let config = toml::from_str::<ConfigFile>(&text)
        .map_err(|error| format!("config file is invalid: {error}"))?;
    validate(&config)?;
    Ok(Some((text, config)))
}

pub(crate) fn read_text(path: &Path) -> Result<Option<String>, String> {
    let metadata = match path.metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("config file could not be inspected: {error}")),
    };
    if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
        return Err("config path must be a regular file no larger than 64 KiB".to_owned());
    }
    Ok(Some(fs::read_to_string(path).map_err(|error| {
        format!("config file could not be read as UTF-8: {error}")
    })?))
}

pub(crate) fn validate(config: &ConfigFile) -> Result<(), String> {
    if let Some(capture) = &config.capture {
        match (capture.backend, capture.node_name.as_deref()) {
            (CaptureKind::VulkanLayer, Some(_)) => {
                return Err("node_name is valid only for pipewire capture".to_owned());
            }
            (CaptureKind::Pipewire, Some("")) => {
                return Err("capture.node_name must not be empty".to_owned());
            }
            _ => {}
        }
    }
    if config.scores.enabled == Some(false) && config.scores.database.is_some() {
        return Err("scores.database requires scores.enabled = true".to_owned());
    }
    if config.recording.enabled == Some(false) && config.recording.memory_mib.is_some() {
        return Err("recording.memory_mib requires recording.enabled = true".to_owned());
    }
    if config.overlay.wayland == Some(false) && config.overlay.wayland_edit == Some(true) {
        return Err("overlay.wayland_edit requires overlay.wayland = true".to_owned());
    }
    if let Some(catalog) = &config.catalog
        && catalog.url.is_empty()
    {
        return Err("catalog.url must not be empty".to_owned());
    }
    if let Some(catalog) = &config.catalog {
        crate::resources::catalog::acquire::validate_configured_url(&catalog.url)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
