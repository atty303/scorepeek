use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{Read as _, Seek as _, SeekFrom, Write};
use std::ops::Range;
use std::os::unix::fs::{DirBuilderExt as _, FileExt as _, OpenOptionsExt as _};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt as _;
use object_store::aws::{AmazonS3Builder, AmazonS3ConfigKey};
use object_store::path::Path as ObjectPath;
use object_store::{MultipartUpload, ObjectStore, ObjectStoreExt};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt as _;
use url::Url;

use crate::CorpusError;

const URL_ENV: &str = "SCOREPEEK_CORPUS_S3_URL";
const REGION_ENV: &str = "SCOREPEEK_CORPUS_S3_REGION";
const ENDPOINT_ENV: &str = "SCOREPEEK_CORPUS_S3_ENDPOINT";
const PATH_STYLE_ENV: &str = "SCOREPEEK_CORPUS_S3_PATH_STYLE";
const TRANSFER_BUFFER_BYTES: usize = 8 * 1024 * 1024;
const MAX_CONCURRENT_SEGMENT_DOWNLOADS: usize = 2;
const RANGES_PER_SEGMENT: u64 = 4;
const SEGMENT_DOWNLOAD_RETRY_DELAY: Duration = Duration::from_millis(250);
const MAX_DIAGNOSTIC_EVENTS: usize = 4096;
const MAX_DIAGNOSTIC_RUNS: usize = 32;

#[derive(Clone)]
pub(crate) struct SegmentRemote {
    target: Arc<RemoteTarget>,
    transferred: Arc<AtomicU64>,
    reused: Arc<AtomicU64>,
    downloaded_segments: Arc<AtomicU64>,
    downloaded_bytes: Arc<AtomicU64>,
    diagnostics: Arc<RemoteDiagnostics>,
    uploads: Arc<UploadCoordinator>,
    downloads: Arc<DownloadCoordinator>,
}

pub(crate) struct RemoteSegment {
    file: File,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RemoteMetrics {
    pub transferred_objects: u64,
    pub reused_objects: u64,
    pub downloaded_segments: u64,
    pub downloaded_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UploadDisposition {
    Transferred,
    Reused,
}

struct RemoteTarget {
    store: Arc<dyn ObjectStore>,
    prefix: String,
}

struct RemoteEnvironment {
    url: String,
    region: String,
    endpoint: Option<String>,
    path_style: bool,
}

enum RemoteCredentials {
    Static,
    WebIdentity,
    ContainerRelative,
    ContainerFull,
}

struct RemoteDiagnostics {
    path: Option<PathBuf>,
    started: Instant,
    events: Mutex<Vec<RemoteDiagnosticEvent>>,
    dropped: AtomicU64,
}

#[derive(Default)]
struct UploadCoordinator {
    active: Mutex<BTreeSet<String>>,
    ready: Condvar,
}

struct UploadPermit<'a> {
    coordinator: &'a UploadCoordinator,
    sha256: String,
}

#[derive(Default)]
struct DownloadCoordinator {
    active: Mutex<usize>,
    ready: Condvar,
}

struct DownloadPermit<'a> {
    coordinator: &'a DownloadCoordinator,
}

#[derive(Serialize)]
struct RemoteDiagnosticEvent {
    schema: &'static str,
    operation: String,
    status: &'static str,
    error_type: Option<String>,
    object_sha256: String,
    object_bytes: u64,
    elapsed_us: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_delay_us: Option<u64>,
}

impl SegmentRemote {
    pub(crate) fn from_environment() -> Result<Option<Self>, CorpusError> {
        let values = std::env::vars_os()
            .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
            .collect::<BTreeMap<_, _>>();
        let Some(environment) = RemoteEnvironment::parse(&values)? else {
            return Ok(None);
        };
        let (bucket, prefix) = parse_s3_url(&environment.url)?;
        let endpoint = environment
            .endpoint
            .as_deref()
            .map(|origin| client_endpoint(origin, &bucket, environment.path_style))
            .transpose()?;
        let credentials = RemoteCredentials::parse(&values)?;
        let mut builder = credentials
            .apply(AmazonS3Builder::new(), &values)
            .with_bucket_name(bucket)
            .with_region(environment.region)
            .with_virtual_hosted_style_request(!environment.path_style)
            .with_unsigned_payload(false);
        if let Some(endpoint) = endpoint {
            builder = builder.with_endpoint(endpoint);
        }
        let store = builder.build().map_err(|_| {
            remote_error(
                "invalid_configuration",
                "remote client configuration failed",
            )
        })?;
        Self::new_with_diagnostics(Arc::new(store), prefix, RemoteDiagnostics::production())
            .map(Some)
    }

    #[cfg(test)]
    pub(crate) fn new(store: Arc<dyn ObjectStore>, prefix: String) -> Result<Self, CorpusError> {
        Self::new_with_diagnostics(store, prefix, RemoteDiagnostics::disabled())
    }

    fn new_with_diagnostics(
        store: Arc<dyn ObjectStore>,
        prefix: String,
        diagnostics: RemoteDiagnostics,
    ) -> Result<Self, CorpusError> {
        let target = RemoteTarget { store, prefix };
        target.path("frame-corpus/v1/configuration-check")?;
        Ok(Self {
            target: Arc::new(target),
            transferred: Arc::new(AtomicU64::new(0)),
            reused: Arc::new(AtomicU64::new(0)),
            downloaded_segments: Arc::new(AtomicU64::new(0)),
            downloaded_bytes: Arc::new(AtomicU64::new(0)),
            diagnostics: Arc::new(diagnostics),
            uploads: Arc::new(UploadCoordinator::default()),
            downloads: Arc::new(DownloadCoordinator::default()),
        })
    }

    pub(crate) fn upload_verified(
        &self,
        source: File,
        sha256: &str,
        bytes: u64,
    ) -> Result<UploadDisposition, CorpusError> {
        let started = Instant::now();
        let result = self.upload_verified_inner(source, sha256, bytes);
        self.diagnostics
            .record("segment_upload", sha256, bytes, started, &result);
        result
    }

