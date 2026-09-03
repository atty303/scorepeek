use std::collections::BTreeMap;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{Seek as _, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt as _;
use object_store::aws::{AmazonS3Builder, AmazonS3ConfigKey, S3CopyIfNotExists};
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
const MAX_DIAGNOSTIC_EVENTS: usize = 4096;
const MAX_DIAGNOSTIC_RUNS: usize = 32;
const STAGING_RECOVERY_AGE_NANOS: u128 = 7 * 24 * 60 * 60 * 1_000_000_000;
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub(crate) struct SegmentRemote {
    target: Arc<RemoteTarget>,
    transferred: Arc<AtomicU64>,
    reused: Arc<AtomicU64>,
    downloaded_segments: Arc<AtomicU64>,
    downloaded_bytes: Arc<AtomicU64>,
    diagnostics: Arc<RemoteDiagnostics>,
    recovery: Arc<OnceLock<Result<(), RemoteRecoveryFailure>>>,
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

#[derive(Clone, Debug)]
struct RemoteRecoveryFailure {
    error_type: &'static str,
    detail: &'static str,
}

impl RemoteRecoveryFailure {
    fn into_corpus_error(self) -> CorpusError {
        remote_error(self.error_type, self.detail)
    }
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
        let credentials = RemoteCredentials::parse(&values)?;
        let mut builder = credentials
            .apply(AmazonS3Builder::new(), &values)
            .with_bucket_name(bucket)
            .with_region(environment.region)
            .with_virtual_hosted_style_request(!environment.path_style)
            .with_copy_if_not_exists(S3CopyIfNotExists::Multipart);
        if let Some(endpoint) = environment.endpoint {
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
            recovery: Arc::new(OnceLock::new()),
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
            .record("segment_publish", sha256, bytes, started, &result);
        result
    }

    fn upload_verified_inner(
        &self,
        source: File,
        sha256: &str,
        bytes: u64,
    ) -> Result<UploadDisposition, CorpusError> {
        self.ensure_recovered()?;
        let runtime = remote_runtime()?;
        let final_path = self.target.object_path(sha256)?;
        if runtime.block_on(remote_matches(
            Arc::clone(&self.target.store),
            &final_path,
            sha256,
            bytes,
        ))? {
            self.reused.fetch_add(1, Ordering::AcqRel);
            return Ok(UploadDisposition::Reused);
        }
        let staging_path = self.target.staging_path(sha256)?;
        let operation = runtime.block_on(async {
            upload_file(Arc::clone(&self.target.store), &staging_path, source).await?;
            if !remote_matches(Arc::clone(&self.target.store), &staging_path, sha256, bytes).await?
            {
                return Err(remote_error(
                    "digest_mismatch",
                    "staged remote object differs",
                ));
            }
            match self
                .target
                .store
                .copy_if_not_exists(&staging_path, &final_path)
                .await
            {
                Ok(()) | Err(object_store::Error::AlreadyExists { .. }) => {}
                Err(_) => {
                    return Err(remote_error(
                        "publish_failed",
                        "remote object publication failed",
                    ));
                }
            }
            if !remote_matches(Arc::clone(&self.target.store), &final_path, sha256, bytes).await? {
                return Err(remote_error(
                    "digest_mismatch",
                    "published remote object differs",
                ));
            }
            Ok(())
        });
        let cleanup = runtime.block_on(self.target.store.delete(&staging_path));
        if !matches!(cleanup, Ok(()) | Err(object_store::Error::NotFound { .. })) {
            return Err(remote_error(
                "staging_cleanup_failed",
                "remote staging cleanup failed",
            ));
        }
        operation?;
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
        self.ensure_recovered()?;
        let mut file = tempfile::tempfile().map_err(CorpusError::Io)?;
        remote_runtime()?.block_on(download_verified(
            Arc::clone(&self.target.store),
            &self.target.object_path(sha256)?,
            sha256,
            bytes,
            &mut file,
        ))?;
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

    #[cfg(test)]
    pub(crate) fn recovery_started(&self) -> bool {
        self.recovery.get().is_some()
    }

    fn recover_staging(&self) -> Result<(), RemoteRecoveryFailure> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let runtime = remote_runtime().map_err(|_| RemoteRecoveryFailure {
            error_type: "invalid_configuration",
            detail: "remote runtime initialization failed",
        })?;
        let prefix = self
            .target
            .staging_prefix()
            .map_err(|_| RemoteRecoveryFailure {
                error_type: "invalid_configuration",
                detail: "remote staging namespace is invalid",
            })?;
        runtime.block_on(recover_staging(
            Arc::clone(&self.target.store),
            &prefix,
            now,
        ))
    }

    fn ensure_recovered(&self) -> Result<(), CorpusError> {
        self.recovery
            .get_or_init(|| self.recover_staging())
            .clone()
            .map_err(RemoteRecoveryFailure::into_corpus_error)
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

    fn staging_path(&self, sha256: &str) -> Result<ObjectPath, CorpusError> {
        validate_sha256(sha256)?;
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        self.path(&format!(
            "frame-corpus/v1/staging/scorepeek-{}-{nanos}-{sequence}-{sha256}",
            std::process::id()
        ))
    }

    fn staging_prefix(&self) -> Result<ObjectPath, CorpusError> {
        self.path("frame-corpus/v1/staging")
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

async fn upload_file(
    store: Arc<dyn ObjectStore>,
    path: &ObjectPath,
    source: File,
) -> Result<(), CorpusError> {
    let upload = store
        .put_multipart(path)
        .await
        .map_err(|_| remote_error("upload_failed", "remote object upload failed"))?;
    upload_parts(source, upload).await
}

async fn upload_parts(
    source: File,
    mut upload: Box<dyn MultipartUpload>,
) -> Result<(), CorpusError> {
    let mut source = tokio::fs::File::from_std(source);
    let result = async {
        loop {
            let mut bytes = vec![0_u8; TRANSFER_BUFFER_BYTES];
            let read = source
                .read(&mut bytes)
                .await
                .map_err(|_| remote_error("upload_failed", "local segment read failed"))?;
            if read == 0 {
                break;
            }
            bytes.truncate(read);
            upload
                .put_part(bytes.into())
                .await
                .map_err(|_| remote_error("upload_failed", "remote object upload failed"))?;
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
            "staging_cleanup_failed",
            "remote multipart cleanup failed",
        ));
    }
    result
}

async fn recover_staging(
    store: Arc<dyn ObjectStore>,
    prefix: &ObjectPath,
    now_nanos: u128,
) -> Result<(), RemoteRecoveryFailure> {
    let mut objects = store.list(Some(prefix));
    while let Some(object) = objects.next().await {
        let object = object.map_err(|_| RemoteRecoveryFailure {
            error_type: "staging_cleanup_failed",
            detail: "remote staging inventory failed",
        })?;
        let direct_prefix = format!("{}/", prefix.as_ref());
        let Some(name) = object
            .location
            .as_ref()
            .strip_prefix(&direct_prefix)
            .filter(|name| !name.contains('/'))
        else {
            continue;
        };
        if !owned_staging_name(name)
            || !object_is_expired(object.last_modified.timestamp_nanos_opt(), now_nanos)
        {
            continue;
        }
        match store.delete(&object.location).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => {}
            Err(_) => {
                return Err(RemoteRecoveryFailure {
                    error_type: "staging_cleanup_failed",
                    detail: "remote staging cleanup failed",
                });
            }
        }
    }
    Ok(())
}

fn owned_staging_name(name: &str) -> bool {
    let Some(parts) = name.strip_prefix("scorepeek-") else {
        return false;
    };
    let fields = parts.split('-').collect::<Vec<_>>();
    let [pid, created, sequence, sha256] = fields.as_slice() else {
        return false;
    };
    if pid.parse::<u32>().is_err()
        || sequence.parse::<u64>().is_err()
        || validate_sha256(sha256).is_err()
    {
        return false;
    }
    created.parse::<u128>().is_ok()
}

fn object_is_expired(last_modified_nanos: Option<i64>, now_nanos: u128) -> bool {
    let Some(last_modified_nanos) = last_modified_nanos else {
        return false;
    };
    let Ok(last_modified_nanos) = u128::try_from(last_modified_nanos) else {
        return false;
    };
    now_nanos.saturating_sub(last_modified_nanos) >= STAGING_RECOVERY_AGE_NANOS
}

async fn remote_matches(
    store: Arc<dyn ObjectStore>,
    path: &ObjectPath,
    expected_sha256: &str,
    expected_bytes: u64,
) -> Result<bool, CorpusError> {
    match store.head(path).await {
        Ok(metadata) => {
            if metadata.size != expected_bytes {
                return Err(remote_error("size_mismatch", "remote object size differs"));
            }
            let mut sink = std::io::sink();
            digest_remote_into(store, path, expected_sha256, expected_bytes, &mut sink).await?;
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
    digest_remote_into(store, path, expected_sha256, expected_bytes, destination).await?;
    destination.sync_all().map_err(CorpusError::Io)
}

async fn digest_remote_into(
    store: Arc<dyn ObjectStore>,
    path: &ObjectPath,
    expected_sha256: &str,
    expected_bytes: u64,
    destination: &mut impl Write,
) -> Result<(), CorpusError> {
    let result = match store.get(path).await {
        Ok(result) => result,
        Err(object_store::Error::NotFound { .. }) => {
            return Err(remote_error("not_found", "remote object is unavailable"));
        }
        Err(
            object_store::Error::PermissionDenied { .. }
            | object_store::Error::Unauthenticated { .. },
        ) => {
            return Err(remote_error(
                "permission_denied",
                "remote object GET was denied",
            ));
        }
        Err(_) => return Err(remote_error("download_failed", "remote object GET failed")),
    };
    if result.meta.size != expected_bytes || result.range != (0..expected_bytes) {
        return Err(remote_error("size_mismatch", "remote object size differs"));
    }
    let mut stream = result.into_stream();
    let mut received = 0_u64;
    let mut digest = Sha256::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| remote_error("download_failed", "remote stream failed"))?;
        received = received.checked_add(chunk.len() as u64).ok_or_else(|| {
            remote_error("size_mismatch", "remote object exceeded its declared size")
        })?;
        if received > expected_bytes {
            return Err(remote_error(
                "size_mismatch",
                "remote object exceeded its declared size",
            ));
        }
        digest.update(&chunk);
        destination.write_all(&chunk).map_err(CorpusError::Io)?;
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
    use object_store::{PutPayload, PutResult, UploadPart};
    use std::collections::BTreeSet;
    use std::io::Read as _;
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
        let bytes = b"five!";
        let sha256 = crate::encode_digest(Sha256::digest(bytes));
        let path = remote.target.object_path(&sha256).unwrap();
        remote_runtime()
            .unwrap()
            .block_on(store.put(&path, PutPayload::from_static(bytes)))
            .unwrap();
        assert!(remote.materialize(&"0".repeat(64), 5).is_err());
    }

    #[derive(Debug)]
    struct FailingMultipartUpload {
        aborted: Arc<AtomicBool>,
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
        let result = remote_runtime().unwrap().block_on(upload_parts(
            source,
            Box::new(FailingMultipartUpload {
                aborted: Arc::clone(&aborted),
            }),
        ));
        assert!(result.is_err());
        assert!(aborted.load(Ordering::Acquire));
    }

    #[test]
    fn recovery_removes_only_expired_owned_staging_objects() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let remote = SegmentRemote::new(Arc::clone(&store), "test".to_owned()).unwrap();
        let sha256 = "a".repeat(64);
        let expired = remote
            .target
            .path(&format!("frame-corpus/v1/staging/scorepeek-1-0-0-{sha256}"))
            .unwrap();
        let current_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let current = remote
            .target
            .path(&format!(
                "frame-corpus/v1/staging/scorepeek-1-{current_nanos}-1-{sha256}"
            ))
            .unwrap();
        let foreign = remote
            .target
            .path("frame-corpus/v1/staging/provider-owned")
            .unwrap();
        let nested_foreign = remote
            .target
            .path(&format!(
                "frame-corpus/v1/staging/foreign/scorepeek-1-0-0-{sha256}"
            ))
            .unwrap();
        let runtime = remote_runtime().unwrap();
        for path in [&expired, &current, &foreign, &nested_foreign] {
            runtime
                .block_on(store.put(path, PutPayload::from_static(b"x")))
                .unwrap();
        }
        runtime
            .block_on(recover_staging(
                Arc::clone(&store),
                &remote.target.staging_prefix().unwrap(),
                current_nanos,
            ))
            .unwrap();
        assert!(runtime.block_on(store.head(&expired)).is_ok());
        assert!(runtime.block_on(store.head(&current)).is_ok());
        runtime
            .block_on(recover_staging(
                Arc::clone(&store),
                &remote.target.staging_prefix().unwrap(),
                current_nanos + STAGING_RECOVERY_AGE_NANOS + 1_000_000_000,
            ))
            .unwrap();
        assert!(matches!(
            runtime.block_on(store.head(&expired)),
            Err(object_store::Error::NotFound { .. })
        ));
        assert!(matches!(
            runtime.block_on(store.head(&current)),
            Err(object_store::Error::NotFound { .. })
        ));
        assert!(runtime.block_on(store.head(&foreign)).is_ok());
        assert!(runtime.block_on(store.head(&nested_foreign)).is_ok());
    }

    #[test]
    fn cached_recovery_failure_retains_its_stable_error_type() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let remote = SegmentRemote::new(store, "test".to_owned()).unwrap();
        remote
            .recovery
            .set(Err(RemoteRecoveryFailure {
                error_type: "staging_cleanup_failed",
                detail: "remote staging inventory failed",
            }))
            .unwrap();

        let first = remote.ensure_recovered().unwrap_err();
        let second = remote.ensure_recovered().unwrap_err();
        assert_eq!(remote_error_type(&first), Some("staging_cleanup_failed"));
        assert_eq!(remote_error_type(&second), Some("staging_cleanup_failed"));
        assert_eq!(
            first.to_string(),
            "invalid ingest request: remote staging_cleanup_failed: remote staging inventory failed"
        );
        assert_eq!(first.to_string(), second.to_string());
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
        }
        let bytes = fs::read_to_string(path).unwrap();
        assert!(bytes.contains("\"error_type\":\"permission_denied\""));
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
