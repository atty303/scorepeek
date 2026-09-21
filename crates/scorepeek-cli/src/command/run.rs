use clap::{Args, ValueEnum};
use std::path::PathBuf;

#[derive(Clone, Copy, ValueEnum)]
pub(super) enum CaptureKind {
    Pipewire,
    VulkanLayer,
}

#[derive(Args, Default)]
#[allow(clippy::struct_excessive_bools)]
pub(super) struct RunArgs {
    #[arg(long, value_enum)]
    pub capture: Option<CaptureKind>,
    #[arg(long, value_name = "NAME")]
    pub node_name: Option<String>,
    #[arg(long)]
    pub crop_left: Option<u32>,
    #[arg(long)]
    pub crop_top: Option<u32>,
    #[arg(long)]
    pub crop_right: Option<u32>,
    #[arg(long)]
    pub crop_bottom: Option<u32>,
    #[arg(long, value_name = "FILE", conflicts_with = "no_scores")]
    pub scores_db: Option<PathBuf>,
    #[arg(long, conflicts_with = "scores")]
    pub no_scores: bool,
    #[arg(long, conflicts_with = "no_scores")]
    pub scores: bool,
    #[arg(long, conflicts_with_all = ["no_record", "record_all"])]
    pub record: bool,
    #[arg(long, conflicts_with_all = ["no_record", "record"])]
    pub record_all: bool,
    #[arg(long, conflicts_with_all = ["record", "record_all"])]
    pub no_record: bool,
    #[arg(long, value_name = "MIB")]
    pub record_memory_mib: Option<usize>,
    #[arg(long, conflicts_with = "no_overlay_wayland")]
    pub overlay_wayland: bool,
    #[arg(long, conflicts_with = "overlay_wayland")]
    pub no_overlay_wayland: bool,
    #[arg(long, conflicts_with = "no_overlay_wayland_edit")]
    pub overlay_wayland_edit: bool,
    #[arg(long, conflicts_with = "overlay_wayland_edit")]
    pub no_overlay_wayland_edit: bool,
    #[arg(long, conflicts_with = "no_overlay_obs")]
    pub overlay_obs: bool,
    #[arg(long, conflicts_with = "overlay_obs")]
    pub no_overlay_obs: bool,
    #[arg(long, value_name = "FILE")]
    pub overlay_config: Option<PathBuf>,
}
