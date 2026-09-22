//! Shared source-cache capacity admission.

use std::fs;
use std::path::Path;

#[derive(Debug)]
pub enum CapacityError {
    Io(std::io::Error),
    InvalidEntry,
    Exceeded,
}

impl From<std::io::Error> for CapacityError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Admits new content only after measuring every existing cache generation.
///
/// # Errors
/// Returns `InvalidEntry` for an unrecognized generation, `Exceeded` at either configured bound,
/// or `Io` when inventory cannot be read.
pub fn ensure(
    directory: &Path,
    incoming_bytes: u64,
    maximum_generations: usize,
    maximum_bytes: u64,
    mut measure: impl FnMut(&fs::DirEntry) -> Result<u64, CapacityError>,
) -> Result<(), CapacityError> {
    let mut generations = 0_usize;
    let mut total_bytes = 0_u64;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let bytes = measure(&entry)?;
        generations = generations.saturating_add(1);
        total_bytes = total_bytes.saturating_add(bytes);
        if generations >= maximum_generations || total_bytes > maximum_bytes {
            return Err(CapacityError::Exceeded);
        }
    }
    if incoming_bytes > maximum_bytes || total_bytes.saturating_add(incoming_bytes) > maximum_bytes
    {
        return Err(CapacityError::Exceeded);
    }
    Ok(())
}