    fn upload_verified_inner(
        &self,
        source: File,
        sha256: &str,
        bytes: u64,
    ) -> Result<UploadDisposition, CorpusError> {
        validate_sha256(sha256)?;
        let _permit = self.uploads.acquire(sha256);
        let runtime = remote_runtime()?;
        let final_path = self.target.object_path(sha256)?;
        if runtime.block_on(remote_exists_with_size(
            Arc::clone(&self.target.store),
            &final_path,
            bytes,
        ))? {
            self.reused.fetch_add(1, Ordering::AcqRel);
            return Ok(UploadDisposition::Reused);
        }
        runtime.block_on(upload_file_verified(
            Arc::clone(&self.target.store),
            &final_path,
            source,
            sha256,
            bytes,
        ))?;
        self.transferred.fetch_add(1, Ordering::AcqRel);
        Ok(UploadDisposition::Transferred)
    }

    pub(crate) fn materialize(
        &self,
        sha256: &str,
        bytes: u64,
    ) -> Result<RemoteSegment, CorpusError> {
        let started = Instant::now();
        let result = self.materialize_inner(sha256, bytes);
        self.diagnostics
            .record("segment_get", sha256, bytes, started, &result);
        result
    }

    fn materialize_inner(&self, sha256: &str, bytes: u64) -> Result<RemoteSegment, CorpusError> {
        let _permit = self.downloads.acquire();
        let mut file = tempfile::tempfile().map_err(CorpusError::Io)?;
        let runtime = remote_runtime()?;
        let path = self.target.object_path(sha256)?;
        retry_segment_download(
            || {
                runtime.block_on(download_verified(
                    Arc::clone(&self.target.store),
                    &path,
                    sha256,
                    bytes,
                    &mut file,
                ))
            },
            |error| {
                self.diagnostics.record_retry(
                    "segment_get",
                    sha256,
                    bytes,
                    SEGMENT_DOWNLOAD_RETRY_DELAY,
                    error,
                );
                std::thread::sleep(SEGMENT_DOWNLOAD_RETRY_DELAY);
            },
        )?;
        file.seek(SeekFrom::Start(0)).map_err(CorpusError::Io)?;
        self.downloaded_segments.fetch_add(1, Ordering::AcqRel);
        self.downloaded_bytes.fetch_add(bytes, Ordering::AcqRel);
        Ok(RemoteSegment { file })
    }

    pub(crate) fn metrics(&self) -> RemoteMetrics {
        RemoteMetrics {
            transferred_objects: self.transferred.load(Ordering::Acquire),
            reused_objects: self.reused.load(Ordering::Acquire),
            downloaded_segments: self.downloaded_segments.load(Ordering::Acquire),
            downloaded_bytes: self.downloaded_bytes.load(Ordering::Acquire),
        }
    }
}

impl UploadCoordinator {
    fn acquire(&self, sha256: &str) -> UploadPermit<'_> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !active.insert(sha256.to_owned()) {
            active = self
                .ready
                .wait(active)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        UploadPermit {
            coordinator: self,
            sha256: sha256.to_owned(),
        }
    }
}

impl DownloadCoordinator {
    fn acquire(&self) -> DownloadPermit<'_> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *active == MAX_CONCURRENT_SEGMENT_DOWNLOADS {
            active = self
                .ready
                .wait(active)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *active = active.saturating_add(1);
        DownloadPermit { coordinator: self }
    }
}

impl Drop for UploadPermit<'_> {
    fn drop(&mut self) {
        let mut active = self
            .coordinator
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        active.remove(&self.sha256);
        self.coordinator.ready.notify_all();
    }
}

impl Drop for DownloadPermit<'_> {
    fn drop(&mut self) {
        let mut active = self
            .coordinator
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *active = active.saturating_sub(1);
        self.coordinator.ready.notify_one();
    }
}

impl RemoteDiagnostics {
    #[cfg(test)]
    fn disabled() -> Self {
        Self {
            path: None,
            started: Instant::now(),
            events: Mutex::new(Vec::new()),
            dropped: AtomicU64::new(0),
        }
    }

    fn production() -> Self {
        let root = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            })
            .map(|root| root.join("scorepeek/corpus-remote-diagnostics"));
        let path = root.and_then(|root| {
            if DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&root)
                .is_err()
            {
                return None;
            }
            reserve_diagnostic_path(&root)
        });
        Self {
            path,
            started: Instant::now(),
            events: Mutex::new(Vec::new()),
            dropped: AtomicU64::new(0),
        }
    }

    fn record<T>(
        &self,
        operation: &str,
        sha256: &str,
        bytes: u64,
        started: Instant,
        result: &Result<T, CorpusError>,
    ) {
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if events.len() == MAX_DIAGNOSTIC_EVENTS {
            self.dropped.fetch_add(1, Ordering::AcqRel);
            return;
        }
        let (status, error_type) = match result {
            Ok(_) => ("success", None),
            Err(error) => ("error", remote_error_type(error)),
        };
        events.push(RemoteDiagnosticEvent {
            schema: "scorepeek-corpus-remote-operation-v1",
            operation: operation.to_owned(),
            status,
            error_type: error_type.map(str::to_owned),
            object_sha256: sha256.to_owned(),
            object_bytes: bytes,
            elapsed_us: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
            retry_delay_us: None,
        });
    }

    fn record_retry(
        &self,
        operation: &str,
        sha256: &str,
        bytes: u64,
        delay: Duration,
        error: &CorpusError,
    ) {
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if events.len() == MAX_DIAGNOSTIC_EVENTS {
            self.dropped.fetch_add(1, Ordering::AcqRel);
            return;
        }
        events.push(RemoteDiagnosticEvent {
            schema: "scorepeek-corpus-remote-operation-v1",
            operation: operation.to_owned(),
            status: "retry",
            error_type: remote_error_type(error).map(str::to_owned),
            object_sha256: sha256.to_owned(),
            object_bytes: bytes,
            elapsed_us: 0,
            retry_delay_us: Some(u64::try_from(delay.as_micros()).unwrap_or(u64::MAX)),
        });
    }
}

