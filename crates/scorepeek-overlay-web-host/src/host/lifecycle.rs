use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, path::PathBuf};

pub use crate::bridge::data::Feed;
pub use scorepeek_overlay::Backend;

/// Immutable configuration admitted for one Web-host process lifetime.
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
