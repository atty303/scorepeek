use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use scorepeek_corpus::{
    CorpusReplayOptions, MusicSelectDwellPolicy, MusicSelectTemporalCandidatePolicy,
    TemporalEvaluationPolicy, apply_music_list_motion_review, apply_music_select_motion_review,
    apply_review, author_numeric_dataset, author_numeric_sentinel,
    evaluate_music_select_correctness, evaluate_music_select_dwell, evaluate_temporal_corpus,
    import_run_diagnostic, inspect_music_list_row_observation_draft, inspect_review,
    measure_music_list_motion, plan_music_list_motion_review, plan_music_select_motion_review,
    render_synthetic_title_set, replay_corpus_with_options, verify_music_list_motion,
    verify_music_list_row_observation_draft, verify_run_diagnostic,
};

#[derive(Parser)]
#[command(
    name = "scorepeek-corpus",
    version,
    about = "Private corpus authoring and evaluation"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Diagnostic(DiagnosticArgs),
    Corpus(CorpusArgs),
    Review(ReviewArgs),
    Numeric(NumericArgs),
    Temporal(TemporalArgs),
    MusicList(MusicListArgs),
    MusicSelect(MusicSelectArgs),
    Synthetic(SyntheticArgs),
}

#[derive(Args)]
struct DiagnosticArgs {
    #[command(subcommand)]
    command: DiagnosticCommand,
}

#[derive(Subcommand)]
enum DiagnosticCommand {
    Verify {
        run_directory: PathBuf,
        #[arg(long)]
        capture_session_id: String,
    },
}

#[derive(Args)]
struct CorpusArgs {
    #[command(subcommand)]
    command: CorpusCommand,
}

#[derive(Subcommand)]
enum CorpusCommand {
    ImportDiagnostic {
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        diagnostic: PathBuf,
        #[arg(long)]
        capture_session_id: String,
        #[arg(long)]
        review_draft: PathBuf,
    },
    Replay {
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        text_workers: Option<usize>,
        #[arg(long, default_value_t = 2_048)]
        memory_mib: usize,
        #[arg(long)]
        trace_dir: Option<PathBuf>,
    },
}

#[derive(Args)]
struct ReviewArgs {
    #[command(subcommand)]
    command: ReviewCommand,
}

#[derive(Subcommand)]
enum ReviewCommand {
    Show {
        #[arg(long)]
        draft: PathBuf,
    },
    Apply {
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        draft: PathBuf,
        #[arg(long)]
        labels: PathBuf,
    },
}

#[derive(Args)]
struct NumericArgs {
    #[command(subcommand)]
    command: NumericCommand,
}

#[derive(Subcommand)]
enum NumericCommand {
    Dataset(NumericDatasetArgs),
    Sentinel(NumericSentinelArgs),
}

#[derive(Args)]
struct NumericDatasetArgs {
    #[command(subcommand)]
    command: NumericDatasetCommand,
}

#[derive(Subcommand)]
enum NumericDatasetCommand {
    Author {
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Args)]
struct NumericSentinelArgs {
    #[command(subcommand)]
    command: NumericSentinelCommand,
}

#[derive(Subcommand)]
enum NumericSentinelCommand {
    Author {
        #[arg(long)]
        frame: PathBuf,
        #[arg(long)]
        frame_sha256: String,
        #[arg(long)]
        labels: PathBuf,
        #[arg(long)]
        labels_sha256: String,
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Args)]
struct TemporalArgs {
    #[command(subcommand)]
    command: TemporalCommand,
}

#[derive(Subcommand)]
enum TemporalCommand {
    Evaluate {
        #[arg(long)]
        store: PathBuf,
        #[arg(long, value_name = "OBSERVATIONS:GAP_MS", value_parser = parse_temporal_policy)]
        policy: Vec<TemporalEvaluationPolicy>,
    },
}

#[derive(Args)]
struct MusicListArgs {
    #[command(subcommand)]
    command: MusicListCommand,
}

#[derive(Subcommand)]
enum MusicListCommand {
    Motion(MusicListMotionArgs),
    ObservationDraft(MusicListObservationArgs),
}

#[derive(Args)]
struct MusicListMotionArgs {
    #[command(subcommand)]
    command: MusicListMotionCommand,
}

