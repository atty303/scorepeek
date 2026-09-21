//! Catalog artifact selection command.

use super::absolute_path;
use clap::Args;
use scorepeek_core::catalog::artifact;
use std::path::PathBuf;

#[derive(Args)]
pub struct SelectOptions {
    #[arg(long, value_parser = absolute_path)]
    candidate: PathBuf,
    #[arg(long, value_parser = absolute_path)]
    current: PathBuf,
    #[arg(long, value_parser = absolute_path)]
    output: PathBuf,
    #[arg(long, value_parser = absolute_path)]
    work_directory: PathBuf,
}

pub(crate) fn run(options: &SelectOptions) -> Result<(), String> {
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
