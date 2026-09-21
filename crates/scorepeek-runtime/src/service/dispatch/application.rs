use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs::{self, DirBuilder, File, OpenOptions};
#[cfg(test)]
use std::io::BufWriter;
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::{
    capture_live,
    config::document as local_profiles,
    diagnostics::{inspect as diagnostic_stream, writer as diagnostic_recording},
    events::server as routine_output,
    inventory::{doctor as inventory, vulkan_layer},
    platform::signal as live_control,
    recognition_artifact, recognition_live,
    recording::{simulation as recording_simulation, writer as canonical_recording},
    service::session as routine_watcher,
};
use scorepeek::catalog::CatalogStore;
use scorepeek_core::diagnostics::{DiagnosticBinding, DiagnosticResource, DiagnosticRunDescriptor};
use scorepeek_core::event::{RUN_EVENT_SCHEMA, RunEvent, RunEventKind};
use scorepeek_core::recognition::{
    self, CanonicalFrame, DIAGNOSTIC_TITLE_COMPARISON_KEY_ID, DIAGNOSTIC_TITLE_MINIMUM_CONFIDENCE,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub(crate) fn frontend_event(event: scorepeek_frontend_api::FrontendEvent) -> bool {
    crate::service::state::event(event)
}

fn frontend_output(stream: scorepeek_frontend_api::OutputStream, text: String) -> bool {
    crate::service::state::output(stream, text)
}

struct FrontendWriter(scorepeek_frontend_api::OutputStream);

impl io::Write for FrontendWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(bytes).into_owned();
        frontend_output(self.0, text)
            .then_some(bytes.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "frontend event sink closed"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn diagnostic_observe(replay: Option<u64>) -> Result<(), String> {
    diagnostic_stream::observe(
        replay,
        &mut FrontendWriter(scorepeek_frontend_api::OutputStream::Stdout),
        &mut FrontendWriter(scorepeek_frontend_api::OutputStream::Stderr),
    )
}

fn diagnostic_inspect(
    store: &Path,
    run_id: Option<&str>,
    format: diagnostic_stream::InspectionFormat,
) -> Result<i32, String> {
    diagnostic_stream::inspect_latest_with_format(
        store,
        run_id,
        format,
        &mut FrontendWriter(scorepeek_frontend_api::OutputStream::Stdout),
    )
}

macro_rules! print {
    ($($argument:tt)*) => {{
        let text = format!($($argument)*);
        let _ = frontend_output(scorepeek_frontend_api::OutputStream::Stdout, text);
    }};
}

macro_rules! println {
    () => { print!("\n") };
    ($($argument:tt)*) => { print!("{}\n", format_args!($($argument)*)) };
}

macro_rules! eprintln {
    () => {{
        let text = "\n".to_owned();
        let _ = frontend_output(scorepeek_frontend_api::OutputStream::Stderr, text);
    }};
    ($($argument:tt)*) => {{
        let text = format!("{}\n", format_args!($($argument)*));
        let _ = frontend_output(scorepeek_frontend_api::OutputStream::Stderr, text);
    }};
}

const CAPTURE_DIAGNOSTIC_SCHEMA: &str = "scorepeek-capture-diagnostic-v2";
const INTERRUPTED_ERROR: &str = "__scorepeek_interrupted__";
const TERMINATED_ERROR: &str = "__scorepeek_terminated__";

struct PublicCli {
    /// Read configuration from this file instead of the default path.
    config: Option<PathBuf>,
    command: PublicCommand,
}

enum PublicCommand {
    /// Run live capture, recognition, score persistence, and overlays.
    Run(RunArgs),
    /// Inspect installation and runtime prerequisites.
    Doctor(FormatArgs),
    /// Inspect or validate the optional configuration file.
    Config { command: ConfigCommand },
    /// Observe or inspect the out-of-band diagnostic stream.
    Diagnostic { command: DiagnosticCommand },
    /// Manage installed overlay skins.
    Skin { command: SkinCommand },
    /// Install or remove the embedded explicit Vulkan capture layer.
    VulkanLayer { command: VulkanLayerCommand },
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CaptureKind {
    Pipewire,
    VulkanLayer,
}

#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "paired frontend flags preserve explicit enable and disable overrides"
)]
struct RunArgs {
    /// Capture backend. Required unless configuration or environment supplies it.
    capture: Option<CaptureKind>,
    /// `PipeWire` node.name selected when the effective backend is pipewire.
    node_name: Option<String>,
    crop_left: Option<u32>,
    crop_top: Option<u32>,
    crop_right: Option<u32>,
    crop_bottom: Option<u32>,
    scores_db: Option<PathBuf>,
    no_scores: bool,
    scores: bool,
    record: bool,
    /// Retain every canonical 10 Hz tick, including stable screen interiors.
    record_all: bool,
    no_record: bool,
    record_memory_mib: Option<usize>,
    overlay_wayland: bool,
    no_overlay_wayland: bool,
    overlay_wayland_edit: bool,
    no_overlay_wayland_edit: bool,
    overlay_obs: bool,
    no_overlay_obs: bool,
    overlay_config: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default)]
enum OutputFormat {
    #[default]
    Human,
    Json,
}

#[derive(Default)]
struct FormatArgs {
    format: OutputFormat,
}

enum ConfigCommand {
    /// Print the resolved optional config-file path.
    Path(FormatArgs),
    /// Show only the content written in the config file, without merging environment or CLI values.
    Show(FormatArgs),
    /// Validate only the content written in the config file.
    Check(FormatArgs),
}

enum DiagnosticCommand {
    /// Stream diagnostics as NDJSON.
    Observe { replay: Option<u64> },
    /// Print a retained diagnostic run.
    Inspect(DiagnosticInspectArgs),
}

struct DiagnosticInspectArgs {
    /// Inspect one retained run by ID.
    run_id: Option<String>,
    format: FormatArgs,
}

enum SkinCommand {
    Install { package: PathBuf },
    Uninstall { id: String },
    List(FormatArgs),
}

#[derive(Clone, Copy)]
enum VulkanLayerCommand {
    /// Install the embedded layer for Vulkan Loader discovery by this user.
    Install,
    /// Remove the Scorepeek-managed manifest and layer library.
    Uninstall,
}

#[must_use]
pub fn dev_operation_main(operation: &'static str) -> ExitCode {
    let mut args = vec![OsString::from("recognition"), OsString::from(operation)];
    args.extend(env::args_os().skip(1));
    exit_for_result(run(&args))
}

pub(crate) fn exit_for_result(result: Result<(), String>) -> ExitCode {
    let status = result_exit_status(&result);
    if let Err(error) = result
        && error != TERMINATED_ERROR
        && error != INTERRUPTED_ERROR
    {
        eprintln!("scorepeek: {error}");
    }
    ExitCode::from(status)
}