#[derive(Subcommand)]
enum MusicListMotionCommand {
    Measure {
        document: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Verify {
        document: PathBuf,
    },
    ReviewPlan {
        artifact: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    ReviewApply {
        artifact: PathBuf,
        plan: PathBuf,
        decisions: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Args)]
struct MusicListObservationArgs {
    #[command(subcommand)]
    command: MusicListObservationCommand,
}

#[derive(Subcommand)]
enum MusicListObservationCommand {
    Inspect { document: PathBuf },
    Verify { document: PathBuf },
}

#[derive(Args)]
struct MusicSelectArgs {
    #[command(subcommand)]
    command: MusicSelectCommand,
}

#[derive(Subcommand)]
enum MusicSelectCommand {
    Motion(MusicSelectMotionArgs),
    Dwell(MusicSelectDwellArgs),
}

#[derive(Args)]
struct MusicSelectMotionArgs {
    #[command(subcommand)]
    command: MusicSelectMotionCommand,
}

#[derive(Subcommand)]
enum MusicSelectMotionCommand {
    ReviewPlan {
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        session_sha256: String,
        #[arg(long)]
        video: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    ReviewApply {
        draft: PathBuf,
        decisions: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Args)]
struct MusicSelectDwellArgs {
    #[command(subcommand)]
    command: MusicSelectDwellCommand,
}

#[derive(Subcommand)]
enum MusicSelectDwellCommand {
    Evaluate {
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        catalog_store: PathBuf,
        #[arg(long)]
        reviewed: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, value_name = "DWELL_MS", value_parser = parse_dwell_policy)]
        policy: Vec<MusicSelectDwellPolicy>,
    },
    EvaluateCorrectness {
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        catalog_store: PathBuf,
        #[arg(long)]
        reviewed: PathBuf,
        #[arg(long)]
        labels: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, value_name = "DWELL_MS:UNKNOWN_GRACE_MS", value_parser = parse_correctness_policy)]
        policy: Vec<MusicSelectTemporalCandidatePolicy>,
    },
}

#[derive(Args)]
struct SyntheticArgs {
    #[command(subcommand)]
    command: SyntheticCommand,
}

#[derive(Subcommand)]
enum SyntheticCommand {
    Render {
        request: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse_from(std::env::args_os()) {
        Ok(cli) => cli,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            return ExitCode::from(code);
        }
    };
    match dispatch(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("scorepeek-corpus: {error}");
            ExitCode::from(1)
        }
    }
}

#[allow(clippy::too_many_lines)]
fn dispatch(command: Command) -> Result<(), String> {
    match command {
        Command::Diagnostic(DiagnosticArgs {
            command:
                DiagnosticCommand::Verify {
                    run_directory,
                    capture_session_id,
                },
        }) => verify_run_diagnostic(&run_directory, &capture_session_id)
            .map_err(|error| format!("diagnostic verification failed: {error}"))
            .and_then(|value| print_json(&value, "diagnostic verification")),
        Command::Corpus(CorpusArgs { command }) => match command {
            CorpusCommand::ImportDiagnostic {
                store,
                diagnostic,
                capture_session_id,
                review_draft,
            } => import_run_diagnostic(&store, &diagnostic, &capture_session_id, &review_draft)
                .map_err(|error| format!("diagnostic import failed: {error}"))
                .and_then(|value| print_json(&value, "diagnostic import")),
            CorpusCommand::Replay {
                store,
                text_workers,
                memory_mib,
                trace_dir,
            } => replay_corpus_with_options(
                &store,
                CorpusReplayOptions {
                    trace_dir,
                    text_workers,
                    memory_mib,
                },
            )
            .map_err(|error| format!("corpus replay failed: {error}"))
            .and_then(|value| print_json(&value, "corpus replay")),
        },
        Command::Review(ReviewArgs { command }) => match command {
            ReviewCommand::Show { draft } => inspect_review(&draft)
                .map_err(|error| format!("review inspection failed: {error}"))
                .and_then(|value| print_json(&value, "review inspection")),
            ReviewCommand::Apply {
                store,
                draft,
                labels,
            } => apply_review(&store, &draft, &labels)
                .map_err(|error| format!("review application failed: {error}"))
                .and_then(|value| print_json(&value, "review application")),
        },
        Command::Numeric(NumericArgs { command }) => match command {
            NumericCommand::Dataset(NumericDatasetArgs {
                command: NumericDatasetCommand::Author { store, output },
            }) => author_numeric_dataset(&store, &output)
                .map_err(|error| format!("numeric dataset authoring failed: {error}"))
                .and_then(|value| print_json(&value, "numeric dataset authoring")),
            NumericCommand::Sentinel(NumericSentinelArgs {
                command:
                    NumericSentinelCommand::Author {
                        frame,
                        frame_sha256,
                        labels,
                        labels_sha256,
                        output,
                    },
            }) => author_numeric_sentinel(&frame, &frame_sha256, &labels, &labels_sha256, &output)
                .map_err(|error| format!("numeric sentinel authoring failed: {error}"))
                .and_then(|value| print_json(&value, "numeric sentinel authoring")),
        },
        Command::Temporal(TemporalArgs {
            command: TemporalCommand::Evaluate { store, policy },
        }) => {
            let policies = temporal_policies(&policy);
            evaluate_temporal_corpus(&store, &policies)
                .map_err(|error| format!("temporal corpus evaluation failed: {error}"))
                .and_then(|value| print_json(&value, "temporal corpus evaluation"))
        }
        Command::MusicList(MusicListArgs { command }) => dispatch_music_list(command),
        Command::MusicSelect(MusicSelectArgs { command }) => dispatch_music_select(command),
        Command::Synthetic(SyntheticArgs {
            command: SyntheticCommand::Render { request, output },
        }) => render_synthetic_title_set(request, output)
            .map_err(|error| format!("synthetic rendering failed: {error}"))
            .and_then(|value| print_json(&value, "synthetic rendering")),
    }
}

