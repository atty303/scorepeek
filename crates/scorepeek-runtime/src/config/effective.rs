//! Effective runtime configuration derived from the persisted document.

use std::env;
use std::path::PathBuf;

use super::document::{CaptureKind, ConfigFile};
use crate::recording::policy::{DEFAULT_RECORDING_MEMORY_MIB, RecordingMemoryLimit};
use crate::recording::retention::RecordingRetention;

#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "paired frontend flags preserve explicit enable and disable overrides"
)]
pub(crate) struct RunArgs {
    pub(crate) capture: Option<CaptureKind>,
    pub(crate) node_name: Option<String>,
    pub(crate) crop_left: Option<u32>,
    pub(crate) crop_top: Option<u32>,
    pub(crate) crop_right: Option<u32>,
    pub(crate) crop_bottom: Option<u32>,
    pub(crate) scores_db: Option<PathBuf>,
    pub(crate) no_scores: bool,
    pub(crate) scores: bool,
    pub(crate) record: bool,
    pub(crate) record_all: bool,
    pub(crate) no_record: bool,
    pub(crate) record_memory_mib: Option<usize>,
    pub(crate) overlay_wayland: bool,
    pub(crate) no_overlay_wayland: bool,
    pub(crate) overlay_wayland_edit: bool,
    pub(crate) no_overlay_wayland_edit: bool,
    pub(crate) overlay_obs: bool,
    pub(crate) no_overlay_obs: bool,
    pub(crate) overlay_config: Option<PathBuf>,
}

pub(crate) struct RoutineRunOptions {
    pub(crate) overlays: OverlayOptions,
    pub(crate) capture: RoutineCapture,
    pub(crate) crop: scorepeek::capture::EdgeCrop,
    pub(crate) scores_db: Option<PathBuf>,
    pub(crate) no_scores: bool,
    pub(crate) recording: bool,
    pub(crate) recording_memory_limit: RecordingMemoryLimit,
    pub(crate) recording_retention: RecordingRetention,
}

#[derive(Clone)]
pub(crate) enum RoutineCapture {
    Pipewire { node_name: String },
    VulkanLayer,
}

#[derive(Default)]
pub(crate) struct OverlayOptions {
    pub(crate) wayland: bool,
    pub(crate) wayland_edit: bool,
    pub(crate) obs: bool,
    pub(crate) config_path: Option<PathBuf>,
}

