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

#[cfg(test)]
use crate::config::document::ConfigFile;
use crate::service::session::recognition as recognition_session;
use crate::{
    capture_live,
    config::{
        document::{self as config_document, CaptureKind},
        effective::{
            self as config_effective, OverlayOptions, RoutineCapture, RoutineRunOptions, RunArgs,
        },
        paths as config_paths,
    },
    diagnostics::inspect as diagnostic_stream,
    events::server as routine_output,
    inventory::{doctor as inventory, vulkan_layer},
    platform::signal as live_control,
    platform::state as local_profiles,
    recognition_artifact,
    recording::{
        policy::{DEFAULT_RECORDING_MEMORY_MIB, RecordingMemoryLimit},
        retention::RecordingRetention,
        simulation as recording_simulation, writer as canonical_recording,
    },
    service::session as routine_watcher,
};
use scorepeek_core::catalog::CatalogStore;
use scorepeek_core::diagnostics::{
    DiagnosticBinding, DiagnosticPolicy, DiagnosticResource, DiagnosticRetention,
    DiagnosticRunDescriptor,
};
use scorepeek_core::event::{RUN_EVENT_SCHEMA, RunEvent, RunEventKind};
use scorepeek_core::frame::{CanonicalFrame, CanonicalLayout};
use scorepeek_core::recognition::title::{
    DIAGNOSTIC_TITLE_COMPARISON_KEY_ID, DIAGNOSTIC_TITLE_MINIMUM_CONFIDENCE,
};
use scorepeek_core::recognition::{screen as recognition, title as recognition_title};
use serde::Serialize;
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
pub(crate) fn development_operation_main(operation: &'static str) -> ExitCode {
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

fn result_exit_status<T>(result: &Result<T, String>) -> u8 {
    match result {
        Ok(_) => 0,
        Err(error) if error == TERMINATED_ERROR => 0,
        Err(error) if error == INTERRUPTED_ERROR => 130,
        Err(_) => 1,
    }
}

fn dispatch_public(
    cli: PublicCli,
) -> Result<Option<scorepeek_frontend_api::CommandResult>, String> {
    let PublicCli { config, command } = cli;
    match command {
        PublicCommand::Run(args) => run_public(args, config).map(|()| None),
        PublicCommand::Doctor(format) => collect_frontend_doctor(format.format).map(Some),
        PublicCommand::Config { command } => {
            let config_path = config_paths::resolve(config)?;
            run_config_command(command, &config_path).map(Some)
        }
        PublicCommand::Diagnostic { command } => run_diagnostic_command(command).map(|()| None),
        PublicCommand::Skin { command } => run_skin_command(command).map(Some),
        PublicCommand::VulkanLayer { command } => run_vulkan_layer_command(command).map(Some),
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
    if let Err(error) = &result
        && error != TERMINATED_ERROR
        && error != INTERRUPTED_ERROR
    {
        return scorepeek_frontend_api::FrontendReply::Error {
            error: scorepeek_frontend_api::FrontendError {
                error_type: "runtime_operation_failed".to_owned(),
                message: error.clone(),
            },
        };
    }
    scorepeek_frontend_api::FrontendReply::Completed {
        exit_code,
        result: result.ok().flatten(),
    }
}

const fn frontend_output_format(format: scorepeek_frontend_api::OutputFormat) -> OutputFormat {
    match format {
        scorepeek_frontend_api::OutputFormat::Human => OutputFormat::Human,
        scorepeek_frontend_api::OutputFormat::Json => OutputFormat::Json,
    }
}

fn run_public(args: RunArgs, config_override: Option<PathBuf>) -> Result<(), String> {
    run_public_with_model_initializer(args, config_override, |override_bundle| {
        scorepeek::resources::model::acquire::ensure_small_model(override_bundle, |event| {
            match event {
                scorepeek::resources::model::cache::ModelCacheEvent::DownloadStarted => {
                    frontend_event(scorepeek_frontend_api::FrontendEvent::ModelDownload {
                        state: scorepeek_frontend_api::ModelDownload::Started,
                    });
                }
                scorepeek::resources::model::cache::ModelCacheEvent::DownloadCompleted => {
                    frontend_event(scorepeek_frontend_api::FrontendEvent::ModelDownload {
                        state: scorepeek_frontend_api::ModelDownload::Completed,
                    });
                }
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
        config_paths::resolve(config_override)
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
        config_document::read(config_path)
            .map(|value| value.map(|(_, config)| config).unwrap_or_default())
    });
    let config = settle_startup_result(diagnostics, monitor, config_result)?;
    let merge_result = run_startup_stage(&sink, "config_merge", || {
        config_effective::merge_run_options(config, args)
    });
    settle_startup_result(diagnostics, monitor, merge_result)
}

fn run_config_command(
    command: ConfigCommand,
    path: &Path,
) -> Result<scorepeek_frontend_api::CommandResult, String> {
    let (format, result) = match command {
        ConfigCommand::Path(format) => {
            let rendered_path = frontend_path(path, format.format)?;
            (
                frontend_api_output_format(format.format),
                scorepeek_frontend_api::ConfigResult::Path {
                    path: rendered_path,
                },
            )
        }
        ConfigCommand::Show(format) => {
            let content = config_document::read_text(path)?;
            let rendered_path = frontend_path(path, format.format)?;
            (
                frontend_api_output_format(format.format),
                scorepeek_frontend_api::ConfigResult::Show {
                    path: rendered_path,
                    present: content.is_some(),
                    content,
                },
            )
        }
        ConfigCommand::Check(format) => {
            let present = config_document::read(path)?.is_some();
            let rendered_path = frontend_path(path, format.format)?;
            (
                frontend_api_output_format(format.format),
                scorepeek_frontend_api::ConfigResult::Check {
                    path: rendered_path,
                    present,
                    valid: true,
                },
            )
        }
    };
    Ok(scorepeek_frontend_api::CommandResult::Config { format, result })
}

fn frontend_path(path: &Path, format: OutputFormat) -> Result<String, String> {
    match format {
        OutputFormat::Human => Ok(path.display().to_string()),
        OutputFormat::Json => config_path_for_json(path).map(ToOwned::to_owned),
    }
}

const fn frontend_api_output_format(format: OutputFormat) -> scorepeek_frontend_api::OutputFormat {
    match format {
        OutputFormat::Human => scorepeek_frontend_api::OutputFormat::Human,
        OutputFormat::Json => scorepeek_frontend_api::OutputFormat::Json,
    }
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

fn run_skin_command(command: SkinCommand) -> Result<scorepeek_frontend_api::CommandResult, String> {
    let store = scorepeek_overlay_wayland::skin::StoreRoot::discover();
    let (format, result) = match command {
        SkinCommand::Install { package } => {
            let outcome = store.install(&package)?;
            let outcome = match outcome {
                scorepeek_overlay_wayland::skin::InstallOutcome::Installed => {
                    scorepeek_frontend_api::SkinInstallResult::Installed
                }
                scorepeek_overlay_wayland::skin::InstallOutcome::Replaced { previous_release } => {
                    scorepeek_frontend_api::SkinInstallResult::Replaced { previous_release }
                }
                scorepeek_overlay_wayland::skin::InstallOutcome::Unchanged => {
                    scorepeek_frontend_api::SkinInstallResult::Unchanged
                }
            };
            (
                scorepeek_frontend_api::OutputFormat::Human,
                scorepeek_frontend_api::SkinResult::Installed { outcome },
            )
        }
        SkinCommand::Uninstall { id } => {
            store.uninstall(&id)?;
            (
                scorepeek_frontend_api::OutputFormat::Human,
                scorepeek_frontend_api::SkinResult::Uninstalled,
            )
        }
        SkinCommand::List(format) => {
            let installed = store.list()?;
            let output_format = format.format;
            let skins = installed
                .into_iter()
                .map(|skin| {
                    let path = match output_format {
                        OutputFormat::Human => skin.path.display().to_string(),
                        OutputFormat::Json => skin
                            .path
                            .to_str()
                            .ok_or_else(|| {
                                "skin list serialization failed: path contains invalid UTF-8 characters"
                                    .to_owned()
                            })?
                            .to_owned(),
                    };
                    Ok(scorepeek_frontend_api::InstalledSkin {
                        id: skin.id,
                        release: skin.release,
                        name: skin.name,
                        path,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            (
                frontend_api_output_format(output_format),
                scorepeek_frontend_api::SkinResult::Listed { skins },
            )
        }
    };
    Ok(scorepeek_frontend_api::CommandResult::Skin { format, result })
}

fn run_vulkan_layer_command(
    command: VulkanLayerCommand,
) -> Result<scorepeek_frontend_api::CommandResult, String> {
    let result = match command {
        VulkanLayerCommand::Install => {
            match vulkan_layer::install().map_err(|error| error.to_string())? {
                vulkan_layer::InstallOutcome::Installed => {
                    scorepeek_frontend_api::VulkanLayerResult::Installed
                }
                vulkan_layer::InstallOutcome::Updated => {
                    scorepeek_frontend_api::VulkanLayerResult::Updated
                }
                vulkan_layer::InstallOutcome::Unchanged => {
                    scorepeek_frontend_api::VulkanLayerResult::Unchanged
                }
            }
        }
        VulkanLayerCommand::Uninstall => {
            match vulkan_layer::uninstall().map_err(|error| error.to_string())? {
                vulkan_layer::UninstallOutcome::Uninstalled => {
                    scorepeek_frontend_api::VulkanLayerResult::Uninstalled
                }
                vulkan_layer::UninstallOutcome::NotInstalled => {
                    scorepeek_frontend_api::VulkanLayerResult::NotInstalled
                }
            }
        }
    };
    Ok(scorepeek_frontend_api::CommandResult::VulkanLayer { result })
}

#[allow(clippy::too_many_lines)]
fn run(args: &[OsString]) -> Result<(), String> {
    run_with_model_initializer(args, |override_bundle| {
        scorepeek::resources::model::acquire::ensure_small_model(override_bundle, |event| {
            match event {
                scorepeek::resources::model::cache::ModelCacheEvent::DownloadStarted => {
                    eprintln!("scorepeek: downloading PP-OCRv6-small model...");
                }
                scorepeek::resources::model::cache::ModelCacheEvent::DownloadCompleted => {
                    eprintln!("scorepeek: PP-OCRv6-small model download complete");
                }
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
            &config_paths::default_path(),
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
    _resources: recognition_title::RegisteredRecognitionResources,
}

impl recognition_session::field_observer::FieldObserver for RegisteredResourceOwner {
    type Output = ();

    fn observe(
        &mut self,
        _input: &recognition_session::field_observer::FieldObserverInput,
    ) -> Self::Output {
    }
}

impl From<recognition_title::RegisteredResourceLoadErrorType> for RegisteredResourceGateErrorType {
    fn from(error: recognition_title::RegisteredResourceLoadErrorType) -> Self {
        match error {
            recognition_title::RegisteredResourceLoadErrorType::InvalidLocation => {
                Self::InvalidLocation
            }
            recognition_title::RegisteredResourceLoadErrorType::ModelBindingMismatch => {
                Self::ModelBindingMismatch
            }
            recognition_title::RegisteredResourceLoadErrorType::RuntimeBindingMismatch => {
                Self::RuntimeBindingMismatch
            }
            recognition_title::RegisteredResourceLoadErrorType::CatalogUnavailable => {
                Self::CatalogUnavailable
            }
            recognition_title::RegisteredResourceLoadErrorType::CatalogBindingMismatch => {
                Self::CatalogBindingMismatch
            }
            recognition_title::RegisteredResourceLoadErrorType::CatalogLoadFailed => {
                Self::CatalogLoadFailed
            }
            recognition_title::RegisteredResourceLoadErrorType::ModelBundleInvalid => {
                Self::ModelBundleInvalid
            }
            recognition_title::RegisteredResourceLoadErrorType::RuntimeInitializationFailed => {
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
    let model_digest = recognition_title::LIVE_MODEL_SHA256.to_owned();
    let runtime_digest = recognition_title::LIVE_RUNTIME_SHA256.to_owned();
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
            canonical_layout_sha256: CanonicalLayout::sha256(),
            catalog_sha256: catalog_digest.clone(),
            model_sha256: model_digest.clone(),
            runtime_sha256: runtime_digest.clone(),
            replay: None,
        },
    };
    let worker =
        recognition_session::field_observer::FieldObserverWorker::start(&descriptor, |binding| {
            binding
                .load_registered_resources(Path::new(catalog_root), bundle_root)
                .map(|resources| RegisteredResourceOwner {
                    _resources: resources,
                })
        });
    match worker {
        Ok(worker) => {
            let outcome = worker
                .finish(recognition_session::field_observer::DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT);
            if outcome.status
                != recognition_session::field_observer::FieldObserverFinishStatus::Complete
            {
                let error_type = match outcome.status {
                    recognition_session::field_observer::FieldObserverFinishStatus::Timeout => {
                        RegisteredResourceGateErrorType::FinishTimeout
                    }
                    recognition_session::field_observer::FieldObserverFinishStatus::WorkerUnavailable => {
                        RegisteredResourceGateErrorType::WorkerUnavailable
                    }
                    recognition_session::field_observer::FieldObserverFinishStatus::Complete => {
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
    error: recognition_session::field_observer::FieldObserverStartError<
        recognition_title::RegisteredResourceLoadError,
    >,
) -> (RegisteredResourceGateErrorType, String) {
    use recognition_session::field_observer::FieldObserverStartError;
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

#[path = "application/live_event.rs"]
mod live_event;

#[allow(
    clippy::wildcard_imports,
    reason = "live_event is an implementation partition shared with application tests"
)]
use live_event::*;

#[path = "application/live_session.rs"]
mod live_session;

#[allow(
    clippy::wildcard_imports,
    reason = "live_session is an implementation partition shared with application tests"
)]
use live_session::*;

#[cfg(test)]
fn write_ndjson(output: &mut impl io::Write, value: &impl Serialize) -> Result<(), String> {
    serde_json::to_writer(&mut *output, value)
        .map_err(|error| format!("live result serialization failed: {error}"))?;
    output
        .write_all(b"\n")
        .and_then(|()| output.flush())
        .map_err(|error| format!("live result output failed: {error}"))
}

#[path = "application/capture_tools.rs"]
mod capture_tools;

#[allow(
    clippy::wildcard_imports,
    reason = "capture_tools is an implementation partition shared with application tests"
)]
use capture_tools::*;

#[path = "application/inventory_tools.rs"]
mod inventory_tools;

#[allow(
    clippy::wildcard_imports,
    reason = "inventory_tools is an implementation partition shared with application tests"
)]
use inventory_tools::*;

#[path = "application/recording_tools.rs"]
mod recording_tools;

#[allow(
    clippy::wildcard_imports,
    reason = "recording_tools is an implementation partition of application dispatch"
)]
use recording_tools::*;

#[path = "application/recognition_tools.rs"]
mod recognition_tools;

#[allow(
    clippy::wildcard_imports,
    reason = "recognition_tools is an implementation partition shared with application tests"
)]
use recognition_tools::*;

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
#[path = "application/tests.rs"]
mod tests;