fn dispatch_music_list(command: MusicListCommand) -> Result<(), String> {
    match command {
        MusicListCommand::Motion(MusicListMotionArgs { command }) => match command {
            MusicListMotionCommand::Measure { document, output } => {
                measure_music_list_motion(document, output)
                    .map_err(|error| format!("music-list motion measurement failed: {error}"))
                    .and_then(|value| print_json(&value, "music-list motion measurement"))
            }
            MusicListMotionCommand::Verify { document } => verify_music_list_motion(document)
                .map_err(|error| format!("music-list motion verification failed: {error}"))
                .and_then(|value| print_json(&value, "music-list motion verification")),
            MusicListMotionCommand::ReviewPlan { artifact, output } => {
                plan_music_list_motion_review(artifact, output)
                    .map_err(|error| format!("music-list motion review planning failed: {error}"))
                    .and_then(|value| print_json(&value, "music-list motion review planning"))
            }
            MusicListMotionCommand::ReviewApply {
                artifact,
                plan,
                decisions,
                output,
            } => apply_music_list_motion_review(artifact, plan, decisions, output)
                .map_err(|error| format!("music-list motion review application failed: {error}"))
                .and_then(|value| print_json(&value, "music-list motion review application")),
        },
        MusicListCommand::ObservationDraft(MusicListObservationArgs { command }) => match command {
            MusicListObservationCommand::Inspect { document } => {
                inspect_music_list_row_observation_draft(document)
                    .map_err(|error| {
                        format!("music-list row observation draft inspection failed: {error}")
                    })
                    .and_then(|value| {
                        print_json(&value, "music-list row observation draft inspection")
                    })
            }
            MusicListObservationCommand::Verify { document } => {
                verify_music_list_row_observation_draft(document)
                    .map_err(|error| {
                        format!("music-list row observation draft verification failed: {error}")
                    })
                    .and_then(|value| {
                        print_json(&value, "music-list row observation draft verification")
                    })
            }
        },
    }
}

fn dispatch_music_select(command: MusicSelectCommand) -> Result<(), String> {
    match command {
        MusicSelectCommand::Motion(MusicSelectMotionArgs { command }) => match command {
            MusicSelectMotionCommand::ReviewPlan {
                store,
                session_sha256,
                video,
                output,
            } => plan_music_select_motion_review(&store, &session_sha256, &video, &output)
                .map_err(|error| format!("music-select motion review planning failed: {error}"))
                .and_then(|value| print_json(&value, "music-select motion review planning")),
            MusicSelectMotionCommand::ReviewApply {
                draft,
                decisions,
                output,
            } => apply_music_select_motion_review(&draft, &decisions, &output)
                .map_err(|error| format!("music-select motion review application failed: {error}"))
                .and_then(|value| print_json(&value, "music-select motion review application")),
        },
        MusicSelectCommand::Dwell(MusicSelectDwellArgs { command }) => match command {
            MusicSelectDwellCommand::Evaluate {
                store,
                catalog_store,
                reviewed,
                output,
                policy,
            } => {
                let policies = dwell_policies(&policy);
                evaluate_music_select_dwell(&store, &catalog_store, &reviewed, &policies, &output)
                    .map_err(|error| format!("music-select dwell evaluation failed: {error}"))
                    .and_then(|value| print_json(&value, "music-select dwell evaluation"))
            }
            MusicSelectDwellCommand::EvaluateCorrectness {
                store,
                catalog_store,
                reviewed,
                labels,
                output,
                policy,
            } => {
                let policies = correctness_policies(&policy);
                evaluate_music_select_correctness(
                    &store,
                    &catalog_store,
                    &reviewed,
                    &labels,
                    &output,
                    &policies,
                )
                .map_err(|error| format!("music-select correctness evaluation failed: {error}"))
                .and_then(|value| print_json(&value, "music-select correctness evaluation"))
            }
        },
    }
}

