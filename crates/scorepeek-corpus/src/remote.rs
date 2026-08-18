use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Seek as _, SeekFrom, Write as _};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::StreamExt as _;
use object_store::aws::{AmazonS3Builder, AmazonS3ConfigKey, S3CopyIfNotExists};
use object_store::buffered::BufWriter;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt, PutMode};
use serde::{Deserialize, Serialize};
use tempfile::Builder;
use tokio::io::{AsyncWriteExt as _, copy};
use url::Url;

use super::*;

const REMOTE_SCHEMA: &str = "scorepeek-corpus-s3-remote-v1";
const REMOTE_SUMMARY_SCHEMA: &str = "scorepeek-corpus-remote-summary-v1";
const TRANSFER_BUFFER_BYTES: usize = 8 * 1024 * 1024;
const CONDITIONAL_PUT_FALLBACK_BYTES: u64 = 64 * 1024 * 1024;
static REMOTE_STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteConfig {
    schema: String,
    url: String,
    region: String,
    endpoint: Option<String>,
    path_style: bool,
    allow_http_loopback: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DatasetRemoteSummary {
    pub schema: String,
    pub generation_sha256: String,
    pub object_count: u64,
    pub transferred_objects: u64,
    pub reused_objects: u64,
    pub total_bytes: u64,
}

struct RemoteTarget {
    store: Arc<dyn ObjectStore>,
    prefix: String,
}

struct DownloadedObject {
    file: File,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RemotePublishDisposition {
    Transferred,
    Reused,
}

impl CorpusStore {
    /// Uploads and read-back verifies one local recording dataset generation.
    ///
    /// # Errors
    /// Returns an error if local verification fails, remote configuration is invalid, or any
    /// remote object cannot be uploaded and verified without exposing provider response details.
    pub fn push_recording_dataset(
        &self,
        remote_config_path: impl AsRef<Path>,
        generation_sha256: &str,
    ) -> Result<DatasetRemoteSummary, CorpusError> {
        self.verify_recording_dataset(generation_sha256)?;
        let generation = self.load_recording_generation(generation_sha256)?;
        let target = load_remote(remote_config_path.as_ref())?;
        let runtime = remote_runtime()?;
        let mut transferred = 0_u64;
        let mut reused = 0_u64;
        for object in &generation.objects {
            let remote_path = target.object_path(&object.sha256)?;
            if runtime.block_on(remote_matches(
                Arc::clone(&target.store),
                &remote_path,
                &object.sha256,
                object.bytes,
            ))? {
                reused += 1;
                continue;
            }
            let local_file = if object.kind == dataset::DatasetObjectKind::SourceMedia {
                self.open_verified_source(&ContentRef {
                    sha256: object.sha256.clone(),
                    bytes: object.bytes,
                })?
            } else {
                File::open(self.dataset_object_path(object))?
            };
            let mut post_upload_source = if object.kind == dataset::DatasetObjectKind::SourceMedia {
                Some(local_file.try_clone()?)
            } else {
                None
            };
            let disposition = runtime.block_on(stage_and_publish_file(
                Arc::clone(&target.store),
                &target.staging_path(&object.sha256)?,
                &remote_path,
                local_file,
                post_upload_source.take(),
                &object.sha256,
                object.bytes,
            ))?;
            match disposition {
                RemotePublishDisposition::Transferred => transferred += 1,
                RemotePublishDisposition::Reused => reused += 1,
            }
        }

        let generation_path = self
            .root
            .join("dataset-generations")
            .join(format!("{generation_sha256}.json"));
        let generation_bytes = read_bounded_regular(
            &generation_path,
            dataset::MAX_DATASET_DOCUMENT_BYTES,
            ErrorContext::Request,
        )?;
        let remote_generation_path = target.generation_path(generation_sha256)?;
        if runtime.block_on(remote_matches(
            Arc::clone(&target.store),
            &remote_generation_path,
            generation_sha256,
            generation_bytes.len() as u64,
        ))? {
            reused += 1;
        } else {
            if digest_bytes(&generation_bytes) != generation_sha256 {
                return Err(remote_error("local generation digest changed"));
            }
            let generation_bytes_len = generation_bytes.len() as u64;
            let disposition = runtime.block_on(stage_and_publish_bytes(
                Arc::clone(&target.store),
                &target.staging_path(generation_sha256)?,
                &remote_generation_path,
                generation_bytes,
                generation_sha256,
                generation_bytes_len,
            ))?;
            match disposition {
                RemotePublishDisposition::Transferred => transferred += 1,
                RemotePublishDisposition::Reused => reused += 1,
            }
        }
        Ok(remote_summary(
            generation_sha256,
            &generation,
            transferred,
            reused,
        ))
    }

    /// Downloads and verifies one recording dataset generation into this private store.
    ///
    /// # Errors
    /// Returns an error if remote configuration or bytes are invalid, any binding is incomplete,
    /// or verified local publication cannot complete durably.
    pub fn pull_recording_dataset(
        &self,
        remote_config_path: impl AsRef<Path>,
        generation_sha256: &str,
    ) -> Result<DatasetRemoteSummary, CorpusError> {
        let remote_config_path = remote_config_path.as_ref();
        validate_sha256(
            generation_sha256,
            "generation_sha256",
            ErrorContext::Request,
        )?;
        preflight_managed_components(&self.root)?;
        create_private_directory(&self.root)?;
        self.verify_remote_recording_dataset(remote_config_path, generation_sha256)?;
        let target = load_remote(remote_config_path)?;
        let runtime = remote_runtime()?;
        let generation_temp = download_verified_bounded(
            &runtime,
            Arc::clone(&target.store),
            &target.generation_path(generation_sha256)?,
            generation_sha256,
            dataset::MAX_DATASET_DOCUMENT_BYTES as u64,
            &self.root,
        )?;
        let generation_bytes =
            read_downloaded_bounded(&generation_temp, dataset::MAX_DATASET_DOCUMENT_BYTES)?;
        let generation: dataset::RecordingDatasetGeneration =
            serde_json::from_slice(&generation_bytes)?;
        generation.validate()?;
        if canonical_json(&generation)? != generation_bytes {
            return Err(remote_error("remote generation is not canonical"));
        }
        self.preflight_pull_capacity(&generation, generation_bytes.len() as u64)?;

        let mut transferred = 0_u64;
        let mut reused = 0_u64;
        for object in &generation.objects {
            if self.dataset_object_is_present(object)? {
                reused += 1;
                continue;
            }
            let temporary = download_verified(
                &runtime,
                Arc::clone(&target.store),
                &target.object_path(&object.sha256)?,
                &object.sha256,
                object.bytes,
                &self.root,
            )?;
            self.publish_downloaded_object(object, &temporary)?;
            transferred += 1;
        }
        self.publish_dataset_document("dataset-generations", generation_sha256, &generation_bytes)?;
        self.verify_recording_dataset(generation_sha256)?;
        Ok(remote_summary(
            generation_sha256,
            &generation,
            transferred,
            reused,
        ))
    }

    /// Downloads and hashes every remote byte bound by one recording dataset generation.
    ///
    /// # Errors
    /// Returns an error if the generation or any referenced object is unavailable, malformed,
    /// oversized, digest-mismatched, or semantically inconsistent.
    pub fn verify_remote_recording_dataset(
        &self,
        remote_config_path: impl AsRef<Path>,
        generation_sha256: &str,
    ) -> Result<DatasetRemoteSummary, CorpusError> {
        preflight_managed_components(&self.root)?;
        create_private_directory(&self.root)?;
        let target = load_remote(remote_config_path.as_ref())?;
        let runtime = remote_runtime()?;
        let generation_temp = download_verified_bounded(
            &runtime,
            Arc::clone(&target.store),
            &target.generation_path(generation_sha256)?,
            generation_sha256,
            dataset::MAX_DATASET_DOCUMENT_BYTES as u64,
            &self.root,
        )?;
        let bytes = read_downloaded_bounded(&generation_temp, dataset::MAX_DATASET_DOCUMENT_BYTES)?;
        let generation: dataset::RecordingDatasetGeneration = serde_json::from_slice(&bytes)?;
        generation.validate()?;
        if canonical_json(&generation)? != bytes {
            return Err(remote_error("remote generation is not canonical"));
        }
        for object in generation
            .objects
            .iter()
            .filter(|object| object.kind == dataset::DatasetObjectKind::SourceMedia)
        {
            if !runtime.block_on(remote_matches(
                Arc::clone(&target.store),
                &target.object_path(&object.sha256)?,
                &object.sha256,
                object.bytes,
            ))? {
                return Err(remote_error("remote dataset object is unavailable"));
            }
        }
        for recording_object in generation
            .objects
            .iter()
            .filter(|object| object.kind == dataset::DatasetObjectKind::RecordingManifest)
        {
            let source_manifest = dataset::required_object(
                &generation,
                &recording_object.recording_sha256,
                dataset::DatasetObjectKind::SourceManifest,
            )?;
            let capture_profile = dataset::required_object(
                &generation,
                &recording_object.recording_sha256,
                dataset::DatasetObjectKind::CaptureProfile,
            )?;
            let media_probe = dataset::required_object(
                &generation,
                &recording_object.recording_sha256,
                dataset::DatasetObjectKind::MediaProbe,
            )?;
            let recording_bytes = download_document_bytes(
                &runtime,
                &target,
                recording_object,
                MAX_REQUEST_BYTES,
                &self.root,
            )?;
            let source_bytes = download_document_bytes(
                &runtime,
                &target,
                source_manifest,
                MAX_REQUEST_BYTES,
                &self.root,
            )?;
            let profile_bytes = download_document_bytes(
                &runtime,
                &target,
                capture_profile,
                MAX_REQUEST_BYTES,
                &self.root,
            )?;
            let probe_bytes = download_document_bytes(
                &runtime,
                &target,
                media_probe,
                dataset::MAX_DATASET_DOCUMENT_BYTES,
                &self.root,
            )?;
            dataset::validate_recording_bundle(
                &generation,
                recording_object,
                &recording_bytes,
                &source_bytes,
                &profile_bytes,
                &probe_bytes,
            )?;
        }
        Ok(remote_summary(
            generation_sha256,
            &generation,
            0,
            generation.objects.len() as u64 + 1,
        ))
    }

    fn preflight_pull_capacity(
        &self,
        generation: &dataset::RecordingDatasetGeneration,
        generation_bytes: u64,
    ) -> Result<(), CorpusError> {
        let lock = open_store_lock(&self.root, true)?;
        lock.lock()?;
        preflight_managed_components(&self.root)?;
        for directory in [
            "content",
            "manifests",
            "profiles",
            "probes",
            "recordings",
            "dataset-generations",
        ] {
            create_private_directory(&self.root.join(directory))?;
        }
        recover_staging(&self.root.join("content"), &self.root.join("manifests"))?;

        let mut seen = BTreeSet::new();
        let mut content_count = 0_usize;
        let mut content_bytes = 0_u64;
        let mut manifest_count = 0_usize;
        let mut manifest_bytes = 0_u64;
        let mut documents = BTreeMap::<&str, (usize, u64)>::new();
        for object in &generation.objects {
            let destination = self.dataset_object_path(object);
            if !seen.insert(destination.clone()) {
                continue;
            }
            if self.dataset_object_is_present(object)? {
                continue;
            }
            match object.kind {
                dataset::DatasetObjectKind::SourceMedia => {
                    content_count = content_count
                        .checked_add(1)
                        .ok_or(CorpusError::CapacityExceeded)?;
                    content_bytes = content_bytes
                        .checked_add(object.bytes)
                        .ok_or(CorpusError::CapacityExceeded)?;
                }
                dataset::DatasetObjectKind::SourceManifest => {
                    manifest_count = manifest_count
                        .checked_add(1)
                        .ok_or(CorpusError::CapacityExceeded)?;
                    manifest_bytes = manifest_bytes
                        .checked_add(object.bytes)
                        .ok_or(CorpusError::CapacityExceeded)?;
                }
                dataset::DatasetObjectKind::CaptureProfile => {
                    add_document_capacity(&mut documents, "profiles", object.bytes)?;
                }
                dataset::DatasetObjectKind::MediaProbe => {
                    add_document_capacity(&mut documents, "probes", object.bytes)?;
                }
                dataset::DatasetObjectKind::RecordingManifest => {
                    add_document_capacity(&mut documents, "recordings", object.bytes)?;
                }
            }
        }
        let generation_sha256 = digest_bytes(&canonical_json(generation)?);
        let generation_path = self
            .root
            .join("dataset-generations")
            .join(format!("{generation_sha256}.json"));
        match generation_path.symlink_metadata() {
            Ok(_) => {
                validate_private_file_mode(&generation_path, ErrorContext::Request)?;
                let existing = read_bounded_regular(
                    &generation_path,
                    dataset::MAX_DATASET_DOCUMENT_BYTES,
                    ErrorContext::Request,
                )?;
                if existing.len() as u64 != generation_bytes
                    || digest_bytes(&existing) != generation_sha256
                {
                    return Err(CorpusError::InvalidRequest(
                        "local dataset generation differs from remote bytes".to_owned(),
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                add_document_capacity(&mut documents, "dataset-generations", generation_bytes)?;
            }
            Err(error) => return Err(error.into()),
        }
        ensure_content_capacity(&self.root.join("content"), content_count, content_bytes)?;
        ensure_manifest_capacity_additions(
            &self.root.join("manifests"),
            manifest_count,
            manifest_bytes,
        )?;
        for (directory, (count, bytes)) in documents {
            dataset::ensure_dataset_document_capacity(
                &self.root.join(directory),
                directory,
                count,
                bytes,
            )?;
        }
        drop(lock);
        Ok(())
    }

    fn publish_downloaded_object(
        &self,
        object: &dataset::DatasetObject,
        temporary: &DownloadedObject,
    ) -> Result<(), CorpusError> {
        if object.kind != dataset::DatasetObjectKind::SourceMedia {
            let maximum = match object.kind {
                dataset::DatasetObjectKind::SourceManifest
                | dataset::DatasetObjectKind::CaptureProfile
                | dataset::DatasetObjectKind::RecordingManifest => MAX_REQUEST_BYTES,
                dataset::DatasetObjectKind::MediaProbe => dataset::MAX_DATASET_DOCUMENT_BYTES,
                dataset::DatasetObjectKind::SourceMedia => unreachable!(),
            };
            let bytes = read_downloaded_bounded(temporary, maximum)?;
            let (directory, name) = match object.kind {
                dataset::DatasetObjectKind::SourceManifest => {
                    ("manifests", object.recording_sha256.as_str())
                }
                dataset::DatasetObjectKind::CaptureProfile => ("profiles", object.sha256.as_str()),
                dataset::DatasetObjectKind::MediaProbe => ("probes", object.sha256.as_str()),
                dataset::DatasetObjectKind::RecordingManifest => {
                    ("recordings", object.recording_sha256.as_str())
                }
                dataset::DatasetObjectKind::SourceMedia => unreachable!(),
            };
            self.publish_named_dataset_document(directory, name, &bytes)?;
            return self.validate_dataset_object(object);
        }

        let destination = self.dataset_object_path(object);
        let lock = open_store_lock(&self.root, true)?;
        lock.lock()?;
        let content_dir = self.root.join("content");
        let manifest_dir = self.root.join("manifests");
        create_private_directory(&content_dir)?;
        create_private_directory(&manifest_dir)?;
        recover_staging(&content_dir, &manifest_dir)?;
        if self.dataset_object_is_present(object)? {
            self.validate_dataset_object(object)?;
            return Ok(());
        }
        ensure_capacity(&content_dir, object.bytes)?;
        let staging = Builder::new()
            .prefix(SOURCE_STAGING_PREFIX)
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(&content_dir)?;
        let staged_source = staging.path().join(SOURCE_FILE);
        let mut source = temporary.file.try_clone()?;
        source.seek(SeekFrom::Start(0))?;
        let mut staged = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staged_source)?;
        if io::copy(&mut source, &mut staged)? != object.bytes {
            return Err(remote_error("downloaded source changed before publication"));
        }
        staged.flush()?;
        staged.sync_all()?;
        drop(staged);
        if digest_regular_file(&staged_source, object.bytes)? != object.sha256 {
            return Err(remote_error(
                "downloaded source changed during publication copy",
            ));
        }
        File::open(staging.path())?.sync_all()?;
        let staging_path = staging.keep();
        let destination_dir = destination.parent().ok_or_else(|| {
            CorpusError::InvalidRequest("source destination has no parent".to_owned())
        })?;
        fs::rename(staging_path, destination_dir)?;
        sync_stored_source_and_parent(destination_dir, &content_dir)?;
        drop(lock);
        self.validate_dataset_object(object)
    }
}

impl RemoteConfig {
    fn validate(&self) -> Result<(String, String, bool), CorpusError> {
        if self.schema != REMOTE_SCHEMA {
            return Err(remote_error("unsupported remote schema"));
        }
        validate_token(&self.region, "region", ErrorContext::Request)?;
        let remainder = self
            .url
            .strip_prefix("s3://")
            .ok_or_else(|| remote_error("remote URL must use s3 scheme"))?;
        let (bucket, prefix) = remainder.split_once('/').unwrap_or((remainder, ""));
        if bucket.is_empty()
            || !bucket.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
            })
        {
            return Err(remote_error("remote bucket is invalid"));
        }
        validate_prefix(prefix)?;
        let allow_http = match self.endpoint.as_deref() {
            Some(endpoint) if is_strict_https_origin(endpoint) => {
                if self.allow_http_loopback {
                    return Err(remote_error(
                        "loopback HTTP allowance requires a loopback HTTP endpoint",
                    ));
                }
                false
            }
            Some(endpoint) if self.allow_http_loopback && is_explicit_loopback_http(endpoint) => {
                true
            }
            Some(_) => return Err(remote_error("remote endpoint must use HTTPS")),
            None if self.allow_http_loopback => {
                return Err(remote_error(
                    "loopback HTTP allowance requires an explicit endpoint",
                ));
            }
            None => false,
        };
        Ok((
            bucket.to_owned(),
            prefix.trim_matches('/').to_owned(),
            allow_http,
        ))
    }
}

impl RemoteTarget {
    fn object_path(&self, sha256: &str) -> Result<ObjectPath, CorpusError> {
        validate_sha256(sha256, "remote object sha256", ErrorContext::Request)?;
        self.path(&format!("v1/objects/sha256/{}/{sha256}", &sha256[..2]))
    }

    fn generation_path(&self, sha256: &str) -> Result<ObjectPath, CorpusError> {
        validate_sha256(sha256, "remote generation sha256", ErrorContext::Request)?;
        self.path(&format!("v1/generations/{sha256}.json"))
    }

    fn staging_path(&self, sha256: &str) -> Result<ObjectPath, CorpusError> {
        validate_sha256(sha256, "remote object sha256", ErrorContext::Request)?;
        let sequence = REMOTE_STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        self.path(&format!(
            "v1/staging/scorepeek-{}-{nanos}-{}-{sha256}",
            std::process::id(),
            sequence,
        ))
    }

    fn path(&self, suffix: &str) -> Result<ObjectPath, CorpusError> {
        let value = if self.prefix.is_empty() {
            suffix.to_owned()
        } else {
            format!("{}/{suffix}", self.prefix)
        };
        ObjectPath::parse(value).map_err(|_| remote_error("remote object key is invalid"))
    }
}

fn load_remote(path: &Path) -> Result<RemoteTarget, CorpusError> {
    let bytes = read_bounded_regular(path, MAX_REQUEST_BYTES, ErrorContext::Request)?;
    let config: RemoteConfig = serde_json::from_slice(&bytes)?;
    let (bucket, prefix, allow_http) = config.validate()?;
    let mut builder = credential_environment(AmazonS3Builder::new())
        .with_bucket_name(bucket)
        .with_region(config.region)
        .with_virtual_hosted_style_request(!config.path_style)
        .with_allow_http(allow_http);
    if let Some(endpoint) = config.endpoint {
        builder = builder.with_endpoint(endpoint);
    }
    builder = builder.with_copy_if_not_exists(S3CopyIfNotExists::Multipart);
    let store = builder
        .build()
        .map_err(|_| remote_error("remote client configuration failed"))?;
    Ok(RemoteTarget {
        store: Arc::new(store),
        prefix,
    })
}

fn is_explicit_loopback_http(endpoint: &str) -> bool {
    let Ok(url) = Url::parse(endpoint) else {
        return false;
    };
    url.scheme() == "http"
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
        && url.port().is_some_and(|port| port != 0)
        && url
            .host_str()
            .is_some_and(|host| host == "127.0.0.1" || host == "[::1]" || host == "::1")
}

fn is_strict_https_origin(endpoint: &str) -> bool {
    let Ok(url) = Url::parse(endpoint) else {
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

fn credential_environment(mut builder: AmazonS3Builder) -> AmazonS3Builder {
    for (name, key) in [
        ("AWS_ACCESS_KEY_ID", AmazonS3ConfigKey::AccessKeyId),
        ("AWS_SECRET_ACCESS_KEY", AmazonS3ConfigKey::SecretAccessKey),
        ("AWS_SESSION_TOKEN", AmazonS3ConfigKey::Token),
        (
            "AWS_WEB_IDENTITY_TOKEN_FILE",
            AmazonS3ConfigKey::WebIdentityTokenFile,
        ),
        ("AWS_ROLE_ARN", AmazonS3ConfigKey::RoleArn),
        ("AWS_ROLE_SESSION_NAME", AmazonS3ConfigKey::RoleSessionName),
        (
            "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
            AmazonS3ConfigKey::ContainerCredentialsRelativeUri,
        ),
        (
            "AWS_CONTAINER_CREDENTIALS_FULL_URI",
            AmazonS3ConfigKey::ContainerCredentialsFullUri,
        ),
        (
            "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
            AmazonS3ConfigKey::ContainerAuthorizationTokenFile,
        ),
    ] {
        if let Ok(value) = std::env::var(name) {
            builder = builder.with_config(key, value);
        }
    }
    builder
}

fn remote_runtime() -> Result<tokio::runtime::Runtime, CorpusError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| remote_error("remote runtime could not start"))
}

async fn upload_file(
    store: Arc<dyn ObjectStore>,
    remote_path: &ObjectPath,
    source: File,
) -> Result<(), CorpusError> {
    let mut source = tokio::fs::File::from_std(source);
    let mut writer = BufWriter::with_capacity(store, remote_path.clone(), TRANSFER_BUFFER_BYTES);
    if copy(&mut source, &mut writer).await.is_err() {
        writer
            .abort()
            .await
            .map_err(|_| remote_error("remote multipart cleanup failed"))?;
        return Err(remote_error("remote object upload failed"));
    }
    writer
        .shutdown()
        .await
        .map_err(|_| remote_error("remote object upload did not complete"))
}

async fn stage_and_publish_file(
    store: Arc<dyn ObjectStore>,
    staging_path: &ObjectPath,
    final_path: &ObjectPath,
    source: File,
    mut post_upload_source: Option<File>,
    expected_sha256: &str,
    expected_bytes: u64,
) -> Result<RemotePublishDisposition, CorpusError> {
    let operation = async {
        upload_file(Arc::clone(&store), staging_path, source).await?;
        if let Some(source) = &mut post_upload_source {
            verify_open_source(
                source,
                &ContentRef {
                    sha256: expected_sha256.to_owned(),
                    bytes: expected_bytes,
                },
            )?;
        }
        if !remote_matches(
            Arc::clone(&store),
            staging_path,
            expected_sha256,
            expected_bytes,
        )
        .await?
        {
            return Err(remote_error("staged remote object was not readable"));
        }
        let disposition = publish_staged_object(
            Arc::clone(&store),
            staging_path,
            final_path,
            expected_sha256,
            expected_bytes,
        )
        .await?;
        if !remote_matches(
            Arc::clone(&store),
            final_path,
            expected_sha256,
            expected_bytes,
        )
        .await?
        {
            return Err(remote_error("published remote object was not readable"));
        }
        Ok(disposition)
    }
    .await;

    let cleanup = match store.delete(staging_path).await {
        Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
        Err(_) => Err(remote_error("remote staging cleanup failed")),
    };
    cleanup?;
    operation
}

async fn stage_and_publish_bytes(
    store: Arc<dyn ObjectStore>,
    staging_path: &ObjectPath,
    final_path: &ObjectPath,
    bytes: Vec<u8>,
    expected_sha256: &str,
    expected_bytes: u64,
) -> Result<RemotePublishDisposition, CorpusError> {
    let operation = async {
        store
            .put(staging_path, bytes.into())
            .await
            .map_err(|_| remote_error("remote staging upload failed"))?;
        if !remote_matches(
            Arc::clone(&store),
            staging_path,
            expected_sha256,
            expected_bytes,
        )
        .await?
        {
            return Err(remote_error("staged remote object was not readable"));
        }
        let disposition = publish_staged_object(
            Arc::clone(&store),
            staging_path,
            final_path,
            expected_sha256,
            expected_bytes,
        )
        .await?;
        if !remote_matches(
            Arc::clone(&store),
            final_path,
            expected_sha256,
            expected_bytes,
        )
        .await?
        {
            return Err(remote_error("published remote object was not readable"));
        }
        Ok(disposition)
    }
    .await;

    let cleanup = match store.delete(staging_path).await {
        Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
        Err(_) => Err(remote_error("remote staging cleanup failed")),
    };
    cleanup?;
    operation
}

async fn publish_staged_object(
    store: Arc<dyn ObjectStore>,
    staging_path: &ObjectPath,
    final_path: &ObjectPath,
    expected_sha256: &str,
    expected_bytes: u64,
) -> Result<RemotePublishDisposition, CorpusError> {
    match store.copy_if_not_exists(staging_path, final_path).await {
        Ok(()) => Ok(RemotePublishDisposition::Transferred),
        Err(object_store::Error::AlreadyExists { .. }) => Ok(RemotePublishDisposition::Reused),
        Err(_) => {
            let bytes = read_remote_bytes_bounded(
                Arc::clone(&store),
                staging_path,
                expected_sha256,
                expected_bytes,
                CONDITIONAL_PUT_FALLBACK_BYTES,
            )
            .await?;
            match store
                .put_opts(final_path, bytes.into(), PutMode::Create.into())
                .await
            {
                Ok(_) => Ok(RemotePublishDisposition::Transferred),
                Err(object_store::Error::AlreadyExists { .. }) => {
                    Ok(RemotePublishDisposition::Reused)
                }
                Err(_) => Err(remote_error("remote object publication failed")),
            }
        }
    }
}

async fn read_remote_bytes_bounded(
    store: Arc<dyn ObjectStore>,
    remote_path: &ObjectPath,
    expected_sha256: &str,
    expected_bytes: u64,
    maximum_bytes: u64,
) -> Result<Vec<u8>, CorpusError> {
    if expected_bytes > maximum_bytes {
        return Err(remote_error(
            "remote conditional publication fallback exceeded its size limit",
        ));
    }
    let result = store
        .get(remote_path)
        .await
        .map_err(|_| remote_error("staged remote object download failed"))?;
    if result.meta.size != expected_bytes || result.range != (0..expected_bytes) {
        return Err(remote_error("staged remote object changed before download"));
    }
    let capacity = usize::try_from(expected_bytes).map_err(|_| CorpusError::CapacityExceeded)?;
    let mut bytes = Vec::with_capacity(capacity);
    let mut stream = result.into_stream();
    let mut hasher = Sha256::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| remote_error("staged remote object download failed"))?;
        if bytes.len().saturating_add(chunk.len()) > capacity {
            return Err(remote_error(
                "staged remote object exceeded its declared size",
            ));
        }
        hasher.update(&chunk);
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() != capacity || encode_digest(hasher.finalize()) != expected_sha256 {
        return Err(remote_error("staged remote object digest differs"));
    }
    Ok(bytes)
}

async fn remote_matches(
    store: Arc<dyn ObjectStore>,
    remote_path: &ObjectPath,
    expected_sha256: &str,
    expected_bytes: u64,
) -> Result<bool, CorpusError> {
    match store.head(remote_path).await {
        Ok(metadata) => {
            if metadata.size != expected_bytes {
                return Err(remote_error("existing remote object size differs"));
            }
            if digest_remote_object(store, &metadata).await? != expected_sha256 {
                return Err(remote_error("existing remote object digest differs"));
            }
            Ok(true)
        }
        Err(object_store::Error::NotFound { .. }) => Ok(false),
        Err(_) => Err(remote_error("remote object lookup failed")),
    }
}

async fn digest_remote_object(
    store: Arc<dyn ObjectStore>,
    metadata: &object_store::ObjectMeta,
) -> Result<String, CorpusError> {
    let result = store
        .get(&metadata.location)
        .await
        .map_err(|_| remote_error("remote object download failed"))?;
    if result.meta.size != metadata.size || result.range != (0..metadata.size) {
        return Err(remote_error("remote object changed before download"));
    }
    let mut stream = result.into_stream();
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| remote_error("remote object download failed"))?;
        let count = chunk.len();
        total = total
            .checked_add(count as u64)
            .ok_or(CorpusError::CapacityExceeded)?;
        if total > metadata.size {
            return Err(remote_error("remote object exceeded its declared size"));
        }
        hasher.update(&chunk);
    }
    if total != metadata.size {
        return Err(remote_error("remote object size changed during download"));
    }
    Ok(encode_digest(hasher.finalize()))
}

