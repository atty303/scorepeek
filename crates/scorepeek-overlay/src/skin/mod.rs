//! Skin identity and schema shared by all overlay backends.

mod manifest;

pub use manifest::Skin;

/// Validates the reverse-domain skin ID used by the Wasm and native adapters.
///
/// # Errors
/// Returns an error when the ID is not a lowercase ASCII reverse-domain name.
pub fn validate_id(value: &str) -> Result<(), String> {
    let segments = value.split('.').collect::<Vec<_>>();
    if segments.len() < 2
        || segments.iter().any(|segment| {
            segment.is_empty()
                || (!segment.as_bytes()[0].is_ascii_lowercase()
                    && !segment.as_bytes()[0].is_ascii_digit())
                || !segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
    {
        return Err("skin id must be a lowercase ASCII reverse-domain name".into());
    }
    Ok(())
}
