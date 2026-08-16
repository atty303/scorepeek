use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use scorepeek_corpus::CorpusStore;

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[OsString]) -> Result<(), String> {
    match args {
        [command, store, root, source, request] if command == "ingest" && store == "--store" => {
            let manifest = CorpusStore::new(PathBuf::from(root))
                .ingest(PathBuf::from(source), PathBuf::from(request))
                .map_err(|error| format!("scorepeek-corpus ingest failed: {error}"))?;
            let summary = manifest
                .summary()
                .map_err(|error| format!("ingest result validation failed: {error}"))?;
            println!(
                "{}",
                serde_json::to_string(&summary)
                    .map_err(|error| format!("ingest result encoding failed: {error}"))?
            );
            Ok(())
        }
        [replay, validate, store, root, suite]
            if replay == "replay" && validate == "validate" && store == "--store" =>
        {
            let summary = CorpusStore::new(PathBuf::from(root))
                .validate_replay_suite(PathBuf::from(suite))
                .map_err(|error| format!("invalid replay suite: {error}"))?;
            println!(
                "{}",
                serde_json::to_string(&summary)
                    .map_err(|error| format!("replay summary encoding failed: {error}"))?
            );
            Ok(())
        }
        [generation, seal, store, root, generation_id]
            if generation == "generation" && seal == "seal" && store == "--store" =>
        {
            let summary = CorpusStore::new(PathBuf::from(root))
                .seal_generation(&generation_id.to_string_lossy())
                .map_err(|error| format!("scorepeek-corpus generation seal failed: {error}"))?;
            println!(
                "{}",
                serde_json::to_string(&summary)
                    .map_err(|error| format!("generation summary encoding failed: {error}"))?
            );
            Ok(())
        }
        [flag] if flag == "--help" || flag == "-h" => {
            print_usage();
            Ok(())
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("scorepeek-corpus {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => Err(
            "usage: scorepeek-corpus <ingest --store ROOT SOURCE REQUEST|generation seal --store ROOT GENERATION_ID|replay validate --store ROOT SUITE>"
                .to_owned(),
        ),
    }
}

fn print_usage() {
    println!(
        "scorepeek-corpus {}\n\nUsage:\n  scorepeek-corpus ingest --store ROOT SOURCE REQUEST\n  scorepeek-corpus generation seal --store ROOT GENERATION_ID\n  scorepeek-corpus replay validate --store ROOT SUITE",
        env!("CARGO_PKG_VERSION")
    );
}
