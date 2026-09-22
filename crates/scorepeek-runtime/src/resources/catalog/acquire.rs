use std::env;
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _};
use std::net::IpAddr;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tempfile::Builder;
use url::Url;

use scorepeek_core::catalog::artifact;
use scorepeek_core::catalog::{ActiveCatalog, CatalogOrigin, CatalogStore};

use super::cache::{
    CatalogUpdateState, DOWNLOAD_STAGING_PREFIX, EXTRACTION_STAGING_PREFIX, UpdateFailure,
    load_attempt_state, recover_client_staging, state_for_active, write_state,
};
use super::schedule::{UpdateMode, update_due};

pub const DEFAULT_CATALOG_URL: &str = "https://atty303.github.io/scorepeek/catalog/v1/catalog.zip";
const CONFIG_MAX_BYTES: u64 = 64 * 1024;
const UPDATE_LOCK_FILE: &str = "catalog-client-update.lock";
const MAX_VALIDATOR_BYTES: usize = 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Deserialize)]
struct ConfigFile {
    catalog: Option<CatalogConfig>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogConfig {
    url: String,
}

#[derive(Clone, Debug)]
pub struct EffectiveUrl {
    value: Url,
    fingerprint: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStage {
    Resolve,
    Fetch,
    Extract,
    Activate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStatus {
    Started,
    Success,
    Error,
    NotModified,
}

#[derive(Clone, Debug, Serialize)]
pub struct UpdateEvent {
    pub mode: UpdateMode,
    pub stage: UpdateStage,
    pub status: UpdateStatus,
    pub source_url_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_type: Option<UpdateErrorType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_sha256: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateErrorType {
    ConfigurationInvalid,
    TransportFailed,
    HttpStatus,
    ArtifactInvalid,
    ActivationFailed,
    StateFailed,
}

#[derive(Debug)]
pub struct UpdateError {
    pub error_type: UpdateErrorType,
    detail: String,
}

#[derive(Clone, Debug)]
pub struct PreparedCatalog {
    pub active: ActiveCatalog,
    pub background_due: bool,
}

enum FetchResult {
    NotModified,
    Downloaded {
        artifact: tempfile::NamedTempFile,
        etag: Option<String>,
        last_modified: Option<String>,
    },
}

impl fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl Error for UpdateError {}

impl EffectiveUrl {
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

/// Resolves `SCOREPEEK_CATALOG_URL`, the ordinary config file, then the binary default.
///
/// # Errors
/// Returns an error for a malformed config or a URL outside HTTPS, loopback HTTP, and `file://`.
pub fn resolve_effective_url(config_path: &Path) -> Result<EffectiveUrl, UpdateError> {
    let configured = match env::var("SCOREPEEK_CATALOG_URL") {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => read_config_url(config_path)?,
        Err(env::VarError::NotUnicode(_)) => {
            return Err(failure(
                UpdateErrorType::ConfigurationInvalid,
                "SCOREPEEK_CATALOG_URL must be UTF-8",
            ));
        }
    };
    parse_url(configured.as_deref().unwrap_or(DEFAULT_CATALOG_URL))
}

/// Validates one configured catalog URL without consulting environment or filesystem state.
///
/// # Errors
/// Returns an error outside the accepted HTTPS, loopback HTTP, and `file://` URL contract.
pub fn validate_configured_url(value: &str) -> Result<(), UpdateError> {
    parse_url(value).map(|_| ())
}

/// Returns an immediately usable catalog or performs the required synchronous acquisition.
///
/// # Errors
/// If no catalog from the effective URL is active, any acquisition or activation failure blocks
/// startup and leaves an older active catalog untouched.
pub fn prepare(
    store_root: &Path,
    effective: &EffectiveUrl,
    mut report: impl FnMut(UpdateEvent),
) -> Result<PreparedCatalog, UpdateError> {
    report(event(
        UpdateMode::Required,
        UpdateStage::Resolve,
        UpdateStatus::Started,
        effective,
        None,
        None,
    ));
    let store = CatalogStore::new(store_root);
    let active = store.load_active_for_run().map_err(|error| {
        failure(
            UpdateErrorType::ActivationFailed,
            format!("active catalog load failed: {error}"),
        )
    })?;
    if let Some(active) = active.filter(|active| active_matches_url(active, effective)) {
        let state = state_for_active(store_root, &active);
        report(event(
            UpdateMode::Required,
            UpdateStage::Resolve,
            UpdateStatus::Success,
            effective,
            None,
            Some(active.digest.clone()),
        ));
        return Ok(PreparedCatalog {
            background_due: update_due(&state, effective),
            active,
        });
    }

    let _update_lock = begin_update_operation(store_root)?;
    if let Some(active) = store
        .load_active_for_run()
        .map_err(|error| {
            failure(
                UpdateErrorType::ActivationFailed,
                format!("active catalog load failed after update wait: {error}"),
            )
        })?
        .filter(|active| active_matches_url(active, effective))
    {
        let state = state_for_active(store_root, &active);
        report(event(
            UpdateMode::Required,
            UpdateStage::Resolve,
            UpdateStatus::Success,
            effective,
            None,
            Some(active.digest.clone()),
        ));
        return Ok(PreparedCatalog {
            background_due: update_due(&state, effective),
            active,
        });
    }
    run_update(store_root, effective, UpdateMode::Required, &mut report)?;
    let active = store
        .load_active_for_run()
        .map_err(|error| {
            failure(
                UpdateErrorType::ActivationFailed,
                format!("activated catalog load failed: {error}"),
            )
        })?
        .ok_or_else(|| {
            failure(
                UpdateErrorType::ActivationFailed,
                "catalog activation completed without an active catalog",
            )
        })?;
    if !active_matches_url(&active, effective) {
        return Err(failure(
            UpdateErrorType::ActivationFailed,
            "catalog activation was superseded before startup",
        ));
    }
    Ok(PreparedCatalog {
        active,
        background_due: false,
    })
}

#[allow(clippy::too_many_lines)]
pub(super) fn run_update(
    store_root: &Path,
    effective: &EffectiveUrl,
    mode: UpdateMode,
    report: &mut impl FnMut(UpdateEvent),
) -> Result<(), UpdateError> {
    let result: Result<(), UpdateError> = (|| {
        report(event(
            mode,
            UpdateStage::Fetch,
            UpdateStatus::Started,
            effective,
            None,
            None,
        ));
        let active_before = CatalogStore::new(store_root)
            .load_active_for_run()
            .map_err(|error| {
                failure(
                    UpdateErrorType::ActivationFailed,
                    format!("active catalog load failed: {error}"),
                )
            })?;
        let prior = active_before
            .as_ref()
            .map_or_else(CatalogUpdateState::default, |active| {
                state_for_active(store_root, active)
            });
        let use_validators = active_before.as_ref().is_some_and(|active| {
            active.origin.as_ref().is_some_and(|origin| {
                origin.source_url_sha256 == effective.fingerprint()
                    && (origin.etag.is_some() || origin.last_modified.is_some())
            })
        });
        let mut fetched = fetch(store_root, effective, use_validators.then_some(&prior))?;
        if matches!(fetched, FetchResult::NotModified)
            && !same_active_acquisition(store_root, active_before.as_ref())?
        {
            fetched = fetch(store_root, effective, None)?;
        }
        match fetched {
            FetchResult::NotModified => {
                let active = active_before.ok_or_else(|| {
                    failure(
                        UpdateErrorType::StateFailed,
                        "server returned 304 without an active catalog identity",
                    )
                })?;
                let previous_origin = active.origin.ok_or_else(|| {
                    failure(
                        UpdateErrorType::StateFailed,
                        "server returned 304 without active acquisition metadata",
                    )
                })?;
                let success_time = now_seconds();
                let origin = CatalogOrigin {
                    source_url_sha256: effective.fingerprint().to_owned(),
                    etag: previous_origin.etag,
                    last_modified: previous_origin.last_modified,
                    last_success_unix_seconds: success_time,
                };
                CatalogStore::new(store_root)
                    .refresh_active_origin(&active.digest, origin)
                    .map_err(|error| {
                        failure(
                            UpdateErrorType::ActivationFailed,
                            format!("catalog activation metadata refresh failed: {error}"),
                        )
                    })?;
                let digest = active.digest;
                let mut state = prior;
                state.source_url_sha256 = Some(effective.fingerprint().to_owned());
                state.active_catalog_sha256 = Some(digest.clone());
                state.last_success_unix_seconds = Some(success_time);
                state.last_failure = None;
                let _ = write_state(store_root, &state);
                report(event(
                    mode,
                    UpdateStage::Fetch,
                    UpdateStatus::NotModified,
                    effective,
                    None,
                    Some(digest),
                ));
                Ok(())
            }
            FetchResult::Downloaded {
                artifact,
                etag,
                last_modified,
            } => {
                report(event(
                    mode,
                    UpdateStage::Fetch,
                    UpdateStatus::Success,
                    effective,
                    None,
                    None,
                ));
                report(event(
                    mode,
                    UpdateStage::Extract,
                    UpdateStatus::Started,
                    effective,
                    None,
                    None,
                ));
                let extraction = tempfile::Builder::new()
                    .prefix(EXTRACTION_STAGING_PREFIX)
                    .tempdir_in(store_root)
                    .map_err(state_error)?;
                let extracted = artifact::extract_client_verified(
                    artifact.path(),
                    &extraction.path().join("artifact"),
                )
                .map_err(|error| failure(UpdateErrorType::ArtifactInvalid, error.to_string()))?;
                report(event(
                    mode,
                    UpdateStage::Extract,
                    UpdateStatus::Success,
                    effective,
                    None,
                    Some(extracted.manifest.sqlite_sha256.clone()),
                ));
                report(event(
                    mode,
                    UpdateStage::Activate,
                    UpdateStatus::Started,
                    effective,
                    None,
                    Some(extracted.manifest.sqlite_sha256.clone()),
                ));
                let expected_digest = extracted.manifest.sqlite_sha256.clone();
                let success_time = now_seconds();
                let origin = CatalogOrigin {
                    source_url_sha256: effective.fingerprint().to_owned(),
                    etag: etag.clone(),
                    last_modified: last_modified.clone(),
                    last_success_unix_seconds: success_time,
                };
                let state = CatalogUpdateState::activated(
                    effective.fingerprint().to_owned(),
                    expected_digest.clone(),
                    success_time,
                    etag,
                    last_modified,
                );
                let digest = CatalogStore::new(store_root)
                    .install_verified_snapshot_from(
                        &extracted.catalog_path,
                        &expected_digest,
                        Some(origin),
                    )
                    .map_err(|error| {
                        failure(
                            UpdateErrorType::ActivationFailed,
                            format!("catalog activation failed: {error}"),
                        )
                    })?;
                let _ = write_state(store_root, &state);
                report(event(
                    mode,
                    UpdateStage::Activate,
                    UpdateStatus::Success,
                    effective,
                    None,
                    Some(digest),
                ));
                Ok(())
            }
        }
    })();
    if let Err(error) = &result {
        let mut state = load_attempt_state(store_root).unwrap_or_default();
        state.last_failure = Some(UpdateFailure {
            source_url_sha256: effective.fingerprint().to_owned(),
            failed_unix_seconds: now_seconds(),
            error_type: error.error_type,
        });
        let _ = write_state(store_root, &state);
        report(event(
            mode,
            stage_for_error(error.error_type),
            UpdateStatus::Error,
            effective,
            Some(error.error_type),
            None,
        ));
    }
    result
}

fn fetch(
    store_root: &Path,
    effective: &EffectiveUrl,
    prior: Option<&CatalogUpdateState>,
) -> Result<FetchResult, UpdateError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(store_root)
        .map_err(state_error)?;
    match effective.value.scheme() {
        "file" => {
            let mut temporary = download_temporary(store_root)?;
            let path = effective.value.to_file_path().map_err(|()| {
                failure(
                    UpdateErrorType::ConfigurationInvalid,
                    "file catalog URL cannot be converted to a local path",
                )
            })?;
            copy_download(
                &mut File::open(path).map_err(transport_error)?,
                &mut temporary,
            )?;
            temporary.as_file().sync_all().map_err(transport_error)?;
            Ok(FetchResult::Downloaded {
                artifact: temporary,
                etag: None,
                last_modified: None,
            })
        }
        "https" | "http" => {
            let mut config = ureq::Agent::config_builder()
                .http_status_as_error(false)
                .https_only(effective.value.scheme() == "https")
                .max_redirects(0)
                .timeout_global(Some(REQUEST_TIMEOUT))
                .user_agent(format!(
                    "scorepeek/{} (+https://github.com/atty303/scorepeek)",
                    env!("CARGO_PKG_VERSION")
                ));
            if effective.value.scheme() == "http" {
                config = config.proxy(None);
            }
            let config = config.build();
            let agent = config.new_agent();
            let mut response = request_catalog(&agent, effective, prior)?;
            if prior.is_some() && !matches!(response.status().as_u16(), 200 | 304) {
                response = request_catalog(&agent, effective, None)?;
            }
            match response.status().as_u16() {
                304 => Ok(FetchResult::NotModified),
                200 => {
                    if response
                        .body()
                        .content_length()
                        .is_some_and(|value| value > artifact::MAX_ARTIFACT_BYTES)
                    {
                        return Err(failure(
                            UpdateErrorType::ArtifactInvalid,
                            "catalog response exceeds the ZIP size limit",
                        ));
                    }
                    let etag = response_header(&response, "etag")?;
                    let last_modified = response_header(&response, "last-modified")?;
                    let mut temporary = download_temporary(store_root)?;
                    let mut reader = response.body_mut().as_reader();
                    copy_download(&mut reader, &mut temporary)?;
                    temporary.as_file().sync_all().map_err(transport_error)?;
                    Ok(FetchResult::Downloaded {
                        artifact: temporary,
                        etag,
                        last_modified,
                    })
                }
                status => Err(failure(
                    UpdateErrorType::HttpStatus,
                    format!("catalog request returned HTTP {status}"),
                )),
            }
        }
        _ => unreachable!("effective URL validation admits only supported schemes"),
    }
}

fn request_catalog(
    agent: &ureq::Agent,
    effective: &EffectiveUrl,
    prior: Option<&CatalogUpdateState>,
) -> Result<ureq::http::Response<ureq::Body>, UpdateError> {
    let mut request = agent.get(effective.value.as_str());
    if let Some(etag) = prior.and_then(|state| state.etag.as_deref()) {
        request = request.header("If-None-Match", etag);
    }
    if let Some(modified) = prior.and_then(|state| state.last_modified.as_deref()) {
        request = request.header("If-Modified-Since", modified);
    }
    request
        .call()
        .map_err(|_| failure(UpdateErrorType::TransportFailed, "catalog request failed"))
}

fn download_temporary(store_root: &Path) -> Result<tempfile::NamedTempFile, UpdateError> {
    Builder::new()
        .prefix(DOWNLOAD_STAGING_PREFIX)
        .tempfile_in(store_root)
        .map_err(state_error)
}

fn same_active_acquisition(
    store_root: &Path,
    expected: Option<&ActiveCatalog>,
) -> Result<bool, UpdateError> {
    let current = CatalogStore::new(store_root)
        .load_active_for_run()
        .map_err(|error| {
            failure(
                UpdateErrorType::ActivationFailed,
                format!("active catalog load failed: {error}"),
            )
        })?;
    Ok(match (expected, current.as_ref()) {
        (Some(expected), Some(current)) => {
            expected.digest == current.digest && expected.origin == current.origin
        }
        (None, None) => true,
        _ => false,
    })
}

fn active_matches_url(active: &ActiveCatalog, effective: &EffectiveUrl) -> bool {
    active
        .origin
        .as_ref()
        .is_some_and(|origin| origin.source_url_sha256 == effective.fingerprint())
}

fn response_header(
    response: &ureq::http::Response<ureq::Body>,
    name: &str,
) -> Result<Option<String>, UpdateError> {
    let Some(value) = response.headers().get(name) else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| {
        failure(
            UpdateErrorType::TransportFailed,
            format!("catalog response {name} header is invalid"),
        )
    })?;
    if value.len() > MAX_VALIDATOR_BYTES || value.chars().any(char::is_control) {
        return Err(failure(
            UpdateErrorType::TransportFailed,
            format!("catalog response {name} header exceeds its contract"),
        ));
    }
    Ok(Some(value.to_owned()))
}

fn copy_download(
    input: &mut impl io::Read,
    output: &mut tempfile::NamedTempFile,
) -> Result<(), UpdateError> {
    let copied = io::copy(
        &mut input.take(artifact::MAX_ARTIFACT_BYTES + 1),
        output.as_file_mut(),
    )
    .map_err(transport_error)?;
    if copied > artifact::MAX_ARTIFACT_BYTES {
        return Err(failure(
            UpdateErrorType::ArtifactInvalid,
            "catalog ZIP exceeds its size limit while reading",
        ));
    }
    Ok(())
}

fn read_config_url(path: &Path) -> Result<Option<String>, UpdateError> {
    let metadata = match path.metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(state_error(error)),
    };
    if !metadata.is_file() || metadata.len() > CONFIG_MAX_BYTES {
        return Err(failure(
            UpdateErrorType::ConfigurationInvalid,
            "scorepeek config is not a bounded regular file",
        ));
    }
    let mut text = String::new();
    File::open(path)
        .and_then(|file| file.take(CONFIG_MAX_BYTES + 1).read_to_string(&mut text))
        .map_err(state_error)?;
    if text.len() as u64 > CONFIG_MAX_BYTES {
        return Err(failure(
            UpdateErrorType::ConfigurationInvalid,
            "scorepeek config exceeds its size limit while reading",
        ));
    }
    let config: ConfigFile = toml::from_str(&text).map_err(|_| {
        failure(
            UpdateErrorType::ConfigurationInvalid,
            "scorepeek config is invalid",
        )
    })?;
    Ok(config.catalog.map(|catalog| catalog.url))
}

fn parse_url(value: &str) -> Result<EffectiveUrl, UpdateError> {
    let url = Url::parse(value).map_err(|error| {
        failure(
            UpdateErrorType::ConfigurationInvalid,
            format!("catalog URL is invalid: {error}"),
        )
    })?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(failure(
            UpdateErrorType::ConfigurationInvalid,
            "catalog URL must not contain credentials or a fragment",
        ));
    }
    let accepted = match url.scheme() {
        "https" => true,
        "file" => url.host_str().is_none_or(str::is_empty),
        "http" => url.host_str().is_some_and(is_loopback_host),
        _ => false,
    };
    if !accepted {
        return Err(failure(
            UpdateErrorType::ConfigurationInvalid,
            "catalog URL must use HTTPS, loopback HTTP, or file://",
        ));
    }
    let fingerprint = hex(&Sha256::digest(url.as_str().as_bytes()));
    Ok(EffectiveUrl {
        value: url,
        fingerprint,
    })
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
            .unwrap_or(host)
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

pub(super) fn begin_update_operation(store_root: &Path) -> Result<File, UpdateError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(store_root)
        .map_err(state_error)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(store_root.join(UPDATE_LOCK_FILE))
        .map_err(state_error)?;
    lock.lock().map_err(state_error)?;
    recover_client_staging(store_root)?;
    Ok(lock)
}

fn event(
    mode: UpdateMode,
    stage: UpdateStage,
    status: UpdateStatus,
    effective: &EffectiveUrl,
    error_type: Option<UpdateErrorType>,
    catalog_sha256: Option<String>,
) -> UpdateEvent {
    UpdateEvent {
        mode,
        stage,
        status,
        source_url_sha256: effective.fingerprint().to_owned(),
        error_type,
        catalog_sha256,
    }
}

const fn stage_for_error(error: UpdateErrorType) -> UpdateStage {
    match error {
        UpdateErrorType::ConfigurationInvalid | UpdateErrorType::StateFailed => {
            UpdateStage::Resolve
        }
        UpdateErrorType::TransportFailed | UpdateErrorType::HttpStatus => UpdateStage::Fetch,
        UpdateErrorType::ArtifactInvalid => UpdateStage::Extract,
        UpdateErrorType::ActivationFailed => UpdateStage::Activate,
    }
}

pub(super) fn failure(error_type: UpdateErrorType, detail: impl Into<String>) -> UpdateError {
    UpdateError {
        error_type,
        detail: detail.into(),
    }
}

#[allow(clippy::needless_pass_by_value)]
pub(super) fn state_error(error: io::Error) -> UpdateError {
    failure(
        UpdateErrorType::StateFailed,
        format!("catalog update state I/O failed: {error}"),
    )
}

#[allow(clippy::needless_pass_by_value)]
fn transport_error(error: io::Error) -> UpdateError {
    failure(
        UpdateErrorType::TransportFailed,
        format!("catalog transfer failed: {error}"),
    )
}

pub(super) fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(value, "{byte:02x}").expect("writing to a String cannot fail");
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::path::PathBuf;