fn temporal_policies(values: &[TemporalEvaluationPolicy]) -> Vec<TemporalEvaluationPolicy> {
    if values.is_empty() {
        return [
            TemporalEvaluationPolicy::new(2, 250).expect("default temporal policy is valid"),
            TemporalEvaluationPolicy::new(3, 250).expect("default temporal policy is valid"),
        ]
        .into();
    }
    values.to_vec()
}

fn parse_temporal_policy(value: &str) -> Result<TemporalEvaluationPolicy, String> {
    let (required, gap) = value
        .split_once(':')
        .ok_or_else(|| "policy requires OBSERVATIONS:GAP_MS".to_owned())?;
    TemporalEvaluationPolicy::new(
        required
            .parse()
            .map_err(|_| "policy observations must be an integer".to_owned())?,
        gap.parse()
            .map_err(|_| "policy gap must be an integer".to_owned())?,
    )
    .map_err(|error| error.to_string())
}

fn parse_dwell_policy(value: &str) -> Result<MusicSelectDwellPolicy, String> {
    MusicSelectDwellPolicy::new(
        value
            .parse()
            .map_err(|_| "policy dwell must be an integer".to_owned())?,
    )
    .map_err(|error| error.to_string())
}

fn dwell_policies(values: &[MusicSelectDwellPolicy]) -> Vec<MusicSelectDwellPolicy> {
    if values.is_empty() {
        return [100, 200, 300, 500]
            .map(|value| MusicSelectDwellPolicy::new(value).expect("default dwell policy is valid"))
            .into();
    }
    values.to_vec()
}

fn correctness_policies(
    values: &[MusicSelectTemporalCandidatePolicy],
) -> Vec<MusicSelectTemporalCandidatePolicy> {
    if values.is_empty() {
        return [100, 200, 300, 500]
            .into_iter()
            .flat_map(|dwell| [100, 200, 300].map(move |grace| (dwell, grace)))
            .map(|(dwell, grace)| {
                MusicSelectTemporalCandidatePolicy::new(dwell, grace)
                    .expect("default correctness policy is valid")
            })
            .collect();
    }
    values.to_vec()
}

fn parse_correctness_policy(value: &str) -> Result<MusicSelectTemporalCandidatePolicy, String> {
    let (dwell, grace) = value
        .split_once(':')
        .ok_or_else(|| "policy requires DWELL_MS:UNKNOWN_GRACE_MS".to_owned())?;
    MusicSelectTemporalCandidatePolicy::new(
        dwell
            .parse()
            .map_err(|_| "policy dwell must be an integer".to_owned())?,
        grace
            .parse()
            .map_err(|_| "policy grace must be an integer".to_owned())?,
    )
    .map_err(|error| error.to_string())
}

fn print_json(value: &impl serde::Serialize, context: &str) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(value)
            .map_err(|error| format!("{context} encoding failed: {error}"))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::{Cli, parse_temporal_policy};

    #[test]
    fn replay_options_are_order_independent() {
        assert!(
            Cli::try_parse_from([
                "scorepeek-corpus",
                "corpus",
                "replay",
                "--memory-mib",
                "2048",
                "--store",
                "/tmp/corpus",
                "--text-workers",
                "7",
            ])
            .is_ok()
        );
    }

    #[test]
    fn temporal_policy_requires_bounded_pair() {
        assert!(parse_temporal_policy("3:250").is_ok());
        for invalid in ["1:250", "2:0", "17:250", "2:60001", "2", "two:250"] {
            assert!(parse_temporal_policy(invalid).is_err());
        }
    }

    #[test]
    fn malformed_policies_are_usage_errors() {
        for arguments in [
            vec![
                "scorepeek-corpus",
                "temporal",
                "evaluate",
                "--store",
                "/tmp/corpus",
                "--policy",
                "nope",
            ],
            vec![
                "scorepeek-corpus",
                "music-select",
                "dwell",
                "evaluate-correctness",
                "--store",
                "/tmp/corpus",
                "--catalog-store",
                "/tmp/catalog",
                "--reviewed",
                "/tmp/reviewed",
                "--labels",
                "/tmp/labels",
                "--output",
                "/tmp/output",
                "--policy",
                "nope",
            ],
        ] {
            let Err(error) = Cli::try_parse_from(arguments) else {
                panic!("malformed policy must be rejected");
            };
            assert_eq!(error.exit_code(), 2);
        }
    }
}
