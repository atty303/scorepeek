use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

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
