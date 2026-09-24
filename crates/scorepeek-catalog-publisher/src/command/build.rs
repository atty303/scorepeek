//! Catalog build command.

use super::absolute_path;
use crate::artifact::{self};
use crate::store::CatalogStore;
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
