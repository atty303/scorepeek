//! Atomic cache publication primitives.

/// Synchronizes the parent directory after publishing a cache entry.
///
/// # Errors
/// Returns an I/O error when the parent is absent or cannot be synchronized.
pub fn sync_parent(path: &std::path::Path) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "cache path has no parent")
    })?;
    std::fs::File::open(parent)?.sync_all()
}