fn download_verified(
    runtime: &tokio::runtime::Runtime,
    store: Arc<dyn ObjectStore>,
    remote_path: &ObjectPath,
    expected_sha256: &str,
    expected_bytes: u64,
    staging_root: &Path,
) -> Result<DownloadedObject, CorpusError> {
    let metadata = runtime
        .block_on(store.head(remote_path))
        .map_err(|_| remote_error("remote object is unavailable"))?;
    if metadata.size != expected_bytes {
        return Err(remote_error("remote object size differs"));
    }
    let temporary = runtime.block_on(download_to_temporary(store, &metadata, staging_root))?;
    if digest_downloaded(&temporary, expected_bytes)? != expected_sha256 {
        return Err(remote_error("remote object digest differs"));
    }
    Ok(temporary)
}

fn download_verified_bounded(
    runtime: &tokio::runtime::Runtime,
    store: Arc<dyn ObjectStore>,
    remote_path: &ObjectPath,
    expected_sha256: &str,
    maximum_bytes: u64,
    staging_root: &Path,
) -> Result<DownloadedObject, CorpusError> {
    let metadata = runtime
        .block_on(store.head(remote_path))
        .map_err(|_| remote_error("remote object is unavailable"))?;
    if metadata.size == 0 || metadata.size > maximum_bytes {
        return Err(remote_error(
            "remote object size is outside the admitted range",
        ));
    }
    let expected_bytes = metadata.size;
    let temporary = runtime.block_on(download_to_temporary(store, &metadata, staging_root))?;
    if digest_downloaded(&temporary, expected_bytes)? != expected_sha256 {
        return Err(remote_error("remote object digest differs"));
    }
    Ok(temporary)
}