impl Drop for RemoteDiagnostics {
    fn drop(&mut self) {
        let Some(path) = &self.path else { return };
        let Ok(mut file) = OpenOptions::new().write(true).truncate(true).open(path) else {
            return;
        };
        let events = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for event in events.iter() {
            let Ok(mut bytes) = serde_json::to_vec(event) else {
                return;
            };
            bytes.push(b'\n');
            if file.write_all(&bytes).is_err() {
                return;
            }
        }
        let summary = serde_json::json!({
            "schema": "scorepeek-corpus-remote-run-v1",
            "status": "complete",
            "event_count": events.len(),
            "dropped_events": self.dropped.load(Ordering::Acquire),
            "elapsed_us": u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX),
        });
        if let Ok(mut bytes) = serde_json::to_vec(&summary) {
            bytes.push(b'\n');
            let _ = file.write_all(&bytes);
            let _ = file.sync_all();
        }
    }
}

fn prune_diagnostics(root: &std::path::Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_str()?;
            entry
                .file_type()
                .ok()?
                .is_file()
                .then_some(())
                .filter(|()| name.starts_with("remote-") && name.ends_with(".ndjson"))
                .map(|()| path)
        })
        .collect::<Vec<_>>();
    paths.sort();
    let remove = paths
        .len()
        .saturating_sub(MAX_DIAGNOSTIC_RUNS.saturating_sub(1));
    for path in paths.into_iter().take(remove) {
        let _ = fs::remove_file(path);
    }
}

fn reserve_diagnostic_path(root: &std::path::Path) -> Option<PathBuf> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(root.join(".scorepeek-corpus-remote-diagnostics.lock"))
        .ok()?;
    lock.lock().ok()?;
    prune_diagnostics(root);
    let reserved = tempfile::Builder::new()
        .prefix("remote-")
        .suffix(".ndjson")
        .tempfile_in(root)
        .ok()
        .and_then(|file| file.keep().ok().map(|(_, path)| path));
    let _ = lock.unlock();
    reserved
}

fn remote_error_type(error: &CorpusError) -> Option<&str> {
    let (CorpusError::InvalidRequest(text) | CorpusError::InvalidReplay(text)) = error else {
        return Some("local_io_failed");
    };
    text.strip_prefix("remote ")?
        .split_once(':')
        .map(|(kind, _)| kind)
}

impl RemoteSegment {
    pub(crate) fn input(&self) -> Result<File, CorpusError> {
        self.file.try_clone().map_err(CorpusError::Io)
    }
}

impl RemoteEnvironment {
    fn parse(values: &BTreeMap<String, String>) -> Result<Option<Self>, CorpusError> {
        let url = values.get(URL_ENV);
        let has_related = [REGION_ENV, ENDPOINT_ENV, PATH_STYLE_ENV]
            .iter()
            .any(|name| values.contains_key(*name));
        let Some(url) = url else {
            if has_related {
                return Err(remote_error(
                    "invalid_configuration",
                    "remote settings require SCOREPEEK_CORPUS_S3_URL",
                ));
            }
            return Ok(None);
        };
        let region = values
            .get(REGION_ENV)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                remote_error(
                    "invalid_configuration",
                    "remote settings require SCOREPEEK_CORPUS_S3_REGION",
                )
            })?;
        let endpoint = values.get(ENDPOINT_ENV).cloned();
        if endpoint
            .as_deref()
            .is_some_and(|value| !is_https_origin(value))
        {
            return Err(remote_error(
                "invalid_configuration",
                "remote endpoint must be an HTTPS origin",
            ));
        }
        let path_style = match values.get(PATH_STYLE_ENV).map(String::as_str) {
            None | Some("false") => false,
            Some("true") => true,
            Some(_) => {
                return Err(remote_error(
                    "invalid_configuration",
                    "remote path style must be true or false",
                ));
            }
        };
        Ok(Some(Self {
            url: url.clone(),
            region: region.clone(),
            endpoint,
            path_style,
        }))
    }
}

impl RemoteTarget {
    fn object_path(&self, sha256: &str) -> Result<ObjectPath, CorpusError> {
        validate_sha256(sha256)?;
        self.path(&format!(
            "frame-corpus/v1/objects/sha256/{}/{sha256}",
            &sha256[..2]
        ))
    }

    fn path(&self, suffix: &str) -> Result<ObjectPath, CorpusError> {
        let value = if self.prefix.is_empty() {
            suffix.to_owned()
        } else {
            format!("{}/{suffix}", self.prefix)
        };
        ObjectPath::parse(value)
            .map_err(|_| remote_error("invalid_configuration", "remote object key is invalid"))
    }
}

fn parse_s3_url(value: &str) -> Result<(String, String), CorpusError> {
    let remainder = value.strip_prefix("s3://").ok_or_else(|| {
        remote_error("invalid_configuration", "remote URL must use the s3 scheme")
    })?;
    let (bucket, prefix) = remainder.split_once('/').unwrap_or((remainder, ""));
    if bucket.is_empty()
        || !bucket.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
        || prefix.split('/').any(|part| part == "." || part == "..")
    {
        return Err(remote_error(
            "invalid_configuration",
            "remote URL bucket or prefix is invalid",
        ));
    }
    Ok((bucket.to_owned(), prefix.trim_matches('/').to_owned()))
}

fn is_https_origin(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
}

fn client_endpoint(origin: &str, bucket: &str, path_style: bool) -> Result<String, CorpusError> {
    if path_style {
        return Ok(origin.to_owned());
    }
    let mut endpoint = Url::parse(origin).map_err(|_| {
        remote_error(
            "invalid_configuration",
            "remote endpoint must be an HTTPS origin",
        )
    })?;
    let host = endpoint.host_str().ok_or_else(|| {
        remote_error(
            "invalid_configuration",
            "virtual-hosted remote endpoint requires a DNS hostname",
        )
    })?;
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Err(remote_error(
            "invalid_configuration",
            "virtual-hosted remote endpoint requires a DNS hostname",
        ));
    }
    let bucket_host = format!("{bucket}.{host}");
    endpoint.set_host(Some(&bucket_host)).map_err(|_| {
        remote_error(
            "invalid_configuration",
            "virtual-hosted remote endpoint is invalid",
        )
    })?;
    Ok(endpoint.as_str().trim_end_matches('/').to_owned())
}

