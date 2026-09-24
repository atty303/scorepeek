use super::doctor::FormatArgs;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub(super) enum SkinCommand {
    Install {
        package: PathBuf,
        #[arg(long)]
        force: bool,
    },
    Uninstall {
        id: String,
    },
    List(FormatArgs),
}
