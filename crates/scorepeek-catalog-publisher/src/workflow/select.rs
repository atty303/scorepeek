//! Whole-artifact selection workflow.
use std::path::Path;

/// Selects and publishes the verified catalog artifact.
///
/// # Errors
/// Returns an error when selection, verification, or publication fails.
pub fn select(
    candidate: &Path,
    current: &Path,
    output: &Path,
    work: &Path,
) -> Result<serde_json::Value, String> {
    crate::artifact::select(candidate, current, output, work)
        .map_err(|error| error.to_string())
        .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string()))
}