impl RemoteCredentials {
    fn parse(values: &BTreeMap<String, String>) -> Result<Self, CorpusError> {
        let nonempty = |name: &str| values.get(name).is_some_and(|value| !value.is_empty());
        let any = |names: &[&str]| names.iter().any(|name| values.contains_key(*name));
        if any(&[
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
        ]) {
            return if nonempty("AWS_ACCESS_KEY_ID") && nonempty("AWS_SECRET_ACCESS_KEY") {
                Ok(Self::Static)
            } else {
                Err(missing_credentials())
            };
        }
        if any(&[
            "AWS_WEB_IDENTITY_TOKEN_FILE",
            "AWS_ROLE_ARN",
            "AWS_ROLE_SESSION_NAME",
        ]) {
            return if nonempty("AWS_WEB_IDENTITY_TOKEN_FILE") && nonempty("AWS_ROLE_ARN") {
                Ok(Self::WebIdentity)
            } else {
                Err(missing_credentials())
            };
        }
        if values.contains_key("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI") {
            return if nonempty("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI") {
                Ok(Self::ContainerRelative)
            } else {
                Err(missing_credentials())
            };
        }
        if any(&[
            "AWS_CONTAINER_CREDENTIALS_FULL_URI",
            "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
        ]) {
            return if nonempty("AWS_CONTAINER_CREDENTIALS_FULL_URI")
                && nonempty("AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE")
            {
                Ok(Self::ContainerFull)
            } else {
                Err(missing_credentials())
            };
        }
        Err(missing_credentials())
    }

    fn apply(
        &self,
        mut builder: AmazonS3Builder,
        values: &BTreeMap<String, String>,
    ) -> AmazonS3Builder {
        let keys: &[(&str, AmazonS3ConfigKey)] = match self {
            Self::Static => &[
                ("AWS_ACCESS_KEY_ID", AmazonS3ConfigKey::AccessKeyId),
                ("AWS_SECRET_ACCESS_KEY", AmazonS3ConfigKey::SecretAccessKey),
                ("AWS_SESSION_TOKEN", AmazonS3ConfigKey::Token),
            ],
            Self::WebIdentity => &[
                (
                    "AWS_WEB_IDENTITY_TOKEN_FILE",
                    AmazonS3ConfigKey::WebIdentityTokenFile,
                ),
                ("AWS_ROLE_ARN", AmazonS3ConfigKey::RoleArn),
                ("AWS_ROLE_SESSION_NAME", AmazonS3ConfigKey::RoleSessionName),
            ],
            Self::ContainerRelative => &[(
                "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
                AmazonS3ConfigKey::ContainerCredentialsRelativeUri,
            )],
            Self::ContainerFull => &[
                (
                    "AWS_CONTAINER_CREDENTIALS_FULL_URI",
                    AmazonS3ConfigKey::ContainerCredentialsFullUri,
                ),
                (
                    "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
                    AmazonS3ConfigKey::ContainerAuthorizationTokenFile,
                ),
            ],
        };
        for (name, key) in keys {
            if let Some(value) = values.get(*name).filter(|value| !value.is_empty()) {
                builder = builder.with_config(*key, value);
            }
        }
        builder
    }
}

fn missing_credentials() -> CorpusError {
    remote_error(
        "permission_denied",
        "remote credentials are unavailable in the process environment",
    )
}

async fn upload_file_verified(
    store: Arc<dyn ObjectStore>,
    path: &ObjectPath,
    source: File,
    expected_sha256: &str,
    expected_bytes: u64,
) -> Result<(), CorpusError> {
    let upload = store
        .put_multipart(path)
        .await
        .map_err(|_| remote_error("upload_failed", "remote object upload failed"))?;
    upload_parts_verified(source, upload, expected_sha256, expected_bytes).await
}

async fn upload_parts_verified(
    source: File,
    mut upload: Box<dyn MultipartUpload>,
    expected_sha256: &str,
    expected_bytes: u64,
) -> Result<(), CorpusError> {
    let mut source = tokio::fs::File::from_std(source);
    let result = async {
        let mut received = 0_u64;
        let mut digest = Sha256::new();
        loop {
            let mut bytes = vec![0_u8; TRANSFER_BUFFER_BYTES];
            let mut filled = 0;
            while filled < bytes.len() {
                let read = source
                    .read(&mut bytes[filled..])
                    .await
                    .map_err(|_| remote_error("upload_failed", "local segment read failed"))?;
                if read == 0 {
                    break;
                }
                filled += read;
            }
            if filled == 0 {
                break;
            }
            bytes.truncate(filled);
            received = received.checked_add(filled as u64).ok_or_else(|| {
                remote_error("size_mismatch", "local segment exceeded its declared size")
            })?;
            if received > expected_bytes {
                return Err(remote_error(
                    "size_mismatch",
                    "local segment exceeded its declared size",
                ));
            }
            digest.update(&bytes);
            upload
                .put_part(bytes.into())
                .await
                .map_err(|_| remote_error("upload_failed", "remote object upload failed"))?;
        }
        if received != expected_bytes {
            return Err(remote_error(
                "size_mismatch",
                "local segment size differs from its declaration",
            ));
        }
        if crate::encode_digest(digest.finalize()) != expected_sha256 {
            return Err(remote_error(
                "digest_mismatch",
                "local segment digest differs from its declaration",
            ));
        }
        upload
            .complete()
            .await
            .map(|_| ())
            .map_err(|_| remote_error("upload_failed", "remote object upload did not complete"))
    }
    .await;
    if result.is_err() && upload.abort().await.is_err() {
        return Err(remote_error(
            "upload_abort_failed",
            "remote multipart abort failed",
        ));
    }
    result
}

async fn remote_exists_with_size(
    store: Arc<dyn ObjectStore>,
    path: &ObjectPath,
    expected_bytes: u64,
) -> Result<bool, CorpusError> {
    match store.head(path).await {
        Ok(metadata) => {
            if metadata.size != expected_bytes {
                return Err(remote_error("size_mismatch", "remote object size differs"));
            }
            Ok(true)
        }
        Err(object_store::Error::NotFound { .. }) => Ok(false),
        Err(
            object_store::Error::PermissionDenied { .. }
            | object_store::Error::Unauthenticated { .. },
        ) => Err(remote_error(
            "permission_denied",
            "remote object lookup was denied",
        )),
        Err(_) => Err(remote_error("lookup_failed", "remote object lookup failed")),
    }
}

