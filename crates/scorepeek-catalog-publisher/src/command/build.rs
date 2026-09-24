//! Catalog build command.

use super::absolute_path;
use crate::artifact::{self};
use crate::{CatalogSync, CatalogSyncSource, QuarantineReason};
use clap::Args;
use scorepeek_resources::artifact::ArtifactManifest;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize)]
struct BuildSummary {
    schema: &'static str,
    published_candidate: bool,
    sqlite_sha256: String,
    semantic_digest: String,
    artifact_revision: u64,
    sources: std::collections::BTreeMap<scorepeek_core::catalog::SourceId, CatalogSyncSource>,
    quarantine_counts: std::collections::BTreeMap<QuarantineReason, usize>,
}

#[derive(Args)]
pub struct BuildOptions {
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

pub(crate) fn run(options: BuildOptions) -> Result<(), String> {
    let metadata = options
        .work_directory
        .metadata()
        .map_err(|error| format!("work directory inspection failed: {error}"))?;
    if !metadata.is_dir() {
        return Err("work directory must be an existing directory".to_owned());
    }
    let cache_root = options.work_directory.join("source-cache");
    let result = CatalogSync::new(&options.work_directory, cache_root)
        .sync()
        .map_err(|error| error.to_string())?;
    let catalog = result
        .catalog
        .as_ref()
        .ok_or_else(|| "candidate was rejected by whole-catalog policy".to_owned())?;
    let snapshot = tempfile::tempdir_in(&options.work_directory)
        .map_err(|error| format!("snapshot staging failed: {error}"))?;
    let catalog_path = snapshot.path().join("catalog.sqlite3");
    crate::snapshot::write_snapshot(&catalog_path, catalog).map_err(|error| error.to_string())?;
    let sqlite_sha256 = artifact::digest_bounded(&catalog_path, 128 * 1024 * 1024)
        .map_err(|error| error.to_string())?;
    let semantic_digest = catalog.semantic_digest();
    let manifest = ArtifactManifest {
        schema: artifact::ARTIFACT_SCHEMA.to_owned(),
        sqlite_sha256: sqlite_sha256.clone(),
        semantic_digest: semantic_digest.clone(),
        artifact_revision: options.artifact_revision,
        generator_commit: options.generator_commit,
        workflow_run_url: Some(options.workflow_run_url),
    };
    let output_parent = options.output.parent().ok_or("output has no parent")?;
    let candidate = tempfile::tempdir_in(output_parent)
        .map_err(|error| format!("candidate staging failed: {error}"))?;
    let candidate_path = candidate.path().join("candidate.zip");
    artifact::build(&catalog_path, &options.notices, &candidate_path, &manifest)
        .map_err(|error| error.to_string())?;
    let verification = tempfile::tempdir_in(&options.work_directory)
        .map_err(|error| format!("verification staging failed: {error}"))?;
    artifact::verify_publisher(&candidate_path, &verification.path().join("extracted"))
        .map_err(|error| error.to_string())?;
    std::fs::hard_link(&candidate_path, &options.output)
        .map_err(|error| format!("candidate output failed: {error}"))?;
    if let Err(error) = std::fs::File::open(output_parent).and_then(|parent| parent.sync_all()) {
        std::fs::remove_file(&options.output).map_err(|remove| {
            format!("candidate output sync failed: {error}; cleanup failed: {remove}")
        })?;
        return Err(format!("candidate output sync failed: {error}"));
    }
    let summary = result.into_summary();
    println!(
        "{}",
        serde_json::to_string(&BuildSummary {
            schema: "scorepeek-catalog-publisher-build-v1",
            published_candidate: true,
            sqlite_sha256,
            semantic_digest,
            artifact_revision: options.artifact_revision,
            sources: summary.sources,
            quarantine_counts: summary.quarantine_counts,
        })
        .map_err(|error| format!("build summary encoding failed: {error}"))?
    );
    Ok(())
}
