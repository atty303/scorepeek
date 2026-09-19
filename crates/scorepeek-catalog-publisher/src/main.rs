#[cfg(test)]
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use scorepeek::catalog::CatalogStore;
use scorepeek::catalog::artifact::{self, ArtifactManifest};
use serde::Serialize;

mod catalog_generation;

use catalog_generation::{CatalogSync, CatalogSyncSource, QuarantineReason};

#[derive(Serialize)]
struct BuildSummary {
    schema: &'static str,
    published_candidate: bool,
    sqlite_sha256: String,
    semantic_digest: String,
    artifact_revision: u64,
    sources: std::collections::BTreeMap<scorepeek::catalog::SourceId, CatalogSyncSource>,
    quarantine_counts: std::collections::BTreeMap<QuarantineReason, usize>,
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
    let result = match cli.command {
        Command::Build(options) => build(options),
        Command::Select(options) => select(&options),
        Command::Verify(options) => verify(&options),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("scorepeek catalog publisher failed: {error}");
            ExitCode::from(1)
        }
    }
}

#[derive(Parser)]
#[command(name = "scorepeek-catalog-publisher", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Build(BuildOptions),
    Select(SelectOptions),
    Verify(VerifyOptions),
}

#[derive(Args)]
struct BuildOptions {
    #[arg(long, value_parser = absolute_path)]
    output: PathBuf,
    #[arg(long, value_parser = absolute_path)]
    notices: PathBuf,
    #[arg(long, value_parser = absolute_path)]
    work_directory: PathBuf,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    artifact_revision: u64,
    #[arg(long)]
    generator_commit: String,
    #[arg(long)]
    workflow_run_url: String,
}

fn build(options: BuildOptions) -> Result<(), String> {
    let metadata = options
        .work_directory
        .metadata()
        .map_err(|error| format!("work directory inspection failed: {error}"))?;
    if !metadata.is_dir() {
        return Err("work directory must be an existing directory".to_owned());
    }
    let store_root = options.work_directory.join("catalog-store");
    let cache_root = options.work_directory.join("source-cache");
    let result = CatalogSync::new(&store_root, cache_root)
        .sync()
        .map_err(|error| error.to_string())?;
    if !result.activated {
        return Err("candidate was rejected by whole-catalog policy".to_owned());
    }
    let active = CatalogStore::new(&store_root)
        .load_active()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "generation completed without an active candidate".to_owned())?;
    let semantic_digest = active.catalog.semantic_digest();
    let manifest = ArtifactManifest {
        schema: artifact::ARTIFACT_SCHEMA.to_owned(),
        sqlite_sha256: active.digest.clone(),
        semantic_digest: semantic_digest.clone(),
        artifact_revision: options.artifact_revision,
        generator_commit: options.generator_commit,
        workflow_run_url: Some(options.workflow_run_url),
    };
    let catalog_path = CatalogStore::new(&store_root)
        .snapshot_path(&active.digest)
        .map_err(|error| error.to_string())?;
    artifact::build(&catalog_path, &options.notices, &options.output, &manifest)
        .map_err(|error| error.to_string())?;
    let verification = options.work_directory.join("publisher-verification");
    artifact::verify_publisher(&options.output, &verification)
        .map_err(|error| error.to_string())?;
    let summary = result.into_summary();
    println!(
        "{}",
        serde_json::to_string(&BuildSummary {
            schema: "scorepeek-catalog-publisher-build-v1",
            published_candidate: true,
            sqlite_sha256: active.digest,
            semantic_digest,
            artifact_revision: options.artifact_revision,
            sources: summary.sources,
            quarantine_counts: summary.quarantine_counts,
        })
        .map_err(|error| format!("build summary encoding failed: {error}"))?
    );
    Ok(())
}

#[derive(Args)]
struct SelectOptions {
    #[arg(long, value_parser = absolute_path)]
    candidate: PathBuf,
    #[arg(long, value_parser = absolute_path)]
    current: PathBuf,
    #[arg(long, value_parser = absolute_path)]
    output: PathBuf,
    #[arg(long, value_parser = absolute_path)]
    work_directory: PathBuf,
}

fn select(options: &SelectOptions) -> Result<(), String> {
    let selection = artifact::select(
        &options.candidate,
        &options.current,
        &options.output,
        &options.work_directory,
    )
    .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::json!({
            "schema": "scorepeek-catalog-publisher-selection-v1",
            "selection": selection,
        })
    );
    Ok(())
}

#[derive(Args)]
struct VerifyOptions {
    #[arg(long, value_parser = absolute_path)]
    artifact: PathBuf,
    #[arg(long, value_parser = absolute_path)]
    output_directory: PathBuf,
}

fn verify(options: &VerifyOptions) -> Result<(), String> {
    let manifest = artifact::verify_publisher(&options.artifact, &options.output_directory)
        .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&manifest)
            .map_err(|error| format!("manifest encoding failed: {error}"))?
    );
    Ok(())
}

fn absolute_path(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if !path.is_absolute() || path.as_os_str().is_empty() {
        return Err("path must be absolute and non-empty".to_owned());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publisher_commands_accept_order_independent_named_options() {
        let args = [
            "scorepeek-catalog-publisher",
            "select",
            "--candidate",
            "/tmp/candidate.zip",
            "--current",
            "/tmp/current.zip",
            "--output",
            "/tmp/output.zip",
            "--work-directory",
            "/tmp/work",
        ]
        .map(OsString::from);
        assert!(Cli::try_parse_from(args.clone()).is_ok());
        let mut swapped = args;
        swapped.swap(2, 4);
        assert!(Cli::try_parse_from(swapped).is_ok());
    }

    #[test]
    fn distribution_notices_retain_each_source_policy() {
        let notices = include_str!("../../../THIRD_PARTY_NOTICES.md");
        for required in [
            "Tachi IIDX seeds",
            "Unlicense",
            "Textage",
            "textage.cc/score/readme.html",
            "dqn/iidxapi",
            "License: ISC",
        ] {
            assert!(notices.contains(required), "missing notice: {required}");
        }
    }
}
