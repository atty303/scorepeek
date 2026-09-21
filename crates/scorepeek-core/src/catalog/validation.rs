//! Catalog invariant validation entrypoint.

use super::Catalog;

/// Validates the complete portable catalog contract.
///
/// # Errors
/// Returns the first violated catalog invariant.
pub fn validate(catalog: &Catalog) -> Result<(), String> {
    catalog.validate()
}