fn download_document_bytes(
    runtime: &tokio::runtime::Runtime,
    target: &RemoteTarget,
    object: &dataset::DatasetObject,
    maximum: usize,
    staging_root: &Path,
) -> Result<Vec<u8>, CorpusError> {
    if object.bytes == 0 || object.bytes > maximum as u64 {
        return Err(remote_error(
            "remote typed document size is outside the admitted range",
        ));
    }
    let downloaded = download_verified(
        runtime,
        Arc::clone(&target.store),
        &target.object_path(&object.sha256)?,
        &object.sha256,
        object.bytes,
        staging_root,
    )?;
    read_downloaded_bounded(&downloaded, maximum)
}

fn add_document_capacity<'a>(
    documents: &mut BTreeMap<&'a str, (usize, u64)>,
    directory: &'a str,
    bytes: u64,
) -> Result<(), CorpusError> {
    let addition = documents.entry(directory).or_default();
    addition.0 = addition
        .0
        .checked_add(1)
        .ok_or(CorpusError::CapacityExceeded)?;
    addition.1 = addition
        .1
        .checked_add(bytes)
        .ok_or(CorpusError::CapacityExceeded)?;
    Ok(())
}

async fn download_to_temporary(
    store: Arc<dyn ObjectStore>,
    metadata: &object_store::ObjectMeta,
    staging_root: &Path,
) -> Result<DownloadedObject, CorpusError> {
    let file = tempfile::tempfile_in(staging_root)
        .map_err(|_| remote_error("anonymous download staging could not be created"))?;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|_| remote_error("download staging permissions failed"))?;
    let mut destination = tokio::fs::File::from_std(file);
    let result = store
        .get(&metadata.location)
        .await
        .map_err(|_| remote_error("remote object download failed"))?;
    if result.meta.size != metadata.size || result.range != (0..metadata.size) {
        return Err(remote_error("remote object changed before download"));
    }
    let mut stream = result.into_stream();
    let mut total = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| remote_error("remote object download failed"))?;
        total = total
            .checked_add(chunk.len() as u64)
            .ok_or(CorpusError::CapacityExceeded)?;
        if total > metadata.size {
            return Err(remote_error("remote object exceeded its declared size"));
        }
        destination
            .write_all(&chunk)
            .await
            .map_err(|_| remote_error("download staging write failed"))?;
    }
    if total != metadata.size {
        return Err(remote_error("remote object size changed during download"));
    }
    destination
        .flush()
        .await
        .map_err(|_| remote_error("download staging flush failed"))?;
    destination
        .sync_all()
        .await
        .map_err(|_| remote_error("download staging sync failed"))?;
    Ok(DownloadedObject {
        file: destination.into_std().await,
    })
}

