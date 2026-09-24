pub use scorepeek_overlay::{Backend, CanvasPresentation, Skin};
pub use scorepeek_overlay::{editor::EditorAction, editor::model::SCREENS};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead as _, BufReader},
    net::SocketAddr,
    path::PathBuf,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    pub backend: Backend,
    pub canvases: Vec<crate::config::Canvas>,
    pub config_path: PathBuf,
    pub control_socket: PathBuf,
    pub skin_store: PathBuf,
    pub socket: PathBuf,
    pub invocation: String,
    pub scores_db: Option<PathBuf>,
    pub listen: SocketAddr,
    pub unknown_grace_ms: u32,
    #[serde(default)]
    pub edit_on_start: bool,
}

pub use scorepeek_overlay_runtime::data::Feed;

/// Reads one configuration line; the remaining stdin pipe is the parent lifetime lease.
/// # Errors
/// Returns malformed configuration or stdin errors.
pub fn read_config() -> Result<(Config, BufReader<std::io::Stdin>), String> {
    let mut input = BufReader::new(std::io::stdin());
    let mut line = String::new();
    input
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    let config =
        serde_json::from_str(&line).map_err(|error| format!("overlay configuration: {error}"))?;
    Ok((config, input))
}

impl From<Config> for scorepeek_overlay_runtime::data::FeedConfig {
    fn from(config: Config) -> Self {
        Self {
            socket: config.socket,
            invocation: config.invocation,
            scores_db: config.scores_db,
            unknown_grace_ms: config.unknown_grace_ms,
        }
    }
}