fn result_exit_status(result: &Result<(), String>) -> u8 {
    match result {
        Ok(()) => 0,
        Err(error) if error == TERMINATED_ERROR => 0,
        Err(error) if error == INTERRUPTED_ERROR => 130,
        Err(_) => 1,
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigFile {
    capture: Option<CaptureConfig>,
    crop: CropConfig,
    scores: ScoresConfig,
    overlay: OverlayConfig,
    recording: RecordingConfig,
    catalog: Option<CatalogConfig>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureConfig {
    backend: CaptureKind,
    node_name: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CropConfig {
    left: Option<u32>,
    top: Option<u32>,
    right: Option<u32>,
    bottom: Option<u32>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ScoresConfig {
    enabled: Option<bool>,
    database: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct OverlayConfig {
    wayland: Option<bool>,
    wayland_edit: Option<bool>,
    obs: Option<bool>,
    config: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RecordingConfig {
    enabled: Option<bool>,
    memory_mib: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogConfig {
    url: String,
}

fn dispatch_public(cli: PublicCli) -> Result<(), String> {
    let PublicCli { config, command } = cli;
    match command {
        PublicCommand::Run(args) => run_public(args, config),
        PublicCommand::Doctor(format) => print_doctor(format.format),
        PublicCommand::Config { command } => {
            let config_path = resolve_config_path(config)?;
            run_config_command(command, &config_path)
        }
        PublicCommand::Diagnostic { command } => run_diagnostic_command(command),
        PublicCommand::Skin { command } => run_skin_command(command),
        PublicCommand::VulkanLayer { command } => run_vulkan_layer_command(command),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "exhaustive DTO conversion keeps every public command field explicit at the frontend boundary"
)]
pub(crate) fn dispatch_frontend(
    command: scorepeek_frontend_api::FrontendCommand,
) -> scorepeek_frontend_api::FrontendReply {
    use scorepeek_frontend_api as api;

    let cli = match command {
        api::FrontendCommand::Run {
            config, command, ..
        } => PublicCli {
            config: config.map(PathBuf::from),
            command: PublicCommand::Run(RunArgs {
                capture: command.capture.map(|capture| match capture {
                    api::CaptureKind::Pipewire => CaptureKind::Pipewire,
                    api::CaptureKind::VulkanLayer => CaptureKind::VulkanLayer,
                }),
                node_name: command.node_name,
                crop_left: command.crop_left,
                crop_top: command.crop_top,
                crop_right: command.crop_right,
                crop_bottom: command.crop_bottom,
                scores_db: command.scores_db.map(PathBuf::from),
                no_scores: command.no_scores,
                scores: command.scores,
                record: command.record,
                record_all: command.record_all,
                no_record: command.no_record,
                record_memory_mib: command.record_memory_mib,
                overlay_wayland: command.overlay_wayland,
                no_overlay_wayland: command.no_overlay_wayland,
                overlay_wayland_edit: command.overlay_wayland_edit,
                no_overlay_wayland_edit: command.no_overlay_wayland_edit,
                overlay_obs: command.overlay_obs,
                no_overlay_obs: command.no_overlay_obs,
                overlay_config: command.overlay_config.map(PathBuf::from),
            }),
        },
        api::FrontendCommand::Doctor { format, .. } => PublicCli {
            config: None,
            command: PublicCommand::Doctor(FormatArgs {
                format: frontend_output_format(format),
            }),
        },
        api::FrontendCommand::Config { config, action, .. } => PublicCli {
            config: config.map(PathBuf::from),
            command: PublicCommand::Config {
                command: match action {
                    api::ConfigAction::Path { format } => ConfigCommand::Path(FormatArgs {
                        format: frontend_output_format(format),
                    }),
                    api::ConfigAction::Show { format } => ConfigCommand::Show(FormatArgs {
                        format: frontend_output_format(format),
                    }),
                    api::ConfigAction::Check { format } => ConfigCommand::Check(FormatArgs {
                        format: frontend_output_format(format),
                    }),
                },
            },
        },
        api::FrontendCommand::Diagnostic { action, .. } => PublicCli {
            config: None,
            command: PublicCommand::Diagnostic {
                command: match action {
                    api::DiagnosticAction::Observe { replay_seconds } => {
                        DiagnosticCommand::Observe {
                            replay: replay_seconds,
                        }
                    }
                    api::DiagnosticAction::Inspect { run_id, format } => {
                        DiagnosticCommand::Inspect(DiagnosticInspectArgs {
                            run_id,
                            format: FormatArgs {
                                format: frontend_output_format(format),
                            },
                        })
                    }
                },
            },
        },
        api::FrontendCommand::Skin { action, .. } => PublicCli {
            config: None,
            command: PublicCommand::Skin {
                command: match action {
                    api::SkinAction::Install { package } => SkinCommand::Install {
                        package: PathBuf::from(package),
                    },
                    api::SkinAction::Uninstall { id } => SkinCommand::Uninstall { id },
                    api::SkinAction::List { format } => SkinCommand::List(FormatArgs {
                        format: frontend_output_format(format),
                    }),
                },
            },
        },
        api::FrontendCommand::VulkanLayer { action, .. } => PublicCli {
            config: None,
            command: PublicCommand::VulkanLayer {
                command: match action {
                    api::VulkanLayerAction::Install => VulkanLayerCommand::Install,
                    api::VulkanLayerAction::Uninstall => VulkanLayerCommand::Uninstall,
                },
            },
        },
    };
    let result = dispatch_public(cli);
    let exit_code = result_exit_status(&result);
    if let Err(error) = result
        && error != TERMINATED_ERROR
        && error != INTERRUPTED_ERROR
    {
        return scorepeek_frontend_api::FrontendReply::Error {
            error: scorepeek_frontend_api::FrontendError {
                error_type: "runtime_operation_failed".to_owned(),
                message: error,
            },
        };
    }
    scorepeek_frontend_api::FrontendReply::Completed { exit_code }
}

const fn frontend_output_format(format: scorepeek_frontend_api::OutputFormat) -> OutputFormat {
    match format {
        scorepeek_frontend_api::OutputFormat::Human => OutputFormat::Human,
        scorepeek_frontend_api::OutputFormat::Json => OutputFormat::Json,
    }
}

fn resolve_config_path(cli: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = cli {
        return nonempty_path(path, "--config");
    }
    match env::var_os("SCOREPEEK_CONFIG") {
        Some(path) => nonempty_path(PathBuf::from(path), "SCOREPEEK_CONFIG"),
        None => Ok(scorepeek::catalog::update::default_config_path()),
    }
}

fn nonempty_path(path: PathBuf, label: &str) -> Result<PathBuf, String> {
    (!path.as_os_str().is_empty())
        .then_some(path)
        .ok_or_else(|| format!("{label} must not be empty"))
}

fn read_config(path: &Path) -> Result<Option<(String, ConfigFile)>, String> {
    let Some(text) = read_config_text(path)? else {
        return Ok(None);
    };
    let config = toml::from_str::<ConfigFile>(&text)
        .map_err(|error| format!("config file is invalid: {error}"))?;
    validate_config_file(&config)?;
    Ok(Some((text, config)))
}

fn read_config_text(path: &Path) -> Result<Option<String>, String> {
    const MAX_CONFIG_BYTES: u64 = 64 * 1024;
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

fn validate_config_file(config: &ConfigFile) -> Result<(), String> {
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
        scorepeek::catalog::update::validate_configured_url(&catalog.url)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
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

fn run_public(args: RunArgs, config_override: Option<PathBuf>) -> Result<(), String> {
    run_public_with_model_initializer(args, config_override, |override_bundle| {
        scorepeek::resources::model::cache::ensure_small_model(override_bundle, |event| match event
        {
            scorepeek::resources::model::cache::ModelCacheEvent::DownloadStarted => {
                eprintln!("scorepeek: downloading PP-OCRv6-small model...");
            }
            scorepeek::resources::model::cache::ModelCacheEvent::DownloadCompleted => {
                eprintln!("scorepeek: PP-OCRv6-small model download complete");
            }
        })
        .map_err(|error| format!("model initialization failed: {error}"))
    })
}

fn run_public_with_model_initializer(
    args: RunArgs,
    config_override: Option<PathBuf>,
    initialize: impl FnOnce(Option<&Path>) -> Result<PathBuf, String>,
) -> Result<(), String> {
    let invocation_id = new_run_id();
    let mut diagnostics = diagnostic_stream::RunDiagnostics::start_default(&invocation_id);
    let sink = diagnostics.sink();
    let monitor = run_startup_stage(&sink, "signal_monitor", || {
        live_control::SignalStopMonitor::start()
    })?;
    let config_path_result = run_startup_stage(&sink, "config_path", || {
        resolve_config_path(config_override)
    });
    let config_path = settle_startup_result(&mut diagnostics, &monitor, config_path_result)?;
    let options = load_run_options(&mut diagnostics, &monitor, &config_path, args)?;
    let bundle_result = initialize_routine_model(&sink, None, initialize);
    let bundle = settle_startup_result(&mut diagnostics, &monitor, bundle_result)?;
    run_routine_live_session(
        &options.capture,
        options.crop,
        if options.recording {
            "enabled"
        } else {
            "disabled"
        },
        options.recording_memory_limit,
        options.recording_retention,
        options.scores_db.as_deref(),
        options.no_scores,
        options.overlays,
        &bundle,
        &config_path,
        invocation_id,
        diagnostics,
        &monitor,
    )
}

fn load_run_options(
    diagnostics: &mut diagnostic_stream::RunDiagnostics,
    monitor: &live_control::SignalStopMonitor,
    config_path: &Path,
    args: RunArgs,
) -> Result<RoutineRunOptions, String> {
    let sink = diagnostics.sink();
    let config_result = run_startup_stage(&sink, "config_load", || {
        read_config(config_path).map(|value| value.map(|(_, config)| config).unwrap_or_default())
    });
    let config = settle_startup_result(diagnostics, monitor, config_result)?;
    let merge_result = run_startup_stage(&sink, "config_merge", || merge_run_options(config, args));
    settle_startup_result(diagnostics, monitor, merge_result)
}

#[allow(
    clippy::too_many_lines,
    reason = "one function keeps the config, environment, and CLI precedence visible in order"
)]
fn merge_run_options(config: ConfigFile, cli: RunArgs) -> Result<RoutineRunOptions, String> {
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
    let mut recording_retention = canonical_recording::RecordingRetention::Selective;
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
        recording_retention = canonical_recording::RecordingRetention::All;
    }
    if recording_memory_mib.is_some() && !recording {
        return Err("recording memory limit requires recording to be enabled".to_owned());
    }
    let recording_memory_limit = canonical_recording::RecordingMemoryLimit::from_mib(
        recording_memory_mib.unwrap_or(canonical_recording::DEFAULT_RECORDING_MEMORY_MIB),
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
        .map(|value| nonempty_path(PathBuf::from(value), name))
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

fn run_config_command(command: ConfigCommand, path: &Path) -> Result<(), String> {
    match command {
        ConfigCommand::Path(format) => match format.format {
            OutputFormat::Human => println!("{}", path.display()),
            OutputFormat::Json => {
                let path = config_path_for_json(path)?;
                println!("{}", serde_json::json!({"path": path}));
            }
        },
        ConfigCommand::Show(format) => {
            let content = read_config_text(path)?;
            match format.format {
                OutputFormat::Human => {
                    if let Some(content) = content {
                        print!("{content}");
                    } else {
                        println!("config file is not present: {}", path.display());
                    }
                }
                OutputFormat::Json => {
                    let path = config_path_for_json(path)?;
                    println!(
                        "{}",
                        serde_json::json!({"path": path, "present": content.is_some(), "content": content})
                    );
                }
            }
        }
        ConfigCommand::Check(format) => {
            let present = read_config(path)?.is_some();
            match format.format {
                OutputFormat::Human => {
                    if present {
                        println!("config is valid: {}", path.display());
                    } else {
                        println!("config file is not present (optional): {}", path.display());
                    }
                }
                OutputFormat::Json => {
                    let path = config_path_for_json(path)?;
                    println!(
                        "{}",
                        serde_json::json!({"path": path, "present": present, "valid": true})
                    );
                }
            }
        }
    }
    Ok(())
}

fn config_path_for_json(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| "config path must be UTF-8 for JSON output".to_owned())
}

fn run_diagnostic_command(command: DiagnosticCommand) -> Result<(), String> {
    match command {
        DiagnosticCommand::Observe { replay } => diagnostic_observe(replay),
        DiagnosticCommand::Inspect(args) => {
            let store = diagnostic_stream::default_store()?;
            let format = match args.format.format {
                OutputFormat::Human => diagnostic_stream::InspectionFormat::Human,
                OutputFormat::Json => diagnostic_stream::InspectionFormat::Json,
            };
            let status = diagnostic_inspect(&store, args.run_id.as_deref(), format)?;
            (status == 0)
                .then_some(())
                .ok_or_else(|| "diagnostic recording is partial".to_owned())
        }
    }
}

fn run_skin_command(command: SkinCommand) -> Result<(), String> {
    let store = scorepeek_overlay_wayland::skin::StoreRoot::discover();
    match command {
        SkinCommand::Install { package } => {
            let outcome = store.install(&package)?;
            match outcome {
                scorepeek_overlay_wayland::skin::InstallOutcome::Installed => println!("installed"),
                scorepeek_overlay_wayland::skin::InstallOutcome::Replaced { previous_release } => {
                    println!("replaced {previous_release}");
                }
                scorepeek_overlay_wayland::skin::InstallOutcome::Unchanged => println!("unchanged"),
            }
        }
        SkinCommand::Uninstall { id } => {
            store.uninstall(&id)?;
            println!("uninstalled");
        }
        SkinCommand::List(format) => {
            let installed = store.list()?;
            match format.format {
                OutputFormat::Human => {
                    for skin in installed {
                        println!("{}\t{}\t{}", skin.id, skin.release, skin.name);
                    }
                }
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string(&installed)
                        .map_err(|error| format!("skin list serialization failed: {error}"))?
                ),
            }
        }
    }
    Ok(())
}

fn run_vulkan_layer_command(command: VulkanLayerCommand) -> Result<(), String> {
    match command {
        VulkanLayerCommand::Install => {
            match vulkan_layer::install().map_err(|error| error.to_string())? {
                vulkan_layer::InstallOutcome::Installed => println!("installed"),
                vulkan_layer::InstallOutcome::Updated => println!("updated"),
                vulkan_layer::InstallOutcome::Unchanged => println!("unchanged"),
            }
        }
        VulkanLayerCommand::Uninstall => {
            match vulkan_layer::uninstall().map_err(|error| error.to_string())? {
                vulkan_layer::UninstallOutcome::Uninstalled => println!("uninstalled"),
                vulkan_layer::UninstallOutcome::NotInstalled => println!("not installed"),
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn run(args: &[OsString]) -> Result<(), String> {
    run_with_model_initializer(args, |override_bundle| {
        scorepeek::resources::model::cache::ensure_small_model(override_bundle, |event| match event
        {
            scorepeek::resources::model::cache::ModelCacheEvent::DownloadStarted => {
                eprintln!("scorepeek: downloading PP-OCRv6-small model...");
            }
            scorepeek::resources::model::cache::ModelCacheEvent::DownloadCompleted => {
                eprintln!("scorepeek: PP-OCRv6-small model download complete");
            }
        })
        .map_err(|error| format!("scorepeek model initialization failed: {error}"))
    })
}

fn run_with_model_initializer(
    args: &[OsString],
    initialize: impl FnOnce(Option<&Path>) -> Result<PathBuf, String>,
) -> Result<(), String> {
    if let Some(result) = try_diagnostic_stream_command(args) {
        return result;
    }
    if let Some(result) = try_offline_program_information(args)
        .or_else(|| try_skin_command(args))
        .or_else(|| try_doctor(args))
    {
        return result;
    }
    let (override_bundle, args) = parse_global_model_bundle(args)?;
    if let [run, options @ ..] = args
        && run == "run"
    {
        let options = parse_routine_run_options(options)?;
        let invocation_id = new_run_id();
        let mut diagnostics = diagnostic_stream::RunDiagnostics::start_default(&invocation_id);
        let monitor = run_startup_stage(&diagnostics.sink(), "signal_monitor", || {
            live_control::SignalStopMonitor::start()
        })?;
        let bundle_result =
            initialize_routine_model(&diagnostics.sink(), override_bundle, initialize);
        let bundle = settle_startup_result(&mut diagnostics, &monitor, bundle_result)?;
        return run_routine_live_session(
            &options.capture,
            options.crop,
            if options.recording {
                "enabled"
            } else {
                "disabled"
            },
            options.recording_memory_limit,
            options.recording_retention,
            options.scores_db.as_deref(),
            options.no_scores,
            options.overlays,
            &bundle,
            &scorepeek::catalog::update::default_config_path(),
            invocation_id,
            diagnostics,
            &monitor,
        );
    }
    let bundle = initialize(override_bundle)?;
    run_command(args, &bundle)
}

fn initialize_routine_model(
    diagnostics: &diagnostic_stream::DiagnosticSink,
    override_bundle: Option<&Path>,
    initialize: impl FnOnce(Option<&Path>) -> Result<PathBuf, String>,
) -> Result<PathBuf, String> {
    run_startup_stage(diagnostics, "model_initialization", || {
        initialize(override_bundle)
    })
}

fn new_run_id() -> String {
    let elapsed = std::time::SystemTime::UNIX_EPOCH
        .elapsed()
        .unwrap_or_default();
    format!(
        "run-{}-{}-{}",
        elapsed.as_secs(),
        elapsed.subsec_nanos(),
        std::process::id()
    )
}

fn try_diagnostic_stream_command(args: &[OsString]) -> Option<Result<(), String>> {
    let result = match args {
        [diagnostic, inspect, latest]
            if diagnostic == "diagnostic" && inspect == "inspect" && latest == "--latest" =>
        {
            diagnostic_stream::default_store().and_then(|store| {
                let status =
                    diagnostic_inspect(&store, None, diagnostic_stream::InspectionFormat::Ndjson)?;
                (status == 0)
                    .then_some(())
                    .ok_or_else(|| "diagnostic recording is partial".to_owned())
            })
        }
        [diagnostic, inspect, run_id_flag, run_id]
            if diagnostic == "diagnostic" && inspect == "inspect" && run_id_flag == "--run-id" =>
        {
            let run_id = run_id
                .to_str()
                .ok_or_else(|| "diagnostic run ID must be UTF-8".to_owned());
            run_id.and_then(|run_id| {
                diagnostic_stream::default_store().and_then(|store| {
                    let status = diagnostic_inspect(
                        &store,
                        Some(run_id),
                        diagnostic_stream::InspectionFormat::Ndjson,
                    )?;
                    (status == 0)
                        .then_some(())
                        .ok_or_else(|| "diagnostic recording is partial".to_owned())
                })
            })
        }
        [diagnostic, observe] if diagnostic == "diagnostic" && observe == "observe" => {
            diagnostic_observe(None)
        }
        [diagnostic, observe, replay, seconds]
            if diagnostic == "diagnostic" && observe == "observe" && replay == "--replay" =>
        {
            seconds
                .to_str()
                .ok_or_else(|| "diagnostic replay seconds must be UTF-8".to_owned())
                .and_then(|seconds| {
                    seconds
                        .parse::<u64>()
                        .ok()
                        .filter(|seconds| *seconds > 0)
                        .ok_or_else(|| {
                            "diagnostic replay seconds must be a positive integer".to_owned()
                        })
                })
                .and_then(|seconds| diagnostic_observe(Some(seconds)))
        }
        _ => return None,
    };
    Some(result)
}

fn try_skin_command(args: &[OsString]) -> Option<Result<(), String>> {
    let store = scorepeek_overlay_wayland::skin::StoreRoot::discover();
    match args {
        [skin, install, path] if skin == "skin" && install == "install" => {
            Some(store.install(Path::new(path)).map(|outcome| {
                use scorepeek_overlay_wayland::skin::InstallOutcome;
                match outcome {
                    InstallOutcome::Installed => println!("installed"),
                    InstallOutcome::Replaced { previous_release } => {
                        println!("replaced {previous_release}");
                    }
                    InstallOutcome::Unchanged => println!("unchanged"),
                }
            }))
        }
        [skin, uninstall, id] if skin == "skin" && uninstall == "uninstall" => Some(
            id.to_str()
                .ok_or_else(|| "skin id must be UTF-8".to_owned())
                .and_then(|id| store.uninstall(id))
                .map(|()| println!("uninstalled")),
        ),
        [skin, list] if skin == "skin" && list == "list" => Some(store.list().map(|installed| {
            for skin in installed {
                println!("{}\t{}\t{}", skin.id, skin.release, skin.name);
            }
        })),
        _ => None,
    }
}

fn parse_global_model_bundle(args: &[OsString]) -> Result<(Option<&Path>, &[OsString]), String> {
    match args {
        [flag, bundle, rest @ ..] if flag == "--model-bundle" => {
            if rest.is_empty() {
                return Err("--model-bundle requires a command".to_owned());
            }
            Ok((Some(Path::new(bundle)), rest))
        }
        _ => Ok((None, args)),
    }
}

#[allow(clippy::too_many_lines)]
fn run_command(args: &[OsString], bundle: &Path) -> Result<(), String> {
    if let Some(result) = try_recording_simulation(args, bundle)
        .or_else(|| try_provisional_title_candidates(args))
        .or_else(|| try_integrated_context_crop(args))
        .or_else(|| try_integrated_context_observe(args, bundle))
        .or_else(|| try_registered_resource_gate(args, bundle))
        .or_else(|| try_dynamic_official_onnx_decode(args))
        .or_else(|| try_official_onnx_decode(args))
        .or_else(|| try_title_model_contract_parity(args))
        .or_else(|| try_title_onnx_parity(args))
        .or_else(|| try_title_dictionary_audit(args))
        .or_else(|| try_title_model_export_requirements(args))
        .or_else(|| try_program_information(args))
    {
        return result;
    }
    match args {
        [
            recognition,
            inspect,
            extraction_flag,
            extraction,
            digest_flag,
            digest,
            frame_flag,
            frame_id,
        ] if recognition == "recognition"
            && inspect == "inspect"
            && extraction_flag == "--extraction"
            && digest_flag == "--extraction-sha256"
            && frame_flag == "--frame-id" =>
        {
            inspect_canonical_frame(extraction, digest, frame_id)
        }
        [
            recognition,
            crop,
            extraction_flag,
            extraction,
            digest_flag,
            digest,
            frame_flag,
            frame_id,
            output_flag,
            output,
        ] if recognition == "recognition"
            && crop == "crop"
            && extraction_flag == "--extraction"
            && digest_flag == "--extraction-sha256"
            && frame_flag == "--frame-id"
            && output_flag == "--output" =>
        {
            crop_canonical_result(extraction, digest, frame_id, output)
        }
        [
            recognition,
            crop,
            extraction_flag,
            extraction,
            digest_flag,
            digest,
            frame_flag,
            frame_id,
            output_flag,
            output,
        ] if recognition == "recognition"
            && crop == "music-select-crop"
            && extraction_flag == "--extraction"
            && digest_flag == "--extraction-sha256"
            && frame_flag == "--frame-id"
            && output_flag == "--output" =>
        {
            crop_canonical_music_select(extraction, digest, frame_id, output)
        }
        [
            recognition,
            title_spike,
            store_flag,
            store,
            text_flag,
            text,
            confidence_flag,
            confidence,
        ] if recognition == "recognition"
            && title_spike == "title-spike"
            && store_flag == "--catalog-store"
            && text_flag == "--ocr-text"
            && confidence_flag == "--ocr-confidence" =>
        {
            diagnostic_title_spike(store, text, confidence)
        }
        _ => Err("usage: scorepeek --help".to_owned()),
    }
}

#[derive(Serialize)]
struct RegisteredResourceGateReport<'a> {
    schema: &'static str,
    status: &'static str,
    error_type: Option<RegisteredResourceGateErrorType>,
    catalog_sha256: &'a str,
    model_sha256: &'a str,
    runtime_sha256: &'a str,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum RegisteredResourceGateErrorType {
    InvalidBinding,
    WorkerUnavailable,
    FinishTimeout,
    InvalidLocation,
    ModelBindingMismatch,
    RuntimeBindingMismatch,
    CatalogUnavailable,
    CatalogBindingMismatch,
    CatalogLoadFailed,
    ModelBundleInvalid,
    RuntimeInitializationFailed,
}

struct RegisteredResourceOwner {
    _resources: recognition::RegisteredRecognitionResources,
}

impl recognition_live::field_observer::FieldObserver for RegisteredResourceOwner {
    type Output = ();

    fn observe(
        &mut self,
        _input: &recognition_live::field_observer::FieldObserverInput,
    ) -> Self::Output {
    }
}

impl From<recognition::RegisteredResourceLoadErrorType> for RegisteredResourceGateErrorType {
    fn from(error: recognition::RegisteredResourceLoadErrorType) -> Self {
        match error {
            recognition::RegisteredResourceLoadErrorType::InvalidLocation => Self::InvalidLocation,
            recognition::RegisteredResourceLoadErrorType::ModelBindingMismatch => {
                Self::ModelBindingMismatch
            }
            recognition::RegisteredResourceLoadErrorType::RuntimeBindingMismatch => {
                Self::RuntimeBindingMismatch
            }
            recognition::RegisteredResourceLoadErrorType::CatalogUnavailable => {
                Self::CatalogUnavailable
            }
            recognition::RegisteredResourceLoadErrorType::CatalogBindingMismatch => {
                Self::CatalogBindingMismatch
            }
            recognition::RegisteredResourceLoadErrorType::CatalogLoadFailed => {
                Self::CatalogLoadFailed
            }
            recognition::RegisteredResourceLoadErrorType::ModelBundleInvalid => {
                Self::ModelBundleInvalid
            }
            recognition::RegisteredResourceLoadErrorType::RuntimeInitializationFailed => {
                Self::RuntimeInitializationFailed
            }
        }
    }
}

fn try_registered_resource_gate(
    args: &[OsString],
    bundle_root: &Path,
) -> Option<Result<(), String>> {
    let [
        recognition_command,
        gate,
        catalog_flag,
        catalog_root,
        catalog_digest_flag,
        catalog_digest,
    ] = args
    else {
        return None;
    };
    if recognition_command != "recognition"
        || gate != "field-resource-load-gate"
        || catalog_flag != "--catalog-store"
        || catalog_digest_flag != "--catalog-sha256"
    {
        return None;
    }
    Some(registered_resource_gate(
        catalog_root,
        bundle_root,
        catalog_digest,
    ))
}

fn registered_resource_gate(
    catalog_root: &OsStr,
    bundle_root: &Path,
    catalog_digest: &OsStr,
) -> Result<(), String> {
    let catalog_digest = parse_cli_sha256(catalog_digest, "catalog SHA-256")?;
    let model_digest = recognition::LIVE_MODEL_SHA256.to_owned();
    let runtime_digest = recognition::LIVE_RUNTIME_SHA256.to_owned();
    let descriptor = DiagnosticRunDescriptor {
        run_id: "field-resource-load-gate".to_owned(),
        monotonic_start_ms: 0,
        resource: DiagnosticResource {
            program: "scorepeek",
            version: env!("CARGO_PKG_VERSION"),
            build_sha256: "0".repeat(64),
        },
        binding: DiagnosticBinding {
            capture_generation: 1,
            capture_profile_sha256: "0".repeat(64),
            normalizer_sha256: "0".repeat(64),
            canonical_layout_sha256: recognition::CanonicalLayout::sha256(),
            catalog_sha256: catalog_digest.clone(),
            model_sha256: model_digest.clone(),
            runtime_sha256: runtime_digest.clone(),
            replay: None,
        },
    };
    let worker =
        recognition_live::field_observer::FieldObserverWorker::start(&descriptor, |binding| {
            binding
                .load_registered_resources(Path::new(catalog_root), bundle_root)
                .map(|resources| RegisteredResourceOwner {
                    _resources: resources,
                })
        });
    match worker {
        Ok(worker) => {
            let outcome = worker
                .finish(recognition_live::field_observer::DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT);
            if outcome.status
                != recognition_live::field_observer::FieldObserverFinishStatus::Complete
            {
                let error_type = match outcome.status {
                    recognition_live::field_observer::FieldObserverFinishStatus::Timeout => {
                        RegisteredResourceGateErrorType::FinishTimeout
                    }
                    recognition_live::field_observer::FieldObserverFinishStatus::WorkerUnavailable => {
                        RegisteredResourceGateErrorType::WorkerUnavailable
                    }
                    recognition_live::field_observer::FieldObserverFinishStatus::Complete => {
                        unreachable!("complete outcome was handled above")
                    }
                };
                print_registered_resource_gate_report(
                    "error",
                    Some(error_type),
                    &catalog_digest,
                    &model_digest,
                    &runtime_digest,
                )?;
                return Err("registered resource worker did not finish cleanly".to_owned());
            }
            let report = RegisteredResourceGateReport {
                schema: "scorepeek-field-resource-load-gate-v1",
                status: "success",
                error_type: None,
                catalog_sha256: &catalog_digest,
                model_sha256: &model_digest,
                runtime_sha256: &runtime_digest,
            };
            println!(
                "{}",
                serde_json::to_string(&report)
                    .map_err(|_| "resource gate report serialization failed".to_owned())?
            );
            Ok(())
        }
        Err(error) => {
            let (error_type, message) = registered_resource_start_error(error);
            print_registered_resource_gate_report(
                "error",
                Some(error_type),
                &catalog_digest,
                &model_digest,
                &runtime_digest,
            )?;
            Err(message)
        }
    }
}

fn registered_resource_start_error(
    error: recognition_live::field_observer::FieldObserverStartError<
        recognition::RegisteredResourceLoadError,
    >,
) -> (RegisteredResourceGateErrorType, String) {
    use recognition_live::field_observer::FieldObserverStartError;
    match error {
        FieldObserverStartError::InvalidBinding => (
            RegisteredResourceGateErrorType::InvalidBinding,
            "registered resource worker binding is invalid".to_owned(),
        ),
        FieldObserverStartError::Load(error) => (error.error_type().into(), error.to_string()),
        FieldObserverStartError::WorkerUnavailable => (
            RegisteredResourceGateErrorType::WorkerUnavailable,
            "registered resource worker is unavailable".to_owned(),
        ),
    }
}

fn print_registered_resource_gate_report(
    status: &'static str,
    error_type: Option<RegisteredResourceGateErrorType>,
    catalog_sha256: &str,
    model_sha256: &str,
    runtime_sha256: &str,
) -> Result<(), String> {
    let report = RegisteredResourceGateReport {
        schema: "scorepeek-field-resource-load-gate-v1",
        status,
        error_type,
        catalog_sha256,
        model_sha256,
        runtime_sha256,
    };
    println!(
        "{}",
        serde_json::to_string(&report)
            .map_err(|_| "resource gate report serialization failed".to_owned())?
    );
    Ok(())
}

#[cfg(test)]
#[allow(dead_code)]
fn try_capture_commands(args: &[OsString], bundle: &Path) -> Option<Result<(), String>> {
    try_capture_result_recognition(args, bundle)
        .or_else(|| try_capture_field_observation(args, bundle))
        .or_else(|| try_capture_recognition_handoff(args))
        .or_else(|| try_capture_diagnostic_handoff(args))
        .or_else(|| try_capture_canonical_frame(args))
        .or_else(|| try_capture_binding_admission(args))
        .or_else(|| try_capture_live_gate(args))
}

#[cfg(test)]
const CAPTURE_HANDOFF_FLAGS: &[&str] = &[
    "--binding",
    "--binding-sha256",
    "--capture-generation",
    "--duration-ms",
    "--diagnostic-root",
    "--run-id",
    "--build-sha256",
    "--canonical-layout-sha256",
    "--catalog-sha256",
    "--recording",
];

#[cfg(test)]
const CAPTURE_FIELD_OBSERVATION_FLAGS: &[&str] = &[
    "--binding",
    "--binding-sha256",
    "--capture-generation",
    "--duration-ms",
    "--diagnostic-root",
    "--catalog-store",
    "--run-id",
    "--build-sha256",
    "--canonical-layout-sha256",
    "--catalog-sha256",
    "--recording",
];

#[cfg(test)]
const CAPTURE_RESULT_RECOGNITION_FLAGS: &[&str] = &[
    "--binding",
    "--binding-sha256",
    "--capture-generation",
    "--duration-ms",
    "--diagnostic-root",
    "--catalog-store",
    "--run-id",
    "--build-sha256",
    "--canonical-layout-sha256",
    "--catalog-sha256",
    "--recording",
    "--recognition-artifact",
];

#[cfg(test)]
const LIVE_SESSION_FLAGS: &[&str] = &[
    "--binding",
    "--binding-sha256",
    "--capture-generation",
    "--diagnostic-root",
    "--catalog-store",
    "--run-id",
    "--build-sha256",
    "--canonical-layout-sha256",
    "--catalog-sha256",
    "--recording",
    "--recognition-artifact",
];

struct RoutineRunOptions {
    overlays: OverlayOptions,
    capture: RoutineCapture,
    crop: scorepeek::capture::EdgeCrop,
    scores_db: Option<PathBuf>,
    no_scores: bool,
    recording: bool,
    recording_memory_limit: canonical_recording::RecordingMemoryLimit,
    recording_retention: canonical_recording::RecordingRetention,
}

#[derive(Clone)]
enum RoutineCapture {
    Pipewire { node_name: String },
    VulkanLayer,
}

#[derive(Default)]
struct OverlayOptions {
    wayland: bool,
    wayland_edit: bool,
    obs: bool,
    config_path: Option<PathBuf>,
}

fn run_startup_stage<T>(
    diagnostics: &diagnostic_stream::DiagnosticSink,
    stage: &str,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    match operation() {
        Ok(value) => {
            diagnostics.record(
                "run_startup_stage",
                &serde_json::json!({"stage":stage, "status":"success"}),
                true,
            );
            Ok(value)
        }
        Err(error) => {
            diagnostics.record(
                "run_startup_stage",
                &serde_json::json!({"stage":stage, "status":"error", "error":error}),
                true,
            );
            Err(error)
        }
    }
}

fn check_startup_stop(
    diagnostics: &diagnostic_stream::DiagnosticSink,
    monitor: &live_control::SignalStopMonitor,
) -> Result<(), String> {
    if !monitor.stop_requested() {
        return Ok(());
    }
    diagnostics.record(
        "run_startup_stage",
        &serde_json::json!({"stage":"interrupt", "status":"cancel"}),
        true,
    );
    if monitor.interrupted() {
        Err(INTERRUPTED_ERROR.to_owned())
    } else {
        Err(TERMINATED_ERROR.to_owned())
    }
}

fn settle_startup_result<T>(
    diagnostics: &mut diagnostic_stream::RunDiagnostics,
    monitor: &live_control::SignalStopMonitor,
    result: Result<T, String>,
) -> Result<T, String> {
    let sink = diagnostics.sink();
    if let Err(error) = check_startup_stop(&sink, monitor) {
        diagnostics.finish("cancel");
        return Err(error);
    }
    result
}

fn settle_output_startup_result<T>(
    output: &mut routine_output::RoutineOutput,
    monitor: &live_control::SignalStopMonitor,
    result: Result<T, String>,
) -> Result<T, String> {
    if !monitor.stop_requested() {
        return result;
    }
    output.record_diagnostic(
        "run_startup_stage",
        &serde_json::json!({"stage":"interrupt", "status":"cancel"}),
        true,
    );
    output.finish_diagnostics("cancel");
    if monitor.interrupted() {
        Err(INTERRUPTED_ERROR.to_owned())
    } else {
        Err(TERMINATED_ERROR.to_owned())
    }
}

#[allow(clippy::too_many_lines)]
fn parse_routine_run_options(options: &[OsString]) -> Result<RoutineRunOptions, String> {
    let mut capture = None;
    let mut node_name = None;
    let mut crop = scorepeek::capture::EdgeCrop {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let mut crop_seen = [false; 4];
    let mut recording = false;
    let mut recording_retention = canonical_recording::RecordingRetention::Selective;
    let mut scores_db = None;
    let mut no_scores = false;
    let mut recording_memory_mib = None;
    let mut overlays = OverlayOptions::default();
    let mut index = 0;
    while index < options.len() {
        match options[index].to_str() {
            Some("--overlay-config") if overlays.config_path.is_none() => {
                let option = options[index].to_str().unwrap_or_default();
                index += 1;
                let value = options
                    .get(index)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| format!("{option} requires a value"))?;
                overlays.config_path = Some(PathBuf::from(value));
            }
            Some("--overlay-wayland") if !overlays.wayland => overlays.wayland = true,
            Some("--overlay-wayland-edit") if !overlays.wayland_edit => {
                overlays.wayland = true;
                overlays.wayland_edit = true;
            }
            Some("--overlay-obs") if !overlays.obs => overlays.obs = true,
            Some("--record") if !recording => recording = true,
            Some("--record-all") if !recording => {
                recording = true;
                recording_retention = canonical_recording::RecordingRetention::All;
            }
            Some("--no-scores") if !no_scores => no_scores = true,
            Some("--scores-db") if scores_db.is_none() => {
                index += 1;
                let Some(value) = options.get(index).filter(|value| !value.is_empty()) else {
                    return Err("--scores-db requires a database path".to_owned());
                };
                scores_db = Some(PathBuf::from(value));
            }
            Some("--capture") if capture.is_none() => {
                index += 1;
                let Some(value) = options.get(index).and_then(|value| value.to_str()) else {
                    return Err("--capture requires pipewire or vulkan-layer".to_owned());
                };
                capture = Some(value);
            }
            Some("--node-name") if node_name.is_none() => {
                index += 1;
                let Some(value) = options
                    .get(index)
                    .and_then(|value| value.to_str())
                    .filter(|value| !value.is_empty())
                else {
                    return Err("--node-name requires a non-empty UTF-8 value".to_owned());
                };
                node_name = Some(value);
            }
            Some(option @ ("--crop-left" | "--crop-top" | "--crop-right" | "--crop-bottom")) => {
                let crop_index = match option {
                    "--crop-left" => 0,
                    "--crop-top" => 1,
                    "--crop-right" => 2,
                    _ => 3,
                };
                if crop_seen[crop_index] {
                    return Err(format!("duplicate run option: {option}"));
                }
                crop_seen[crop_index] = true;
                index += 1;
                let Some(value) = options.get(index).and_then(|value| value.to_str()) else {
                    return Err(format!("{option} requires a pixel count"));
                };
                let value = value
                    .parse::<u32>()
                    .map_err(|_| format!("{option} requires a non-negative integer"))?;
                match crop_index {
                    0 => crop.left = value,
                    1 => crop.top = value,
                    2 => crop.right = value,
                    _ => crop.bottom = value,
                }
            }
            Some("--record-memory-mib") if recording_memory_mib.is_none() => {
                index += 1;
                let Some(value) = options.get(index).and_then(|value| value.to_str()) else {
                    return Err("--record-memory-mib requires an integer MiB value".to_owned());
                };
                recording_memory_mib =
                    Some(value.parse::<usize>().map_err(|_| {
                        "--record-memory-mib requires an integer MiB value".to_owned()
                    })?);
            }
            Some(option) => return Err(format!("unknown or duplicate run option: {option}")),
            None => return Err("run option must be UTF-8".to_owned()),
        }
        index += 1;
    }
    if no_scores && scores_db.is_some() {
        return Err("--scores-db conflicts with --no-scores".to_owned());
    }
    if recording_memory_mib.is_some() && !recording {
        return Err("--record-memory-mib requires --record or --record-all".to_owned());
    }
    let recording_memory_limit = canonical_recording::RecordingMemoryLimit::from_mib(
        recording_memory_mib.unwrap_or(canonical_recording::DEFAULT_RECORDING_MEMORY_MIB),
    )?;
    let capture = match (capture, node_name) {
        (Some("pipewire"), Some(node_name)) => RoutineCapture::Pipewire {
            node_name: node_name.to_owned(),
        },
        (Some("pipewire"), None) => {
            return Err("--capture pipewire requires --node-name".to_owned());
        }
        (Some("vulkan-layer"), None) => RoutineCapture::VulkanLayer,
        (Some("vulkan-layer"), Some(_)) => {
            return Err("--node-name is only valid with --capture pipewire".to_owned());
        }
        (Some(value), _) => return Err(format!("unsupported capture backend: {value}")),
        (None, _) => {
            return Err(
                "scorepeek run requires --capture pipewire or --capture vulkan-layer".to_owned(),
            );
        }
    };
    Ok(RoutineRunOptions {
        overlays,
        capture,
        crop,
        scores_db,
        no_scores,
        recording,
        recording_memory_limit,
        recording_retention,
    })
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the admitted run keeps parsed options and its diagnostic ownership explicit"
)]
fn run_routine_live_session(
    capture: &RoutineCapture,
    crop: scorepeek::capture::EdgeCrop,
    recording: &str,
    recording_memory_limit: canonical_recording::RecordingMemoryLimit,
    recording_retention: canonical_recording::RecordingRetention,
    scores_db: Option<&Path>,
    no_scores: bool,
    overlays: OverlayOptions,
    bundle: &Path,
    config_path: &Path,
    invocation_id: String,
    mut diagnostics: diagnostic_stream::RunDiagnostics,
    monitor: &live_control::SignalStopMonitor,
) -> Result<(), String> {
    let recording_enabled = recording == "enabled";
    let diagnostic_sink = diagnostics.sink();
    diagnostic_sink.record(
        "canonical_recording_config",
        &serde_json::json!({
            "enabled": recording_enabled,
            "retention": recording_retention,
            "memory_limit_bytes": recording_memory_limit.bytes(),
        }),
        true,
    );
    let catalog_paths_result = run_startup_stage(&diagnostic_sink, "catalog_paths", || {
        catalog_paths(
            env::var_os("XDG_DATA_HOME").as_deref(),
            env::var_os("XDG_CACHE_HOME").as_deref(),
            env::var_os("HOME").as_deref(),
        )
    });
    let (catalog_root, _) = settle_startup_result(&mut diagnostics, monitor, catalog_paths_result)?;
    let catalog_url_result = run_startup_stage(&diagnostic_sink, "catalog_url", || {
        scorepeek::catalog::update::resolve_effective_url(config_path)
            .map_err(|error| error.to_string())
    });
    let effective_catalog_url =
        settle_startup_result(&mut diagnostics, monitor, catalog_url_result)?;
    let prepared_catalog_result = run_startup_stage(&diagnostic_sink, "active_catalog", || {
        scorepeek::catalog::update::prepare(&catalog_root, &effective_catalog_url, |event| {
            record_catalog_update(&diagnostic_sink, &event);
        })
        .map_err(|error| error.to_string())
    });
    let prepared_catalog =
        settle_startup_result(&mut diagnostics, monitor, prepared_catalog_result)?;
    let run_catalog_digest = prepared_catalog.active.digest;
    settle_startup_result(&mut diagnostics, monitor, Ok(()))?;
    let background_catalog_update = prepared_catalog.background_due.then(|| {
        let background_root = catalog_root.clone();
        let background_url = effective_catalog_url.clone();
        let background_sink = diagnostic_sink.clone();
        let background_stop = monitor.stop_token();
        std::thread::Builder::new()
            .name("catalog-update-low-priority".to_owned())
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(250));
                if background_stop.load(std::sync::atomic::Ordering::Acquire) {
                    return;
                }
                let _ = scorepeek::catalog::update::update_background(
                    &background_root,
                    &background_url,
                    |event| record_catalog_update(&background_sink, &event),
                );
            })
    });
    let background_catalog_update = match background_catalog_update {
        Some(Ok(handle)) => Some(handle),
        Some(Err(error)) => {
            diagnostic_sink.record(
                "catalog_update",
                &serde_json::json!({
                    "mode": "background",
                    "stage": "resolve",
                    "status": "error",
                    "error_type": "worker_start_failed",
                    "source_url_sha256": effective_catalog_url.fingerprint(),
                }),
                true,
            );
            eprintln!("scorepeek: background catalog update unavailable: {error}");
            None
        }
        None => None,
    };
    settle_startup_result(&mut diagnostics, monitor, Ok(()))?;
    if recording_enabled {
        let recorder_result = run_startup_stage(&diagnostic_sink, "canonical_recorder", || {
            canonical_recording::CanonicalRecordingWorker::preflight()
        });
        settle_startup_result(&mut diagnostics, monitor, recorder_result)?;
    }
    let state_result = run_startup_stage(&diagnostic_sink, "state", || {
        local_profiles::state_paths(recording_enabled)
    });
    let state = settle_startup_result(&mut diagnostics, monitor, state_result)?;
    let identity_result = run_startup_stage(&diagnostic_sink, "executable_identity", || {
        current_executable_sha256()
    });
    let build_sha256 = settle_startup_result(&mut diagnostics, monitor, identity_result)?;
    let stop = monitor.stop_token();
    let output_result = routine_output::RoutineOutput::start(
        invocation_id.clone(),
        "0".repeat(64),
        recording_enabled,
        diagnostics,
    );
    let mut output = match output_result {
        Ok(output) => output,
        Err(error) => {
            let (message, mut diagnostics) = error.into_parts();
            return settle_startup_result(&mut diagnostics, monitor, Err(message));
        }
    };
    settle_output_startup_result(&mut output, monitor, Ok(()))?;
    let scores_result = (|| {
        if no_scores {
            return Ok(None);
        }
        let path = if let Some(path) = scores_db {
            path.to_path_buf()
        } else {
            let root = env::var_os("XDG_DATA_HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .or_else(|| {
                    env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
                })
                .ok_or_else(|| "scores database requires XDG_DATA_HOME or HOME".to_owned())?;
            root.join("scorepeek/scores.sqlite3")
        };
        let path =
            std::path::absolute(path).map_err(|error| format!("scores database path: {error}"))?;
        output.enable_scores(&path)?;
        Ok(Some(path))
    })();
    let scores_path = settle_output_startup_result(&mut output, monitor, scores_result)?;
    let mut overlay_children = crate::overlay::supervisor::Children::default();
    let overlay_config_path = overlays
        .config_path
        .unwrap_or_else(scorepeek_overlay_wayland::config::default_path);
    let requested_backends = [
        overlays
            .wayland
            .then_some(scorepeek_overlay_wayland::bridge::data::Backend::Wayland),
        overlays
            .obs
            .then_some(scorepeek_overlay_wayland::bridge::data::Backend::Obs),
    ];
    settle_output_startup_result(&mut output, monitor, Ok(()))?;
    let loaded_overlay_config_result = if overlays.wayland || overlays.obs {
        Some(scorepeek_overlay_wayland::config::load_or_create(
            &overlay_config_path,
        ))
        .transpose()
    } else {
        Ok(None)
    };
    let loaded_overlay_config =
        settle_output_startup_result(&mut output, monitor, loaded_overlay_config_result)?;
    let overlay_controller = loaded_overlay_config
        .as_ref()
        .map(|(config, _)| {
            crate::config::control::Controller::start(&overlay_config_path, config.clone())
        })
        .transpose();
    let overlay_controller =
        settle_output_startup_result(&mut output, monitor, overlay_controller)?;
    let warning_result = (|| {
        if let Some((_, issues)) = &loaded_overlay_config {
            for issue in issues {
                output.warning(format!(
                    "overlay canvas {} ignored: {}",
                    issue.canvas_id, issue.message
                ))?;
            }
        }
        Ok(())
    })();
    settle_output_startup_result(&mut output, monitor, warning_result)?;
    for backend in requested_backends.into_iter().flatten() {
        settle_output_startup_result(&mut output, monitor, Ok(()))?;
        let (overlay_config, _) = loaded_overlay_config
            .as_ref()
            .expect("overlay config loaded");
        let validation_result = overlay_config.validated().map(|validated| {
            validated
                .0
                .into_iter()
                .filter(|canvas| canvas.backend == backend)
                .collect::<Vec<_>>()
        });
        let canvases = settle_output_startup_result(&mut output, monitor, validation_result)?;
        let started = output
            .event_socket_path()
            .ok_or_else(|| "event socket unavailable".to_owned())
            .and_then(|socket| {
                let executable = std::env::current_exe().map_err(|error| error.to_string())?;
                overlay_children.start(
                    &executable,
                    &scorepeek_overlay_wayland::bridge::data::Config {
                        backend,
                        canvases,
                        config_path: overlay_config_path.clone(),
                        control_socket: overlay_controller
                            .as_ref()
                            .expect("overlay controller started")
                            .path()
                            .to_owned(),
                        skin_store: scorepeek_overlay_wayland::skin::StoreRoot::discover()
                            .path()
                            .to_owned(),
                        socket: socket.to_path_buf(),
                        invocation: invocation_id.clone(),
                        scores_db: scores_path.clone(),
                        listen: overlay_config
                            .obs_listen
                            .parse()
                            .map_err(|error| format!("overlay obs_listen: {error}"))?,
                        unknown_grace_ms: overlay_config.unknown_grace_ms,
                        edit_on_start: backend
                            == scorepeek_overlay_wayland::bridge::data::Backend::Wayland
                            && (overlays.wayland_edit
                                || !overlay_config
                                    .canvases
                                    .iter()
                                    .any(|canvas| canvas.backend == backend)),
                    },
                )
            });
        if let Err(error) = settle_output_startup_result(&mut output, monitor, started) {
            if error == INTERRUPTED_ERROR || error == TERMINATED_ERROR {
                return Err(error);
            }
            let publish_result = output.publish(&RunEvent {
                schema: RUN_EVENT_SCHEMA.to_owned(),
                kind: RunEventKind::OverlayObserved {
                    observation: serde_json::json!({
                        "backend": format!("{backend:?}"),
                        "operation": "spawn",
                        "error_type": "start_failed",
                        "error": error,
                    }),
                },
            });
            settle_output_startup_result(&mut output, monitor, publish_result)?;
            return Err(format!(
                "{backend:?} overlay initialization failed: {error}"
            ));
        }
    }
    let watcher_started = output.publish(&RunEvent {
        schema: RUN_EVENT_SCHEMA.to_owned(),
        kind: RunEventKind::WatcherStarted {
            invocation_id: invocation_id.clone(),
        },
    });
    settle_output_startup_result(&mut output, monitor, watcher_started)?;

    let mut lifetimes = routine_watcher::SourceLifetimes::new();
    settle_output_startup_result(&mut output, monitor, Ok(()))?;
    let vulkan_listener_result = match capture {
        RoutineCapture::VulkanLayer => run_startup_stage(
            &diagnostic_sink,
            "vulkan_capture_socket",
            scorepeek::capture::vulkan::VulkanListener::bind_default,
        )
        .map(Some),
        RoutineCapture::Pipewire { .. } => Ok(None),
    };
    let vulkan_listener =
        settle_output_startup_result(&mut output, monitor, vulkan_listener_result)?;
    let mut vulkan_generation = 0_u64;
    let mut announced = None;
    while !stop.load(std::sync::atomic::Ordering::Acquire) {
        output.refresh_scores()?;
        output.refresh_overlays(&mut overlay_children, overlay_controller.as_ref())?;
        let mut vulkan_session = None;
        let decision = match capture {
            RoutineCapture::Pipewire { node_name } => {
                let Ok(snapshot) = scorepeek::capture::snapshot_pipewire_sources(
                    node_name,
                    std::time::Duration::from_millis(500),
                ) else {
                    announce_watcher_state(
                        &mut announced,
                        routine_watcher::WatcherState::RemoteUnavailable,
                        "PipeWire is unavailable; scorepeek will keep waiting",
                        &mut output,
                    )?;
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    continue;
                };
                lifetimes.observe(snapshot)
            }
            RoutineCapture::VulkanLayer => {
                let accepted = vulkan_listener
                    .as_ref()
                    .expect("Vulkan listener exists")
                    .accept(std::time::Duration::from_millis(500));
                let Some(session) = (match accepted {
                    Ok(session) => session,
                    Err(failure) => {
                        let (category, producer_status) = failure.diagnostic();
                        let fact = serde_json::to_value(
                            scorepeek::capture::CaptureDiagnosticFact {
                                sequence: 0,
                                monotonic_start_ms: 0,
                                monotonic_end_ms: 0,
                                operation:
                                    scorepeek::capture::CaptureDiagnosticOperation::SourceAcquisition,
                                status: scorepeek::capture::CaptureDiagnosticStatus::Error,
                                error_type: Some(
                                    scorepeek::capture::CaptureErrorType::ReceiverFailed,
                                ),
                                detail:
                                    scorepeek::capture::CaptureDiagnosticDetail::VulkanFailure {
                                        category,
                                        producer_status,
                                    },
                            },
                        )
                        .map_err(|error| format!("serialize Vulkan admission failure: {error}"))?;
                        output.record_diagnostic("capture", &fact, true);
                        return Err(format!("Vulkan producer admission failed ({category})"));
                    }
                }) else {
                    announce_watcher_state(
                        &mut announced,
                        routine_watcher::WatcherState::WaitingForSource,
                        "waiting for a Vulkan-layer producer",
                        &mut output,
                    )?;
                    continue;
                };
                vulkan_generation = vulkan_generation.saturating_add(1);
                vulkan_session = Some(session);
                routine_watcher::WatchDecision::Admit {
                    node_id: 0,
                    generation: vulkan_generation,
                }
            }
        };
        if stop.load(std::sync::atomic::Ordering::Acquire) {
            break;
        }
        match decision {
            routine_watcher::WatchDecision::WaitAbsent
            | routine_watcher::WatchDecision::WaitConsumed => {
                announce_watcher_state(
                    &mut announced,
                    routine_watcher::WatcherState::WaitingForSource,
                    "waiting for the selected capture source",
                    &mut output,
                )?;
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            routine_watcher::WatchDecision::WaitAmbiguous => {
                announce_watcher_state(
                    &mut announced,
                    routine_watcher::WatcherState::AmbiguousSources,
                    "multiple matching PipeWire sources are present; waiting for exactly one",
                    &mut output,
                )?;
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            routine_watcher::WatchDecision::Admit {
                node_id,
                generation,
            } => {
                let session_id = format!("{invocation_id}-session-{generation}");
                let diagnostic_run_root = output.diagnostic_run_root().map(Path::to_path_buf);
                let session_paths = match state
                    .start_recording_session(diagnostic_run_root.as_deref(), &session_id)
                {
                    Ok(paths) => paths,
                    Err(error) => {
                        output.warning(format!("recording degraded for this session: {error}"))?;
                        None
                    }
                };
                let diagnostic_root = Path::new("/");
                let recognition_root = None;
                let values = routine_live_values(
                    generation,
                    diagnostic_root,
                    &catalog_root,
                    &session_id,
                    &build_sha256,
                    &run_catalog_digest,
                    "disabled",
                    recognition_root,
                );
                let references = values.iter().map(OsString::as_os_str).collect::<Vec<_>>();
                let runtime_capture = match capture {
                    RoutineCapture::Pipewire { node_name } => {
                        capture_live::RuntimeCaptureInput::Pipewire {
                            node_name,
                            crop,
                            expected_node_id: Some(node_id),
                        }
                    }
                    RoutineCapture::VulkanLayer => capture_live::RuntimeCaptureInput::VulkanLayer {
                        session: Box::new(vulkan_session.take().expect("admitted Vulkan session")),
                        crop,
                    },
                };
                if stop.load(std::sync::atomic::Ordering::Acquire) {
                    if let Some(paths) = session_paths.as_ref()
                        && let Err(error) = paths.cleanup()
                    {
                        output.warning(error)?;
                    }
                    break;
                }
                let mut started = false;
                let mut emit = |emission: LiveSessionEmission| {
                    output.refresh_scores()?;
                    output.refresh_overlays(&mut overlay_children, overlay_controller.as_ref())?;
                    let output_started = std::time::Instant::now();
                    if let Some(binding) = emission.public_binding.clone() {
                        output.bind_public_session(binding);
                    }
                    if let Some(identity) = emission.diagnostic_identity.as_ref() {
                        output.record_diagnostic("capture_generation_identity", identity, true);
                    }
                    if let Some(fact) = emission.diagnostic_capture_fact.as_ref() {
                        output.record_diagnostic("capture", fact, true);
                        return Ok(capture_live::LiveEventProcessingTiming::default());
                    }
                    let event = run_event_from_live_emission(emission)?;
                    if matches!(&event.kind, RunEventKind::SessionStarted { .. }) {
                        started = true;
                    }
                    let output_overhead_us =
                        u64::try_from(output_started.elapsed().as_micros()).unwrap_or(u64::MAX);
                    let timing = output.publish_timed(&event)?;
                    Ok(capture_live::LiveEventProcessingTiming {
                        screen_resolver_us: timing.screen_resolver_us,
                        attempt_resolver_us: timing.attempt_resolver_us,
                        output_us: Some(
                            timing
                                .output_us
                                .unwrap_or(0)
                                .saturating_add(output_overhead_us),
                        ),
                    })
                };
                let report = execute_live_session(
                    &references,
                    bundle,
                    false,
                    recording_memory_limit,
                    recording_retention,
                    session_paths.as_ref().map(|paths| paths.root.as_path()),
                    Some(&session_id),
                    Some(node_id),
                    runtime_capture,
                    &stop,
                    &mut emit,
                )?;
                if report.output_failed() {
                    return Err("live result output failed".to_owned());
                }
                if started {
                    let stop_reason = report.stop_reason();
                    let (outcome, readmit_same_node, fatal) =
                        routine_session_disposition(stop_reason);
                    if matches!(capture, RoutineCapture::Pipewire { .. }) {
                        lifetimes.generation_ended(node_id, readmit_same_node);
                    }
                    announced = None;
                    output.publish(&RunEvent {
                        schema: RUN_EVENT_SCHEMA.to_owned(),
                        kind: RunEventKind::SessionFinished {
                            session_id: session_id.clone(),
                            capture_generation: generation,
                            outcome: outcome.to_owned(),
                            report: serde_json::to_value(&report).map_err(|error| {
                                format!("live report serialization failed: {error}")
                            })?,
                        },
                    })?;
                    if state.recording_enabled {
                        if report.canonical_recording_is_complete() {
                            if let Some(session_paths) = session_paths.as_ref() {
                                output.publish(&RunEvent {
                                    schema: RUN_EVENT_SCHEMA.to_owned(),
                                    kind: RunEventKind::RecordingCompleted {
                                        session_id: session_id.clone(),
                                        directory: session_paths.root.display().to_string(),
                                    },
                                })?;
                            }
                        } else {
                            output.status_recording_degraded()?;
                            output
                                .warning("canonical recording is partial and cannot be imported")?;
                        }
                    }
                    if fatal {
                        return Err(report
                            .failure_detail()
                            .unwrap_or("capture generation failed")
                            .to_owned());
                    }
                } else {
                    let startup_report = serde_json::to_value(&report).map_err(|error| {
                        format!("capture startup report serialization failed: {error}")
                    })?;
                    output.record_diagnostic("capture_startup_failure", &startup_report, true);
                    if let Some(paths) = session_paths.as_ref()
                        && let Err(error) = paths.cleanup()
                    {
                        output.warning(error)?;
                    }
                    match report.startup_retry() {
                        Some(capture_live::LiveSessionStartupRetry::Admission)
                            if report
                                .capture_error_type()
                                .is_some_and(transient_admission_capture_error) =>
                        {
                            announce_watcher_state(
                                &mut announced,
                                routine_watcher::WatcherState::WaitingForSource,
                                "capture source disappeared during admission; scorepeek will keep waiting",
                                &mut output,
                            )?;
                        }
                        Some(capture_live::LiveSessionStartupRetry::Admission) => {
                            return Err(report.startup_failure_summary());
                        }
                        Some(capture_live::LiveSessionStartupRetry::Catalog) => {
                            announce_watcher_state(
                                &mut announced,
                                routine_watcher::WatcherState::CatalogUnavailable,
                                "active catalog changed or is temporarily unavailable; scorepeek will retry",
                                &mut output,
                            )?;
                        }
                        None => return Err(report.startup_failure_summary()),
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
        }
    }
    overlay_children.shutdown();
    output.refresh_overlays(&mut overlay_children, overlay_controller.as_ref())?;
    output.publish(&RunEvent {
        schema: RUN_EVENT_SCHEMA.to_owned(),
        kind: RunEventKind::WatcherStopped {
            invocation_id,
            reason: "signal".to_owned(),
        },
    })?;
    if let Some(handle) = background_catalog_update {
        let _ = handle.join();
    }
    output.finish_diagnostics("cancel");
    if monitor.interrupted() {
        Err(INTERRUPTED_ERROR.to_owned())
    } else {
        Ok(())
    }
}

fn record_catalog_update(
    sink: &diagnostic_stream::DiagnosticSink,
    event: &scorepeek::catalog::update::UpdateEvent,
) {
    if let Ok(value) = serde_json::to_value(event) {
        sink.record("catalog_update", &value, true);
    }
}

fn routine_session_disposition(
    reason: Option<capture_live::LiveSessionStopReason>,
) -> (&'static str, bool, bool) {
    match reason {
        Some(capture_live::LiveSessionStopReason::RequestedSignal) => ("stopped", false, false),
        Some(
            capture_live::LiveSessionStopReason::SourceEnded
            | capture_live::LiveSessionStopReason::SourceContractChanged,
        ) => ("source_ended", true, false),
        Some(capture_live::LiveSessionStopReason::TerminalFailure) | None => ("error", false, true),
    }
}

const fn transient_admission_capture_error(
    error_type: scorepeek::capture::CaptureErrorType,
) -> bool {
    matches!(
        error_type,
        scorepeek::capture::CaptureErrorType::SourceUnavailable
            | scorepeek::capture::CaptureErrorType::SourceLost
            | scorepeek::capture::CaptureErrorType::StreamLost
    )
}

#[allow(clippy::too_many_arguments)]
fn routine_live_values(
    generation: u64,
    diagnostic_root: &Path,
    catalog_root: &Path,
    session_id: &str,
    build_sha256: &str,
    catalog_sha256: &str,
    recording: &str,
    recognition_root: Option<&Path>,
) -> Vec<OsString> {
    vec![
        Path::new("/").as_os_str().to_owned(),
        "0".repeat(64).into(),
        generation.to_string().into(),
        diagnostic_root.as_os_str().to_owned(),
        catalog_root.as_os_str().to_owned(),
        session_id.into(),
        build_sha256.into(),
        recognition::CanonicalLayout::sha256().into(),
        catalog_sha256.into(),
        recording.into(),
        recognition_root
            .unwrap_or(Path::new("/"))
            .as_os_str()
            .to_owned(),
    ]
}

fn announce_watcher_state(
    announced: &mut Option<routine_watcher::WatcherState>,
    state: routine_watcher::WatcherState,
    message: &str,
    output: &mut routine_output::RoutineOutput,
) -> Result<(), String> {
    output.watcher_state(state.as_str(), None, None, message)?;
    if *announced != Some(state) {
        *announced = Some(state);
    }
    Ok(())
}

#[cfg(test)]
#[allow(dead_code)]
fn try_live_session(args: &[OsString], bundle: &Path) -> Option<Result<(), String>> {
    let values = command_flag_values(args, "run", "gamescope", LIVE_SESSION_FLAGS)?;
    Some(run_live_session(&values, bundle, true))
}

#[cfg(test)]
fn try_capture_result_recognition(args: &[OsString], bundle: &Path) -> Option<Result<(), String>> {
    let values = capture_flag_values(
        args,
        "gamescope-result-recognition-gate",
        CAPTURE_RESULT_RECOGNITION_FLAGS,
    )?;
    let (artifact, common) = values
        .split_last()
        .expect("result recognition flags are non-empty");
    Some(run_capture_field_observation(
        common,
        bundle,
        Some(Path::new(artifact)),
    ))
}

#[cfg(test)]
fn try_capture_field_observation(args: &[OsString], bundle: &Path) -> Option<Result<(), String>> {
    let values = capture_flag_values(
        args,
        "gamescope-field-observation-gate",
        CAPTURE_FIELD_OBSERVATION_FLAGS,
    )?;
    Some(run_capture_field_observation(&values, bundle, None))
}

#[cfg(test)]
fn try_capture_recognition_handoff(args: &[OsString]) -> Option<Result<(), String>> {
    let values = capture_flag_values(
        args,
        "gamescope-recognition-handoff-gate",
        CAPTURE_HANDOFF_FLAGS,
    )?;
    Some(run_capture_handoff(&values, true))
}

#[cfg(test)]
fn try_capture_diagnostic_handoff(args: &[OsString]) -> Option<Result<(), String>> {
    let values = capture_flag_values(
        args,
        "gamescope-diagnostic-handoff-gate",
        CAPTURE_HANDOFF_FLAGS,
    )?;
    Some(run_capture_handoff(&values, false))
}

#[cfg(test)]
fn capture_flag_values<'a>(
    args: &'a [OsString],
    command: &str,
    flags: &[&str],
) -> Option<Vec<&'a OsStr>> {
    command_flag_values(args, "capture", command, flags)
}

#[cfg(test)]
fn command_flag_values<'a>(
    args: &'a [OsString],
    namespace: &str,
    command: &str,
    flags: &[&str],
) -> Option<Vec<&'a OsStr>> {
    if args.first()? != namespace || args.get(1)? != command || args.len() != 2 + flags.len() * 2 {
        return None;
    }
    let mut values = Vec::with_capacity(flags.len());
    for (pair, expected_flag) in args[2..].chunks_exact(2).zip(flags) {
        if pair[0] != *expected_flag {
            return None;
        }
        values.push(pair[1].as_os_str());
    }
    Some(values)
}

#[cfg(test)]
fn run_live_session(
    values: &[&OsStr],
    bundle_root: &Path,
    persist_recognition: bool,
) -> Result<(), String> {
    let monitor = live_control::SignalStopMonitor::start()?;
    let stop = monitor.stop_token();
    let stdout = io::stdout();
    let mut output = BufWriter::new(stdout.lock());
    let mut emit = |emission: LiveSessionEmission| {
        let started = std::time::Instant::now();
        write_ndjson(&mut output, &emission.value)?;
        Ok(capture_live::LiveEventProcessingTiming {
            screen_resolver_us: None,
            attempt_resolver_us: None,
            output_us: Some(u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)),
        })
    };
    let legacy_capture = capture_live::RuntimeCaptureInput::LegacyGamescope {
        binding_path: Path::new(&values[0]),
        expected_binding_sha256: values[1]
            .to_str()
            .ok_or_else(|| "binding digest must be UTF-8".to_owned())?,
        expected_source_node_id: None,
    };
    let report = execute_live_session(
        values,
        bundle_root,
        persist_recognition,
        canonical_recording::RecordingMemoryLimit::default_limit(),
        canonical_recording::RecordingRetention::Selective,
        None,
        None,
        None,
        legacy_capture,
        &stop,
        &mut emit,
    )?;
    write_ndjson(&mut output, &report)?;
    report.succeeded().then_some(()).ok_or_else(|| {
        report
            .failure_detail()
            .unwrap_or("Gamescope live recognition session failed")
            .to_owned()
    })
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn execute_live_session(
    values: &[&OsStr],
    bundle_root: &Path,
    persist_recognition: bool,
    recording_memory_limit: canonical_recording::RecordingMemoryLimit,
    recording_retention: canonical_recording::RecordingRetention,
    canonical_recording_root: Option<&Path>,
    session_id: Option<&str>,
    expected_source_node_id: Option<u32>,
    runtime_capture: capture_live::RuntimeCaptureInput<'_>,
    stop: &std::sync::atomic::AtomicBool,
    emit: &mut impl FnMut(
        LiveSessionEmission,
    ) -> Result<capture_live::LiveEventProcessingTiming, String>,
) -> Result<capture_live::GamescopeFieldObservationGateReport, String> {
    let [
        binding,
        binding_digest,
        generation,
        diagnostic_root,
        catalog_root,
        run_id,
        build_digest,
        layout_digest,
        catalog_digest,
        recording,
        recognition_artifact_root,
    ] = values
    else {
        unreachable!("live session flag parser returns the exact value count");
    };
    let binding_digest = parse_cli_sha256(binding_digest, "binding SHA-256")?;
    let generation = parse_capture_generation(generation)?;
    let descriptor = DiagnosticRunDescriptor {
        run_id: parse_diagnostic_run_id(run_id)?,
        monotonic_start_ms: 0,
        resource: DiagnosticResource {
            program: "scorepeek",
            version: env!("CARGO_PKG_VERSION"),
            build_sha256: parse_cli_sha256(build_digest, "build SHA-256")?,
        },
        binding: DiagnosticBinding {
            capture_generation: generation.get(),
            capture_profile_sha256: String::new(),
            normalizer_sha256: String::new(),
            canonical_layout_sha256: parse_cli_sha256(layout_digest, "canonical layout SHA-256")?,
            catalog_sha256: parse_cli_sha256(catalog_digest, "catalog SHA-256")?,
            model_sha256: recognition::LIVE_MODEL_SHA256.to_owned(),
            runtime_sha256: recognition::LIVE_RUNTIME_SHA256.to_owned(),
            replay: None,
        },
    };
    let policy = parse_diagnostic_recording_policy(recording)?;
    let diagnostic_preflight = prepare_live_diagnostic_root(Path::new(diagnostic_root), &policy);
    if session_id.is_none() {
        emit(LiveSessionEmission {
            public_binding: None,
            value: serde_json::to_value(&diagnostic_preflight)
                .map_err(|error| format!("live result serialization failed: {error}"))?,
            authority_joint_evidence: None,
            diagnostic_identity: None,
            diagnostic_capture_fact: None,
        })?;
    }
    let public_binding = descriptor.binding.clone();
    let report = capture_live::run_runtime_live_session(
        capture_live::GamescopeFieldObservationGateConfig {
            handoff: capture_live::GamescopeDiagnosticHandoffGateConfig {
                binding_path: Path::new(binding),
                expected_binding_sha256: &binding_digest,
                capture_generation: generation,
                descriptor,
                policy,
                duration_ms: 0,
                diagnostic_root: Path::new(diagnostic_root),
                diagnostic_directory_name: session_id.map(|_| "capture"),
                expected_source_node_id,
            },
            catalog_root: Path::new(catalog_root),
            bundle_root,
            recognition_artifact_root: optional_recognition_root(
                persist_recognition,
                Path::new(recognition_artifact_root),
            ),
            canonical_recording_root,
            recognition_artifact_retention:
                recognition_artifact::RecognitionArtifactRetention::Complete,
            recording_memory_limit,
            recording_retention,
            runtime_capture,
        },
        stop,
        &mut |event| {
            let started = std::time::Instant::now();
            let diagnostic_identity = match event {
                capture_live::GamescopeLiveSessionEvent::Started {
                    capture_generation,
                    capture_profile_sha256,
                    normalizer_artifact_sha256,
                    capture_profile_document,
                    normalizer_document,
                } => Some(serde_json::json!({
                    "capture_generation": capture_generation,
                    "capture_profile_sha256": capture_profile_sha256,
                    "capture_profile": capture_profile_document,
                    "normalizer_sha256": normalizer_artifact_sha256,
                    "normalizer": normalizer_document,
                })),
                _ => None,
            };
            let diagnostic_capture_fact = match event {
                capture_live::GamescopeLiveSessionEvent::CaptureDiagnostic { fact } => {
                    Some(serde_json::to_value(fact).map_err(|error| {
                        format!("capture diagnostic serialization failed: {error}")
                    })?)
                }
                _ => None,
            };
            let authority_joint_evidence = if session_id.is_some() {
                match &event {
                    capture_live::GamescopeLiveSessionEvent::Observation { output, .. } => {
                        Some(output.joint_evidence().clone())
                    }
                    _ => None,
                }
            } else {
                None
            };
            let binding = match event {
                capture_live::GamescopeLiveSessionEvent::Started {
                    capture_profile_sha256,
                    normalizer_artifact_sha256,
                    ..
                } => Some(crate::events::snapshot::Binding {
                    capture_profile: capture_profile_sha256.to_owned(),
                    normalizer: normalizer_artifact_sha256.to_owned(),
                    canonical_layout: public_binding.canonical_layout_sha256.clone(),
                    catalog: public_binding.catalog_sha256.clone(),
                    model: public_binding.model_sha256.clone(),
                    runtime: public_binding.runtime_sha256.clone(),
                }),
                _ => None,
            };
            let value =
                live_session_event_value(session_id, session_id.map(|_| generation.get()), event)?;
            let serialization_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            let mut timing = emit(LiveSessionEmission {
                public_binding: binding,
                value,
                authority_joint_evidence,
                diagnostic_identity,
                diagnostic_capture_fact,
            })?;
            timing.add(capture_live::LiveEventProcessingTiming {
                screen_resolver_us: None,
                attempt_resolver_us: None,
                output_us: Some(serialization_us),
            });
            Ok(timing)
        },
    );
    Ok(report)
}

struct LiveSessionEmission {
    public_binding: Option<crate::events::snapshot::Binding>,
    value: serde_json::Value,
    authority_joint_evidence: Option<scorepeek_core::recognition::JointEvidenceObservation>,
    diagnostic_identity: Option<serde_json::Value>,
    diagnostic_capture_fact: Option<serde_json::Value>,
}

fn run_event_from_live_emission(emission: LiveSessionEmission) -> Result<RunEvent, String> {
    let mut event = RunEvent::from_value(emission.value)?;
    if let Some(authority_joint_evidence) = emission.authority_joint_evidence {
        let RunEventKind::FieldObservation { joint_evidence, .. } = &mut event.kind else {
            return Err("full joint evidence was attached to a non-field event".to_owned());
        };
        *joint_evidence = authority_joint_evidence;
    }
    Ok(event)
}

fn optional_recognition_root(enabled: bool, root: &Path) -> Option<&Path> {
    enabled.then_some(root)
}

fn current_executable_sha256() -> Result<String, String> {
    let mut file = File::open("/proc/self/exe")
        .map_err(|error| format!("current scorepeek executable could not be opened: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("current scorepeek executable could not be read: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(encoded)
}

#[derive(Serialize)]
struct LiveDiagnosticPreflight<'a> {
    schema: &'static str,
    event: &'static str,
    status: &'static str,
    root: &'a Path,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_type: Option<&'static str>,
}

fn prepare_live_diagnostic_root<'a>(
    root: &'a Path,
    policy: &diagnostic_recording::DiagnosticPolicy,
) -> LiveDiagnosticPreflight<'a> {
    let ready = if policy.enabled {
        Some(prepare_private_directory(root))
    } else {
        None
    };
    let (status, error_type) = match ready {
        None => ("disabled", None),
        Some(true) => ("ready", None),
        Some(false) => ("degraded", Some("store_unavailable")),
    };
    LiveDiagnosticPreflight {
        schema: "scorepeek-live-session-event-v1",
        event: "diagnostic_status",
        status,
        root,
        error_type,
    }
}

fn prepare_private_directory(path: &Path) -> bool {
    match path.metadata() {
        Ok(metadata) => path.is_absolute() && metadata.is_dir(),
        Err(error) if error.kind() == io::ErrorKind::NotFound && path.is_absolute() => {
            let Some(parent) = path.parent() else {
                return false;
            };
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            builder.create(path).is_ok()
                && File::open(parent)
                    .and_then(|directory| directory.sync_all())
                    .is_ok()
        }
        Err(_) => false,
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the serializer keeps the complete versioned live event mapping together"
)]
fn live_session_event_value(
    session_id: Option<&str>,
    routine_generation: Option<u64>,
    event: capture_live::GamescopeLiveSessionEvent<'_>,
) -> Result<serde_json::Value, String> {
    let schema = if session_id.is_some() {
        RUN_EVENT_SCHEMA
    } else {
        "scorepeek-live-session-event-v1"
    };
    let value = match event {
        capture_live::GamescopeLiveSessionEvent::Started {
            capture_generation,
            capture_profile_sha256,
            normalizer_artifact_sha256,
            ..
        } => {
            let mut value = serde_json::json!({
                "schema": schema,
                "event": "session_started",
                "capture_generation": capture_generation,
                "capture_profile_sha256": capture_profile_sha256,
                "normalizer_artifact_sha256": normalizer_artifact_sha256,
            });
            if let Some(session_id) = session_id {
                value["session_id"] = session_id.into();
            }
            if let Some(capture_generation) = routine_generation {
                value["capture_generation"] = capture_generation.into();
            }
            value
        }
        capture_live::GamescopeLiveSessionEvent::RecordingHealth { snapshot } => {
            let mut value = serde_json::json!({
                "schema": schema,
                "event": "recording_health_changed",
                "state": snapshot.state,
                "memory_limit_bytes": snapshot.memory_limit_bytes,
                "memory_used_bytes": snapshot.memory_used_bytes,
                "memory_high_water_bytes": snapshot.memory_high_water_bytes,
                "dropped_frames": snapshot.dropped_frames,
            });
            if let Some(session_id) = session_id {
                value["session_id"] = session_id.into();
            }
            if let Some(capture_generation) = routine_generation {
                value["capture_generation"] = capture_generation.into();
            }
            value
        }
        capture_live::GamescopeLiveSessionEvent::RecordingFinalizing => {
            let mut value = serde_json::json!({
                "schema": schema,
                "event": "recording_finalizing",
            });
            if let Some(session_id) = session_id {
                value["session_id"] = session_id.into();
            }
            if let Some(capture_generation) = routine_generation {
                value["capture_generation"] = capture_generation.into();
            }
            value
        }
        capture_live::GamescopeLiveSessionEvent::CaptureDiagnostic { fact } => {
            let mut value = serde_json::json!({
                "schema": CAPTURE_DIAGNOSTIC_SCHEMA,
                "event": "capture_diagnostic",
                "fact": fact,
            });
            if let Some(session_id) = session_id {
                value["session_id"] = session_id.into();
            }
            if let Some(capture_generation) = routine_generation {
                value["capture_generation"] = capture_generation.into();
            }
            value
        }
        capture_live::GamescopeLiveSessionEvent::RawScreenObserved {
            semantic_episode_id,
            sequence,
            monotonic_start_ms,
            monotonic_end_ms,
            screen,
            result_presence,
            play_presence,
        } => {
            let mut value = serde_json::json!({
                "schema": schema,
                "event": "raw_screen_observed",
                "semantic_episode_id": semantic_episode_id,
                "sequence": sequence,
                "monotonic_start_ms": monotonic_start_ms,
                "monotonic_end_ms": monotonic_end_ms,
                "screen": screen,
                "result_presence": result_presence,
                "play_presence": play_presence,
                "unknown_reason": (screen == scorepeek_core::recognition::ScreenClass::Unknown)
                    .then_some("predicate_not_matched"),
            });
            if let Some(session_id) = session_id {
                value["session_id"] = session_id.into();
                value["capture_generation"] = routine_generation.into();
            }
            value
        }
        capture_live::GamescopeLiveSessionEvent::SemanticScreenEpisode {
            screen_episode_id,
            sequence,
            monotonic_end_ms,
            screen,
            phase,
        } => {
            let mut value = serde_json::json!({
                "schema": schema,
                "event": "semantic_screen_episode_changed",
                "screen_episode_id": screen_episode_id,
                "sequence": sequence,
                "monotonic_end_ms": monotonic_end_ms,
                "screen": screen,
                "phase": phase,
            });
            if let Some(session_id) = session_id {
                value["session_id"] = session_id.into();
                value["capture_generation"] = routine_generation.into();
            }
            value
        }
        capture_live::GamescopeLiveSessionEvent::GameVersionIdentified {
            source_sequence,
            version,
        } => {
            let mut value = serde_json::json!({
                "schema": schema,
                "event": "game_version_changed",
                "source_sequence": source_sequence,
                "version": version,
            });
            if let Some(session_id) = session_id {
                value["session_id"] = session_id.into();
                value["capture_generation"] = routine_generation.into();
            }
            value
        }
        capture_live::GamescopeLiveSessionEvent::Observation {
            screen_episode_id,
            sequence,
            monotonic_start_ms,
            monotonic_end_ms,
            output: observation,
        } => {
            let (screen, fields) = match observation.fields() {
                scorepeek_core::recognition::ScreenFieldObservations::Title(fields) => (
                    "title",
                    serde_json::json!({
                        "game_version": fields.game_version.open_text,
                    }),
                ),
                scorepeek_core::recognition::ScreenFieldObservations::Result(fields) => (
                    "result",
                    serde_json::json!({
                        "panel_side": fields.panel_side,
                        "title": fields.title.open_text,
                        "artist": fields.artist.open_text,
                        "clear_type": observation.clear_type(),
                        "clear_type_ocr": fields.clear_type.open_text,
                        "difficulty": fields.difficulty.open_text,
                        "play_type": fields.play_type.open_text,
                        "level": fields.level.open_text,
                        "notes": fields.notes.open_text,
                        "current_score": fields.current_score.open_text,
                        "previous_clear_type": fields.previous_clear_type.open_text,
                        "previous_score": fields.previous_score.open_text,
                        "previous_miss_count": fields.previous_miss_count.open_text,
                        "miss_count": fields.miss_count.open_text,
                        "pgreat": fields.pgreat.open_text,
                        "great": fields.great.open_text,
                        "good": fields.good.open_text,
                        "bad": fields.bad.open_text,
                        "poor": fields.poor.open_text,
                        "fast": fields.fast.open_text,
                        "slow": fields.slow.open_text,
                        "combo_break": fields.combo_break.open_text,
                        "play_options": fields.play_options,
                    }),
                ),
                scorepeek_core::recognition::ScreenFieldObservations::MusicSelect(fields) => (
                    "music_select",
                    serde_json::json!({
                        "best": fields.best,
                        "play_type": fields.play_type,
                        "central_title": fields.central_title.open_text,
                        "artist": fields.artist.open_text,
                        "selected_difficulty": fields.selected_difficulty,
                        "play_side": fields.play_side,
                        "active_list_title": fields.active_list_title.open_text,
                        "title_evidence": observation.title_evidence(),
                    }),
                ),
            };
            let mut value = serde_json::json!({
                "schema": schema,
                "event": "field_observation",
                "screen_episode_id": screen_episode_id,
                "sequence": sequence,
                "monotonic_start_ms": monotonic_start_ms,
                "monotonic_end_ms": monotonic_end_ms,
                "screen": screen,
                "fields": fields,
                "result_song_resolution": observation.result_resolution(),
                "music_select_song_resolution": observation.music_select_resolution(),
                "parsed_result_fields": observation.parsed_result_fields(),
                "result_chart_resolution": observation.result_chart_resolution(),
                "result_performance_resolution": observation.result_performance_resolution(),
                "current_score_ocr_resolution": observation.current_score_ocr_resolution(),
                "numeric_batch": observation.numeric_batch(),
                "joint_evidence": observation.joint_evidence().diagnostic_top(),
                "processing_timing": observation.processing_timing(),
            });
            if let Some(session_id) = session_id {
                value["session_id"] = session_id.into();
                value["capture_generation"] = routine_generation.into();
            }
            value["song_resolution_presentation"] =
                serde_json::to_value(song_resolution_presentation(observation)?)
                    .map_err(|error| format!("song presentation serialization failed: {error}"))?;
            value
        }
    };
    Ok(value)
}

fn song_resolution_presentation(
    observation: &recognition_live::screen_field_observer::RegisteredScreenFieldObservation,
) -> Result<scorepeek_core::event::SongResolutionPresentation, String> {
    use scorepeek_core::recognition::{MusicSelectSongResolution, ResultSongResolution};

    match observation.song_resolution() {
        scorepeek_core::recognition::ScreenSongResolution::Title => {
            Ok(scorepeek_core::event::SongResolutionPresentation::Unknown {
                reason: serde_json::Value::String("not_applicable".to_owned()),
                selected: None,
                runner_up: None,
                evidence_summary: None,
            })
        }
        scorepeek_core::recognition::ScreenSongResolution::Result(resolution) => match resolution {
            ResultSongResolution::Accepted {
                selected,
                runner_up,
                title_edit_margin,
                ..
            } => Ok(scorepeek_core::event::SongResolutionPresentation::Accepted {
                reason: None,
                selected: song_presentation(observation, selected.song_id)?,
                runner_up: song_presentation(observation, runner_up.song_id)?,
                evidence_summary: format!(
                    "title edit={} similarity={}/{}; artist similarity={}/{}; runner-up margin={}",
                    selected.title.minimum_edit_distance,
                    selected.title.maximum_normalized_similarity.matching_units,
                    selected.title.maximum_normalized_similarity.compared_units,
                    selected.artist.maximum_normalized_similarity.matching_units,
                    selected.artist.maximum_normalized_similarity.compared_units,
                    title_edit_margin,
                ),
            }),
            ResultSongResolution::Unknown {
                reason,
                selected,
                runner_up,
                title_edit_margin,
                ..
            } => Ok(scorepeek_core::event::SongResolutionPresentation::Unknown {
                reason: serde_json::to_value(reason).map_err(|error| format!("result resolution reason serialization failed: {error}"))?,
                selected: selected.as_ref().map(|candidate| song_presentation(observation, candidate.song_id)).transpose()?,
                runner_up: runner_up.as_ref().map(|candidate| song_presentation(observation, candidate.song_id)).transpose()?,
                evidence_summary: selected.as_ref().map(|candidate| format!(
                    "title edit={} similarity={}/{}; artist similarity={}/{}; runner-up margin={}",
                    candidate.title.minimum_edit_distance,
                    candidate.title.maximum_normalized_similarity.matching_units,
                    candidate.title.maximum_normalized_similarity.compared_units,
                    candidate.artist.maximum_normalized_similarity.matching_units,
                    candidate.artist.maximum_normalized_similarity.compared_units,
                    title_edit_margin.map_or_else(|| "-".to_owned(), |margin| margin.to_string()),
                )),
            }),
        },
        scorepeek_core::recognition::ScreenSongResolution::MusicSelect(resolution) => match resolution {
            MusicSelectSongResolution::Accepted {
                selected,
                runner_up,
                active_prefix_edit_margin,
                corroboration,
                ..
            } => Ok(scorepeek_core::event::SongResolutionPresentation::Accepted {
                reason: None,
                selected: song_presentation(observation, selected.song_id)?,
                runner_up: song_presentation(observation, runner_up.song_id)?,
                evidence_summary: format!(
                    "active-prefix edit={} similarity={}/{}; runner-up margin={}; corroboration central-title={} artist={}",
                    selected.active_list_title_prefix.minimum_edit_distance,
                    selected.active_list_title_prefix.maximum_normalized_similarity.matching_units,
                    selected.active_list_title_prefix.maximum_normalized_similarity.compared_units,
                    active_prefix_edit_margin,
                    corroboration.central_title,
                    corroboration.artist,
                ),
            }),
            MusicSelectSongResolution::Unknown {
                reason,
                selected,
                runner_up,
                active_prefix_edit_margin,
                ..
            } => Ok(scorepeek_core::event::SongResolutionPresentation::Unknown {
                reason: serde_json::to_value(reason).map_err(|error| format!("music-select resolution reason serialization failed: {error}"))?,
                selected: selected.as_ref().map(|candidate| song_presentation(observation, candidate.song_id)).transpose()?,
                runner_up: runner_up.as_ref().map(|candidate| song_presentation(observation, candidate.song_id)).transpose()?,
                evidence_summary: selected.as_ref().map(|candidate| format!(
                    "active-prefix edit={} similarity={}/{}; runner-up margin={}",
                    candidate.active_list_title_prefix.minimum_edit_distance,
                    candidate.active_list_title_prefix.maximum_normalized_similarity.matching_units,
                    candidate.active_list_title_prefix.maximum_normalized_similarity.compared_units,
                    active_prefix_edit_margin.map_or_else(|| "-".to_owned(), |margin| margin.to_string()),
                )),
            }),
        },
    }
}

fn song_presentation(
    observation: &recognition_live::screen_field_observer::RegisteredScreenFieldObservation,
    song_id: scorepeek::catalog::ScorepeekSongId,
) -> Result<scorepeek_core::event::SongPresentation, String> {
    let evidence = observation
        .candidates()
        .catalog_evidence()
        .songs
        .iter()
        .find(|song| song.song_id == song_id)
        .ok_or_else(|| {
            format!("resolved song {song_id:?} is absent from the session catalog evidence")
        })?;
    let artists = &evidence.artist.display;
    let [artist] = artists.as_slice() else {
        return Err(format!(
            "resolved song {song_id:?} does not have exactly one display artist"
        ));
    };
    Ok(scorepeek_core::event::SongPresentation {
        scorepeek_song_id: song_id,
        display_titles: evidence.title.display.clone(),
        artist: artist.clone(),
    })
}

#[cfg(test)]
fn write_ndjson(output: &mut impl io::Write, value: &impl Serialize) -> Result<(), String> {
    serde_json::to_writer(&mut *output, value)
        .map_err(|error| format!("live result serialization failed: {error}"))?;
    output
        .write_all(b"\n")
        .and_then(|()| output.flush())
        .map_err(|error| format!("live result output failed: {error}"))
}

#[cfg(test)]
fn run_capture_handoff(values: &[&OsStr], inspect_screen: bool) -> Result<(), String> {
    let [
        binding,
        binding_digest,
        generation,
        duration,
        diagnostic_root,
        run_id,
        build_digest,
        layout_digest,
        catalog_digest,
        recording,
    ] = values
    else {
        unreachable!("capture flag parser returns the exact value count");
    };
    let binding_digest = parse_cli_sha256(binding_digest, "binding SHA-256")?;
    let generation = parse_capture_generation(generation)?;
    let duration_ms = capture_live::parse_duration_ms(duration)?;
    let run_id = parse_diagnostic_run_id(run_id)?;
    let policy = parse_diagnostic_recording_policy(recording)?;
    let descriptor = DiagnosticRunDescriptor {
        run_id,
        monotonic_start_ms: 0,
        resource: DiagnosticResource {
            program: "scorepeek",
            version: env!("CARGO_PKG_VERSION"),
            build_sha256: parse_cli_sha256(build_digest, "build SHA-256")?,
        },
        binding: DiagnosticBinding {
            capture_generation: generation.get(),
            capture_profile_sha256: String::new(),
            normalizer_sha256: String::new(),
            canonical_layout_sha256: parse_cli_sha256(layout_digest, "canonical layout SHA-256")?,
            catalog_sha256: parse_cli_sha256(catalog_digest, "catalog SHA-256")?,
            model_sha256: recognition::LIVE_MODEL_SHA256.to_owned(),
            runtime_sha256: recognition::LIVE_RUNTIME_SHA256.to_owned(),
            replay: None,
        },
    };
    let config = capture_live::GamescopeDiagnosticHandoffGateConfig {
        binding_path: Path::new(binding),
        expected_binding_sha256: &binding_digest,
        capture_generation: generation,
        descriptor,
        policy,
        duration_ms,
        diagnostic_root: Path::new(diagnostic_root),
        diagnostic_directory_name: None,
        expected_source_node_id: None,
    };
    if inspect_screen {
        let report = capture_live::run_gamescope_recognition_handoff_gate(config);
        print_capture_handoff_report(
            &report,
            report.succeeded(),
            "Gamescope recognition handoff gate failed",
        )
    } else {
        let report = capture_live::run_gamescope_diagnostic_handoff_gate(config);
        print_capture_handoff_report(
            &report,
            report.succeeded(),
            "Gamescope diagnostic handoff gate failed",
        )
    }
}

#[cfg(test)]
fn run_capture_field_observation(
    values: &[&OsStr],
    bundle_root: &Path,
    recognition_artifact_root: Option<&Path>,
) -> Result<(), String> {
    let [
        binding,
        binding_digest,
        generation,
        duration,
        diagnostic_root,
        catalog_root,
        run_id,
        build_digest,
        layout_digest,
        catalog_digest,
        recording,
    ] = values
    else {
        unreachable!("capture flag parser returns the exact value count");
    };
    let binding_digest = parse_cli_sha256(binding_digest, "binding SHA-256")?;
    let generation = parse_capture_generation(generation)?;
    let duration_ms = capture_live::parse_duration_ms(duration)?;
    let run_id = parse_diagnostic_run_id(run_id)?;
    let policy = parse_diagnostic_recording_policy(recording)?;
    let descriptor = DiagnosticRunDescriptor {
        run_id,
        monotonic_start_ms: 0,
        resource: DiagnosticResource {
            program: "scorepeek",
            version: env!("CARGO_PKG_VERSION"),
            build_sha256: parse_cli_sha256(build_digest, "build SHA-256")?,
        },
        binding: DiagnosticBinding {
            capture_generation: generation.get(),
            capture_profile_sha256: String::new(),
            normalizer_sha256: String::new(),
            canonical_layout_sha256: parse_cli_sha256(layout_digest, "canonical layout SHA-256")?,
            catalog_sha256: parse_cli_sha256(catalog_digest, "catalog SHA-256")?,
            model_sha256: recognition::LIVE_MODEL_SHA256.to_owned(),
            runtime_sha256: recognition::LIVE_RUNTIME_SHA256.to_owned(),
            replay: None,
        },
    };
    let handoff = capture_live::GamescopeDiagnosticHandoffGateConfig {
        binding_path: Path::new(binding),
        expected_binding_sha256: &binding_digest,
        capture_generation: generation,
        descriptor,
        policy,
        duration_ms,
        diagnostic_root: Path::new(diagnostic_root),
        diagnostic_directory_name: None,
        expected_source_node_id: None,
    };
    let report = capture_live::run_gamescope_field_observation_gate(
        capture_live::GamescopeFieldObservationGateConfig {
            handoff,
            catalog_root: Path::new(catalog_root),
            bundle_root,
            recognition_artifact_root,
            canonical_recording_root: None,
            recognition_artifact_retention:
                recognition_artifact::RecognitionArtifactRetention::Complete,
            recording_memory_limit: canonical_recording::RecordingMemoryLimit::default_limit(),
            recording_retention: canonical_recording::RecordingRetention::Selective,
            runtime_capture: capture_live::RuntimeCaptureInput::LegacyGamescope {
                binding_path: Path::new(binding),
                expected_binding_sha256: &binding_digest,
                expected_source_node_id: None,
            },
        },
    );
    println!(
        "{}",
        serde_json::to_string(&report)
            .map_err(|_| "capture handoff gate report serialization failed".to_owned())?
    );
    report.succeeded().then_some(()).ok_or_else(|| {
        report
            .failure_detail()
            .unwrap_or("Gamescope field observation or recognition artifact gate failed")
            .to_owned()
    })
}

#[cfg(test)]
fn print_capture_handoff_report(
    report: &impl Serialize,
    succeeded: bool,
    failure: &str,
) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(report)
            .map_err(|_| "capture handoff gate report serialization failed".to_owned())?
    );
    succeeded.then_some(()).ok_or_else(|| failure.to_owned())
}

fn parse_cli_sha256(value: &OsStr, label: &str) -> Result<String, String> {
    let value = value
        .to_str()
        .ok_or_else(|| format!("{label} must be UTF-8"))?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(format!("{label} must be lowercase hexadecimal"));
    }
    Ok(value.to_owned())
}

fn parse_capture_generation(
    value: &OsStr,
) -> Result<scorepeek::capture::CaptureGeneration, String> {
    let generation = value
        .to_str()
        .ok_or_else(|| "capture generation must be UTF-8".to_owned())?
        .parse::<u64>()
        .map_err(|_| "capture generation must be an integer".to_owned())?;
    scorepeek::capture::CaptureGeneration::new(generation)
        .map_err(|_| "capture generation must be nonzero".to_owned())
}

fn parse_diagnostic_run_id(value: &OsStr) -> Result<String, String> {
    let value = value
        .to_str()
        .ok_or_else(|| "diagnostic run ID must be UTF-8".to_owned())?;
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(
            "diagnostic run ID must be 1-64 lowercase ASCII letters, digits, or hyphens".to_owned(),
        );
    }
    Ok(value.to_owned())
}

fn parse_diagnostic_recording_policy(
    value: &OsStr,
) -> Result<diagnostic_recording::DiagnosticPolicy, String> {
    match value.to_str() {
        Some("enabled") => Ok(diagnostic_recording::DiagnosticPolicy {
            retention: diagnostic_recording::DiagnosticRetention::FactsOnly,
            ..diagnostic_recording::DiagnosticPolicy::default()
        }),
        Some("disabled") => Ok(diagnostic_recording::DiagnosticPolicy {
            enabled: false,
            ..diagnostic_recording::DiagnosticPolicy::default()
        }),
        _ => Err("recording must be enabled or disabled".to_owned()),
    }
}

#[cfg(test)]
fn try_capture_canonical_frame(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        capture,
        command,
        binding_flag,
        binding,
        binding_digest_flag,
        binding_digest,
        generation_flag,
        generation,
    ] = args
    else {
        return None;
    };
    (capture == "capture"
        && command == "gamescope-canonical-frame-gate"
        && binding_flag == "--binding"
        && binding_digest_flag == "--binding-sha256"
        && generation_flag == "--capture-generation")
        .then(|| {
            let expected_digest = binding_digest
                .to_str()
                .ok_or_else(|| "binding digest must be UTF-8".to_owned())?;
            let generation = generation
                .to_str()
                .ok_or_else(|| "capture generation must be UTF-8".to_owned())?
                .parse::<u64>()
                .map_err(|_| "capture generation must be an integer".to_owned())?;
            let generation = scorepeek::capture::CaptureGeneration::new(generation)
                .map_err(|_| "capture generation must be nonzero".to_owned())?;
            let report = capture_live::run_gamescope_canonical_frame_gate(
                Path::new(binding),
                expected_digest,
                generation,
            );
            println!(
                "{}",
                serde_json::to_string(&report)
                    .map_err(|_| "canonical frame gate report serialization failed".to_owned())?
            );
            report
                .succeeded()
                .then_some(())
                .ok_or_else(|| "Gamescope canonical frame gate failed".to_owned())
        })
}

