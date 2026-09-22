//! Scorepeek-owned cache staging recovery.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StagingKind {
    File,
    Directory,
}

#[derive(Debug)]
pub enum RecoveryError {
    Io(std::io::Error),
    UnexpectedEntry(PathBuf),
}

impl From<std::io::Error> for RecoveryError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Removes only staging entries owned by one source cache and synchronizes the directory.
///
/// # Errors
/// Returns an I/O error for filesystem failures, or `UnexpectedEntry` when an owned staging name
/// has the wrong filesystem type.
pub fn recover(directory: &Path, prefix: &str, expected: StagingKind) -> Result<(), RecoveryError> {
    let mut removed = false;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(prefix))
        {
            continue;
        }
        let path = entry.path();
        let metadata = path.symlink_metadata()?;
        match expected {
            StagingKind::File if metadata.is_file() => fs::remove_file(path)?,
            StagingKind::Directory if metadata.is_dir() => fs::remove_dir_all(path)?,
            StagingKind::File | StagingKind::Directory => {
                return Err(RecoveryError::UnexpectedEntry(path));
            }
        }
        removed = true;
    }
    if removed {
        File::open(directory)?.sync_all()?;
    }
    Ok(())
}
