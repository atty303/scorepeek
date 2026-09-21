use super::doctor::FormatArgs;
use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub(super) enum DiagnosticCommand {
    Observe {
        #[arg(long, value_name = "SECONDS", value_parser = clap::value_parser!(u64).range(1..))]
        replay: Option<u64>,
    },
    Inspect(DiagnosticInspectArgs),
}

#[derive(Args)]
#[group(required = true, multiple = false, args = ["latest", "run_id"])]
pub(super) struct DiagnosticInspectArgs {
    #[arg(long, conflicts_with = "run_id")]
    pub latest: bool,
    #[arg(long, value_name = "ID", conflicts_with = "latest")]
    pub run_id: Option<String>,
    #[command(flatten)]
    pub format: FormatArgs,
}
