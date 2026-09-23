//! Filesystem adapter for the immutable registered OCR model bundle.

use std::fs::File;
use std::io::Read as _;
use std::path::Path;

use scorepeek_core::model::manifest::{
    registered_live_model_files, verify_registered_live_model_bundle_bytes,
};
use scorepeek_core::recognition::registered_field::RegisteredTextBundleBytes;
use scorepeek_core::recognition::title::OnnxParityError;

fn read_bundle(root: &Path) -> Result<Vec<(String, Vec<u8>)>, OnnxParityError> {
    let mut files = Vec::new();
    for registered in registered_live_model_files()? {
        let path = root.join(&registered.filename);
        let metadata = path.symlink_metadata()?;
        if !metadata.file_type().is_file() || metadata.len() != registered.bytes {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let mut bytes = Vec::with_capacity(
            usize::try_from(registered.bytes).map_err(|_| OnnxParityError::InvalidArtifact)?,
        );
        File::open(&path)?
            .take(registered.bytes + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != registered.bytes {
            return Err(OnnxParityError::InvalidArtifact);
        }
        files.push((registered.filename, bytes));
    }
    Ok(files)
}

fn borrowed(files: &[(String, Vec<u8>)]) -> Vec<(&str, &[u8])> {
    files
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect()
}

/// Verifies every registered file from one bounded directory.
///
/// # Errors
/// Rejects missing, changed, non-regular, or malformed files.
pub fn verify_registered_live_model_bundle(root: &Path) -> Result<(), OnnxParityError> {
    let files = read_bundle(root)?;
    verify_registered_live_model_bundle_bytes(&borrowed(&files)).map_err(Into::into)
}

/// Reads registered model bytes for core-owned OCR worker construction.
///
/// # Errors
/// Rejects changed bundle files before any OCR worker starts.
pub fn load_registered_text_bundle(
    root: &Path,
) -> Result<RegisteredTextBundleBytes, OnnxParityError> {
    let files = read_bundle(root)?;
    RegisteredTextBundleBytes::from_files(files)
}
