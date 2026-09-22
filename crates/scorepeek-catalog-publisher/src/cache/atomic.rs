//! Durable cache directory and publication primitives.

use std::fs::{self, File};
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::Path;

/// Creates missing scorepeek-owned cache directories privately and durably.
///
/// # Errors
/// Returns an I/O error when an ancestor is not a directory or a path cannot be synchronized.
pub fn create_private_directory(path: &Path) -> io::Result<()> {
    let mut missing = Vec::new();
    let mut candidate = path;
    loop {
        match candidate.metadata() {
            Ok(metadata) if metadata.is_dir() => break,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "cache ancestor is not a directory",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(candidate.to_owned());
                candidate = candidate.parent().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "directory has no existing ancestor",
                    )
                })?;
            }
            Err(error) => return Err(error),
        }
    }
    for directory in missing.into_iter().rev() {
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        sync_directory_and_parent(&directory)?;
    }
    sync_directory_and_parent(path)
}

/// Synchronizes a cache directory and its parent after creation or publication.
///
/// # Errors
/// Returns an I/O error when either directory cannot be synchronized.
pub fn sync_directory_and_parent(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}