    use super::super::cache::{STATE_FILE, STATE_SCHEMA, STATE_STAGING_PREFIX};
    use super::super::schedule::update_background;
    use crate::catalog::artifact::{ARTIFACT_SCHEMA, ArtifactManifest};
    use crate::catalog::test_support::{SyntheticTachiRecord, catalog_from_tachi};
    use crate::catalog::{Chart, ChartKey, Difficulty, PlayType};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn url_policy_accepts_only_the_distribution_and_test_schemes() {
        assert!(parse_url("https://example.test/catalog.zip").is_ok());
        assert!(parse_url("http://127.0.0.1:8000/catalog.zip").is_ok());
        assert!(parse_url("http://[::1]:8000/catalog.zip").is_ok());
        assert!(parse_url("file:///tmp/catalog.zip").is_ok());
        assert!(parse_url("http://example.test/catalog.zip").is_err());
        assert!(parse_url("https://user@example.test/catalog.zip").is_err());
        assert!(parse_url("ftp://example.test/catalog.zip").is_err());
    }

    #[test]
    fn malformed_config_never_echoes_a_catalog_url() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.toml");
        let secret = "catalog-secret-value";
        fs::write(
            &config,
            format!(
                "[catalog]\nurl = \"https://example.test/catalog.zip?token={secret}\" trailing\n"
            ),
        )
        .unwrap();

