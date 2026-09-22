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
        recording: Option<PathBuf>,
        #[arg(long)]
        v4_session: Option<PathBuf>,
        #[arg(long)]
        v4_store: Option<PathBuf>,
        #[arg(long)]
        v4_session_sha256: Option<String>,
        #[arg(long)]
        v4_objects: Option<PathBuf>,
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
        Command::Import {
            store,
            recording,
            v4_session,
            v4_store,
            v4_session_sha256,
            v4_objects,
        } => {
            let converted = match (
                recording.as_ref(),
                v4_session.as_ref(),
                v4_store.as_ref(),
                v4_session_sha256.as_ref(),
            ) {
                (Some(recording), Some(session), None, None) if v4_objects.is_none() => {
                    let staging = tempfile::tempdir().map_err(|error| error.to_string())?;
                    let path = staging.path().join("converted-recording");
                    scorepeek_corpus::migration_v4::migrate_recording(recording, session, &path)
                        .map_err(|error| format!("v4 migration failed: {error:?}"))?;
                    Some((staging, path))
                }
                (None, None, Some(old_store), Some(digest)) => {
                    let staging = tempfile::tempdir().map_err(|error| error.to_string())?;
                    let path = staging.path().join("converted-recording");
                    scorepeek_corpus::migration_v4::migrate_imported_session(
                        old_store,
                        digest,
                        v4_objects.as_deref(),
                        &path,
                    )
                    .map_err(|error| format!("v4 corpus migration failed: {error:?}"))?;
                    Some((staging, path))
                }
                (Some(_), None, None, None) if v4_objects.is_none() => None,
                _ => return Err("choose one canonical recording or one v4 store session".into()),
            };
            let source = converted
                .as_ref()
                .map(|(_, path)| path.as_path())
                .or(recording.as_deref())
                .ok_or("import source is missing")?;
            let summary = scorepeek_corpus::store::import_recording(&store, source)
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