#[cfg(test)]
fn try_capture_binding_admission(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        capture,
        command,
        binding_flag,
        binding,
        binding_digest_flag,
        binding_digest,
    ] = args
    else {
        return None;
    };
    (capture == "capture"
        && command == "gamescope-binding-admission-gate"
        && binding_flag == "--binding"
        && binding_digest_flag == "--binding-sha256")
        .then(|| {
            let expected_digest = binding_digest
                .to_str()
                .ok_or_else(|| "binding digest must be UTF-8".to_owned())?;
            let report = capture_live::run_gamescope_binding_admission_gate(
                Path::new(binding),
                expected_digest,
            );
            println!(
                "{}",
                serde_json::to_string(&report)
                    .map_err(|_| "binding admission report serialization failed".to_owned())?
            );
            report
                .succeeded()
                .then_some(())
                .ok_or_else(|| "Gamescope profile binding admission failed".to_owned())
        })
}

#[cfg(test)]
fn try_capture_live_gate(args: &[OsString]) -> Option<Result<(), String>> {
    match args {
        [capture, command, duration_flag, duration]
            if capture == "capture"
                && command == "gamescope-live-gate"
                && duration_flag == "--duration-ms" =>
        {
            Some((|| {
                let duration_ms = capture_live::parse_duration_ms(duration)?;
                let report = capture_live::run_gamescope_live_gate(duration_ms);
                print_capture_gate_report(&report)
            })())
        }
        [
            capture,
            command,
            duration_flag,
            duration,
            interval_flag,
            interval,
        ] if capture == "capture"
            && command == "gamescope-live-gate"
            && duration_flag == "--duration-ms"
            && interval_flag == "--consume-interval-ms" =>
        {
            Some((|| {
                let duration_ms = capture_live::parse_duration_ms(duration)?;
                let consumer_interval_ms = capture_live::parse_consumer_interval_ms(interval)?;
                let report = capture_live::run_gamescope_live_gate_with_interval(
                    duration_ms,
                    consumer_interval_ms,
                );
                print_capture_gate_report(&report)
            })())
        }
        [
            capture,
            command,
            duration_flag,
            duration,
            runs_flag,
            runs,
            interval_flag,
            interval,
        ] if capture == "capture"
            && command == "gamescope-lifecycle-gate"
            && duration_flag == "--duration-ms"
            && runs_flag == "--runs"
            && interval_flag == "--consume-interval-ms" =>
        {
            Some((|| {
                let duration_ms = capture_live::parse_duration_ms(duration)?;
                let runs = capture_live::parse_lifecycle_runs(runs)?;
                let consumer_interval_ms = capture_live::parse_consumer_interval_ms(interval)?;
                let report = capture_live::run_gamescope_lifecycle_gate(
                    duration_ms,
                    runs,
                    consumer_interval_ms,
                );
                println!(
                    "{}",
                    serde_json::to_string(&report).map_err(|_| {
                        "Gamescope lifecycle gate report serialization failed".to_owned()
                    })?
                );
                report
                    .succeeded()
                    .then_some(())
                    .ok_or_else(|| "Gamescope lifecycle capture gate failed".to_owned())
            })())
        }
        _ => None,
    }
}

