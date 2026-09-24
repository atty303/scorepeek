//! Publisher workspace locking, independent of the client catalog store.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

pub struct WorkspaceLock {
    _file: File,
}

impl WorkspaceLock {
    /// Holds the workspace lock throughout source acquisition and candidate construction.
    ///
    /// # Errors
    /// Returns an I/O error when the workspace cannot be locked.
    pub fn acquire(work_directory: &Path) -> io::Result<Self> {
        fs::create_dir_all(work_directory)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(work_directory.join("catalog-sync.lock"))?;
        file.lock()?;
        Ok(Self { _file: file })
    }
}
