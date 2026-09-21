//! Scorepeek-owned cache staging detection.

#[must_use]
pub fn is_owned_staging(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|value| value.starts_with(".scorepeek-cache-staging-"))
}