#[cfg(test)]
fn print_capture_gate_report(report: &capture_live::GamescopeLiveGateReport) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(&report)
            .map_err(|_| "capture live gate report serialization failed".to_owned())?
    );
    report
        .succeeded()
        .then_some(())
        .ok_or_else(|| "Gamescope live capture gate failed".to_owned())
}

fn try_program_information(args: &[OsString]) -> Option<Result<(), String>> {
    match args {
        [flag] if flag == "--help" || flag == "-h" => {
            print_usage();
            Some(Ok(()))
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("scorepeek {}", env!("CARGO_PKG_VERSION"));
            Some(Ok(()))
        }
        _ => None,
    }
}

fn try_offline_program_information(args: &[OsString]) -> Option<Result<(), String>> {
    match args {
        [flag] if flag == "--help" => {
            print_usage();
            Some(Ok(()))
        }
        [flag] if flag == "--version" => {
            println!("scorepeek {}", env!("CARGO_PKG_VERSION"));
            Some(Ok(()))
        }
        _ => None,
    }
}

fn try_doctor(args: &[OsString]) -> Option<Result<(), String>> {
    matches!(args, [command] if command == "doctor").then(|| print_doctor(OutputFormat::Json))
}

fn collect_doctor_report() -> Result<serde_json::Value, String> {
    let target_inventory: serde_json::Value = serde_json::from_str(&inventory::collect().to_json())
        .map_err(|error| format!("doctor report serialization failed: {error}"))?;
    let numeric_model = match recognition::RegisteredNumericRuntime::load_embedded() {
        Ok(runtime) => serde_json::json!({
            "status": "active",
            "model_id": runtime.contract().model_id,
            "model_sha256": runtime.contract().model_sha256,
            "manifest_sha256": recognition::NUMERIC_MODEL_MANIFEST_SHA256,
            "preprocessor_id": runtime.contract().preprocessor_id,
        }),
        Err(error) => serde_json::json!({
            "status": "unavailable",
            "reason": error.to_string(),
            "registered_manifest_sha256": recognition::NUMERIC_MODEL_MANIFEST_SHA256,
        }),
    };
    let catalog = catalog_paths(
        env::var_os("XDG_DATA_HOME").as_deref(),
        env::var_os("XDG_CACHE_HOME").as_deref(),
        env::var_os("HOME").as_deref(),
    )
    .and_then(|(store_root, _)| {
        let active = CatalogStore::new(&store_root)
            .load_active_for_run()
            .map_err(|error| error.to_string())?;
        let state = scorepeek::catalog::update::load_state(&store_root)
            .map_err(|error| error.to_string())?;
        Ok(serde_json::json!({
            "status": if active.is_some() { "active" } else { "unavailable" },
            "active_catalog_sha256": active.as_ref().map(|catalog| catalog.digest.as_str()),
            "source_url_sha256": state.source_url_sha256,
            "last_success_unix_seconds": state.last_success_unix_seconds,
            "etag_present": state.etag.is_some(),
            "last_modified_present": state.last_modified.is_some(),
            "last_failure": state.last_failure,
        }))
    })
    .unwrap_or_else(|error| serde_json::json!({"status": "unavailable", "reason": error}));
    let vulkan_layer = serde_json::to_value(vulkan_layer::inspect())
        .map_err(|error| format!("doctor report serialization failed: {error}"))?;
    Ok(serde_json::json!({
        "schema": "scorepeek-doctor-v5",
        "target_inventory": target_inventory,
        "numeric_model": numeric_model,
        "catalog": catalog,
        "vulkan_layer": vulkan_layer,
    }))
}

