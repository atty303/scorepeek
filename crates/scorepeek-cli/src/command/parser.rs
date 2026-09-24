use clap::{CommandFactory as _, Parser, Subcommand};
use scorepeek_frontend_api as api;
use std::{ffi::OsString, io, path::PathBuf};

use super::{
    config::ConfigCommand,
    diagnostic::DiagnosticCommand,
    doctor::{FormatArgs, OutputFormat},
    run::{CaptureKind, RunArgs},
    skin::SkinCommand,
    vulkan_layer::VulkanLayerCommand,
};
use crate::completion::CompletionShell;

#[derive(Parser)]
#[command(
    name = "scorepeek",
    version,
    about = "Live IIDX score recognition and overlays",
    disable_help_subcommand = true
)]
struct Cli {
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Run(RunArgs),
    Doctor(FormatArgs),
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    Diagnostic {
        #[command(subcommand)]
        command: DiagnosticCommand,
    },
    Skin {
        #[command(subcommand)]
        command: SkinCommand,
    },
    VulkanLayer {
        #[command(subcommand)]
        command: VulkanLayerCommand,
    },
    Completion {
        #[arg(value_enum)]
        shell: CompletionShell,
    },
}

pub enum Action {
    Dispatch(api::FrontendCommand, OutputFormat),
    Complete(CompletionShell),
    Help,
}

pub fn parse(arguments: Vec<OsString>) -> Result<Action, clap::Error> {
    if arguments.len() == 1 {
        return Ok(Action::Help);
    }
    let cli = Cli::try_parse_from(arguments)?;
    let format = match &cli.command {
        Command::Doctor(args)
        | Command::Skin {
            command: SkinCommand::List(args),
        } => args.format,
        Command::Config { command } => match command {
            ConfigCommand::Path(args) | ConfigCommand::Show(args) | ConfigCommand::Check(args) => {
                args.format
            }
        },
        _ => OutputFormat::Human,
    };
    let request_id = api::RequestId(format!("cli-{}", std::process::id()));
    let config = cli
        .config
        .map(path_string)
        .transpose()
        .map_err(value_error)?;
    let command = match cli.command {
        Command::Run(args) => api::FrontendCommand::Run {
            request_id,
            config,
            command: api::RunCommand {
                capture: args.capture.map(|value| match value {
                    CaptureKind::Pipewire => api::CaptureKind::Pipewire,
                    CaptureKind::VulkanLayer => api::CaptureKind::VulkanLayer,
                }),
                node_name: args.node_name,
                crop_left: args.crop_left,
                crop_top: args.crop_top,
                crop_right: args.crop_right,
                crop_bottom: args.crop_bottom,
                scores_db: args
                    .scores_db
                    .map(path_string)
                    .transpose()
                    .map_err(value_error)?,
                no_scores: args.no_scores,
                scores: args.scores,
                record: args.record,
                record_all: args.record_all,
                no_record: args.no_record,
                record_memory_mib: args.record_memory_mib,
                overlay_wayland: args.overlay_wayland,
                no_overlay_wayland: args.no_overlay_wayland,
                overlay_wayland_edit: args.overlay_wayland_edit,
                no_overlay_wayland_edit: args.no_overlay_wayland_edit,
                overlay_obs: args.overlay_obs,
                no_overlay_obs: args.no_overlay_obs,
                overlay_config: args
                    .overlay_config
                    .map(path_string)
                    .transpose()
                    .map_err(value_error)?,
            },
        },
        Command::Doctor(_) => api::FrontendCommand::Doctor { request_id },
        Command::Config { command } => api::FrontendCommand::Config {
            request_id,
            config,
            action: config_action(&command),
        },
        Command::Diagnostic { command } => api::FrontendCommand::Diagnostic {
            request_id,
            action: diagnostic_action(command),
        },
        Command::Skin { command } => api::FrontendCommand::Skin {
            request_id,
            action: skin_action(command).map_err(value_error)?,
        },
        Command::VulkanLayer { command } => api::FrontendCommand::VulkanLayer {
            request_id,
            action: match command {
                VulkanLayerCommand::Install => api::VulkanLayerAction::Install,
                VulkanLayerCommand::Uninstall => api::VulkanLayerAction::Uninstall,
            },
        },
        Command::Completion { shell } => return Ok(Action::Complete(shell)),
    };
    Ok(Action::Dispatch(command, format))
}

fn path_string(path: PathBuf) -> Result<String, &'static str> {
    path.into_os_string()
        .into_string()
        .map_err(|_| "paths must be valid UTF-8")
}

fn value_error(message: &'static str) -> clap::Error {
    clap::Error::raw(clap::error::ErrorKind::InvalidUtf8, message)
}

const fn output_format(value: OutputFormat) -> api::OutputFormat {
    match value {
        OutputFormat::Human => api::OutputFormat::Human,
        OutputFormat::Json => api::OutputFormat::Json,
    }
}

fn config_action(value: &ConfigCommand) -> api::ConfigAction {
    match value {
        ConfigCommand::Path(_) => api::ConfigAction::Path,
        ConfigCommand::Show(_) => api::ConfigAction::Show,
        ConfigCommand::Check(_) => api::ConfigAction::Check,
    }
}

fn diagnostic_action(value: DiagnosticCommand) -> api::DiagnosticAction {
    match value {
        DiagnosticCommand::Observe { replay } => api::DiagnosticAction::Observe {
            replay_seconds: replay,
        },
        DiagnosticCommand::Inspect(value) => {
            let _ = value.latest;
            api::DiagnosticAction::Inspect {
                run_id: value.run_id,
                format: output_format(value.format.format),
            }
        }
    }
}

fn skin_action(value: SkinCommand) -> Result<api::SkinAction, &'static str> {
    Ok(match value {
        SkinCommand::Install { package } => api::SkinAction::Install {
            package: path_string(package)?,
        },
        SkinCommand::Uninstall { id } => api::SkinAction::Uninstall { id },
        SkinCommand::List(_) => api::SkinAction::List,
    })
}

pub fn print_help() -> io::Result<()> {
    Cli::command().print_help()?;
    println!();
    Ok(())
}

pub fn generate_completion(shell: CompletionShell) {
    crate::completion::generate(shell, Cli::command());
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::Parser as _;

    #[test]
    fn public_cli_exposes_only_the_seven_application_commands() {
        for command in [
            "run",
            "doctor",
            "config",
            "diagnostic",
            "skin",
            "vulkan-layer",
            "completion",
        ] {
            let result = Cli::try_parse_from(["scorepeek", command, "--help"]);
            assert!(result.is_err_and(|error| !error.use_stderr()));
        }
        assert!(Cli::try_parse_from(["scorepeek", "recognition"]).is_err());
        assert!(Cli::try_parse_from(["scorepeek", "capture"]).is_err());
    }
}
