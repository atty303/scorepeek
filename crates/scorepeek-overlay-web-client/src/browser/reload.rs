//! One-shot browser reload handling for host/client asset mismatches.

use crate::transport::connection::ASSET_VERSION;

const VERSION_RELOAD_KEY: &str = "scorepeek-overlay-version-reload";

pub(crate) fn reload_once_for_version_mismatch() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(Some(storage)) = window.session_storage() else {
        return;
    };
    if storage
        .get_item(VERSION_RELOAD_KEY)
        .ok()
        .flatten()
        .as_deref()
        == Some(ASSET_VERSION)
    {
        return;
    }
    if storage.set_item(VERSION_RELOAD_KEY, ASSET_VERSION).is_err() {
        return;
    }
    if window.location().reload().is_err() {
        let _ = storage.remove_item(VERSION_RELOAD_KEY);
    }
}

pub(crate) fn clear_version_reload_guard() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(Some(storage)) = window.session_storage() else {
        return;
    };
    let _ = storage.remove_item(VERSION_RELOAD_KEY);
}