fn print_doctor(format: OutputFormat) -> Result<(), String> {
    let report = collect_doctor_report()?;
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string(&report)
                .map_err(|error| format!("doctor report serialization failed: {error}"))?
        ),
        OutputFormat::Human => {
            println!("scorepeek doctor");
            println!(
                "  numeric model: {} ({})",
                report["numeric_model"]["status"]
                    .as_str()
                    .unwrap_or("unknown"),
                report["numeric_model"]["model_id"]
                    .as_str()
                    .unwrap_or("not available")
            );
            println!(
                "  catalog: {}",
                report["catalog"]["status"].as_str().unwrap_or("unknown")
            );
            println!(
                "  capture inventory: {}",
                if report["target_inventory"].is_object() {
                    "available"
                } else {
                    "unavailable"
                }
            );
            println!(
                "  Vulkan layer: {}",
                report["vulkan_layer"]["status"]
                    .as_str()
                    .unwrap_or("unknown")
            );
        }
    }
    Ok(())
}

fn try_recording_simulation(args: &[OsString], bundle: &Path) -> Option<Result<(), String>> {
    try_recording_simulation_profile_author(args)
        .or_else(|| try_recording_recognition_evidence_run(args, bundle))
        .or_else(|| try_recording_simulation_run(args, bundle))
}

fn try_recording_recognition_evidence_run(
    args: &[OsString],
    bundle: &Path,
) -> Option<Result<(), String>> {
    let [
        recognition,
        simulate,
        profile_flag,
        profile,
        profile_digest_flag,
        profile_digest,
        extraction_flag,
        extraction,
        diagnostic_root_flag,
        diagnostic_root,
        catalog_store_flag,
        catalog_store,
        run_id_flag,
        run_id,
        build_digest_flag,
        build_digest,
        recording_flag,
        recording,
        artifact_flag,
        artifact,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && (simulate == "recording-recognition-evidence"
            || simulate == "recording-recognition-simulation")
        && profile_flag == "--profile"
        && profile_digest_flag == "--profile-sha256"
        && extraction_flag == "--extraction"
        && diagnostic_root_flag == "--diagnostic-root"
        && catalog_store_flag == "--catalog-store"
        && run_id_flag == "--run-id"
        && build_digest_flag == "--build-sha256"
        && recording_flag == "--recording"
        && artifact_flag == "--recognition-artifact")
        .then(|| {
            execute_recording_simulation(
                profile,
                profile_digest,
                extraction,
                diagnostic_root,
                catalog_store,
                bundle,
                run_id,
                build_digest,
                recording,
                Some(Path::new(artifact)),
                simulate == "recording-recognition-simulation",
            )
        })
}

fn try_recording_simulation_profile_author(args: &[OsString]) -> Option<Result<(), String>> {
    if let [
        recognition,
        author,
        candidate_flag,
        candidate,
        candidate_digest_flag,
        candidate_digest,
        recording_manifest_flag,
        recording_manifest,
        coverage_label_flag,
        coverage_label,
        extraction_flag,
        extraction,
        output_flag,
        output,
    ] = args
        && recognition == "recognition"
        && author == "recording-simulation-profile-author"
        && candidate_flag == "--candidate"
        && candidate_digest_flag == "--candidate-sha256"
        && recording_manifest_flag == "--recording-manifest"
        && coverage_label_flag == "--coverage-label"
        && extraction_flag == "--extraction"
        && output_flag == "--output"
    {
        return Some((|| {
            let candidate_digest = parse_cli_sha256(candidate_digest, "candidate SHA-256")?;
            let profile_digest = recording_simulation::author_recording_simulation_profile(
                Path::new(candidate),
                &candidate_digest,
                Path::new(recording_manifest),
                Path::new(extraction),
                Path::new(coverage_label),
                Path::new(output),
            )?;
            println!(
                "{}",
                serde_json::json!({
                    "schema": "scorepeek-recording-field-simulation-profile-author-report-v1",
                    "status": "success",
                    "profile_sha256": profile_digest,
                })
            );
            Ok(())
        })());
    }

    None
}

fn try_recording_simulation_run(args: &[OsString], bundle: &Path) -> Option<Result<(), String>> {
    let [
        recognition,
        simulate,
        profile_flag,
        profile,
        profile_digest_flag,
        profile_digest,
        extraction_flag,
        extraction,
        diagnostic_root_flag,
        diagnostic_root,
        catalog_store_flag,
        catalog_store,
        run_id_flag,
        run_id,
        build_digest_flag,
        build_digest,
        recording_flag,
        recording,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && simulate == "recording-simulation"
        && profile_flag == "--profile"
        && profile_digest_flag == "--profile-sha256"
        && extraction_flag == "--extraction"
        && diagnostic_root_flag == "--diagnostic-root"
        && catalog_store_flag == "--catalog-store"
        && run_id_flag == "--run-id"
        && build_digest_flag == "--build-sha256"
        && recording_flag == "--recording")
        .then(|| {
            execute_recording_simulation(
                profile,
                profile_digest,
                extraction,
                diagnostic_root,
                catalog_store,
                bundle,
                run_id,
                build_digest,
                recording,
                None,
                false,
            )
        })
}

