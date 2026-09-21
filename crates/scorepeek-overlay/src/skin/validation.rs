//! Skin identifier validation.

/// Validates a skin identifier at the package boundary.
///
/// # Errors
/// Returns an error when the identifier violates the skin contract.
pub fn validate_id(value: &str) -> Result<(), String> {
    crate::validate_skin_id(value)
}
