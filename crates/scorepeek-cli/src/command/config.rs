use super::doctor::FormatArgs;
use clap::Subcommand;

#[derive(Subcommand)]
pub(super) enum ConfigCommand {
    Path(FormatArgs),
    Show(FormatArgs),
    Check(FormatArgs),
}