#[allow(clippy::too_many_arguments)]
fn execute_recording_simulation(
    profile: &OsStr,
    profile_digest: &OsStr,
    extraction: &OsStr,
    diagnostic_root: &OsStr,
    catalog_store: &OsStr,
    bundle: &Path,
    run_id: &OsStr,
    build_digest: &OsStr,
    recording: &OsStr,
    recognition_artifact_root: Option<&Path>,
    require_song_resolution: bool,
) -> Result<(), String> {
    let profile_digest = parse_cli_sha256(profile_digest, "profile SHA-256")?;
    let report = recording_simulation::run_recording_simulation(
        recording_simulation::RecordingSimulationRunConfig {
            profile_path: Path::new(profile),
            expected_profile_sha256: &profile_digest,
            extraction_directory: Path::new(extraction),
            diagnostic_root: Path::new(diagnostic_root),
            catalog_root: Path::new(catalog_store),
            bundle_root: bundle,
            run_id: parse_diagnostic_run_id(run_id)?,
            build_sha256: parse_cli_sha256(build_digest, "build SHA-256")?,
            policy: parse_diagnostic_recording_policy(recording)?,
            recognition_artifact_root,
            require_song_resolution,
        },
    );
    let succeeded = report.succeeded();
    println!(
        "{}",
        serde_json::to_string(&report)
            .map_err(|_| "recording simulation report serialization failed".to_owned())?
    );
    succeeded
        .then_some(())
        .ok_or_else(|| "recording field simulation failed".to_owned())
}

fn try_integrated_context_crop(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        recognition,
        crop,
        extraction_flag,
        extraction,
        digest_flag,
        digest,
        frame_flag,
        frame_id,
        output_flag,
        output,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && crop == "integrated-context-crop"
        && extraction_flag == "--extraction"
        && digest_flag == "--extraction-sha256"
        && frame_flag == "--frame-id"
        && output_flag == "--output")
        .then(|| crop_integrated_context(extraction, digest, frame_id, output))
}

fn try_dynamic_official_onnx_decode(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        recognition,
        decode,
        model_id_flag,
        model_id,
        bundle_flag,
        bundle,
        request_flag,
        request,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && decode == "title-official-dynamic-onnx-decode"
        && model_id_flag == "--model-id"
        && bundle_flag == "--bundle"
        && request_flag == "--request")
        .then(|| {
            let summary = recognition::decode_dynamic_official_onnx_crops(
                &model_id.to_string_lossy(),
                Path::new(bundle),
                Path::new(request),
            )
            .map_err(|error| error.to_string())?;
            println!(
                "{}",
                serde_json::to_string(&summary).map_err(|error| format!(
                    "dynamic official ONNX decode summary failed: {error}"
                ))?
            );
            Ok(())
        })
}

fn try_integrated_context_observe(args: &[OsString], bundle: &Path) -> Option<Result<(), String>> {
    let [
        recognition,
        observe,
        crops_flag,
        crops,
        digest_flag,
        digest,
        output_flag,
        output,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && observe == "integrated-context-observe"
        && crops_flag == "--crop-artifact"
        && digest_flag == "--crop-artifact-sha256"
        && output_flag == "--output")
        .then(|| {
            let digest = digest
                .to_str()
                .ok_or_else(|| "crop artifact SHA-256 must be UTF-8".to_owned())?;
            let summary = recognition::observe_integrated_context(
                Path::new(crops),
                digest,
                recognition::LIVE_MODEL_ID,
                bundle,
                Path::new(output),
            )
            .map_err(|error| error.to_string())?;
            println!(
                "{}",
                serde_json::to_string(&summary).map_err(|error| format!(
                    "integrated context observation summary failed: {error}"
                ))?
            );
            Ok(())
        })
}

fn try_official_onnx_decode(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        recognition,
        decode,
        model_flag,
        model,
        dictionary_flag,
        dictionary,
        request_flag,
        request,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && decode == "title-official-onnx-decode"
        && model_flag == "--model"
        && dictionary_flag == "--dictionary"
        && request_flag == "--request")
        .then(|| {
            let summary = recognition::decode_official_onnx_crops(
                Path::new(model),
                Path::new(dictionary),
                Path::new(request),
            )
            .map_err(|error| error.to_string())?;
            println!(
                "{}",
                serde_json::to_string(&summary)
                    .map_err(|error| format!("official ONNX decode summary failed: {error}"))?
            );
            Ok(())
        })
}

fn try_provisional_title_candidates(args: &[OsString]) -> Option<Result<(), String>> {
    let [recognition, export, store_flag, store, output_flag, output] = args else {
        return None;
    };
    (recognition == "recognition"
        && export == "provisional-title-candidates"
        && store_flag == "--catalog-store"
        && output_flag == "--output")
        .then(|| provisional_title_candidates(store, output))
}

fn try_title_model_contract_parity(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        recognition,
        parity,
        model_flag,
        model,
        model_digest_flag,
        model_digest,
        reference_flag,
        reference,
        reference_digest_flag,
        reference_digest,
        dictionary_flag,
        dictionary,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && parity == "title-model-contract-parity"
        && model_flag == "--model"
        && model_digest_flag == "--model-sha256"
        && reference_flag == "--reference"
        && reference_digest_flag == "--reference-sha256"
        && dictionary_flag == "--dictionary")
        .then(|| {
            title_model_contract_parity([
                model,
                model_digest,
                reference,
                reference_digest,
                dictionary,
            ])
        })
}

fn try_title_model_export_requirements(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        recognition,
        export,
        store_flag,
        store,
        dictionary_flag,
        dictionary,
        output_flag,
        output,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && export == "title-model-export-requirements"
        && store_flag == "--catalog-store"
        && dictionary_flag == "--baseline-dictionary"
        && output_flag == "--output")
        .then(|| title_model_export_requirements(store, dictionary, output))
}

fn try_title_dictionary_audit(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        recognition,
        audit,
        store_flag,
        store,
        dictionary_flag,
        dictionary,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && audit == "title-dictionary-audit"
        && store_flag == "--catalog-store"
        && dictionary_flag == "--dictionary")
        .then(|| title_dictionary_audit(store, dictionary))
}

fn try_title_onnx_parity(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        recognition,
        parity,
        model_flag,
        model,
        reference_flag,
        reference,
        digest_flag,
        digest,
        crop_flag,
        crop,
        store_flag,
        store,
        dictionary_flag,
        dictionary,
        minimum_score_flag,
        minimum_score,
        minimum_margin_flag,
        minimum_margin,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && parity == "title-onnx-parity"
        && model_flag == "--model"
        && reference_flag == "--reference"
        && digest_flag == "--reference-sha256"
        && crop_flag == "--crop-artifact"
        && store_flag == "--catalog-store"
        && dictionary_flag == "--dictionary"
        && minimum_score_flag == "--minimum-log-probability"
        && minimum_margin_flag == "--minimum-runner-up-margin")
        .then(|| {
            title_onnx_parity([
                model,
                reference,
                digest,
                crop,
                store,
                dictionary,
                minimum_score,
                minimum_margin,
            ])
        })
}

fn inspect_canonical_frame(
    extraction: &OsStr,
    extraction_sha256: &OsStr,
    frame_id: &OsStr,
) -> Result<(), String> {
    let extraction_sha256 = extraction_sha256
        .to_str()
        .ok_or_else(|| "canonical extraction SHA-256 must be UTF-8".to_owned())?;
    let frame_id = frame_id
        .to_str()
        .ok_or_else(|| "canonical frame ID must be UTF-8".to_owned())?;
    let frame = CanonicalFrame::read_extraction(extraction, frame_id, extraction_sha256)
        .map_err(|error| error.to_string())?;
    let snapshot = recognition::inspect(&frame).map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&snapshot)
            .map_err(|error| format!("recognition result encoding failed: {error}"))?
    );
    Ok(())
}

fn crop_canonical_result(
    extraction: &OsStr,
    extraction_sha256: &OsStr,
    frame_id: &OsStr,
    output: &OsStr,
) -> Result<(), String> {
    let extraction_sha256 = extraction_sha256
        .to_str()
        .ok_or_else(|| "canonical extraction SHA-256 must be UTF-8".to_owned())?;
    let frame_id = frame_id
        .to_str()
        .ok_or_else(|| "canonical frame ID must be UTF-8".to_owned())?;
    let frame = CanonicalFrame::read_extraction(extraction, frame_id, extraction_sha256)
        .map_err(|error| error.to_string())?;
    let summary = recognition::export_result_crops(&frame, frame_id, output)
        .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("crop export summary encoding failed: {error}"))?
    );
    Ok(())
}

fn crop_canonical_music_select(
    extraction: &OsStr,
    extraction_sha256: &OsStr,
    frame_id: &OsStr,
    output: &OsStr,
) -> Result<(), String> {
    let extraction_sha256 = extraction_sha256
        .to_str()
        .ok_or_else(|| "canonical extraction SHA-256 must be UTF-8".to_owned())?;
    let frame_id = frame_id
        .to_str()
        .ok_or_else(|| "canonical frame ID must be UTF-8".to_owned())?;
    let frame = CanonicalFrame::read_extraction(extraction, frame_id, extraction_sha256)
        .map_err(|error| error.to_string())?;
    let summary = recognition::export_music_select_crops(&frame, frame_id, output)
        .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("crop export summary encoding failed: {error}"))?
    );
    Ok(())
}

fn crop_integrated_context(
    extraction: &OsStr,
    extraction_sha256: &OsStr,
    frame_id: &OsStr,
    output: &OsStr,
) -> Result<(), String> {
    let extraction_sha256 = extraction_sha256
        .to_str()
        .ok_or_else(|| "canonical extraction SHA-256 must be UTF-8".to_owned())?;
    let frame_id = frame_id
        .to_str()
        .ok_or_else(|| "canonical frame ID must be UTF-8".to_owned())?;
    let frame = CanonicalFrame::read_extraction(extraction, frame_id, extraction_sha256)
        .map_err(|error| error.to_string())?;
    let summary = recognition::export_integrated_context_crops(&frame, frame_id, output)
        .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("crop export summary encoding failed: {error}"))?
    );
    Ok(())
}

#[derive(Serialize)]
struct DiagnosticTitleSpikeSummary {
    schema: &'static str,
    catalog_sha256: String,
    comparison_key_id: &'static str,
    minimum_confidence: f64,
    candidate: recognition::DiagnosticTitleCandidate,
}

#[derive(Serialize)]
struct ProvisionalTitleCandidatesArtifact {
    schema: &'static str,
    catalog_sha256: String,
    #[serde(flatten)]
    candidates: recognition::ProvisionalTitleCandidateSet,
}

#[derive(Serialize)]
struct ProvisionalTitleCandidatesSummary {
    schema: &'static str,
    output: PathBuf,
    artifact_sha256: String,
    catalog_sha256: String,
    candidate_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrivatePublicationPoint {
    FileSynced,
    Linked,
    StagingRemoved,
}

fn publish_private_file(output: &Path, bytes: &[u8]) -> std::io::Result<()> {
    publish_private_file_with(output, bytes, |_| Ok(()))
}

fn publish_private_file_with(
    output: &Path,
    bytes: &[u8],
    mut checkpoint: impl FnMut(PrivatePublicationPoint) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let parent = output.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "output has no parent")
    })?;
    let mut staging = tempfile::Builder::new()
        .prefix(".scorepeek-private-staging-")
        .tempfile_in(parent)?;
    staging.as_file_mut().write_all(bytes)?;
    staging.as_file_mut().sync_all()?;
    checkpoint(PrivatePublicationPoint::FileSynced)?;

    let staging_path = staging.path().to_owned();
    let mut linked = false;
    let publication = (|| {
        fs::hard_link(&staging_path, output)?;
        linked = true;
        checkpoint(PrivatePublicationPoint::Linked)?;
        fs::remove_file(&staging_path)?;
        checkpoint(PrivatePublicationPoint::StagingRemoved)?;
        fs::File::open(parent)?.sync_all()
    })();
    if let Err(error) = publication {
        if linked {
            let _ = fs::remove_file(output);
        }
        let _ = fs::remove_file(&staging_path);
        let _ = fs::File::open(parent).and_then(|directory| directory.sync_all());
        return Err(error);
    }
    Ok(())
}

fn provisional_title_candidates(catalog_store: &OsStr, output: &OsStr) -> Result<(), String> {
    let catalog_store = absolute_directory(PathBuf::from(catalog_store), "catalog store")?;
    let output = PathBuf::from(output);
    if !output.is_absolute() || output.as_os_str().is_empty() {
        return Err("provisional title candidate output must be an absolute path".to_owned());
    }
    let parent = output
        .parent()
        .ok_or_else(|| "provisional title candidate output must have a parent".to_owned())?;
    let metadata = parent.metadata().map_err(|error| {
        format!("provisional title candidate output parent inspection failed: {error}")
    })?;
    if !metadata.is_dir() {
        return Err(
            "provisional title candidate output parent must be a regular directory".to_owned(),
        );
    }
    let active = CatalogStore::new(catalog_store)
        .load_active()
        .map_err(|error| format!("active catalog load failed: {error}"))?
        .ok_or_else(|| "catalog store has no active catalog".to_owned())?;
    let candidates = recognition::provisional_title_candidates(&active.catalog);
    let candidate_count = candidates.candidates.len();
    let artifact = ProvisionalTitleCandidatesArtifact {
        schema: "scorepeek-private-provisional-title-candidates-v1",
        catalog_sha256: active.digest.clone(),
        candidates,
    };
    let mut bytes = serde_json::to_vec(&artifact)
        .map_err(|error| format!("provisional title candidate encoding failed: {error}"))?;
    bytes.push(b'\n');
    publish_private_file(&output, &bytes)
        .map_err(|error| format!("provisional title candidate publication failed: {error}"))?;
    let summary = ProvisionalTitleCandidatesSummary {
        schema: "scorepeek-private-provisional-title-candidates-summary-v1",
        output,
        artifact_sha256: encode_sha256(&bytes),
        catalog_sha256: active.digest,
        candidate_count,
    };
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("provisional title candidate summary failed: {error}"))?
    );
    Ok(())
}

fn diagnostic_title_spike(
    catalog_store: &OsStr,
    ocr_text: &OsStr,
    ocr_confidence: &OsStr,
) -> Result<(), String> {
    let catalog_store = absolute_directory(PathBuf::from(catalog_store), "catalog store")?;
    let ocr_text = ocr_text
        .to_str()
        .ok_or_else(|| "diagnostic OCR text must be UTF-8".to_owned())?;
    let ocr_confidence = ocr_confidence
        .to_str()
        .ok_or_else(|| "diagnostic OCR confidence must be UTF-8".to_owned())?
        .parse::<f64>()
        .map_err(|_| "diagnostic OCR confidence must be a decimal number".to_owned())?;
    let active = CatalogStore::new(catalog_store)
        .load_active()
        .map_err(|error| format!("active catalog load failed: {error}"))?
        .ok_or_else(|| "catalog store has no active catalog".to_owned())?;
    let candidate =
        recognition::diagnostic_title_candidate(&active.catalog, ocr_text, ocr_confidence)
            .map_err(|error| error.to_string())?;
    let summary = DiagnosticTitleSpikeSummary {
        schema: "scorepeek-diagnostic-title-spike-v1",
        catalog_sha256: active.digest,
        comparison_key_id: DIAGNOSTIC_TITLE_COMPARISON_KEY_ID,
        minimum_confidence: DIAGNOSTIC_TITLE_MINIMUM_CONFIDENCE,
        candidate,
    };
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("title spike result encoding failed: {error}"))?
    );
    Ok(())
}

#[derive(Serialize)]
struct TitleDictionaryAuditSummary {
    schema: &'static str,
    catalog_sha256: String,
    audit: recognition::CatalogTitleDictionaryAudit,
}

fn title_dictionary_audit(catalog_store: &OsStr, dictionary: &OsStr) -> Result<(), String> {
    let catalog_store = absolute_directory(PathBuf::from(catalog_store), "catalog store")?;
    let active = CatalogStore::new(catalog_store)
        .load_active()
        .map_err(|error| format!("active catalog load failed: {error}"))?
        .ok_or_else(|| "catalog store has no active catalog".to_owned())?;
    let audit = recognition::audit_catalog_title_dictionary(&active.catalog, Path::new(dictionary))
        .map_err(|error| error.to_string())?;
    let summary = TitleDictionaryAuditSummary {
        schema: "scorepeek-catalog-title-dictionary-audit-v1",
        catalog_sha256: active.digest,
        audit,
    };
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("title dictionary audit encoding failed: {error}"))?
    );
    Ok(())
}

#[derive(Serialize)]
struct TitleModelExportRequirementsArtifact {
    schema: &'static str,
    catalog_sha256: String,
    requirements: recognition::TitleModelExportRequirements,
}

#[derive(Serialize)]
struct TitleModelExportRequirementsSummary {
    schema: &'static str,
    output: PathBuf,
    manifest_sha256: String,
    catalog_sha256: String,
    output_timesteps: usize,
    output_classes: usize,
    non_search_variant_count: usize,
}

fn title_model_export_requirements(
    catalog_store: &OsStr,
    dictionary: &OsStr,
    output: &OsStr,
) -> Result<(), String> {
    let catalog_store = absolute_directory(PathBuf::from(catalog_store), "catalog store")?;
    let output = absolute_directory(PathBuf::from(output), "model export requirements output")?;
    let parent = output
        .parent()
        .ok_or_else(|| "model export requirements output must have a parent".to_owned())?;
    let parent_metadata = parent
        .metadata()
        .map_err(|error| format!("model export requirements parent inspection failed: {error}"))?;
    if !parent_metadata.is_dir() {
        return Err("model export requirements parent must be a regular directory".to_owned());
    }
    let active = CatalogStore::new(catalog_store)
        .load_active()
        .map_err(|error| format!("active catalog load failed: {error}"))?
        .ok_or_else(|| "catalog store has no active catalog".to_owned())?;
    let requirements =
        recognition::title_model_export_requirements(&active.catalog, Path::new(dictionary))
            .map_err(|error| error.to_string())?;
    let summary = TitleModelExportRequirementsSummary {
        schema: "scorepeek-title-model-export-requirements-summary-v1",
        output: output.clone(),
        manifest_sha256: String::new(),
        catalog_sha256: active.digest.clone(),
        output_timesteps: requirements.output_timesteps,
        output_classes: requirements.output_classes,
        non_search_variant_count: requirements.non_search_variant_count,
    };
    let artifact = TitleModelExportRequirementsArtifact {
        schema: "scorepeek-private-title-model-export-requirements-v1",
        catalog_sha256: active.digest,
        requirements,
    };
    let mut bytes = serde_json::to_vec(&artifact)
        .map_err(|error| format!("model export requirements encoding failed: {error}"))?;
    bytes.push(b'\n');
    DirBuilder::new()
        .mode(0o700)
        .create(&output)
        .map_err(|error| format!("model export requirements output creation failed: {error}"))?;
    let publication = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(output.join("manifest.json"))
            .map_err(|error| format!("model export requirements publication failed: {error}"))?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("model export requirements publication failed: {error}"))?;
        fs::File::open(&output)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("model export requirements sync failed: {error}"))
    })();
    if let Err(error) = publication {
        fs::remove_dir_all(&output)
            .map_err(|cleanup| format!("{error}; failed to remove incomplete output: {cleanup}"))?;
        return Err(error);
    }
    let summary = TitleModelExportRequirementsSummary {
        manifest_sha256: encode_sha256(&bytes),
        ..summary
    };
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("model export requirements summary failed: {error}"))?
    );
    Ok(())
}

fn encode_sha256(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn title_onnx_parity(arguments: [&OsStr; 8]) -> Result<(), String> {
    let [
        model,
        reference,
        reference_sha256,
        crop,
        catalog_store,
        dictionary,
        minimum_log_probability,
        minimum_runner_up_margin,
    ] = arguments;
    let reference_sha256 = reference_sha256
        .to_str()
        .ok_or_else(|| "parity reference SHA-256 must be UTF-8".to_owned())?;
    let catalog_store = absolute_directory(PathBuf::from(catalog_store), "catalog store")?;
    let active = CatalogStore::new(catalog_store)
        .load_active()
        .map_err(|error| format!("active catalog load failed: {error}"))?
        .ok_or_else(|| "catalog store has no active catalog".to_owned())?;
    let minimum_log_probability =
        parse_f64(minimum_log_probability, "minimum title log probability")?;
    let minimum_runner_up_margin =
        parse_f64(minimum_runner_up_margin, "minimum title runner-up margin")?;
    let thresholds = recognition::DiagnosticTitleThresholds {
        minimum_log_probability,
        minimum_runner_up_margin,
    };
    let request = recognition::OnnxTitleDiagnosticRequest {
        model_path: Path::new(model),
        reference_directory: Path::new(reference),
        reference_sha256,
        crop_directory: Path::new(crop),
        catalog_sha256: &active.digest,
        inference_yml: Path::new(dictionary),
    };
    let summary = recognition::compare_paddle_onnx(request, &active.catalog, thresholds)
        .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("ONNX parity summary encoding failed: {error}"))?
    );
    Ok(())
}

fn title_model_contract_parity(arguments: [&OsStr; 5]) -> Result<(), String> {
    let [model, model_sha256, reference, reference_sha256, dictionary] = arguments;
    let model_sha256 = model_sha256
        .to_str()
        .ok_or_else(|| "model SHA-256 must be UTF-8".to_owned())?;
    let reference_sha256 = reference_sha256
        .to_str()
        .ok_or_else(|| "parity reference SHA-256 must be UTF-8".to_owned())?;
    let request = recognition::ExportContractParityRequest {
        model_path: Path::new(model),
        model_sha256,
        reference_directory: Path::new(reference),
        reference_sha256,
        inference_yml: Path::new(dictionary),
    };
    let summary =
        recognition::compare_export_contract(request).map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("export contract parity encoding failed: {error}"))?
    );
    Ok(())
}

fn parse_f64(value: &OsStr, label: &str) -> Result<f64, String> {
    value
        .to_str()
        .ok_or_else(|| format!("{label} must be UTF-8"))?
        .parse::<f64>()
        .map_err(|_| format!("{label} must be a decimal number"))
}

fn catalog_paths(
    xdg_data_home: Option<&OsStr>,
    xdg_cache_home: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Result<(PathBuf, PathBuf), String> {
    let data = xdg_base_directory(xdg_data_home, home, ".local/share")?;
    let cache = xdg_base_directory(xdg_cache_home, home, ".cache")?;
    Ok((
        data.join("scorepeek/catalog"),
        cache.join("scorepeek/catalog/sources"),
    ))
}

fn xdg_base_directory(
    configured: Option<&OsStr>,
    home: Option<&OsStr>,
    fallback: &str,
) -> Result<PathBuf, String> {
    if let Some(configured) = configured {
        let path = PathBuf::from(configured);
        return absolute_directory(path, "XDG base directory");
    }
    let home =
        home.ok_or_else(|| "HOME is required when an XDG base directory is unset".to_owned())?;
    let home = absolute_directory(PathBuf::from(home), "HOME")?;
    Ok(home.join(fallback))
}

fn absolute_directory(path: PathBuf, name: &str) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() || !Path::new(&path).is_absolute() {
        return Err(format!("{name} must be an absolute, non-empty path"));
    }
    Ok(path)
}