async fn download_verified(
    store: Arc<dyn ObjectStore>,
    path: &ObjectPath,
    expected_sha256: &str,
    expected_bytes: u64,
    destination: &mut File,
) -> Result<(), CorpusError> {
    let metadata = store
        .head(path)
        .await
        .map_err(|error| map_download_error(&error))?;
    if metadata.size != expected_bytes {
        return Err(remote_error("size_mismatch", "remote object size differs"));
    }
    destination
        .set_len(expected_bytes)
        .map_err(CorpusError::Io)?;
    let requests = split_download_ranges(expected_bytes)
        .into_iter()
        .map(|range| {
            let store = Arc::clone(&store);
            let path = path.clone();
            let file = destination.try_clone().map_err(CorpusError::Io)?;
            let e_tag = metadata.e_tag.clone();
            let version = metadata.version.clone();
            Ok(async move {
                download_range(store, path, range, expected_bytes, e_tag, version, file).await
            })
        })
        .collect::<Result<Vec<_>, CorpusError>>()?;
    futures_util::future::try_join_all(requests).await?;

    destination
        .seek(SeekFrom::Start(0))
        .map_err(CorpusError::Io)?;
    let mut digest = Sha256::new();
    let mut received = 0_u64;
    let mut buffer = vec![0_u8; TRANSFER_BUFFER_BYTES];
    loop {
        let read = destination.read(&mut buffer).map_err(CorpusError::Io)?;
        if read == 0 {
            break;
        }
        received = received.checked_add(read as u64).ok_or_else(|| {
            remote_error("size_mismatch", "remote object exceeded its declared size")
        })?;
        if received > expected_bytes {
            return Err(remote_error(
                "size_mismatch",
                "remote object exceeded its declared size",
            ));
        }
        digest.update(&buffer[..read]);
    }
    if received != expected_bytes {
        return Err(remote_error(
            "size_mismatch",
            "remote object ended before its declared size",
        ));
    }
    if crate::encode_digest(digest.finalize()) != expected_sha256 {
        return Err(remote_error(
            "digest_mismatch",
            "remote object digest differs",
        ));
    }
    Ok(())
}

async fn download_range(
    store: Arc<dyn ObjectStore>,
    path: ObjectPath,
    range: Range<u64>,
    expected_bytes: u64,
    e_tag: Option<String>,
    version: Option<String>,
    destination: File,
) -> Result<(), CorpusError> {
    let options = object_store::GetOptions::new()
        .with_range(Some(range.clone()))
        .with_if_match(e_tag)
        .with_version(version);
    let result = store
        .get_opts(&path, options)
        .await
        .map_err(|error| map_download_error(&error))?;
    if result.meta.size != expected_bytes || result.range != range {
        return Err(remote_error("size_mismatch", "remote object range differs"));
    }
    let expected_range_bytes = range.end.saturating_sub(range.start);
    let mut stream = result.into_stream();
    let mut received = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| map_download_error(&error))?;
        let offset = range
            .start
            .checked_add(received)
            .ok_or_else(|| remote_error("size_mismatch", "remote range offset overflowed"))?;
        received = received.checked_add(chunk.len() as u64).ok_or_else(|| {
            remote_error("size_mismatch", "remote range exceeded its declared size")
        })?;
        if received > expected_range_bytes {
            return Err(remote_error(
                "size_mismatch",
                "remote range exceeded its declared size",
            ));
        }
        destination
            .write_all_at(&chunk, offset)
            .map_err(CorpusError::Io)?;
    }
    if received != expected_range_bytes {
        return Err(remote_error(
            "size_mismatch",
            "remote range ended before its declared size",
        ));
    }
    Ok(())
}

fn split_download_ranges(bytes: u64) -> Vec<Range<u64>> {
    let count = RANGES_PER_SEGMENT.min(bytes);
    (0..count)
        .map(|index| {
            let boundary = |part| {
                u64::try_from((u128::from(bytes) * u128::from(part)) / u128::from(count))
                    .expect("a divided u64 product remains within u64")
            };
            boundary(index)..boundary(index + 1)
        })
        .collect()
}

fn map_download_error(error: &object_store::Error) -> CorpusError {
    match download_error_type(error) {
        Some("not_found") => remote_error("not_found", "remote object is unavailable"),
        Some("permission_denied") => {
            remote_error("permission_denied", "remote object GET was denied")
        }
        Some("object_changed") => {
            remote_error("object_changed", "remote object changed during GET")
        }
        _ => remote_error("download_failed", "remote object GET failed"),
    }
}

fn download_error_type(error: &(dyn std::error::Error + 'static)) -> Option<&'static str> {
    if let Some(error) = error.downcast_ref::<object_store::Error>() {
        match error {
            object_store::Error::NotFound { .. } => return Some("not_found"),
            object_store::Error::PermissionDenied { .. }
            | object_store::Error::Unauthenticated { .. } => return Some("permission_denied"),
            object_store::Error::Precondition { .. } => return Some("object_changed"),
            _ => {}
        }
    }
    error.source().and_then(download_error_type)
}

fn retry_segment_download<T>(
    mut download: impl FnMut() -> Result<T, CorpusError>,
    mut before_retry: impl FnMut(&CorpusError),
) -> Result<T, CorpusError> {
    match download() {
        Err(error) if remote_error_type(&error) == Some("download_failed") => {
            before_retry(&error);
            download()
        }
        result => result,
    }
}

fn validate_sha256(value: &str) -> Result<(), CorpusError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(remote_error(
            "invalid_configuration",
            "remote digest is invalid",
        ))
    }
}

fn remote_runtime() -> Result<tokio::runtime::Runtime, CorpusError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| remote_error("invalid_configuration", "remote runtime could not start"))
}

