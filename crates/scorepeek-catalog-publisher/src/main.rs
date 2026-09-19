use std::env;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::ExitCode;

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
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("scorepeek catalog publisher failed: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[OsString]) -> Result<(), String> {
    match args {
        [command, rest @ ..] if command == "build" => build(parse_build(rest)?),
        [command, rest @ ..] if command == "select" => select(&parse_select(rest)?),
        [command, rest @ ..] if command == "verify" => verify(&parse_verify(rest)?),
        [command] if command == "--help" || command == "-h" => {
            print_usage();
            Ok(())
        }
        _ => Err("usage: scorepeek-catalog-publisher --help".to_owned()),
    }
}

struct BuildOptions {
    output: PathBuf,
    notices: PathBuf,
    work_directory: PathBuf,
    artifact_revision: u64,
    generator_commit: String,
    workflow_run_url: String,
}

fn parse_build(args: &[OsString]) -> Result<BuildOptions, String> {
    let values = exact_flags(
        args,
        &[
            "--output",
            "--notices",
            "--work-directory",
            "--artifact-revision",
            "--generator-commit",
            "--workflow-run-url",
        ],
    )?;
    Ok(BuildOptions {
        output: absolute(values[0], "output")?,
        notices: absolute(values[1], "notices")?,
        work_directory: absolute(values[2], "work directory")?,
        artifact_revision: text(values[3], "artifact revision")?
            .parse()
            .map_err(|_| "artifact revision must be a positive integer".to_owned())?,
        generator_commit: text(values[4], "generator commit")?.to_owned(),
        workflow_run_url: text(values[5], "workflow run URL")?.to_owned(),
    })
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

struct SelectOptions {
    candidate: PathBuf,
    current: PathBuf,
    output: PathBuf,
    work_directory: PathBuf,
}

fn parse_select(args: &[OsString]) -> Result<SelectOptions, String> {
    let values = exact_flags(
        args,
        &["--candidate", "--current", "--output", "--work-directory"],
    )?;
    Ok(SelectOptions {
        candidate: absolute(values[0], "candidate")?,
        current: absolute(values[1], "current")?,
        output: absolute(values[2], "output")?,
        work_directory: absolute(values[3], "work directory")?,
    })
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

struct VerifyOptions {
    artifact: PathBuf,
    output_directory: PathBuf,
}

fn parse_verify(args: &[OsString]) -> Result<VerifyOptions, String> {
    let values = exact_flags(args, &["--artifact", "--output-directory"])?;
    Ok(VerifyOptions {
        artifact: absolute(values[0], "artifact")?,
        output_directory: absolute(values[1], "output directory")?,
    })
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

fn exact_flags<'a>(args: &'a [OsString], flags: &[&str]) -> Result<Vec<&'a OsStr>, String> {
    if args.len() != flags.len() * 2 {
        return Err("command has a missing or unexpected argument".to_owned());
    }
    flags
        .iter()
        .enumerate()
        .map(|(index, expected)| {
            let flag = &args[index * 2];
            let value = &args[index * 2 + 1];
            if flag != expected || value.is_empty() {
                return Err(format!("expected {expected} VALUE"));
            }
            Ok(value.as_os_str())
        })
        .collect()
}

fn absolute(value: &OsStr, label: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if !path.is_absolute() || path.as_os_str().is_empty() {
        return Err(format!("{label} must be an absolute, non-empty path"));
    }
    Ok(path)
}

fn text<'a>(value: &'a OsStr, label: &str) -> Result<&'a str, String> {
    value
        .to_str()
        .ok_or_else(|| format!("{label} must be UTF-8"))
}

fn print_usage() {
    println!(
        "Usage:\n  scorepeek-catalog-publisher build --output ZIP --notices FILE --work-directory DIRECTORY --artifact-revision N --generator-commit SHA --workflow-run-url URL\n  scorepeek-catalog-publisher select --candidate ZIP --current ZIP --output ZIP --work-directory DIRECTORY\n  scorepeek-catalog-publisher verify --artifact ZIP --output-directory DIRECTORY"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publisher_commands_require_exact_ordered_flags() {
        let args = [
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
        assert!(parse_select(&args).is_ok());
        let mut swapped = args;
        swapped.swap(0, 2);
        assert!(parse_select(&swapped).is_err());
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
