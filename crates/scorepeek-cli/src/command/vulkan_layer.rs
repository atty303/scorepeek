use clap::Subcommand;

#[derive(Clone, Copy, Subcommand)]
pub(super) enum VulkanLayerCommand {
    Install,
    Uninstall,
}
