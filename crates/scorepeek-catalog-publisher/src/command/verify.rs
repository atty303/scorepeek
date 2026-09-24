//! Catalog artifact verification command.

use super::absolute_path;
use crate::artifact;
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct VerifyOptions {
    #[arg(long, value_parser = absolute_path)]
    artifact: PathBuf,
    #[arg(long, value_parser = absolute_path)]
    output_directory: PathBuf,
    #[arg(long)]
    client_only: bool,
}

pub(crate) fn run(options: &VerifyOptions) -> Result<(), String> {
    let manifest = if options.client_only {
        scorepeek_resources::artifact::extract_client_verified(
            &options.artifact,
            &options.output_directory,
        )
        .map_err(|error| error.to_string())?
        .manifest
    } else {
        artifact::verify_publisher(&options.artifact, &options.output_directory)
            .map_err(|error| error.to_string())?
    };
    println!(
        "{}",
        serde_json::to_string(&manifest)
            .map_err(|error| format!("manifest encoding failed: {error}"))?
    );
    Ok(())
}
