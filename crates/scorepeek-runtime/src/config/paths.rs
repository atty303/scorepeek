//! XDG-owned scorepeek configuration paths and precedence.

use std::path::PathBuf;

pub fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
}

#[must_use]
pub fn default_path() -> PathBuf {
    config_home().map_or_else(
        || {
            std::env::var_os("HOME").map_or_else(
                || PathBuf::from(".config/scorepeek/config.toml"),
                |home| PathBuf::from(home).join(".config/scorepeek/config.toml"),
            )
        },
        |root| root.join("scorepeek/config.toml"),
    )
}

pub(crate) fn resolve(cli: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = cli {
        return nonempty(path, "--config");
    }
    match std::env::var_os("SCOREPEEK_CONFIG") {
        Some(path) => nonempty(PathBuf::from(path), "SCOREPEEK_CONFIG"),
        None => Ok(default_path()),
    }
}

fn nonempty(path: PathBuf, label: &str) -> Result<PathBuf, String> {
    (!path.as_os_str().is_empty())
        .then_some(path)
        .ok_or_else(|| format!("{label} must not be empty"))
}