        let error = read_config_url(&config).unwrap_err().to_string();
        assert_eq!(error, "scorepeek config is invalid");
        assert!(!error.contains(secret));
    }

    #[test]
    fn http_redirect_is_not_followed_outside_the_validated_url() {
        let root = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request(&mut stream);
            stream
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: http://example.com/catalog.zip\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            stream.flush().unwrap();
        });

        let effective = parse_url(&format!("http://{address}/catalog.zip")).unwrap();
        let error = prepare(&root.path().join("client"), &effective, |_| {}).unwrap_err();
        assert_eq!(error.error_type, UpdateErrorType::HttpStatus);
        server.join().unwrap();
    }

    #[test]
    fn failed_effective_url_is_due_even_after_recent_success() {
        let effective = parse_url("https://example.test/catalog.zip").unwrap();
        let state = CatalogUpdateState {
            schema: STATE_SCHEMA.to_owned(),
            source_url_sha256: Some(effective.fingerprint().to_owned()),
            active_catalog_sha256: Some("a".repeat(64)),
            last_success_unix_seconds: Some(now_seconds()),
            etag: None,
            last_modified: None,
            last_failure: Some(UpdateFailure {
                source_url_sha256: effective.fingerprint().to_owned(),
                failed_unix_seconds: now_seconds(),
                error_type: UpdateErrorType::TransportFailed,
            }),
        };
        assert!(update_due(&state, &effective));
    }

    #[test]
    fn file_artifact_first_install_and_url_change_failure_keep_the_old_active_catalog() {
        let root = tempfile::tempdir().unwrap();
        let artifact = build_synthetic_artifact(root.path(), "ALPHA");
        let client = root.path().join("client");
        let effective = parse_url(Url::from_file_path(&artifact).unwrap().as_str()).unwrap();
        let prepared = prepare(&client, &effective, |_| {}).unwrap();
        assert!(!prepared.background_due);
        let original_digest = prepared.active.digest;

        let broken = root.path().join("broken.zip");
        fs::write(&broken, b"not a zip").unwrap();
        let changed = parse_url(Url::from_file_path(&broken).unwrap().as_str()).unwrap();
        let error = prepare(&client, &changed, |_| {}).unwrap_err();
        assert_eq!(error.error_type, UpdateErrorType::ArtifactInvalid);
        assert_eq!(
            CatalogStore::new(&client)
                .load_active()
                .unwrap()
                .unwrap()
                .digest,
            original_digest
        );
    }

    #[test]
    fn valid_active_starts_without_waiting_for_a_background_update_lock() {
        let root = tempfile::tempdir().unwrap();
        let artifact = build_synthetic_artifact(root.path(), "ALPHA");
        let client = root.path().join("client-nonblocking");
        let effective = parse_url(Url::from_file_path(&artifact).unwrap().as_str()).unwrap();
        prepare(&client, &effective, |_| {}).unwrap();
        let held_lock = begin_update_operation(&client).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let client_for_thread = client.clone();
        let effective_for_thread = effective.clone();
        let runner = thread::spawn(move || {
            sender
                .send(prepare(&client_for_thread, &effective_for_thread, |_| {}).is_ok())
                .unwrap();
        });

        assert!(receiver.recv_timeout(Duration::from_secs(1)).unwrap());
        drop(held_lock);
        runner.join().unwrap();
    }

    #[test]
    fn lock_wait_recheck_emits_a_terminal_resolve_event() {
        let root = tempfile::tempdir().unwrap();
        let artifact_path = build_synthetic_artifact(root.path(), "ALPHA");
        let extracted = artifact::extract_client_verified(
            &artifact_path,
            &root.path().join("lock-wait-extracted"),
        )
        .unwrap();
        let client = root.path().join("client-lock-wait");
        let effective = parse_url(Url::from_file_path(&artifact_path).unwrap().as_str()).unwrap();
        let held_lock = begin_update_operation(&client).unwrap();
        let (started_sender, started_receiver) = std::sync::mpsc::channel();
        let (result_sender, result_receiver) = std::sync::mpsc::channel();
        let client_for_thread = client.clone();
        let effective_for_thread = effective.clone();
        let runner = thread::spawn(move || {
            let mut events = Vec::new();
            let result = prepare(&client_for_thread, &effective_for_thread, |event| {
                if events.is_empty() {
                    started_sender.send(()).unwrap();
                }
                events.push(event);
            });
            result_sender.send((result.is_ok(), events)).unwrap();
        });
        started_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        CatalogStore::new(&client)
            .install_verified_snapshot_from(
                &extracted.catalog_path,
                &extracted.manifest.sqlite_sha256,
                Some(CatalogOrigin {
                    source_url_sha256: effective.fingerprint().to_owned(),
                    etag: None,
                    last_modified: None,
                    last_success_unix_seconds: now_seconds(),
                }),
            )
            .unwrap();
        drop(held_lock);

        let (succeeded, events) = result_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert!(succeeded);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].stage, UpdateStage::Resolve);
        assert_eq!(events[0].status, UpdateStatus::Started);
        assert_eq!(events[1].stage, UpdateStage::Resolve);
        assert_eq!(events[1].status, UpdateStatus::Success);
        runner.join().unwrap();
    }

    #[test]
    fn corrupt_diagnostic_state_does_not_block_a_valid_active_catalog() {
        let root = tempfile::tempdir().unwrap();
        let artifact = build_synthetic_artifact(root.path(), "ALPHA");
        let client = root.path().join("client-corrupt-state");
        let effective = parse_url(Url::from_file_path(&artifact).unwrap().as_str()).unwrap();
        let first = prepare(&client, &effective, |_| {}).unwrap();
        fs::write(client.join(STATE_FILE), b"not json").unwrap();

        let recovered = prepare(&client, &effective, |_| {}).unwrap();
        assert_eq!(recovered.active.digest, first.active.digest);
        assert!(!recovered.background_due);
    }

    #[test]
    fn activation_failure_keeps_same_url_lkg_usable_and_due_for_retry() {
        let root = tempfile::tempdir().unwrap();
        let first_artifact = build_synthetic_artifact(root.path(), "ALPHA");
        let candidate_artifact = build_synthetic_artifact(root.path(), "BETA");
        let candidate_bytes = fs::read(candidate_artifact).unwrap();
        let client = root.path().join("client-capacity");
        let first_url = parse_url(Url::from_file_path(&first_artifact).unwrap().as_str()).unwrap();
        let first = prepare(&client, &first_url, |_| {}).unwrap();
        let first_digest = first.active.digest;

        for index in 0..31 {
            let filler = client.join("content").join(format!("filler-{index}"));
            fs::create_dir(&filler).unwrap();
            fs::write(filler.join("catalog.sqlite3"), []).unwrap();
        }

        fs::write(&first_artifact, candidate_bytes).unwrap();
        let error = update_background(&client, &first_url, |_| {}).unwrap_err();
        assert_eq!(error.error_type, UpdateErrorType::ActivationFailed);
        assert_eq!(
            CatalogStore::new(&client)
                .load_active_for_run()
                .unwrap()
                .unwrap()
                .digest,
            first_digest
        );
        let recovered = prepare(&client, &first_url, |_| {}).unwrap();
        assert_eq!(recovered.active.digest, first_digest);
        assert!(recovered.background_due);
    }

    #[test]
    fn update_lock_recovers_only_owned_root_staging_entries() {
        let root = tempfile::tempdir().unwrap();
        let download = root.path().join(format!("{DOWNLOAD_STAGING_PREFIX}orphan"));
        let state = root.path().join(format!("{STATE_STAGING_PREFIX}orphan"));
        let extraction = root
            .path()
            .join(format!("{EXTRACTION_STAGING_PREFIX}orphan"));
        let unrelated = root.path().join("keep-me");
        fs::write(&download, b"partial").unwrap();
        fs::write(&state, b"partial").unwrap();
        fs::create_dir(&extraction).unwrap();
        fs::write(extraction.join("partial"), b"partial").unwrap();
        fs::write(&unrelated, b"unrelated").unwrap();

        drop(begin_update_operation(root.path()).unwrap());

        assert!(!download.exists());
        assert!(!state.exists());
        assert!(!extraction.exists());
        assert!(unrelated.exists());
    }

    #[test]
    fn conditional_get_uses_etag_and_304_keeps_the_artifact() {
        let root = tempfile::tempdir().unwrap();
        let artifact_path = build_synthetic_artifact(root.path(), "ALPHA");
        let artifact = fs::read(artifact_path).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let first_request = read_request(&mut first);
            assert!(!first_request.contains("If-None-Match"));
            write!(
                first,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"catalog-v1\"\r\nConnection: close\r\n\r\n",
                artifact.len()
            )
            .unwrap();
            first.write_all(&artifact).unwrap();
            first.flush().unwrap();
            drop(first);

            let (mut second, _) = listener.accept().unwrap();
            let second_request = read_request(&mut second);
            assert!(
                second_request
                    .to_ascii_lowercase()
                    .contains("if-none-match: \"catalog-v1\"")
            );
            second
                .write_all(
                    b"HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            second.flush().unwrap();
        });

        let effective = parse_url(&format!("http://{address}/catalog.zip")).unwrap();
        let client = root.path().join("client-http");
        let first = prepare(&client, &effective, |_| {}).unwrap();
        let digest = first.active.digest;
        age_active_origin(&client, &effective);
        let second = prepare(&client, &effective, |_| {}).unwrap();
        assert!(second.background_due);
        update_background(&client, &effective, |_| {}).unwrap();
        assert_eq!(
            CatalogStore::new(&client)
                .load_active()
                .unwrap()
                .unwrap()
                .digest,
            digest
        );
        server.join().unwrap();
    }

    #[test]
    fn unsupported_conditional_get_retries_without_validators() {
        let root = tempfile::tempdir().unwrap();
        let artifact_path = build_synthetic_artifact(root.path(), "ALPHA");
        let artifact = fs::read(artifact_path).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let _ = read_request(&mut first);
            write!(
                first,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"catalog-v1\"\r\nConnection: close\r\n\r\n",
                artifact.len()
            )
            .unwrap();
            first.write_all(&artifact).unwrap();
            first.flush().unwrap();

            let (mut conditional, _) = listener.accept().unwrap();
            let conditional_request = read_request(&mut conditional);
            assert!(
                conditional_request
                    .to_ascii_lowercase()
                    .contains("if-none-match: \"catalog-v1\"")
            );
            conditional
                .write_all(
                    b"HTTP/1.1 412 Precondition Failed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            conditional.flush().unwrap();

            let (mut fallback, _) = listener.accept().unwrap();
            let fallback_request = read_request(&mut fallback);
            assert!(
                !fallback_request
                    .to_ascii_lowercase()
                    .contains("if-none-match")
            );
            write!(
                fallback,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"catalog-v1\"\r\nConnection: close\r\n\r\n",
                artifact.len()
            )
            .unwrap();
            fallback.write_all(&artifact).unwrap();
            fallback.flush().unwrap();
        });

        let effective = parse_url(&format!("http://{address}/catalog.zip")).unwrap();
        let client = root.path().join("client-fallback");
        prepare(&client, &effective, |_| {}).unwrap();
        age_active_origin(&client, &effective);
        assert!(prepare(&client, &effective, |_| {}).unwrap().background_due);
        update_background(&client, &effective, |_| {}).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn stale_validator_is_not_used_after_an_active_digest_change() {
        let root = tempfile::tempdir().unwrap();
        let artifact_path = build_synthetic_artifact(root.path(), "ALPHA");
        let replacement_path = build_synthetic_artifact(root.path(), "BETA");
        let artifact = fs::read(&artifact_path).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_request(&mut stream);
                assert!(!request.to_ascii_lowercase().contains("if-none-match"));
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"catalog-v1\"\r\nConnection: close\r\n\r\n",
                    artifact.len()
                )
                .unwrap();
                stream.write_all(&artifact).unwrap();
                stream.flush().unwrap();
            }
        });

        let effective = parse_url(&format!("http://{address}/catalog.zip")).unwrap();
        let client = root.path().join("client-stale-validator");
        let first = prepare(&client, &effective, |_| {}).unwrap();
        let first_digest = first.active.digest;

        let replacement = artifact::extract_client_verified(
            &replacement_path,
            &root.path().join("replacement-extracted"),
        )
        .unwrap();
        let replacement_digest = CatalogStore::new(&client)
            .install_verified_snapshot(
                &replacement.catalog_path,
                &replacement.manifest.sqlite_sha256,
            )
            .unwrap();
        assert_ne!(replacement_digest, first_digest);

        let recovered = prepare(&client, &effective, |_| {}).unwrap();
        assert_eq!(recovered.active.digest, first_digest);
        server.join().unwrap();
    }

    fn build_synthetic_artifact(root: &Path, title: &str) -> PathBuf {
        let catalog = catalog_from_tachi(&[SyntheticTachiRecord {
            id: "anchor-1",
            title,
            title_kind: crate::catalog::DisplayVariantKind::InGameDisplay,
            artist: "ARTIST A",
            version: "V1",
            charts: vec![Chart {
                key: ChartKey {
                    play_type: PlayType::Single,
                    difficulty: Difficulty::Normal,
                },
                level: 4,
                notes: 400,
            }],
            primary_infinitas: true,
        }]);
        let store_root = root.join(format!("producer-{title}"));
        let active = CatalogStore::new(&store_root)
            .begin_update()
            .unwrap()
            .publish(&catalog)
            .unwrap();
        let notices = root.join(format!("notices-{title}.md"));
        fs::write(&notices, b"synthetic notice\n").unwrap();
        let artifact_path = root.join(format!("catalog-{title}.zip"));
        artifact::build(
            &CatalogStore::new(&store_root)
                .snapshot_path(&active.digest)
                .unwrap(),
            &notices,
            &artifact_path,
            &ArtifactManifest {
                schema: ARTIFACT_SCHEMA.to_owned(),
                sqlite_sha256: active.digest,
                semantic_digest: catalog.semantic_digest(),
                artifact_revision: 1,
                generator_commit: "a".repeat(40),
                workflow_run_url: None,
            },
        )
        .unwrap();
        artifact::verify_publisher(
            &artifact_path,
            &root.join(format!("publisher-verify-{title}")),
        )
        .unwrap();
        artifact_path
    }

    fn age_active_origin(client: &Path, effective: &EffectiveUrl) {
        let active = CatalogStore::new(client)
            .load_active_for_run()
            .unwrap()
            .unwrap();
        let origin = active.origin.unwrap();
        CatalogStore::new(client)
            .refresh_active_origin(
                &active.digest,
                CatalogOrigin {
                    source_url_sha256: effective.fingerprint().to_owned(),
                    etag: origin.etag,
                    last_modified: origin.last_modified,
                    last_success_unix_seconds: 0,
                },
            )
            .unwrap();
    }

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !bytes.ends_with(b"\r\n\r\n") {
            let read = stream.read(&mut buffer).unwrap();
            assert!(read > 0);
            bytes.extend_from_slice(&buffer[..read]);
            assert!(bytes.len() < 16 * 1024);
        }
        String::from_utf8(bytes).unwrap()
    }
}
