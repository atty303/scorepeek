use crate::host::lifecycle::Config;
pub use scorepeek_overlay_runtime::data::Feed;
use std::io::{BufRead as _, BufReader};

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