fn validate_capture_specific(backend: CaptureKind, node_name: Option<&str>) -> Result<(), String> {
    match (backend, node_name) {
        (CaptureKind::Pipewire, Some(node)) if !node.is_empty() => Ok(()),
        (CaptureKind::Pipewire, _) => {
            Err("pipewire capture requires a non-empty node_name".to_owned())
        }
        (CaptureKind::VulkanLayer, None) => Ok(()),
        (CaptureKind::VulkanLayer, Some(_)) => {
            Err("node_name is valid only for pipewire capture".to_owned())
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one function keeps the config, environment, and CLI precedence visible in order"
)]
pub(crate) fn merge_run_options(
    config: ConfigFile,
    cli: RunArgs,
) -> Result<RoutineRunOptions, String> {
    let mut capture = config
        .capture
        .map(|capture| (capture.backend, capture.node_name));
    if let Some(backend) = env_capture()? {
        capture = Some((backend, None));
    }
    if let Some(node_name) = env_text("SCOREPEEK_PIPEWIRE_NODE_NAME")? {
        let backend = capture.as_ref().map(|value| value.0).ok_or_else(|| {
            "SCOREPEEK_PIPEWIRE_NODE_NAME requires SCOREPEEK_CAPTURE or config capture.backend"
                .to_owned()
        })?;
        capture = Some((backend, Some(node_name)));
    }
    if let Some(backend) = cli.capture {
        capture = Some((backend, None));
    }
    if let Some(node_name) = cli.node_name {
        let backend = capture.as_ref().map(|value| value.0).ok_or_else(|| {
            "--node-name requires --capture or configured capture.backend".to_owned()
        })?;
        capture = Some((backend, Some(node_name)));
    }
    let (backend, node_name) = capture.ok_or_else(|| {
        "capture backend is required in config, SCOREPEEK_CAPTURE, or --capture".to_owned()
    })?;
    validate_capture_specific(backend, node_name.as_deref())?;
    let capture = match backend {
        CaptureKind::Pipewire => RoutineCapture::Pipewire {
            node_name: node_name.expect("validated pipewire node"),
        },
        CaptureKind::VulkanLayer => RoutineCapture::VulkanLayer,
    };

    let crop = scorepeek::capture::EdgeCrop {
        left: cli
            .crop_left
            .or(env_u32("SCOREPEEK_CROP_LEFT")?)
            .or(config.crop.left)
            .unwrap_or(0),
        top: cli
            .crop_top
            .or(env_u32("SCOREPEEK_CROP_TOP")?)
            .or(config.crop.top)
            .unwrap_or(0),
        right: cli
            .crop_right
            .or(env_u32("SCOREPEEK_CROP_RIGHT")?)
            .or(config.crop.right)
            .unwrap_or(0),
        bottom: cli
            .crop_bottom
            .or(env_u32("SCOREPEEK_CROP_BOTTOM")?)
            .or(config.crop.bottom)
            .unwrap_or(0),
    };

    let mut scores_enabled = config.scores.enabled.unwrap_or(true);
    let mut scores_db = config.scores.database;
    if let Some(value) = env_bool("SCOREPEEK_SCORES_ENABLED")? {
        scores_enabled = value;
    }
    if let Some(path) = env_path("SCOREPEEK_SCORES_DB")? {
        scores_db = Some(path);
    }
    if cli.no_scores {
        scores_enabled = false;
        scores_db = None;
    } else if cli.scores {
        scores_enabled = true;
    }
    if let Some(path) = cli.scores_db {
        scores_enabled = true;
        scores_db = Some(path);
    }

    let mut recording = config.recording.enabled.unwrap_or(false);
    let mut recording_memory_mib = config.recording.memory_mib;
    let environment_recording = env_bool("SCOREPEEK_RECORDING_ENABLED")?;
    let environment_recording_memory = env_usize("SCOREPEEK_RECORDING_MEMORY_MIB")?;
    if let Some(value) = environment_recording {
        recording = value;
        if !value {
            recording_memory_mib = None;
        }
    }
    if environment_recording == Some(false) && environment_recording_memory.is_some() {
        return Err(
            "SCOREPEEK_RECORDING_MEMORY_MIB conflicts with SCOREPEEK_RECORDING_ENABLED=false"
                .to_owned(),
        );
    }
    if let Some(value) = environment_recording_memory {
        recording_memory_mib = Some(value);
    }
    let mut recording_retention = RecordingRetention::Selective;
    if cli.record || cli.record_all {
        recording = true;
    } else if cli.no_record {
        recording = false;
        recording_memory_mib = None;
    }
    if let Some(value) = cli.record_memory_mib {
        recording_memory_mib = Some(value);
    }
    if cli.record_all {
        recording_retention = RecordingRetention::All;
    }
    if recording_memory_mib.is_some() && !recording {
        return Err("recording memory limit requires recording to be enabled".to_owned());
    }
    let recording_memory_limit = RecordingMemoryLimit::from_mib(
        recording_memory_mib.unwrap_or(DEFAULT_RECORDING_MEMORY_MIB),
    )?;

    let mut overlays = OverlayOptions {
        wayland: config.overlay.wayland.unwrap_or(false)
            || config.overlay.wayland_edit.unwrap_or(false),
        wayland_edit: config.overlay.wayland_edit.unwrap_or(false),
        obs: config.overlay.obs.unwrap_or(false),
        config_path: config.overlay.config,
    };
    apply_overlay_environment(&mut overlays)?;
    if cli.overlay_wayland {
        overlays.wayland = true;
    } else if cli.no_overlay_wayland {
        overlays.wayland = false;
        overlays.wayland_edit = false;
    }
    if cli.overlay_wayland_edit {
        overlays.wayland = true;
        overlays.wayland_edit = true;
    } else if cli.no_overlay_wayland_edit {
        overlays.wayland_edit = false;
    }
    if cli.overlay_obs {
        overlays.obs = true;
    } else if cli.no_overlay_obs {
        overlays.obs = false;
    }
    if let Some(path) = cli.overlay_config {
        overlays.config_path = Some(path);
    }

    Ok(RoutineRunOptions {
        overlays,
        capture,
        crop,
        scores_db,
        no_scores: !scores_enabled,
        recording,
        recording_memory_limit,
        recording_retention,
    })
}

fn env_capture() -> Result<Option<CaptureKind>, String> {
    env_text("SCOREPEEK_CAPTURE")?
        .map(|value| match value.as_str() {
            "pipewire" => Ok(CaptureKind::Pipewire),
            "vulkan-layer" => Ok(CaptureKind::VulkanLayer),
            _ => Err("SCOREPEEK_CAPTURE requires pipewire or vulkan-layer".to_owned()),
        })
        .transpose()
}

fn env_text(name: &str) -> Result<Option<String>, String> {
    match env::var(name) {
        Ok(value) if !value.is_empty() => Ok(Some(value)),
        Ok(_) => Err(format!("{name} must not be empty")),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(format!("{name} must be UTF-8")),
    }
}

fn env_bool(name: &str) -> Result<Option<bool>, String> {
    env_text(name)?
        .map(|value| match value.as_str() {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            _ => Err(format!("{name} requires true, false, 1, or 0")),
        })
        .transpose()
}

fn env_u32(name: &str) -> Result<Option<u32>, String> {
    env_text(name)?
        .map(|value| {
            value
                .parse()
                .map_err(|_| format!("{name} requires a non-negative integer"))
        })
        .transpose()
}

fn env_usize(name: &str) -> Result<Option<usize>, String> {
    env_text(name)?
        .map(|value| {
            value
                .parse()
                .map_err(|_| format!("{name} requires a non-negative integer"))
        })
        .transpose()
}

fn env_path(name: &str) -> Result<Option<PathBuf>, String> {
    env::var_os(name)
        .map(|value| {
            let path = PathBuf::from(value);
            (!path.as_os_str().is_empty())
                .then_some(path)
                .ok_or_else(|| format!("{name} must not be empty"))
        })
        .transpose()
}

fn apply_overlay_environment(overlays: &mut OverlayOptions) -> Result<(), String> {
    if let Some(value) = env_bool("SCOREPEEK_OVERLAY_WAYLAND")? {
        overlays.wayland = value;
        if !value {
            overlays.wayland_edit = false;
        }
    }
    if let Some(value) = env_bool("SCOREPEEK_OVERLAY_WAYLAND_EDIT")? {
        overlays.wayland_edit = value;
        overlays.wayland |= value;
    }
    if let Some(value) = env_bool("SCOREPEEK_OVERLAY_OBS")? {
        overlays.obs = value;
    }
    if let Some(path) = env_path("SCOREPEEK_OVERLAY_CONFIG")? {
        overlays.config_path = Some(path);
    }
    Ok(())
}