fn print_usage() {
    println!(
        "scorepeek {}\n\nUsage:\n  scorepeek --help\n  scorepeek --version\n  scorepeek doctor\n  scorepeek vulkan-layer install\n  scorepeek vulkan-layer uninstall\n  scorepeek run --capture vulkan-layer [--crop-left PX] [--crop-top PX] [--crop-right PX] [--crop-bottom PX] [--scores-db PATH | --no-scores] [--overlay-wayland] [--overlay-obs] [--overlay-config PATH] [--record | --record-all] [--record-memory-mib MIB]\n  scorepeek run --capture pipewire --node-name NAME [--crop-left PX] [--crop-top PX] [--crop-right PX] [--crop-bottom PX] [OTHER_OPTIONS...]\n  scorepeek diagnostic inspect --latest\n  scorepeek diagnostic inspect --run-id RUN_ID\n  scorepeek diagnostic observe [--replay SECONDS]\n  scorepeek skin install ZIP\n  scorepeek skin uninstall ID\n  scorepeek skin list\n  scorepeek [--model-bundle DIRECTORY] COMMAND ...",
        env!("CARGO_PKG_VERSION")
    );
    println!(
        "  scorepeek recognition field-resource-load-gate --catalog-store DIRECTORY --catalog-sha256 SHA256"
    );
    println!("  run option: --overlay-wayland-edit (enables Wayland and opens the editor)");
}

#[cfg(test)]
mod tests {
    use super::{
        CAPTURE_FIELD_OBSERVATION_FLAGS, CAPTURE_HANDOFF_FLAGS, CAPTURE_RESULT_RECOGNITION_FLAGS,
        ConfigFile, LIVE_SESSION_FLAGS, PrivatePublicationPoint, RunArgs, catalog_paths,
        command_flag_values, initialize_routine_model, live_session_event_value, load_run_options,
        optional_recognition_root, parse_diagnostic_recording_policy, parse_routine_run_options,
        prepare_live_diagnostic_root, publish_private_file, publish_private_file_with,
        routine_session_disposition, run_command, run_config_command, run_startup_stage,
        run_with_model_initializer, transient_admission_capture_error, validate_config_file,
    };
    use super::{LiveSessionEmission, run_event_from_live_emission};
    use crate::capture_live::GamescopeLiveSessionEvent;
    use crate::recognition_live::screen_field_observer::RegisteredScreenFieldObservation;
    use scorepeek::capture::{
        CaptureDiagnosticDetail, CaptureDiagnosticFact, CaptureDiagnosticOperation,
        CaptureDiagnosticStatus,
    };
    use scorepeek::catalog::Catalog;
    use scorepeek::catalog::{
        Chart, ChartKey, Difficulty, DisplayVariantKind, LineageId, PlayType, RevisionStrategy,
        SourceChartObservation, SourceEvidence, SourceId, SourceObservation, SourcePolicy,
        SourceSnapshot, SourceTitleObservation, TachiObservation,
    };
    use scorepeek_core::catalog::FederationInput;
    use scorepeek_core::event::{RunEvent, RunEventKind};
    use scorepeek_core::recognition::{
        CatalogCandidateDomain, DynamicTextObservation, ResultScreenFieldObservations,
        ScreenFieldObservations,
    };
    use std::cell::Cell;
    use std::collections::BTreeSet;
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::path::{Path, PathBuf};

    fn result_presence(
        panel_side: scorepeek_core::recognition::ResultPanelSideState,
    ) -> scorepeek_core::recognition::ResultPresenceEvidence {
        let known = panel_side.known();
        scorepeek_core::recognition::ResultPresenceEvidence {
            warm_pixels: if known.is_some() { 3_100 } else { 2_900 },
            warm_pixels_min: 3_000,
            panel_side,
            panels: [
                scorepeek_core::recognition::ResultPanelPresenceEvidence {
                    panel_side: scorepeek_core::recognition::ResultPanelSide::Left,
                    upper_panel_edge_pixels: if known
                        == Some(scorepeek_core::recognition::ResultPanelSide::Left)
                    {
                        520
                    } else {
                        0
                    },
                    lower_panel_edge_pixels: if known
                        == Some(scorepeek_core::recognition::ResultPanelSide::Left)
                    {
                        520
                    } else {
                        0
                    },
                    qualifies: known == Some(scorepeek_core::recognition::ResultPanelSide::Left),
                },
                scorepeek_core::recognition::ResultPanelPresenceEvidence {
                    panel_side: scorepeek_core::recognition::ResultPanelSide::Right,
                    upper_panel_edge_pixels: if known
                        == Some(scorepeek_core::recognition::ResultPanelSide::Right)
                    {
                        520
                    } else {
                        0
                    },
                    lower_panel_edge_pixels: if known
                        == Some(scorepeek_core::recognition::ResultPanelSide::Right)
                    {
                        520
                    } else {
                        0
                    },
                    qualifies: known == Some(scorepeek_core::recognition::ResultPanelSide::Right),
                },
            ],
            horizontal_edge_pixels_min: 518,
        }
    }

    fn play_presence() -> scorepeek_core::recognition::PlayPresenceEvidence {
        scorepeek_core::recognition::PlayPresenceEvidence {
            qualifying_candidates: 0,
            top_edge_runs: 0,
            bottom_edge_runs: 0,
            candidates: [None, None],
            top_edge_pixels_min: 280,
            top_edge_pixels_max: 305,
            bottom_edge_pixels_min: 300,
            bottom_edge_pixels_max: 320,
            vertical_distance_min: 59,
            vertical_distance_max: 70,
            edge_center_delta_x2_max: 2,
            candidate_cluster_delta_x2_max: 4,
            candidate_cluster_delta_y_max: 12,
        }
    }

    fn resolved_two_player_result_observation() -> RegisteredScreenFieldObservation {
        let catalog = catalog_from_records(&[
            tachi_record("song-1", "SYNTHETIC SONG", "SYNTHETIC ARTIST"),
            tachi_record("song-2", "DISTANT RUNNER UP", "OTHER ARTIST"),
        ]);
        let domain = CatalogCandidateDomain::from_catalog(&catalog).unwrap();
        let numeric = |value: &str| DynamicTextObservation {
            input_width: 1,
            output_timesteps: 1,
            open_text: value.to_owned(),
            constrained_text: Some(value.to_owned()),
        };
        let observation = RegisteredScreenFieldObservation::from_fields_with_catalog(
            &domain,
            &catalog,
            ScreenFieldObservations::Result(ResultScreenFieldObservations {
                panel_side: scorepeek_core::recognition::ResultPanelSide::Right,
                title: text("SYNTHETIC SONG"),
                artist: text("SYNTHETIC ARTIST"),
                clear_type: text("CLEAR"),
                difficulty: text("NORMAL"),
                play_type: text("SP"),
                level: numeric("1"),
                notes: numeric("1"),
                current_score: numeric("2"),
                previous_clear_type: text("NO PLAY"),
                previous_score: numeric("0"),
                previous_miss_count: numeric("0"),
                miss_count: numeric("0"),
                pgreat: numeric("1"),
                great: numeric("0"),
                good: numeric("0"),
                bad: numeric("0"),
                poor: numeric("0"),
                fast: numeric("0"),
                slow: numeric("0"),
                combo_break: numeric("0"),
                ..Default::default()
            }),
        );
        assert!(observation.result_chart_resolution().is_some());
        assert!(observation.result_performance_resolution().is_some());
        observation
    }

    fn publish_headless_live_event(
        routine: &mut crate::events::server::RoutineOutput,
        event: GamescopeLiveSessionEvent<'_>,
    ) {
        let value = live_session_event_value(Some("invocation-session-1"), Some(1), event).unwrap();
        routine
            .publish(&RunEvent::from_value(value).unwrap())
            .unwrap();
    }

    fn publish_two_player_result_episode(
        routine: &mut crate::events::server::RoutineOutput,
        observation: &RegisteredScreenFieldObservation,
    ) {
        for (screen_episode_id, sequence, screen, phase) in [
            (
                1,
                1,
                scorepeek_core::recognition::ScreenClass::MusicSelect,
                crate::capture_live::SemanticScreenEpisodePhase::Started,
            ),
            (
                1,
                2,
                scorepeek_core::recognition::ScreenClass::MusicSelect,
                crate::capture_live::SemanticScreenEpisodePhase::Finalized,
            ),
            (
                2,
                3,
                scorepeek_core::recognition::ScreenClass::Play,
                crate::capture_live::SemanticScreenEpisodePhase::Started,
            ),
            (
                2,
                4,
                scorepeek_core::recognition::ScreenClass::Play,
                crate::capture_live::SemanticScreenEpisodePhase::Finalized,
            ),
            (
                3,
                5,
                scorepeek_core::recognition::ScreenClass::Result,
                crate::capture_live::SemanticScreenEpisodePhase::Started,
            ),
        ] {
            publish_headless_live_event(
                routine,
                GamescopeLiveSessionEvent::SemanticScreenEpisode {
                    screen_episode_id,
                    sequence,
                    monotonic_end_ms: sequence * 100,
                    screen,
                    phase,
                },
            );
        }
        for sequence in [6, 7] {
            publish_headless_live_event(
                routine,
                GamescopeLiveSessionEvent::RawScreenObserved {
                    semantic_episode_id: Some(3),
                    sequence,
                    monotonic_start_ms: sequence * 100,
                    monotonic_end_ms: sequence * 100 + 25,
                    screen: scorepeek_core::recognition::ScreenClass::Result,
                    result_presence: result_presence(
                        scorepeek_core::recognition::ResultPanelSideState::Known(
                            scorepeek_core::recognition::ResultPanelSide::Right,
                        ),
                    ),
                    play_presence: play_presence(),
                },
            );
        }
        for sequence in [8, 9] {
            publish_headless_live_event(
                routine,
                GamescopeLiveSessionEvent::Observation {
                    screen_episode_id: 3,
                    sequence,
                    monotonic_start_ms: sequence * 100,
                    monotonic_end_ms: sequence * 100 + 25,
                    output: observation,
                },
            );
        }
    }

    #[test]
    fn config_uses_top_level_sections_and_rejects_cross_backend_values() {
        let config: ConfigFile = toml::from_str(
            r#"
[capture]
backend = "pipewire"
node_name = "gamescope"
[crop]
left = 8
[scores]
enabled = true
[overlay]
obs = true
[recording]
enabled = true
memory_mib = 64
[catalog]
url = "https://example.invalid/catalog.zip"
"#,
        )
        .unwrap();
        validate_config_file(&config).unwrap();

        let partial_pipewire: ConfigFile =
            toml::from_str("[capture]\nbackend = 'pipewire'\n").unwrap();
        validate_config_file(&partial_pipewire).unwrap();

        let invalid: ConfigFile = toml::from_str(
            r#"
[capture]
backend = "vulkan-layer"
node_name = "must-not-be-inherited"
"#,
        )
        .unwrap();
        assert!(validate_config_file(&invalid).is_err());
        assert!(toml::from_str::<ConfigFile>("[run]\ncapture = 'pipewire'\n").is_err());
    }

    #[test]
    fn config_wayland_editor_enables_the_wayland_overlay() {
        let config: ConfigFile =
            toml::from_str("[capture]\nbackend = 'vulkan-layer'\n[overlay]\nwayland_edit = true\n")
                .unwrap();
        let options = super::merge_run_options(config, RunArgs::default()).unwrap();
        assert!(options.overlays.wayland);
        assert!(options.overlays.wayland_edit);
    }

