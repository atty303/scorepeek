use std::path::PathBuf;
use std::process::ExitCode;

use clap::{ArgGroup, Parser, Subcommand};

#[derive(Parser)]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Import {
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        recording: PathBuf,
    },
    Review {
        #[command(subcommand)]
        command: ReviewCommand,
    },
    #[command(group(ArgGroup::new("source").required(true).multiple(false)))]
    Replay {
        #[arg(long, group = "source", value_parser = absolute_path)]
        store: Option<PathBuf>,
        #[arg(long, group = "source", value_parser = absolute_path)]
        recording: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ReviewCommand {
    Apply {
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        draft: PathBuf,
        #[arg(long)]
        labels: PathBuf,
    },
}

fn absolute_path(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err("replay requires an absolute directory".to_owned())
    }
}

fn print_progress(progress: &scorepeek_corpus::replay::ReplayProgress) {
    let recording = progress
        .recording
        .file_name()
        .unwrap_or(progress.recording.as_os_str())
        .to_string_lossy();
    eprintln!(
        "replay_progress recording={recording} phase={:?} inputs={}/{} retained={}/{} segments={}/{} elapsed={}s",
        progress.phase,
        progress.processed_inputs,
        progress.total_inputs,
        progress.retained_frames,
        progress.total_retained_frames,
        progress.segments_seen,
        progress.total_segments,
        progress.elapsed.as_secs(),
    );
}

fn replay(store: Option<PathBuf>, recording: Option<PathBuf>) -> Result<(), String> {
    if let Some(recording) = recording {
        let report =
            scorepeek_corpus::replay::replay_recording_with_progress(&recording, &print_progress)
                .map_err(|error| format!("recording replay failed: {error:?}"))?;
        let document = serde_json::to_string_pretty(&report)
            .map_err(|error| format!("replay report encoding failed: {error}"))?;
        println!("{document}");
    } else if let Some(store) = store {
        let reports =
            scorepeek_corpus::replay::replay_active_with_progress(&store, &print_progress)
                .map_err(|error| format!("full corpus replay failed: {error:?}"))?;
        println!("replayed_sessions={}", reports.len());
    }
    Ok(())
}

fn run() -> Result<(), String> {
    match Arguments::parse().command {
        Command::Import { store, recording } => {
            let summary = scorepeek_corpus::store::import_recording(&store, &recording)
                .map_err(|error| format!("canonical import failed: {error:?}"))?;
            println!(
                "session_sha256={}\nreview_draft={}",
                summary.session_sha256,
                summary.draft.display()
            );
        }
        Command::Review {
            command:
                ReviewCommand::Apply {
                    store,
                    draft,
                    labels,
                },
        } => {
            scorepeek_corpus::store::review_apply(&store, &draft, &labels)
                .map_err(|error| format!("review apply failed: {error:?}"))?;
        }
        Command::Replay { store, recording } => replay(store, recording)?,
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
