//! Runtime filesystem synchronization helpers.

#![allow(
    clippy::missing_errors_doc,
    reason = "internal platform adapter forwards the underlying I/O error"
)]

pub fn sync_directory(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}
