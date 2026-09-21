//! Current executable discovery for private self-exec roles.

#![allow(
    clippy::missing_errors_doc,
    reason = "internal platform adapter forwards the underlying I/O error"
)]

pub fn current_exe() -> std::io::Result<std::path::PathBuf> {
    std::env::current_exe()
}
