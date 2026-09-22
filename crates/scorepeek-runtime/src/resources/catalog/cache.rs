//! Catalog update-state cache and client staging recovery.

use std::fs::{self, File};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::DirBuilderExt as _;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tempfile::Builder;

use scorepeek_resources::{ActiveCatalog, CatalogStore};

use super::acquire::{UpdateError, UpdateErrorType, failure, state_error};

const STATE_MAX_BYTES: u64 = 16 * 1024;
pub(super) const STATE_SCHEMA: &str = "scorepeek-catalog-update-state-v1";
pub(super) const STATE_FILE: &str = "update-state.json";
pub(super) const DOWNLOAD_STAGING_PREFIX: &str = ".catalog-download-";
pub(super) const EXTRACTION_STAGING_PREFIX: &str = ".catalog-download-extracted-";
pub(super) const STATE_STAGING_PREFIX: &str = ".catalog-update-state-";
const MAX_VALIDATOR_BYTES: usize = 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogUpdateState {
    pub(super) schema: String,
    pub source_url_sha256: Option<String>,
    pub active_catalog_sha256: Option<String>,
    pub last_success_unix_seconds: Option<u64>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub last_failure: Option<UpdateFailure>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateFailure {
    pub source_url_sha256: String,
    pub failed_unix_seconds: u64,
    pub error_type: UpdateErrorType,
}

impl Default for CatalogUpdateState {
    fn default() -> Self {
        Self {
            schema: STATE_SCHEMA.to_owned(),
            source_url_sha256: None,
            active_catalog_sha256: None,
            last_success_unix_seconds: None,
            etag: None,
            last_modified: None,
            last_failure: None,
        }
    }
}

impl CatalogUpdateState {
    pub(super) fn activated(
        source_url_sha256: String,
        active_catalog_sha256: String,
        last_success_unix_seconds: u64,
        etag: Option<String>,
        last_modified: Option<String>,
    ) -> Self {
        Self {
            schema: STATE_SCHEMA.to_owned(),
            source_url_sha256: Some(source_url_sha256),
            active_catalog_sha256: Some(active_catalog_sha256),
            last_success_unix_seconds: Some(last_success_unix_seconds),
            etag,
            last_modified,
            last_failure: None,
        }
    }
}

/// Loads the state exposed through diagnostics and `doctor`.
///
/// # Errors
/// Returns an error when a present state file is malformed or outside its size contract.
pub fn load_state(store_root: &Path) -> Result<CatalogUpdateState, UpdateError> {
    let mut state = load_attempt_state(store_root)?;
    if let Some(active) = CatalogStore::new(store_root)
        .load_active_for_run()
        .map_err(|error| {
            failure(
                UpdateErrorType::ActivationFailed,
                format!("active catalog load failed: {error}"),
            )
        })?
    {
        state.active_catalog_sha256 = Some(active.digest);
        if let Some(origin) = active.origin {
            state.source_url_sha256 = Some(origin.source_url_sha256);
            state.last_success_unix_seconds = Some(origin.last_success_unix_seconds);
            state.etag = origin.etag;
            state.last_modified = origin.last_modified;
        } else {
            state.source_url_sha256 = None;
            state.last_success_unix_seconds = None;
            state.etag = None;
            state.last_modified = None;
        }
    }
    Ok(state)
}

pub(super) fn load_attempt_state(store_root: &Path) -> Result<CatalogUpdateState, UpdateError> {
    let path = store_root.join(STATE_FILE);
    let metadata = match path.metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CatalogUpdateState::default());
        }
        Err(error) => return Err(state_error(error)),
    };
    if !metadata.is_file() || metadata.len() > STATE_MAX_BYTES {
        return Err(failure(
            UpdateErrorType::StateFailed,
            "catalog update state is not a bounded regular file",
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(STATE_MAX_BYTES + 1).read_to_end(&mut bytes))
        .map_err(state_error)?;
    if bytes.len() as u64 > STATE_MAX_BYTES {
        return Err(failure(
            UpdateErrorType::StateFailed,
            "catalog update state exceeds its size limit while reading",
        ));
    }
    let state: CatalogUpdateState = serde_json::from_slice(&bytes).map_err(|error| {
        failure(
            UpdateErrorType::StateFailed,
            format!("catalog update state is invalid: {error}"),
        )
    })?;
    if state.schema != STATE_SCHEMA
        || state
            .source_url_sha256
            .as_ref()
            .is_some_and(|value| !is_lower_hex(value, 64))
        || state
            .active_catalog_sha256
            .as_ref()
            .is_some_and(|value| !is_lower_hex(value, 64))
        || !valid_validator(state.etag.as_deref())
        || !valid_validator(state.last_modified.as_deref())
    {
        return Err(failure(
            UpdateErrorType::StateFailed,
            "catalog update state violates its schema",
        ));
    }
    Ok(state)
}

pub(super) fn state_for_active(store_root: &Path, active: &ActiveCatalog) -> CatalogUpdateState {
    let mut state = load_attempt_state(store_root).unwrap_or_default();
    state.active_catalog_sha256 = Some(active.digest.clone());
    if let Some(origin) = &active.origin {
        state.source_url_sha256 = Some(origin.source_url_sha256.clone());
        state.last_success_unix_seconds = Some(origin.last_success_unix_seconds);
        state.etag.clone_from(&origin.etag);
        state.last_modified.clone_from(&origin.last_modified);
    } else {
        state.source_url_sha256 = None;
        state.last_success_unix_seconds = None;
        state.etag = None;
        state.last_modified = None;
    }
    state
}

pub(super) fn recover_client_staging(store_root: &Path) -> Result<(), UpdateError> {
    let mut removed = false;
    for entry in fs::read_dir(store_root).map_err(state_error)? {
        let entry = entry.map_err(state_error)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let metadata = entry.path().symlink_metadata().map_err(state_error)?;
        if name.starts_with(EXTRACTION_STAGING_PREFIX) {
            if !metadata.is_dir() {
                return Err(failure(
                    UpdateErrorType::StateFailed,
                    "catalog extraction staging entry is not a directory",
                ));
            }
            fs::remove_dir_all(entry.path()).map_err(state_error)?;
            removed = true;
        } else if name.starts_with(DOWNLOAD_STAGING_PREFIX)
            || name.starts_with(STATE_STAGING_PREFIX)
        {
            if !metadata.is_file() {
                return Err(failure(
                    UpdateErrorType::StateFailed,
                    "catalog file staging entry is not a file",
                ));
            }
            fs::remove_file(entry.path()).map_err(state_error)?;
            removed = true;
        }
    }
    if removed {
        File::open(store_root)
            .and_then(|directory| directory.sync_all())
            .map_err(state_error)?;
    }
    Ok(())
}

pub(super) fn write_state(
    store_root: &Path,
    state: &CatalogUpdateState,
) -> Result<(), UpdateError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(store_root)
        .map_err(state_error)?;
    let bytes = serde_json::to_vec(state).map_err(|error| {
        failure(
            UpdateErrorType::StateFailed,
            format!("catalog update state encoding failed: {error}"),
        )
    })?;
    if bytes.len() as u64 > STATE_MAX_BYTES {
        return Err(failure(
            UpdateErrorType::StateFailed,
            "catalog update state exceeds its size limit",
        ));
    }
    let mut temporary = Builder::new()
        .prefix(STATE_STAGING_PREFIX)
        .tempfile_in(store_root)
        .map_err(state_error)?;
    temporary
        .as_file_mut()
        .write_all(&bytes)
        .map_err(state_error)?;
    temporary
        .as_file_mut()
        .write_all(b"\n")
        .map_err(state_error)?;
    temporary.as_file().sync_all().map_err(state_error)?;
    temporary
        .persist(store_root.join(STATE_FILE))
        .map_err(|error| state_error(error.error))?;
    File::open(store_root)
        .and_then(|directory| directory.sync_all())
        .map_err(state_error)
}

fn valid_validator(value: Option<&str>) -> bool {
    value.is_none_or(|value| {
        value.len() <= MAX_VALIDATOR_BYTES && !value.chars().any(char::is_control)
    })
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
