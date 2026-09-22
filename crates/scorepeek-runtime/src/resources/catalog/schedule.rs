//! Catalog refresh scheduling policy.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use super::acquire::{
    EffectiveUrl, UpdateError, UpdateEvent, begin_update_operation, now_seconds, run_update,
};
use super::cache::CatalogUpdateState;

pub const UPDATE_INTERVAL: Duration = Duration::from_hours(24);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateMode {
    Required,
    Background,
}

/// Performs one due background update without changing the caller's already loaded catalog.
///
/// # Errors
/// Returns a typed update error after recording retry state; an existing active catalog is kept.
pub fn update_background(
    store_root: &Path,
    effective: &EffectiveUrl,
    mut report: impl FnMut(UpdateEvent),
) -> Result<(), UpdateError> {
    let _update_lock = begin_update_operation(store_root)?;
    run_update(store_root, effective, UpdateMode::Background, &mut report)
}

pub(super) fn update_due(state: &CatalogUpdateState, effective: &EffectiveUrl) -> bool {
    if state.last_failure.as_ref().is_some_and(|failure| {
        failure.source_url_sha256 == effective.fingerprint()
            && state
                .last_success_unix_seconds
                .is_none_or(|success| failure.failed_unix_seconds >= success)
    }) {
        return true;
    }
    state
        .last_success_unix_seconds
        .is_none_or(|success| now_seconds().saturating_sub(success) >= UPDATE_INTERVAL.as_secs())
}
