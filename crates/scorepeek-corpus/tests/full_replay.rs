//! Explicit private-data entry point. `test = false, harness = false` keeps normal Cargo tests offline.

use std::path::PathBuf;
use std::process::ExitCode;

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

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let (Some(flag), Some(root), None) = (args.next(), args.next(), args.next()) else {
        eprintln!(
            "usage: cargo test --locked -p scorepeek-corpus --test full_replay -- (--store ABSOLUTE_STORE | --recording ABSOLUTE_RECORDING)"
        );
        return ExitCode::from(2);
    };
    if !PathBuf::from(&root).is_absolute() {
        eprintln!("full_replay requires an absolute directory");
        return ExitCode::from(2);
    }
    if flag == "--recording" {
        return match scorepeek_corpus::replay::replay_recording_with_progress(
            &PathBuf::from(root),
            &print_progress,
        ) {
            Ok(report) => match serde_json::to_string_pretty(&report) {
                Ok(document) => {
                    println!("{document}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("replay report encoding failed: {error}");
                    ExitCode::FAILURE
                }
            },
            Err(error) => {
                eprintln!("recording replay failed: {error:?}");
                ExitCode::FAILURE
            }
        };
    }
    if flag != "--store" {
        eprintln!("full_replay requires --store or --recording");
        return ExitCode::from(2);
    }
    match scorepeek_corpus::replay::replay_active_with_progress(
        &PathBuf::from(root),
        &print_progress,
    ) {
        Ok(reports) => {
            println!("replayed_sessions={}", reports.len());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("full corpus replay failed: {error:?}");
            ExitCode::FAILURE
        }
    }
}