fn remote_error(error_type: &str, detail: &str) -> CorpusError {
    CorpusError::InvalidRequest(format!("remote {error_type}: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;
    use object_store::{Extensions, PutPayload, PutResult, UploadPart};
    use std::collections::BTreeSet;
    use std::pin::Pin;
    use std::sync::atomic::AtomicBool;

    fn environment(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn environment_is_optional_but_partial_or_insecure_configuration_fails() {
        assert!(
            RemoteEnvironment::parse(&BTreeMap::new())
                .unwrap()
                .is_none()
        );
        assert!(RemoteEnvironment::parse(&environment(&[(REGION_ENV, "region")])).is_err());
        assert!(
            RemoteEnvironment::parse(&environment(&[
                (URL_ENV, "s3://bucket/prefix"),
                (REGION_ENV, "region"),
                (ENDPOINT_ENV, "http://127.0.0.1:9000"),
            ]))
            .is_err()
        );
        assert!(
            RemoteEnvironment::parse(&environment(&[
                (URL_ENV, "s3://bucket/prefix"),
                (REGION_ENV, "region"),
                (PATH_STYLE_ENV, "yes"),
            ]))
            .is_err()
        );
    }

    #[test]
    fn environment_parses_path_style_without_a_storage_capacity_setting() {
        let parsed = RemoteEnvironment::parse(&environment(&[
            (URL_ENV, "s3://bucket/prefix"),
            (REGION_ENV, "region"),
            (PATH_STYLE_ENV, "true"),
        ]))
        .unwrap()
        .unwrap();
        assert!(parsed.path_style);
    }

    #[test]
    fn custom_endpoint_is_adapted_for_the_object_store_addressing_mode() {
        assert_eq!(
            client_endpoint("https://s3.example.test", "bucket", false).unwrap(),
            "https://bucket.s3.example.test"
        );
        assert_eq!(
            client_endpoint("https://s3.example.test", "bucket", true).unwrap(),
            "https://s3.example.test"
        );
        assert!(client_endpoint("https://127.0.0.1", "bucket", false).is_err());
    }

    #[test]
    fn credentials_require_a_complete_nonempty_environment_provider() {
        assert!(RemoteCredentials::parse(&BTreeMap::new()).is_err());
        assert!(RemoteCredentials::parse(&environment(&[("AWS_ACCESS_KEY_ID", "key")])).is_err());
        assert!(
            RemoteCredentials::parse(&environment(&[
                ("AWS_ACCESS_KEY_ID", ""),
                ("AWS_SECRET_ACCESS_KEY", "secret"),
            ]))
            .is_err()
        );
        assert!(
            RemoteCredentials::parse(&environment(&[(
                "AWS_CONTAINER_CREDENTIALS_FULL_URI",
                "http://127.0.0.1/credentials",
            )]))
            .is_err()
        );
        assert!(
            RemoteCredentials::parse(&environment(&[
                (
                    "AWS_CONTAINER_CREDENTIALS_FULL_URI",
                    "http://127.0.0.1/credentials",
                ),
                ("AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE", "/token"),
            ]))
            .is_ok()
        );
    }

    #[test]
    fn upload_reuses_verified_content_and_materialization_is_temporary() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let remote = SegmentRemote::new(Arc::clone(&store), "test".to_owned()).unwrap();
        let bytes = b"canonical segment";
        let sha256 = crate::encode_digest(Sha256::digest(bytes));
        let mut source = tempfile::tempfile().unwrap();
        source.write_all(bytes).unwrap();
        source.seek(SeekFrom::Start(0)).unwrap();
        assert_eq!(
            remote
                .upload_verified(source.try_clone().unwrap(), &sha256, bytes.len() as u64)
                .unwrap(),
            UploadDisposition::Transferred
        );
        assert_eq!(
            remote
                .upload_verified(source, &sha256, bytes.len() as u64)
                .unwrap(),
            UploadDisposition::Reused
        );
        let segment = remote.materialize(&sha256, bytes.len() as u64).unwrap();
        let mut materialized = segment.input().unwrap();
        let mut actual = Vec::new();
        materialized.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, bytes);
        assert_eq!(remote.metrics().downloaded_segments, 1);
    }

    #[test]
    fn materialization_rejects_wrong_digest() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let remote = SegmentRemote::new(Arc::clone(&store), String::new()).unwrap();
        let bytes = b"wrong";
        let sha256 = crate::encode_digest(Sha256::digest(b"right"));
        let path = remote.target.object_path(&sha256).unwrap();
        remote_runtime()
            .unwrap()
            .block_on(store.put(&path, PutPayload::from_static(bytes)))
            .unwrap();
        let Err(error) = remote.materialize(&sha256, bytes.len() as u64) else {
            panic!("wrong remote bytes must fail digest verification");
        };
        assert_eq!(remote_error_type(&error), Some("digest_mismatch"));
    }

    #[test]
    fn materialization_rejects_wrong_declared_size_before_ranges() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let remote = SegmentRemote::new(Arc::clone(&store), String::new()).unwrap();
        let bytes = b"segment";
        let sha256 = crate::encode_digest(Sha256::digest(bytes));
        let path = remote.target.object_path(&sha256).unwrap();
        remote_runtime()
            .unwrap()
            .block_on(store.put(&path, PutPayload::from_static(bytes)))
            .unwrap();
        let Err(error) = remote.materialize(&sha256, bytes.len() as u64 - 1) else {
            panic!("wrong declared size must fail before ranged download");
        };
        assert_eq!(remote_error_type(&error), Some("size_mismatch"));
    }

    #[test]
    fn download_ranges_are_contiguous_bounded_and_complete() {
        assert!(split_download_ranges(0).is_empty());
        for bytes in 1..1_000 {
            let ranges = split_download_ranges(bytes);
            assert_eq!(
                ranges.len(),
                usize::try_from(RANGES_PER_SEGMENT.min(bytes)).unwrap()
            );
            assert_eq!(ranges.first().unwrap().start, 0);
            assert_eq!(ranges.last().unwrap().end, bytes);
            assert!(ranges.iter().all(|range| range.start < range.end));
            assert!(ranges.windows(2).all(|pair| pair[0].end == pair[1].start));
        }
    }

    #[test]
    fn only_terminal_download_failure_is_retried_once() {
        let mut attempts = 0;
        let mut retries = Vec::new();
        let result = retry_segment_download(
            || {
                attempts += 1;
                if attempts == 1 {
                    Err(remote_error("download_failed", "first attempt failed"))
                } else {
                    Ok("downloaded")
                }
            },
            |error| retries.push(remote_error_type(error).unwrap().to_owned()),
        );
        assert_eq!(result.unwrap(), "downloaded");
        assert_eq!(attempts, 2);
        assert_eq!(retries, ["download_failed"]);

        for error_type in [
            "not_found",
            "permission_denied",
            "object_changed",
            "size_mismatch",
            "digest_mismatch",
        ] {
            let mut attempts = 0;
            let error = retry_segment_download(
                || {
                    attempts += 1;
                    Err::<(), _>(remote_error(error_type, "not retryable"))
                },
                |_| panic!("integrity and permanent errors must not be retried"),
            )
            .unwrap_err();
            assert_eq!(remote_error_type(&error), Some(error_type));
            assert_eq!(attempts, 1);
        }

        let mut attempts = 0;
        let error = retry_segment_download(
            || {
                attempts += 1;
                Err::<(), _>(remote_error("download_failed", "still failing"))
            },
            |_| {},
        )
        .unwrap_err();
        assert_eq!(remote_error_type(&error), Some("download_failed"));
        assert_eq!(attempts, 2);
    }

    #[test]
    fn nested_stream_errors_keep_terminal_download_classification() {
        let source = || Box::new(std::io::Error::other("provider detail")) as _;
        for (error, expected) in [
            (
                object_store::Error::NotFound {
                    path: "object".to_owned(),
                    source: source(),
                },
                "not_found",
            ),
            (
                object_store::Error::PermissionDenied {
                    path: "object".to_owned(),
                    source: source(),
                },
                "permission_denied",
            ),
            (
                object_store::Error::Unauthenticated {
                    path: "object".to_owned(),
                    source: source(),
                },
                "permission_denied",
            ),
            (
                object_store::Error::Precondition {
                    path: "object".to_owned(),
                    source: source(),
                },
                "object_changed",
            ),
        ] {
            let nested = object_store::Error::Generic {
                store: "test",
                source: Box::new(error),
            };
            assert_eq!(
                remote_error_type(&map_download_error(&nested)),
                Some(expected)
            );
        }

        let transient = object_store::Error::Generic {
            store: "test",
            source: source(),
        };
        assert_eq!(
            remote_error_type(&map_download_error(&transient)),
            Some("download_failed")
        );
    }

    #[derive(Debug)]
    struct FailingMultipartUpload {
        aborted: Arc<AtomicBool>,
    }

    #[derive(Debug)]
    struct RecordingMultipartUpload {
        part_bytes: Arc<Mutex<Vec<usize>>>,
        completed: Arc<AtomicBool>,
        aborted: Arc<AtomicBool>,
    }

    impl MultipartUpload for RecordingMultipartUpload {
        fn put_part(&mut self, data: PutPayload) -> UploadPart {
            self.part_bytes.lock().unwrap().push(data.content_length());
            Box::pin(async { Ok(()) })
        }

        fn complete<'life0, 'async_trait>(
            &'life0 mut self,
        ) -> Pin<Box<dyn Future<Output = object_store::Result<PutResult>> + Send + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            let completed = Arc::clone(&self.completed);
            Box::pin(async move {
                completed.store(true, Ordering::Release);
                Ok(PutResult {
                    e_tag: None,
                    version: None,
                    extensions: Extensions::default(),
                })
            })
        }

        fn abort<'life0, 'async_trait>(
            &'life0 mut self,
        ) -> Pin<Box<dyn Future<Output = object_store::Result<()>> + Send + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            let aborted = Arc::clone(&self.aborted);
            Box::pin(async move {
                aborted.store(true, Ordering::Release);
                Ok(())
            })
        }
    }

    impl MultipartUpload for FailingMultipartUpload {
        fn put_part(&mut self, _data: PutPayload) -> UploadPart {
            Box::pin(async {
                Err(object_store::Error::Generic {
                    store: "test",
                    source: Box::new(std::io::Error::other("part failed")),
                })
            })
        }

        fn complete<'life0, 'async_trait>(
            &'life0 mut self,
        ) -> Pin<Box<dyn Future<Output = object_store::Result<PutResult>> + Send + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            Box::pin(async { unreachable!("failed part must not complete") })
        }

        fn abort<'life0, 'async_trait>(
            &'life0 mut self,
        ) -> Pin<Box<dyn Future<Output = object_store::Result<()>> + Send + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            Box::pin(async {
                self.aborted.store(true, Ordering::Release);
                Ok(())
            })
        }
    }

    #[test]
    fn multipart_part_failure_is_aborted() {
        let aborted = Arc::new(AtomicBool::new(false));
        let mut source = tempfile::tempfile().unwrap();
        source.write_all(b"segment").unwrap();
        source.rewind().unwrap();
        let sha256 = crate::encode_digest(Sha256::digest(b"segment"));
        let result = remote_runtime().unwrap().block_on(upload_parts_verified(
            source,
            Box::new(FailingMultipartUpload {
                aborted: Arc::clone(&aborted),
            }),
            &sha256,
            7,
        ));
        assert!(result.is_err());
        assert!(aborted.load(Ordering::Acquire));
    }

    #[test]
    fn multipart_upload_fills_every_nonfinal_part() {
        let part_bytes = Arc::new(Mutex::new(Vec::new()));
        let completed = Arc::new(AtomicBool::new(false));
        let aborted = Arc::new(AtomicBool::new(false));
        let bytes = vec![0x5a; TRANSFER_BUFFER_BYTES + 1];
        let sha256 = crate::encode_digest(Sha256::digest(&bytes));
        let mut source = tempfile::tempfile().unwrap();
        source.write_all(&bytes).unwrap();
        source.rewind().unwrap();
        remote_runtime()
            .unwrap()
            .block_on(upload_parts_verified(
                source,
                Box::new(RecordingMultipartUpload {
                    part_bytes: Arc::clone(&part_bytes),
                    completed: Arc::clone(&completed),
                    aborted: Arc::clone(&aborted),
                }),
                &sha256,
                bytes.len() as u64,
            ))
            .unwrap();
        assert_eq!(*part_bytes.lock().unwrap(), [TRANSFER_BUFFER_BYTES, 1]);
        assert!(completed.load(Ordering::Acquire));
        assert!(!aborted.load(Ordering::Acquire));
    }

    #[test]
    fn local_mismatch_aborts_before_multipart_complete() {
        for (expected_sha256, expected_bytes, expected_error) in [
            ("0".repeat(64), 7, "digest_mismatch"),
            (
                crate::encode_digest(Sha256::digest(b"segment")),
                8,
                "size_mismatch",
            ),
        ] {
            let part_bytes = Arc::new(Mutex::new(Vec::new()));
            let completed = Arc::new(AtomicBool::new(false));
            let aborted = Arc::new(AtomicBool::new(false));
            let mut source = tempfile::tempfile().unwrap();
            source.write_all(b"segment").unwrap();
            source.rewind().unwrap();
            let error = remote_runtime()
                .unwrap()
                .block_on(upload_parts_verified(
                    source,
                    Box::new(RecordingMultipartUpload {
                        part_bytes,
                        completed: Arc::clone(&completed),
                        aborted: Arc::clone(&aborted),
                    }),
                    &expected_sha256,
                    expected_bytes,
                ))
                .unwrap_err();
            assert_eq!(remote_error_type(&error), Some(expected_error));
            assert!(!completed.load(Ordering::Acquire));
            assert!(aborted.load(Ordering::Acquire));
        }
    }

    #[test]
    fn existing_final_object_is_reused_by_size_without_readback() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let remote = SegmentRemote::new(Arc::clone(&store), String::new()).unwrap();
        let declared = b"declared";
        let sha256 = crate::encode_digest(Sha256::digest(declared));
        let path = remote.target.object_path(&sha256).unwrap();
        remote_runtime()
            .unwrap()
            .block_on(store.put(&path, PutPayload::from_static(b"differen")))
            .unwrap();
        let mut source = tempfile::tempfile().unwrap();
        source.write_all(declared).unwrap();
        source.rewind().unwrap();
        assert_eq!(
            remote
                .upload_verified(source, &sha256, declared.len() as u64)
                .unwrap(),
            UploadDisposition::Reused
        );
    }

    #[test]
    fn existing_final_object_with_wrong_size_fails_closed() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let remote = SegmentRemote::new(Arc::clone(&store), String::new()).unwrap();
        let bytes = b"segment";
        let sha256 = crate::encode_digest(Sha256::digest(bytes));
        let path = remote.target.object_path(&sha256).unwrap();
        remote_runtime()
            .unwrap()
            .block_on(store.put(&path, PutPayload::from_static(b"wrong")))
            .unwrap();
        let mut source = tempfile::tempfile().unwrap();
        source.write_all(bytes).unwrap();
        source.rewind().unwrap();
        let error = remote
            .upload_verified(source, &sha256, bytes.len() as u64)
            .unwrap_err();
        assert_eq!(remote_error_type(&error), Some("size_mismatch"));
    }

    #[test]
    fn upload_coordinator_serializes_only_the_same_digest() {
        let coordinator = Arc::new(UploadCoordinator::default());
        let first = coordinator.acquire(&"a".repeat(64));
        let different = coordinator.acquire(&"b".repeat(64));
        drop(different);

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        let waiter = Arc::clone(&coordinator);
        let handle = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let _permit = waiter.acquire(&"a".repeat(64));
            acquired_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(
            acquired_rx
                .recv_timeout(std::time::Duration::from_millis(20))
                .is_err()
        );
        drop(first);
        acquired_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn download_coordinator_limits_concurrency() {
        let coordinator = Arc::new(DownloadCoordinator::default());
        let permits = (0..MAX_CONCURRENT_SEGMENT_DOWNLOADS)
            .map(|_| coordinator.acquire())
            .collect::<Vec<_>>();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        let waiter = Arc::clone(&coordinator);
        let handle = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let _permit = waiter.acquire();
            acquired_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(
            acquired_rx
                .recv_timeout(std::time::Duration::from_millis(20))
                .is_err()
        );
        drop(permits);
        acquired_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn diagnostics_record_allowlisted_operation_shape_only() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("run.ndjson");
        File::create(&path).unwrap();
        {
            let diagnostics = RemoteDiagnostics {
                path: Some(path.clone()),
                started: Instant::now(),
                events: Mutex::new(Vec::new()),
                dropped: AtomicU64::new(0),
            };
            let result: Result<(), CorpusError> = Err(remote_error(
                "permission_denied",
                "provider detail must not be retained",
            ));
            diagnostics.record("segment_get", &"a".repeat(64), 42, Instant::now(), &result);
            diagnostics.record_retry(
                "segment_get",
                &"a".repeat(64),
                42,
                SEGMENT_DOWNLOAD_RETRY_DELAY,
                &remote_error("download_failed", "provider detail must not be retained"),
            );
        }
        let bytes = fs::read_to_string(path).unwrap();
        assert!(bytes.contains("\"error_type\":\"permission_denied\""));
        assert!(bytes.contains("\"status\":\"retry\""));
        assert!(bytes.contains("\"error_type\":\"download_failed\""));
        assert!(bytes.contains("\"retry_delay_us\":250000"));
        assert!(bytes.contains(&"a".repeat(64)));
        assert!(!bytes.contains("provider detail"));
        assert!(!bytes.contains("AWS_"));
    }

    #[test]
    fn diagnostic_reservation_is_unique_and_retention_is_bounded_concurrently() {
        let root = Arc::new(tempfile::tempdir().unwrap());
        let handles = (0..64)
            .map(|_| {
                let root = Arc::clone(&root);
                std::thread::spawn(move || reserve_diagnostic_path(root.path()).unwrap())
            })
            .collect::<Vec<_>>();
        let paths = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(paths.len(), 64);
        let retained = fs::read_dir(root.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("remote-") && name.ends_with(".ndjson"))
            })
            .count();
        assert_eq!(retained, MAX_DIAGNOSTIC_RUNS);
    }
}