    #[test]
    fn environment_can_disable_configured_recording_and_its_memory_limit() {
        use std::process::Command;

        const CHILD: &str = "SCOREPEEK_RECORDING_ENV_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let config: ConfigFile = toml::from_str(
                "[capture]\nbackend = 'vulkan-layer'\n[recording]\nenabled = true\nmemory_mib = 64\n",
            )
            .unwrap();
            let options = super::merge_run_options(config, RunArgs::default()).unwrap();
            assert!(!options.recording);
            return;
        }

        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "application::tests::environment_can_disable_configured_recording_and_its_memory_limit",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("SCOREPEEK_RECORDING_ENABLED", "false")
            .env_remove("SCOREPEEK_RECORDING_MEMORY_MIB")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn run_diagnostics_start_before_config_loading_and_keep_the_failure_stage() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("invalid.toml");
        let diagnostic_store = root.path().join("diagnostics");
        fs::write(&config, "[capture\n").unwrap();
        let run_id = "run-2-0-1";
        let mut diagnostics =
            crate::diagnostics::inspect::RunDiagnostics::start(&diagnostic_store, run_id);
        let sink = diagnostics.sink();
        let monitor = crate::platform::signal::SignalStopMonitor::start().unwrap();
        let error = load_run_options(&mut diagnostics, &monitor, &config, RunArgs::default())
            .err()
            .expect("invalid config must fail");
        assert!(error.starts_with("config file is invalid:"));
        drop(sink);
        drop(diagnostics);

        let stream =
            fs::read_to_string(diagnostic_store.join(run_id).join("diagnostics.ndjson")).unwrap();
        assert!(stream.contains("\"stage\":\"config_load\""));
        assert!(stream.contains("\"status\":\"error\""));
        assert!(!stream.contains("model_initialization"));
    }

    #[test]
    fn config_json_rejects_non_utf8_paths_without_panicking() {
        use std::os::unix::ffi::OsStringExt as _;

        let path = PathBuf::from(OsString::from_vec(b"/tmp/scorepeek-\xff".to_vec()));
        let error = run_config_command(
            super::ConfigCommand::Path(super::FormatArgs {
                format: super::OutputFormat::Json,
            }),
            &path,
        )
        .unwrap_err();
        assert_eq!(error, "config path must be UTF-8 for JSON output");
    }

    #[test]
    fn public_run_interrupt_prioritizes_cancel_over_startup_success_and_failure() {
        use std::process::Command;
        use std::time::Duration;

        const CHILD_MODE: &str = "SCOREPEEK_STARTUP_INTERRUPT_TEST_MODE";
        if let Some(mode) = std::env::var_os(CHILD_MODE) {
            let args = RunArgs {
                capture: Some(super::CaptureKind::VulkanLayer),
                no_scores: true,
                ..RunArgs::default()
            };
            let result = super::run_public_with_model_initializer(args, None, |_| {
                signal_hook::low_level::raise(signal_hook::consts::SIGINT).unwrap();
                std::thread::sleep(Duration::from_millis(25));
                if mode == "success" {
                    Ok(PathBuf::from("/unused-after-interrupt"))
                } else {
                    Err("synthetic concurrent model failure".to_owned())
                }
            });
            assert_eq!(result.as_ref().unwrap_err(), super::INTERRUPTED_ERROR);
            assert_eq!(super::result_exit_status(&result), 130);
            return;
        }

        let root = tempfile::tempdir().unwrap();
        for mode in ["success", "failure"] {
            let state = root.path().join(mode).join("state");
            let runtime = root.path().join(mode).join("runtime");
            fs::create_dir_all(&runtime).unwrap();
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "service::dispatch::application::tests::public_run_interrupt_prioritizes_cancel_over_startup_success_and_failure",
                    "--nocapture",
                ])
                .env(CHILD_MODE, mode)
                .env("XDG_STATE_HOME", &state)
                .env("XDG_RUNTIME_DIR", &runtime)
                .env("XDG_CONFIG_HOME", root.path().join(mode).join("config"))
                .env("HOME", root.path().join(mode).join("home"))
                .env_remove("SCOREPEEK_CONFIG")
                .status()
                .unwrap();
            assert!(status.success());
            let store = state.join("scorepeek/diagnostics");
            let run = fs::read_dir(&store)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| path.is_dir())
                .unwrap();
            let stream = fs::read_to_string(run.join("diagnostics.ndjson")).unwrap();
            assert!(stream.contains("\"stage\":\"interrupt\""));
            assert!(stream.contains("\"status\":\"cancel\""));
            assert_eq!(stream.matches("diagnostic_run_finished").count(), 1);
            assert!(!stream.contains("\"stage\":\"catalog_paths\""));
        }
    }

    #[test]
    fn output_owned_diagnostics_prioritize_interrupt_over_startup_failure() {
        use std::process::Command;
        use std::time::Duration;

        const CHILD_ROOT: &str = "SCOREPEEK_OUTPUT_INTERRUPT_TEST_ROOT";
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            let diagnostics = crate::diagnostics::inspect::RunDiagnostics::start(
                Path::new(&root),
                "run-output-interrupt-test",
            );
            let mut output = crate::events::server::RoutineOutput::start_headless_with_diagnostics(
                "invocation-output-interrupt".to_owned(),
                "0".repeat(64),
                diagnostics,
            );
            let monitor = crate::platform::signal::SignalStopMonitor::start().unwrap();
            signal_hook::low_level::raise(signal_hook::consts::SIGINT).unwrap();
            std::thread::sleep(Duration::from_millis(25));
            let result = super::settle_output_startup_result::<()>(
                &mut output,
                &monitor,
                Err("synthetic output startup failure".to_owned()),
            );
            assert_eq!(result.as_ref().unwrap_err(), super::INTERRUPTED_ERROR);
            assert_eq!(super::result_exit_status(&result), 130);
            return;
        }

        let root = tempfile::tempdir().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "service::dispatch::application::tests::output_owned_diagnostics_prioritize_interrupt_over_startup_failure",
                "--nocapture",
            ])
            .env(CHILD_ROOT, root.path())
            .status()
            .unwrap();
        assert!(status.success());
        let stream = fs::read_to_string(
            root.path()
                .join("run-output-interrupt-test")
                .join("diagnostics.ndjson"),
        )
        .unwrap();
        assert!(stream.contains("\"stage\":\"interrupt\""));
        assert!(stream.contains("\"status\":\"cancel\""));
        assert_eq!(stream.matches("diagnostic_run_finished").count(), 1);
    }

    #[test]
    fn capture_diagnostic_events_use_the_stage_timing_schema() {
        let fact = CaptureDiagnosticFact {
            sequence: 1,
            monotonic_start_ms: 2,
            monotonic_end_ms: 3,
            operation: CaptureDiagnosticOperation::ProfileBindingAdmission,
            status: CaptureDiagnosticStatus::Success,
            error_type: None,
            detail: CaptureDiagnosticDetail::ProfileBindingAdmission,
        };
        let value = live_session_event_value(
            Some("session-1"),
            Some(1),
            GamescopeLiveSessionEvent::CaptureDiagnostic { fact: &fact },
        )
        .unwrap();
        assert_eq!(value["schema"], "scorepeek-capture-diagnostic-v2");
        assert_eq!(value["event"], "capture_diagnostic");
    }

    #[test]
    fn failed_startup_stage_is_saved_in_a_zero_session_run() {
        let root = tempfile::tempdir().unwrap();
        let run_id = "run-1-0-1";
        let diagnostics = crate::diagnostics::inspect::RunDiagnostics::start(root.path(), run_id);
        let sink = diagnostics.sink();
        let error = run_startup_stage::<()>(&sink, "synthetic_preflight", || {
            Err("synthetic failure".to_owned())
        })
        .unwrap_err();
        assert_eq!(error, "synthetic failure");
        drop(sink);
        drop(diagnostics);

        let stream =
            fs::read_to_string(root.path().join(run_id).join("diagnostics.ndjson")).unwrap();
        assert!(stream.contains("synthetic_preflight"));
        assert!(stream.contains("synthetic failure"));
        assert!(stream.contains("diagnostic_run_finished"));
        assert!(!stream.contains("session_started"));
    }

    #[test]
    fn failed_model_initialization_is_saved_in_a_zero_session_run() {
        let root = tempfile::tempdir().unwrap();
        let run_id = "run-1-0-2";
        let diagnostics = crate::diagnostics::inspect::RunDiagnostics::start(root.path(), run_id);
        let sink = diagnostics.sink();
        let error =
            initialize_routine_model(&sink, None, |_| Err("synthetic model failure".to_owned()))
                .unwrap_err();
        assert_eq!(error, "synthetic model failure");
        drop(sink);
        drop(diagnostics);

        let stream =
            fs::read_to_string(root.path().join(run_id).join("diagnostics.ndjson")).unwrap();
        assert!(stream.contains("model_initialization"));
        assert!(stream.contains("synthetic model failure"));
        assert!(stream.contains("diagnostic_run_finished"));
        assert!(!stream.contains("session_started"));
    }

    #[test]
    fn scores_options_are_independent_of_recording_and_reject_conflicts() {
        let options =
            ["--capture", "vulkan-layer", "--scores-db", "guest.sqlite3"].map(OsString::from);
        let parsed = parse_routine_run_options(&options).unwrap();
        assert_eq!(parsed.scores_db, Some(PathBuf::from("guest.sqlite3")));
        assert!(!parsed.recording);
        assert!(!parsed.no_scores);
        let base = ["--capture", "vulkan-layer"].map(OsString::from);
        assert!(!parse_routine_run_options(&base).unwrap().no_scores);
        assert!(
            parse_routine_run_options(
                &["--capture", "vulkan-layer", "--no-scores"].map(OsString::from)
            )
            .unwrap()
            .no_scores
        );
        for options in [
            vec!["--scores-db"],
            vec!["--scores-db", ""],
            vec!["--no-scores", "--scores-db", "guest.db"],
            vec!["--no-scores", "--no-scores"],
        ] {
            let options = ["--capture", "vulkan-layer"]
                .into_iter()
                .chain(options)
                .map(OsString::from)
                .collect::<Vec<_>>();
            assert!(parse_routine_run_options(&options).is_err());
        }
    }

    #[test]
    fn unrecorded_run_disables_the_recognition_artifact_root() {
        let root = Path::new("/tmp/recognition");
        assert_eq!(optional_recognition_root(false, root), None);
        assert_eq!(optional_recognition_root(true, root), Some(root));
    }

    #[test]
    fn overlay_options_allow_independent_and_simultaneous_consumers() {
        for options in [
            vec!["--overlay-wayland"],
            vec!["--overlay-wayland-edit"],
            vec!["--overlay-obs"],
            vec![
                "--overlay-wayland",
                "--overlay-obs",
                "--overlay-config",
                "overlay.toml",
                "--no-scores",
            ],
        ] {
            let values = ["--capture", "vulkan-layer"]
                .into_iter()
                .chain(options)
                .map(OsString::from)
                .collect::<Vec<_>>();
            assert!(parse_routine_run_options(&values).is_ok());
        }
        for options in [
            vec!["--overlay-wayland", "--overlay-wayland"],
            vec!["--overlay-obs", "--overlay-obs"],
            vec!["--overlay-config"],
            vec!["--overlay-config", "a", "--overlay-config", "b"],
        ] {
            let values = ["--capture", "vulkan-layer"]
                .into_iter()
                .chain(options)
                .map(OsString::from)
                .collect::<Vec<_>>();
            assert!(parse_routine_run_options(&values).is_err());
        }
        let base = ["--capture", "vulkan-layer"].map(OsString::from);
        let parsed = parse_routine_run_options(&base).unwrap();
        assert!(!parsed.overlays.wayland && !parsed.overlays.obs);
        let edit_args = ["--capture", "vulkan-layer", "--overlay-wayland-edit"].map(OsString::from);
        let edit = parse_routine_run_options(&edit_args).unwrap();
        assert!(edit.overlays.wayland && edit.overlays.wayland_edit);
    }

    #[test]
    fn overlay_config_path_is_preserved() {
        let args = [
            "--capture",
            "vulkan-layer",
            "--overlay-obs",
            "--overlay-config",
            "custom.toml",
        ]
        .map(OsString::from);
        let parsed = parse_routine_run_options(&args).unwrap();
        assert_eq!(
            parsed.overlays.config_path.as_deref(),
            Some(Path::new("custom.toml"))
        );
    }

    #[test]
    fn ordinary_run_options_are_order_independent_and_record_is_opt_in() {
        assert!(parse_routine_run_options(&[]).is_err());
        let base = ["--capture", "vulkan-layer"].map(OsString::from);
        let parsed = parse_routine_run_options(&base).unwrap();
        assert!(!parsed.recording);
        let record = ["--capture", "vulkan-layer", "--record"].map(OsString::from);
        let parsed = parse_routine_run_options(&record).unwrap();
        assert!(parsed.recording);
        assert_eq!(
            parsed.recording_retention,
            crate::recording::writer::RecordingRetention::Selective
        );
        assert_eq!(
            parsed.recording_memory_limit.bytes(),
            1024_u64 * 1024 * 1024
        );
        let configured = [
            OsString::from("--capture"),
            OsString::from("vulkan-layer"),
            OsString::from("--record-memory-mib"),
            OsString::from("2048"),
            OsString::from("--record"),
        ];
        assert_eq!(
            parse_routine_run_options(&configured)
                .unwrap()
                .recording_memory_limit
                .bytes(),
            2048_u64 * 1024 * 1024
        );
        let record_all = ["--capture", "vulkan-layer", "--record-all"].map(OsString::from);
        let parsed = parse_routine_run_options(&record_all).unwrap();
        assert!(parsed.recording);
        assert_eq!(
            parsed.recording_retention,
            crate::recording::writer::RecordingRetention::All
        );
    }

    #[test]
    fn removed_or_duplicate_recording_options_are_rejected() {
        for options in [
            vec![OsString::from("--no-recording")],
            vec![OsString::from("--record-attempts")],
            vec![OsString::from("--record"), OsString::from("--record")],
            vec![OsString::from("--record"), OsString::from("--record-all")],
            vec![
                OsString::from("--record-all"),
                OsString::from("--record-all"),
            ],
            vec![
                OsString::from("--record-memory-mib"),
                OsString::from("1024"),
            ],
        ] {
            assert!(parse_routine_run_options(&options).is_err());
        }
    }

    #[test]
    fn help_version_and_doctor_skip_model_initialization() {
        for args in [["--help"], ["--version"], ["doctor"]] {
            let initialized = Cell::new(false);
            run_with_model_initializer(&args.map(OsString::from), |_| {
                initialized.set(true);
                Err("must not initialize".to_owned())
            })
            .unwrap();
            assert!(!initialized.get());
        }
    }

    #[test]
    fn every_other_command_initializes_before_dispatch() {
        let initialized = Cell::new(false);
        let error = run_with_model_initializer(&[OsString::from("unknown")], |_| {
            initialized.set(true);
            Ok(PathBuf::from("/unused-model-bundle"))
        })
        .unwrap_err();
        assert!(initialized.get());
        assert_eq!(error, "usage: scorepeek --help");
    }

    #[test]
    fn short_information_aliases_initialize_before_dispatch() {
        for flag in ["-h", "-V"] {
            let initialized = Cell::new(false);
            run_with_model_initializer(&[OsString::from(flag)], |_| {
                initialized.set(true);
                Ok(PathBuf::from("/unused-model-bundle"))
            })
            .unwrap();
            assert!(initialized.get());
        }
    }

    #[test]
    fn global_model_bundle_is_forwarded_to_initialization() {
        let selected = Cell::new(false);
        let args = [
            OsString::from("--model-bundle"),
            OsString::from("/development/small"),
            OsString::from("unknown"),
        ];
        let _ = run_with_model_initializer(&args, |bundle| {
            selected.set(bundle == Some(Path::new("/development/small")));
            Ok(PathBuf::from("/development/small"))
        });
        assert!(selected.get());
    }

    #[test]
    fn private_file_publication_is_no_clobber_and_cleans_every_failed_checkpoint() {
        for failed_point in [
            PrivatePublicationPoint::FileSynced,
            PrivatePublicationPoint::Linked,
            PrivatePublicationPoint::StagingRemoved,
        ] {
            let root = tempfile::tempdir().unwrap();
            let output = root.path().join("artifact.json");
            let error = publish_private_file_with(&output, b"complete\n", |point| {
                if point == failed_point {
                    Err(std::io::Error::other("checkpoint failure"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::Other);
            assert!(!output.exists());
            assert_eq!(root.path().read_dir().unwrap().count(), 0);
        }

        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("artifact.json");
        publish_private_file(&output, b"first\n").unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"first\n");
        assert!(publish_private_file(&output, b"second\n").is_err());
        assert_eq!(fs::read(&output).unwrap(), b"first\n");
    }

    #[test]
    fn catalog_paths_use_absolute_xdg_directories() {
        let paths =
            catalog_paths(Some(OsStr::new("/data")), Some(OsStr::new("/cache")), None).unwrap();
        assert_eq!(paths.0, PathBuf::from("/data/scorepeek/catalog"));
        assert_eq!(paths.1, PathBuf::from("/cache/scorepeek/catalog/sources"));
    }

    #[test]
    fn catalog_paths_fall_back_to_home_and_reject_relative_values() {
        let paths = catalog_paths(None, None, Some(OsStr::new("/home/test"))).unwrap();
        assert_eq!(
            paths.0,
            PathBuf::from("/home/test/.local/share/scorepeek/catalog")
        );
        assert_eq!(
            paths.1,
            PathBuf::from("/home/test/.cache/scorepeek/catalog/sources")
        );
        assert!(
            catalog_paths(
                Some(OsStr::new("relative")),
                Some(OsStr::new("/cache")),
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn live_session_command_requires_the_exact_ordered_contract() {
        let mut args = vec!["run".into(), "gamescope".into()];
        for (index, flag) in LIVE_SESSION_FLAGS.iter().enumerate() {
            args.push((*flag).into());
            args.push(format!("value-{index}").into());
        }
        let values = command_flag_values(&args, "run", "gamescope", LIVE_SESSION_FLAGS).unwrap();
        assert_eq!(values.len(), LIVE_SESSION_FLAGS.len());
        assert_eq!(values[0], OsStr::new("value-0"));

        args[0] = "capture".into();
        assert!(command_flag_values(&args, "run", "gamescope", LIVE_SESSION_FLAGS).is_none());
        args[0] = "run".into();
        args.pop();
        assert!(command_flag_values(&args, "run", "gamescope", LIVE_SESSION_FLAGS).is_none());
    }

    #[test]
    fn runtime_gate_contracts_have_no_launch_metadata_arguments() {
        for flags in [
            LIVE_SESSION_FLAGS,
            CAPTURE_HANDOFF_FLAGS,
            CAPTURE_FIELD_OBSERVATION_FLAGS,
            CAPTURE_RESULT_RECOGNITION_FLAGS,
        ] {
            for removed in [
                "--environment-id",
                "--gamescope-version",
                "--backend",
                "--output-width",
                "--output-height",
                "--nested-width",
                "--nested-height",
                "--nested-refresh",
                "--scaler",
                "--filter",
            ] {
                assert!(!flags.contains(&removed));
            }
        }

        let mut args = vec!["capture".into(), "gamescope-field-observation-gate".into()];
        for (index, flag) in CAPTURE_FIELD_OBSERVATION_FLAGS.iter().enumerate() {
            args.push((*flag).into());
            args.push(format!("value-{index}").into());
        }
        assert!(
            command_flag_values(
                &args,
                "capture",
                "gamescope-field-observation-gate",
                CAPTURE_FIELD_OBSERVATION_FLAGS,
            )
            .is_some()
        );
    }

    #[test]
    fn live_session_prepares_an_absent_private_diagnostic_root() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("diagnostics");
        let preflight = prepare_live_diagnostic_root(
            &root,
            &crate::diagnostics::writer::DiagnosticPolicy::default(),
        );
        assert_eq!(preflight.status, "ready");
        assert_eq!(preflight.error_type, None);
        assert!(root.is_dir());
    }

    #[test]
    fn internal_capture_cli_never_enables_runtime_frame_artifacts() {
        let policy = parse_diagnostic_recording_policy(OsStr::new("enabled")).unwrap();
        assert!(policy.enabled);
        assert_eq!(
            policy.retention,
            crate::diagnostics::writer::DiagnosticRetention::FactsOnly
        );
    }

    #[test]
    fn removed_gamescope_capture_commands_are_not_dispatched() {
        for args in [
            vec!["run", "gamescope"],
            vec!["capture", "gamescope-live-gate", "--duration-ms", "100"],
            vec![
                "capture",
                "gamescope-binding-admission-gate",
                "--binding",
                "/tmp/ignored",
                "--binding-sha256",
                "0",
            ],
        ] {
            let args = args.into_iter().map(OsString::from).collect::<Vec<_>>();
            assert!(run_command(&args, Path::new("/tmp/unused-model-bundle")).is_err());
        }
    }

    #[test]
    fn capture_terminal_failure_is_fatal_but_source_endings_are_readmitted() {
        assert_eq!(
            routine_session_disposition(Some(
                crate::capture_live::LiveSessionStopReason::SourceEnded
            )),
            ("source_ended", true, false)
        );
        assert_eq!(
            routine_session_disposition(Some(
                crate::capture_live::LiveSessionStopReason::TerminalFailure
            )),
            ("error", false, true)
        );
        assert_eq!(
            routine_session_disposition(Some(
                crate::capture_live::LiveSessionStopReason::SourceContractChanged
            )),
            ("source_ended", true, false)
        );
    }

    #[test]
    fn only_source_disappearance_is_retried_during_admission() {
        assert!(transient_admission_capture_error(
            scorepeek::capture::CaptureErrorType::SourceLost
        ));
        assert!(transient_admission_capture_error(
            scorepeek::capture::CaptureErrorType::StreamLost
        ));
        assert!(!transient_admission_capture_error(
            scorepeek::capture::CaptureErrorType::UnsupportedFormat
        ));
        assert!(!transient_admission_capture_error(
            scorepeek::capture::CaptureErrorType::FrameNormalizationFailed
        ));
    }

    #[test]
    fn live_serializer_and_reducer_keep_one_recording_schema() {
        use crate::events::server::RoutineOutput;
        let mut output = RoutineOutput::start_headless("invocation".into(), "a".repeat(64));
        for event in [
            GamescopeLiveSessionEvent::Started {
                capture_generation: 1,
                capture_profile_sha256: "profile",
                normalizer_artifact_sha256: "normalizer",
                capture_profile_document: None,
                normalizer_document: None,
            },
            GamescopeLiveSessionEvent::SemanticScreenEpisode {
                screen_episode_id: 1,
                sequence: 1,
                monotonic_end_ms: 100,
                screen: scorepeek_core::recognition::ScreenClass::MusicSelect,
                phase: crate::capture_live::SemanticScreenEpisodePhase::Started,
            },
        ] {
            let value =
                live_session_event_value(Some("invocation-session-1"), Some(1), event).unwrap();
            output
                .publish(&RunEvent::from_value(value).unwrap())
                .unwrap();
        }
        let events = output.take_headless_events();
        assert!(
            events
                .iter()
                .any(|event| matches!(event.kind, RunEventKind::MusicSelectResolverChanged { .. }))
        );
        let schemas: std::collections::BTreeSet<_> =
            events.iter().map(|event| event.schema.as_str()).collect();
        assert_eq!(
            schemas.len(),
            1,
            "the corpus reader rejects mixed-schema sessions"
        );
        assert_eq!(schemas.first().copied(), Some("scorepeek-run-event-v17"));
    }

    #[test]
    fn routine_screen_events_separate_raw_observation_and_semantic_episode() {
        let value = live_session_event_value(
            Some("invocation-session-2"),
            Some(2),
            GamescopeLiveSessionEvent::RawScreenObserved {
                semantic_episode_id: Some(1),
                sequence: 41,
                monotonic_start_ms: 100,
                monotonic_end_ms: 125,
                screen: scorepeek_core::recognition::ScreenClass::Unknown,
                result_presence: result_presence(
                    scorepeek_core::recognition::ResultPanelSideState::Unknown(
                        scorepeek_core::recognition::ResultPanelSideUnknownReason::NoCandidate,
                    ),
                ),
                play_presence: play_presence(),
            },
        )
        .unwrap();
        assert_eq!(value["schema"], "scorepeek-run-event-v17");
        assert_eq!(value["event"], "raw_screen_observed");
        assert_eq!(value["semantic_episode_id"], 1);
        assert_eq!(value["session_id"], "invocation-session-2");
        assert_eq!(value["capture_generation"], 2);
        assert_eq!(value["sequence"], 41);
        assert_eq!(value["screen"], "unknown");
        assert_eq!(value["result_presence"]["warm_pixels"], 2_900);
        assert_eq!(value["play_presence"]["qualifying_candidates"], 0);
        assert_eq!(
            value["result_presence"]["panel_side"]["value"],
            "no_candidate"
        );

        let mode = live_session_event_value(
            Some("invocation-session-2"),
            Some(2),
            GamescopeLiveSessionEvent::SemanticScreenEpisode {
                screen_episode_id: 1,
                sequence: 42,
                monotonic_end_ms: 150,
                screen: scorepeek_core::recognition::ScreenClass::ModeSelect,
                phase: crate::capture_live::SemanticScreenEpisodePhase::Started,
            },
        )
        .unwrap();
        assert_eq!(mode["screen"], "mode_select");
        assert_eq!(mode["phase"], "started");
    }

    #[test]
    fn live_result_output_retains_exact_ocr_and_typed_resolution() {
        let domain = CatalogCandidateDomain::from_catalog(&Catalog::default()).unwrap();
        let output = RegisteredScreenFieldObservation::from_fields(
            &domain,
            ScreenFieldObservations::Result(ResultScreenFieldObservations {
                panel_side: scorepeek_core::recognition::ResultPanelSide::Right,
                title: text("TITLE EXACT"),
                artist: text("ARTIST EXACT"),
                clear_type: text("FAILED"),
                difficulty: text("HYPER"),
                play_type: text("SP"),
                level: text("8"),
                notes: text("800"),
                current_score: text("1200"),
                ..Default::default()
            }),
        );
        let value = live_session_event_value(
            Some("invocation-session-1"),
            Some(1),
            GamescopeLiveSessionEvent::Observation {
                screen_episode_id: 0,
                sequence: 42,
                monotonic_start_ms: 100,
                monotonic_end_ms: 125,
                output: &output,
            },
        )
        .unwrap();
        assert_eq!(value["event"], "field_observation");
        assert_eq!(value["sequence"], 42);
        assert_eq!(value["fields"]["panel_side"], "right");
        assert_eq!(value["fields"]["title"], "TITLE EXACT");
        assert_eq!(value["fields"]["artist"], "ARTIST EXACT");
        assert_eq!(value["fields"]["clear_type"], "FAILED");
        assert_eq!(value["fields"]["play_type"], "SP");
        assert_eq!(
            value["result_song_resolution"]["reason"],
            "no_catalog_candidates"
        );
        let event = RunEvent::from_value(value).unwrap();
        let RunEventKind::FieldObservation { fields, .. } = event.kind else {
            panic!("live result observation changed event kind");
        };
        assert_eq!(fields["panel_side"], "right");
    }

    #[test]
    fn production_result_serializer_reaches_provisional_and_confirmed_output() {
        use crate::events::server::RoutineOutput;
        use scorepeek_core::event::ResultState;

        let observation = resolved_two_player_result_observation();
        let mut routine = RoutineOutput::start_headless("invocation".into(), "a".repeat(64));
        publish_two_player_result_episode(&mut routine, &observation);
        assert!(routine.take_headless_events().iter().any(|event| matches!(
            event.kind,
            RunEventKind::ResultChanged {
                state: ResultState::Provisional { ref result, .. },
                ..
            } if result.play_side == scorepeek_core::recognition::PlaySide::TwoPlayer
        )));

        publish_headless_live_event(
            &mut routine,
            GamescopeLiveSessionEvent::SemanticScreenEpisode {
                screen_episode_id: 3,
                sequence: 10,
                monotonic_end_ms: 1_000,
                screen: scorepeek_core::recognition::ScreenClass::Result,
                phase: crate::capture_live::SemanticScreenEpisodePhase::Finalized,
            },
        );
        assert!(routine.take_headless_events().iter().any(|event| matches!(
            event.kind,
            RunEventKind::ResultChanged {
                state: ResultState::Confirmed { ref result, .. },
                ..
            } if result.play_side == scorepeek_core::recognition::PlaySide::TwoPlayer
        )));
    }

    #[test]
    fn routine_observation_binds_session_and_generation() {
        let domain = CatalogCandidateDomain::from_catalog(&Catalog::default()).unwrap();
        let output = RegisteredScreenFieldObservation::from_fields(
            &domain,
            ScreenFieldObservations::Result(ResultScreenFieldObservations {
                title: text("TITLE"),
                artist: text("ARTIST"),
                clear_type: text("CLEAR"),
                difficulty: text("HYPER"),
                level: text("8"),
                notes: text("800"),
                current_score: text("1200"),
                ..Default::default()
            }),
        );
        let value = live_session_event_value(
            Some("invocation-session-2"),
            Some(2),
            GamescopeLiveSessionEvent::Observation {
                screen_episode_id: 0,
                sequence: 1,
                monotonic_start_ms: 10,
                monotonic_end_ms: 20,
                output: &output,
            },
        )
        .unwrap();
        assert_eq!(value["schema"], "scorepeek-run-event-v17");
        assert_eq!(value["session_id"], "invocation-session-2");
        assert_eq!(value["capture_generation"], 2);
        assert_eq!(value["sequence"], 1);
    }

    #[test]
    fn routine_live_emission_bounds_json_without_truncating_authority() {
        let records = (0..9)
            .map(|index| {
                tachi_record(
                    &format!("song-{index}"),
                    &format!("COMMON TITLE {index}"),
                    &format!("COMMON ARTIST {index}"),
                )
            })
            .collect::<Vec<_>>();
        let catalog = catalog_from_records(&records);
        let domain = CatalogCandidateDomain::from_catalog(&catalog).unwrap();
        let output = RegisteredScreenFieldObservation::from_fields_with_catalog(
            &domain,
            &catalog,
            ScreenFieldObservations::Result(ResultScreenFieldObservations {
                title: text("COMMON TITLE"),
                artist: text("COMMON ARTIST"),
                ..Default::default()
            }),
        );
        let authority = output.joint_evidence().clone();
        assert!(authority.candidates.len() > 8);
        let value = live_session_event_value(
            Some("invocation-session-2"),
            Some(2),
            GamescopeLiveSessionEvent::Observation {
                screen_episode_id: 7,
                sequence: 8,
                monotonic_start_ms: 10,
                monotonic_end_ms: 20,
                output: &output,
            },
        )
        .unwrap();
        assert_eq!(
            value["joint_evidence"]["candidates"]
                .as_array()
                .unwrap()
                .len(),
            8
        );

        let event = run_event_from_live_emission(LiveSessionEmission {
            public_binding: None,
            value,
            authority_joint_evidence: Some(authority.clone()),
            diagnostic_identity: None,
            diagnostic_capture_fact: None,
        })
        .unwrap();
        let RunEventKind::FieldObservation { joint_evidence, .. } = event.kind else {
            panic!("expected field observation");
        };
        assert_eq!(joint_evidence, authority);
    }

    #[test]
    fn accepted_resolution_includes_catalog_title_artist_and_evidence() {
        let catalog = catalog_from_records(&[
            tachi_record("song-1", "CATALOG TITLE", "CATALOG ARTIST"),
            tachi_record("song-2", "OTHER SONG", "OTHER ARTIST"),
        ]);
        let domain = CatalogCandidateDomain::from_catalog(&catalog).unwrap();
        let output = RegisteredScreenFieldObservation::from_fields(
            &domain,
            ScreenFieldObservations::Result(ResultScreenFieldObservations {
                title: text("CATALOG TITLE"),
                artist: text("CATALOG ARTIST"),
                clear_type: text("CLEAR"),
                difficulty: text("HYPER"),
                level: text("8"),
                notes: text("800"),
                current_score: text("1200"),
                ..Default::default()
            }),
        );
        let value = live_session_event_value(
            Some("invocation-session-1"),
            Some(1),
            GamescopeLiveSessionEvent::Observation {
                screen_episode_id: 0,
                sequence: 1,
                monotonic_start_ms: 10,
                monotonic_end_ms: 20,
                output: &output,
            },
        )
        .unwrap();
        let presentation = &value["song_resolution_presentation"];
        assert_eq!(presentation["status"], "accepted");
        assert_eq!(
            presentation["selected"]["display_titles"][0],
            "CATALOG TITLE"
        );
        assert_eq!(presentation["selected"]["artist"], "CATALOG ARTIST");
        assert!(presentation["selected"]["scorepeek_song_id"].is_string());
        assert!(
            presentation["evidence_summary"]
                .as_str()
                .unwrap()
                .contains("runner-up margin=")
        );
    }

    fn catalog_from_records(records: &[SourceObservation]) -> Catalog {
        let policy = SourcePolicy::tachi();
        let mut field_authority = policy
            .field_authority
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        field_authority.sort();
        let snapshot = SourceSnapshot {
            policy: policy.clone(),
            evidence: SourceEvidence {
                source_id: SourceId::Tachi,
                lineage_id: LineageId::GameMdb,
                revision_strategy: RevisionStrategy::GitCommit,
                revision: "0123456789abcdef0123456789abcdef01234567".to_owned(),
                content_sha256: "a".repeat(64),
                byte_size: records.len(),
                record_count: records.len(),
                parser_version: policy.parser_version.to_owned(),
                declared_scope: policy.declared_scope.to_owned(),
                completeness: policy.completeness,
                field_authority,
                freshness: policy.freshness.to_owned(),
                rights_and_provenance: policy.rights_and_provenance.to_owned(),
            },
            observations: records.to_vec(),
        };
        Catalog::default()
            .federate(FederationInput {
                tachi: Some(snapshot),
                ..FederationInput::default()
            })
            .catalog
    }

    fn tachi_record(id: &str, title: &str, artist: &str) -> SourceObservation {
        SourceObservation::Tachi(TachiObservation {
            source_song_id: id.to_owned(),
            title_variants: BTreeSet::from([SourceTitleObservation {
                value: title.to_owned(),
                kind: DisplayVariantKind::InGameDisplay,
            }]),
            artist: artist.to_owned(),
            version: "SYNTHETIC".to_owned(),
            charts: vec![SourceChartObservation {
                chart: Chart {
                    key: ChartKey {
                        play_type: PlayType::Single,
                        difficulty: Difficulty::Normal,
                    },
                    level: 1,
                    notes: 1,
                },
                source_chart_id: "spn".to_owned(),
                product_versions: BTreeSet::from(["synthetic-v1".to_owned()]),
                primary: true,
            }],
            primary_infinitas: true,
        })
    }

    fn text(value: &str) -> DynamicTextObservation {
        DynamicTextObservation {
            input_width: 1,
            output_timesteps: 1,
            open_text: value.to_owned(),
            constrained_text: None,
        }
    }
}