fn digest_downloaded(
    downloaded: &DownloadedObject,
    expected_bytes: u64,
) -> Result<String, CorpusError> {
    let mut file = downloaded.file.try_clone()?;
    if !file.metadata()?.is_file() || file.metadata()?.len() != expected_bytes {
        return Err(remote_error("download staging size differs"));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut total = 0_u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or(CorpusError::CapacityExceeded)?;
        if total > expected_bytes {
            return Err(remote_error("download staging exceeded its declared size"));
        }
        hasher.update(&buffer[..count]);
    }
    if total != expected_bytes {
        return Err(remote_error("download staging size changed"));
    }
    Ok(encode_digest(hasher.finalize()))
}

fn read_downloaded_bounded(
    downloaded: &DownloadedObject,
    maximum: usize,
) -> Result<Vec<u8>, CorpusError> {
    let mut file = downloaded.file.try_clone()?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum as u64 {
        return Err(remote_error(
            "remote typed document size is outside the admitted range",
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    let capacity = usize::try_from(metadata.len()).map_err(|_| CorpusError::CapacityExceeded)?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take((maximum + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > maximum || bytes.len() as u64 != metadata.len() {
        return Err(remote_error("download staging size changed"));
    }
    Ok(bytes)
}

fn validate_prefix(prefix: &str) -> Result<(), CorpusError> {
    if prefix.len() > 512
        || prefix.split('/').any(|segment| {
            segment == "."
                || segment == ".."
                || segment.chars().any(char::is_control)
                || segment.contains('\\')
        })
    {
        return Err(remote_error("remote prefix is invalid"));
    }
    Ok(())
}

fn remote_summary(
    generation_sha256: &str,
    generation: &dataset::RecordingDatasetGeneration,
    transferred_objects: u64,
    reused_objects: u64,
) -> DatasetRemoteSummary {
    DatasetRemoteSummary {
        schema: REMOTE_SUMMARY_SCHEMA.to_owned(),
        generation_sha256: generation_sha256.to_owned(),
        object_count: generation.objects.len() as u64 + 1,
        transferred_objects,
        reused_objects,
        total_bytes: generation
            .objects
            .iter()
            .fold(0_u64, |total, object| total.saturating_add(object.bytes)),
    }
}

fn remote_error(stage: &str) -> CorpusError {
    CorpusError::InvalidRequest(format!("remote dataset operation failed: {stage}"))
}

#[cfg(test)]
mod tests {
    use object_store::memory::InMemory;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn remote_config_allows_only_explicit_test_loopback_http() {
        let config = |endpoint: Option<&str>, allow_http_loopback| RemoteConfig {
            schema: REMOTE_SCHEMA.to_owned(),
            url: "s3://private-bucket/scorepeek".to_owned(),
            region: "test-region-1".to_owned(),
            endpoint: endpoint.map(str::to_owned),
            path_style: true,
            allow_http_loopback,
        };
        assert!(
            config(Some("http://127.0.0.1:49152"), true)
                .validate()
                .unwrap()
                .2
        );
        assert!(
            config(Some("http://[::1]:49152"), true)
                .validate()
                .unwrap()
                .2
        );
        assert!(
            config(Some("http://localhost:49152"), true)
                .validate()
                .is_err()
        );
        assert!(
            config(Some("http://127.0.0.1.example:49152"), true)
                .validate()
                .is_err()
        );
        assert!(
            config(Some("http://127.0.0.1:49152"), false)
                .validate()
                .is_err()
        );
        assert!(
            config(Some("https://objects.example"), true)
                .validate()
                .is_err()
        );
        assert!(
            !config(Some("https://objects.example"), false)
                .validate()
                .unwrap()
                .2
        );
        for endpoint in [
            "https://access:secret@objects.example",
            "https://objects.example/private-path",
            "https://objects.example?credential=secret",
            "https://objects.example#fragment",
        ] {
            assert!(config(Some(endpoint), false).validate().is_err());
        }
        for endpoint in [
            "http://access:secret@127.0.0.1:49152",
            "http://127.0.0.1:49152/private-path",
            "http://127.0.0.1:49152?credential=secret",
            "http://127.0.0.1:49152#fragment",
        ] {
            assert!(config(Some(endpoint), true).validate().is_err());
        }
    }

    #[test]
    fn remote_reuse_hashes_complete_bytes_and_cleans_staging() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("source");
        fs::write(&source, b"hello").unwrap();
        let expected_sha256 = digest_bytes(b"hello");
        let remote_path = ObjectPath::from("v1/object");
        let staging_path = ObjectPath::from("v1/staging/first");
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let runtime = remote_runtime().unwrap();

        assert_eq!(
            runtime
                .block_on(stage_and_publish_file(
                    Arc::clone(&store),
                    &staging_path,
                    &remote_path,
                    File::open(&source).unwrap(),
                    None,
                    &expected_sha256,
                    5,
                ))
                .unwrap(),
            RemotePublishDisposition::Transferred
        );
        assert!(matches!(
            runtime.block_on(store.head(&staging_path)),
            Err(object_store::Error::NotFound { .. })
        ));
        assert_eq!(
            runtime
                .block_on(stage_and_publish_file(
                    Arc::clone(&store),
                    &ObjectPath::from("v1/staging/second"),
                    &remote_path,
                    File::open(&source).unwrap(),
                    None,
                    &expected_sha256,
                    5,
                ))
                .unwrap(),
            RemotePublishDisposition::Reused
        );
        let failed_staging = ObjectPath::from("v1/staging/failed");
        let failed_final = ObjectPath::from("v1/failed-object");
        assert!(
            runtime
                .block_on(stage_and_publish_file(
                    Arc::clone(&store),
                    &failed_staging,
                    &failed_final,
                    File::open(&source).unwrap(),
                    None,
                    &"0".repeat(64),
                    5,
                ))
                .is_err()
        );
        assert!(matches!(
            runtime.block_on(store.head(&failed_staging)),
            Err(object_store::Error::NotFound { .. })
        ));
        assert!(matches!(
            runtime.block_on(store.head(&failed_final)),
            Err(object_store::Error::NotFound { .. })
        ));
        assert!(
            runtime
                .block_on(remote_matches(
                    Arc::clone(&store),
                    &remote_path,
                    &expected_sha256,
                    5,
                ))
                .unwrap()
        );
        assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 1);

        let downloaded = download_verified_bounded(
            &runtime,
            Arc::clone(&store),
            &remote_path,
            &expected_sha256,
            5,
            temporary.path(),
        )
        .unwrap();
        assert_eq!(read_downloaded_bounded(&downloaded, 5).unwrap(), b"hello");
        drop(downloaded);
        assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 1);

        runtime
            .block_on(store.put(&remote_path, Vec::from(b"world").into()))
            .unwrap();
        assert!(
            runtime
                .block_on(remote_matches(store, &remote_path, &expected_sha256, 5,))
                .is_err()
        );
    }

    #[test]
    fn generation_bytes_use_conditional_publication_and_clean_staging() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let runtime = remote_runtime().unwrap();
        let generation_path = ObjectPath::from("v1/generations/test.json");
        let generation_staging = ObjectPath::from("v1/staging/generation");
        assert_eq!(
            runtime
                .block_on(stage_and_publish_bytes(
                    Arc::clone(&store),
                    &generation_staging,
                    &generation_path,
                    b"generation".to_vec(),
                    &digest_bytes(b"generation"),
                    10,
                ))
                .unwrap(),
            RemotePublishDisposition::Transferred
        );
        assert!(matches!(
            runtime.block_on(store.head(&generation_staging)),
            Err(object_store::Error::NotFound { .. })
        ));
    }
}
