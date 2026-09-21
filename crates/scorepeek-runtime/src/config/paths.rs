//! XDG-owned scorepeek configuration paths.

use std::path::PathBuf;

pub fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
}
