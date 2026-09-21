use clap::{Args, ValueEnum};

#[derive(Clone, Copy, Default, ValueEnum)]
pub(super) enum OutputFormat {
    #[default]
    Human,
    Json,
}

#[derive(Args, Default)]
pub(super) struct FormatArgs {
    #[arg(long, value_enum, default_value_t)]
    pub format: OutputFormat,
}
