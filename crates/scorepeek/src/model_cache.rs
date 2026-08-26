//! Global acquisition and cache publication for the registered live OCR model.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest as _, Sha256};

use crate::recognition::{
    LIVE_MODEL_BUNDLE_MANIFEST_SHA256, RegisteredLiveModelFile, registered_live_model_files,
    verify_registered_live_model_bundle,
};

const STORE_MARKER: &str = ".scorepeek-onnx-bundle-store-v1";
const STORE_MARKER_BYTES: &[u8] = b"scorepeek-owned-onnx-bundle-store-v1\n";
const STAGING_PREFIX: &str = ".scorepeek-staging-";
const STAGING_MARKER: &str = ".scorepeek-onnx-bundle-staging-v1";
const STAGING_MARKER_BYTES: &[u8] = b"scorepeek-owned-onnx-bundle-staging-v1\n";
const WRITER_LOCK: &str = ".writer.lock";
const MAX_BUNDLE_FILE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_BUNDLE_COUNT: usize = 8;
const MAX_BUNDLE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_BUNDLE_OBJECT_BYTES: u64 = 192 * 1024 * 1024;
const MAX_REDIRECTS: u32 = 10;
const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);

/// A user-visible transition of the synchronous model cache operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelCacheEvent {
    DownloadStarted,
    DownloadCompleted,
}

#[derive(Debug)]
pub enum ModelCacheError {
    Location(&'static str),
    Io(io::Error),
    Registration(String),
    Transport {
        filename: String,
        detail: String,
    },
    Timeout {
        filename: String,
    },
    Http {
        filename: String,
        status: u16,
    },
    DeclaredSize {
        filename: String,
        declared: u64,
        expected: u64,
    },
    Size {
        filename: String,
        actual: usize,
        expected: u64,
    },
    Digest {
        filename: String,
    },
    InvalidBundle(String),
}

impl std::fmt::Display for ModelCacheError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Location(message) => formatter.write_str(message),
            Self::Io(error) => write!(formatter, "model cache I/O failed: {error}"),
            Self::Registration(error) => write!(formatter, "registered model is invalid: {error}"),
            Self::Transport { filename, detail } => {
                write!(
                    formatter,
                    "model download transport failed for {filename}: {detail}"
                )
            }
            Self::Timeout { filename } => {
                write!(formatter, "model download timed out for {filename}")
            }
            Self::Http { filename, status } => {
                write!(formatter, "model download HTTP {status} for {filename}")
            }
            Self::DeclaredSize {
                filename,
                declared,
                expected,
            } => write!(
                formatter,
                "model download declared {declared} bytes for {filename}, expected {expected}"
            ),
            Self::Size {
                filename,
                actual,
                expected,
            } => write!(
                formatter,
                "model download contained {actual} bytes for {filename}, expected {expected}"
            ),
            Self::Digest { filename } => {
                write!(formatter, "model download digest mismatch for {filename}")
            }
            Self::InvalidBundle(error) => write!(formatter, "model bundle is invalid: {error}"),
        }
    }
}

impl std::error::Error for ModelCacheError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ModelCacheError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Debug)]
struct ModelHttpResponse {
    status: u16,
    content_length: Option<u64>,
    body: Vec<u8>,
}

trait ModelTransport {
    fn get(&self, file: &RegisteredLiveModelFile) -> Result<ModelHttpResponse, ModelCacheError>;
}

struct UreqModelTransport {
    agent: ureq::Agent,
}

impl UreqModelTransport {
    fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .https_only(true)
            .max_redirects(MAX_REDIRECTS)
            .timeout_global(Some(REQUEST_TIMEOUT))
            .user_agent(format!(
                "scorepeek/{} (+https://github.com/atty303/scorepeek)",
                env!("CARGO_PKG_VERSION")
            ))
            .build();
        Self {
            agent: config.new_agent(),
        }
    }
}

impl ModelTransport for UreqModelTransport {
    fn get(&self, file: &RegisteredLiveModelFile) -> Result<ModelHttpResponse, ModelCacheError> {
        let mut response = self.agent.get(&file.source_url).call().map_err(|error| {
            if matches!(error, ureq::Error::Timeout(_)) {
                ModelCacheError::Timeout {
                    filename: file.filename.clone(),
                }
            } else {
                ModelCacheError::Transport {
                    filename: file.filename.clone(),
                    detail: error.to_string(),
                }
            }
        })?;
        let status = response.status().as_u16();
        let content_length = response.body().content_length();
        if content_length.is_some_and(|declared| declared != file.bytes) {
            return Err(ModelCacheError::DeclaredSize {
                filename: file.filename.clone(),
                declared: content_length.expect("checked content length"),
                expected: file.bytes,
            });
        }
        let mut body = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(file.bytes + 1)
            .read_to_end(&mut body)
            .map_err(|error| {
                if error.kind() == io::ErrorKind::TimedOut {
                    ModelCacheError::Timeout {
                        filename: file.filename.clone(),
                    }
                } else {
                    ModelCacheError::Transport {
                        filename: file.filename.clone(),
                        detail: "response body read failed".to_owned(),
                    }
                }
            })?;
        Ok(ModelHttpResponse {
            status,
            content_length,
            body,
        })
    }
}

/// Resolves the normal XDG cache store used by both Rust and Python model tooling.
///
/// # Errors
/// Returns an error for a relative XDG cache value, missing HOME fallback, or relative HOME.
pub fn default_model_store() -> Result<PathBuf, ModelCacheError> {
    default_model_store_from(env::var_os("XDG_CACHE_HOME"), env::var_os("HOME"))
}

fn default_model_store_from(
    xdg_cache_home: Option<impl AsRef<std::ffi::OsStr>>,
    home: Option<impl AsRef<std::ffi::OsStr>>,
) -> Result<PathBuf, ModelCacheError> {
    let base = if let Some(configured) = xdg_cache_home {
        PathBuf::from(configured.as_ref())
    } else {
        let home = home.ok_or(ModelCacheError::Location(
            "HOME is required when XDG_CACHE_HOME is unset",
        ))?;
        PathBuf::from(home.as_ref()).join(".cache")
    };
    if !base.is_absolute() {
        return Err(ModelCacheError::Location(
            "model cache base must be absolute",
        ));
    }
    Ok(base.join("scorepeek/models"))
}

/// Ensures the registered live model exists, or verifies the explicit development override.
///
/// # Errors
/// Returns before command dispatch when location, download, verification, locking, or publication
/// fails. An existing completed cache is returned without network access or mutation.
pub fn ensure_small_model(
    override_bundle: Option<&Path>,
    observer: impl FnMut(ModelCacheEvent),
) -> Result<PathBuf, ModelCacheError> {
    if let Some(bundle) = override_bundle {
        validate_absolute_directory(bundle)?;
        verify_registered_live_model_bundle(bundle)
            .map_err(|error| ModelCacheError::InvalidBundle(error.to_string()))?;
        return Ok(bundle.to_path_buf());
    }
    let store = default_model_store()?;
    ensure_small_model_with(&store, &UreqModelTransport::new(), observer)
}

fn ensure_small_model_with(
    store: &Path,
    transport: &impl ModelTransport,
    observer: impl FnMut(ModelCacheEvent),
) -> Result<PathBuf, ModelCacheError> {
    let files = registered_live_model_files()
        .map_err(|error| ModelCacheError::Registration(error.to_string()))?;
    ensure_model_with(
        store,
        LIVE_MODEL_BUNDLE_MANIFEST_SHA256,
        &files,
        transport,
        |path| {
            verify_registered_live_model_bundle(path)
                .map_err(|error| ModelCacheError::InvalidBundle(error.to_string()))
        },
        observer,
    )
}

fn ensure_model_with(
    store: &Path,
    manifest_sha256: &str,
    files: &[RegisteredLiveModelFile],
    transport: &impl ModelTransport,
    verify_bundle: impl Fn(&Path) -> Result<(), ModelCacheError>,
    observer: impl FnMut(ModelCacheEvent),
) -> Result<PathBuf, ModelCacheError> {
    ensure_model_with_publish_hook(
        store,
        manifest_sha256,
        files,
        transport,
        verify_bundle,
        |_| Ok(()),
        observer,
    )
}

fn ensure_model_with_publish_hook(
    store: &Path,
    manifest_sha256: &str,
    files: &[RegisteredLiveModelFile],
    transport: &impl ModelTransport,
    verify_bundle: impl Fn(&Path) -> Result<(), ModelCacheError>,
    before_publish: impl Fn(&Path) -> Result<(), ModelCacheError>,
    mut observer: impl FnMut(ModelCacheEvent),
) -> Result<PathBuf, ModelCacheError> {
    if !store.is_absolute() {
        return Err(ModelCacheError::Location("model store must be absolute"));
    }
    let target = store.join("bundles").join(manifest_sha256);
    if completed_target_exists(&target)? {
        validate_absolute_directory(&target)?;
        return Ok(target);
    }
    create_private_directory(store)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(store.join(WRITER_LOCK))?;
    lock.lock()?;
    let bundles = ensure_bundle_store(store)?;
    recover_owned_staging(&bundles)?;
    if completed_target_exists(&target)? {
        validate_absolute_directory(&target)?;
        return Ok(target);
    }
    ensure_store_capacity(&bundles, files)?;

    observer(ModelCacheEvent::DownloadStarted);
    let staging = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempdir_in(&bundles)?;
    write_durable_file(&staging.path().join(STAGING_MARKER), STAGING_MARKER_BYTES)?;
    sync_directory(staging.path())?;
    sync_directory(&bundles)?;
    for file in files {
        let response = transport.get(file)?;
        verify_response(file, &response)?;
        write_durable_file(&staging.path().join(&file.filename), &response.body)?;
    }
    verify_bundle(staging.path())?;
    sync_directory(staging.path())?;
    before_publish(staging.path())?;
    let staging_path = staging.keep();
    let publication = (|| -> Result<(), ModelCacheError> {
        fs::rename(&staging_path, &target)?;
        sync_directory(&bundles)?;
        fs::remove_file(target.join(STAGING_MARKER))?;
        sync_directory(&target)?;
        sync_directory(&bundles)?;
        Ok(())
    })();
    if let Err(error) = publication {
        if target.exists() {
            fs::remove_dir_all(&target)?;
            sync_directory(&bundles)?;
        } else if staging_path.exists() {
            fs::remove_dir_all(&staging_path)?;
            sync_directory(&bundles)?;
        }
        return Err(error);
    }
    observer(ModelCacheEvent::DownloadCompleted);
    Ok(target)
}

fn verify_response(
    file: &RegisteredLiveModelFile,
    response: &ModelHttpResponse,
) -> Result<(), ModelCacheError> {
    if response.status != 200 {
        return Err(ModelCacheError::Http {
            filename: file.filename.clone(),
            status: response.status,
        });
    }
    if response
        .content_length
        .is_some_and(|declared| declared != file.bytes)
    {
        return Err(ModelCacheError::DeclaredSize {
            filename: file.filename.clone(),
            declared: response.content_length.expect("checked content length"),
            expected: file.bytes,
        });
    }
    if response.body.len() as u64 != file.bytes {
        return Err(ModelCacheError::Size {
            filename: file.filename.clone(),
            actual: response.body.len(),
            expected: file.bytes,
        });
    }
    if sha256_hex(&response.body) != file.sha256 {
        return Err(ModelCacheError::Digest {
            filename: file.filename.clone(),
        });
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn create_private_directory(path: &Path) -> Result<(), ModelCacheError> {
    let mut missing = Vec::new();
    let mut current = path;
    while !current.exists() {
        missing.push(current);
        current = current.parent().ok_or(ModelCacheError::Location(
            "model cache directory has no existing parent",
        ))?;
    }
    validate_directory(current, "model cache directory is invalid")?;
    for directory in missing.into_iter().rev() {
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        validate_directory(directory, "model cache directory is invalid")?;
        sync_directory(directory)?;
        sync_directory(directory.parent().ok_or(ModelCacheError::Location(
            "model cache directory has no parent",
        ))?)?;
    }
    Ok(())
}

fn ensure_store_capacity(
    bundles: &Path,
    incoming_files: &[RegisteredLiveModelFile],
) -> Result<(), ModelCacheError> {
    let mut count = 0_usize;
    let mut total = 0_u64;
    for entry in bundles.read_dir()? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == STORE_MARKER {
            continue;
        }
        let path = entry.path();
        let valid_digest = name.len() == 64
            && name
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if name.starts_with(STAGING_PREFIX) || !valid_digest {
            return Err(ModelCacheError::Location(
                "model bundle store contains an unmanaged entry",
            ));
        }
        validate_directory(&path, "model bundle store contains an invalid object")?;
        let mut object_bytes = 0_u64;
        let mut file_count = 0_usize;
        for file in path.read_dir()? {
            let file = file?;
            let metadata = file.path().symlink_metadata()?;
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || metadata.len() == 0
                || metadata.len() > MAX_BUNDLE_FILE_BYTES
            {
                return Err(ModelCacheError::Location(
                    "model bundle store contains an invalid file",
                ));
            }
            file_count += 1;
            object_bytes =
                object_bytes
                    .checked_add(metadata.len())
                    .ok_or(ModelCacheError::Location(
                        "model bundle store size overflow",
                    ))?;
            if file_count > 8 || object_bytes > MAX_BUNDLE_OBJECT_BYTES {
                return Err(ModelCacheError::Location(
                    "model bundle object exceeds its capacity",
                ));
            }
        }
        if file_count == 0 {
            return Err(ModelCacheError::Location("model bundle object is empty"));
        }
        count += 1;
        total = total
            .checked_add(object_bytes)
            .ok_or(ModelCacheError::Location(
                "model bundle store size overflow",
            ))?;
        if count > MAX_BUNDLE_COUNT || total > MAX_BUNDLE_BYTES {
            return Err(ModelCacheError::Location(
                "model bundle store exceeds its capacity",
            ));
        }
    }
    let incoming = incoming_files.iter().try_fold(0_u64, |total, file| {
        total
            .checked_add(file.bytes)
            .ok_or(ModelCacheError::Location("registered model size overflow"))
    })?;
    if incoming > MAX_BUNDLE_OBJECT_BYTES
        || count >= MAX_BUNDLE_COUNT
        || total
            .checked_add(incoming)
            .is_none_or(|combined| combined > MAX_BUNDLE_BYTES)
    {
        return Err(ModelCacheError::Location(
            "model bundle store is at capacity",
        ));
    }
    Ok(())
}

fn ensure_bundle_store(store: &Path) -> Result<PathBuf, ModelCacheError> {
    let bundles = store.join("bundles");
    if bundles.exists() {
        validate_absolute_directory(&bundles)?;
        let marker = bundles.join(STORE_MARKER);
        if marker.exists() {
            if fs::read(&marker)? != STORE_MARKER_BYTES {
                return Err(ModelCacheError::Location(
                    "model bundle store marker is invalid",
                ));
            }
            return Ok(bundles);
        }
        if bundles.read_dir()?.next().is_some() {
            return Err(ModelCacheError::Location(
                "existing model bundle store is not scorepeek-owned",
            ));
        }
    } else {
        create_private_directory(&bundles)?;
    }
    write_durable_file(&bundles.join(STORE_MARKER), STORE_MARKER_BYTES)?;
    sync_directory(&bundles)?;
    sync_directory(store)?;
    Ok(bundles)
}

fn recover_owned_staging(bundles: &Path) -> Result<(), ModelCacheError> {
    let mut changed = false;
    for entry in bundles.read_dir()? {
        let entry = entry?;
        let path = entry.path();
        let metadata = path.symlink_metadata()?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let recoverable_name = name.starts_with(STAGING_PREFIX)
            || (name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit()));
        if recoverable_name
            && fs::read(path.join(STAGING_MARKER)).ok().as_deref() == Some(STAGING_MARKER_BYTES)
        {
            fs::remove_dir_all(path)?;
            changed = true;
        }
    }
    if changed {
        sync_directory(bundles)?;
    }
    Ok(())
}

fn completed_target_exists(target: &Path) -> Result<bool, ModelCacheError> {
    if !target.exists() {
        return Ok(false);
    }
    validate_absolute_directory(target)?;
    Ok(!target.join(STAGING_MARKER).exists())
}

fn write_durable_file(path: &Path, bytes: &[u8]) -> Result<(), ModelCacheError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), ModelCacheError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn validate_absolute_directory(path: &Path) -> Result<(), ModelCacheError> {
    if !path.is_absolute() {
        return Err(ModelCacheError::Location(
            "model bundle must be an absolute directory",
        ));
    }
    validate_directory(path, "model bundle must be an absolute directory")
}

fn validate_directory(path: &Path, message: &'static str) -> Result<(), ModelCacheError> {
    let metadata = path.symlink_metadata()?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ModelCacheError::Location(message));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    use super::*;

    #[derive(Clone)]
    struct FakeTransport {
        bodies: Arc<BTreeMap<String, Vec<u8>>>,
        requests: Arc<AtomicUsize>,
    }

    impl ModelTransport for FakeTransport {
        fn get(
            &self,
            file: &RegisteredLiveModelFile,
        ) -> Result<ModelHttpResponse, ModelCacheError> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            let body = self
                .bodies
                .get(&file.filename)
                .expect("registered fake body")
                .clone();
            Ok(ModelHttpResponse {
                status: 200,
                content_length: Some(body.len() as u64),
                body,
            })
        }
    }

    struct FixedTransport(Result<ModelHttpResponse, ModelCacheError>);

    impl ModelTransport for FixedTransport {
        fn get(&self, _: &RegisteredLiveModelFile) -> Result<ModelHttpResponse, ModelCacheError> {
            match &self.0 {
                Ok(response) => Ok(response.clone()),
                Err(ModelCacheError::Timeout { filename }) => Err(ModelCacheError::Timeout {
                    filename: filename.clone(),
                }),
                Err(error) => panic!("unsupported fixed error: {error}"),
            }
        }
    }

    fn fixture() -> (Vec<RegisteredLiveModelFile>, BTreeMap<String, Vec<u8>>) {
        let bodies = BTreeMap::from([
            ("inference.onnx".to_owned(), b"onnx".to_vec()),
            ("inference.json".to_owned(), b"json".to_vec()),
            ("inference.yml".to_owned(), b"yml".to_vec()),
        ]);
        let files = bodies
            .iter()
            .map(|(filename, body)| RegisteredLiveModelFile {
                filename: filename.clone(),
                source_url: format!("https://example.invalid/{filename}"),
                sha256: sha256_hex(body),
                bytes: body.len() as u64,
            })
            .collect();
        (files, bodies)
    }

    fn verify_fixture(path: &Path) -> Result<(), ModelCacheError> {
        let (_, bodies) = fixture();
        for (filename, expected) in bodies {
            if fs::read(path.join(filename))? != expected {
                return Err(ModelCacheError::InvalidBundle("fixture changed".to_owned()));
            }
        }
        Ok(())
    }

    #[test]
    fn xdg_cache_path_and_home_fallback_are_explicit() {
        assert_eq!(
            default_model_store_from(Some("/cache"), Some("/home/test")).unwrap(),
            Path::new("/cache/scorepeek/models")
        );
        assert_eq!(
            default_model_store_from(None::<&str>, Some("/home/test")).unwrap(),
            Path::new("/home/test/.cache/scorepeek/models")
        );
        assert!(default_model_store_from(Some("relative"), Some("/home/test")).is_err());
        assert!(default_model_store_from(None::<&str>, None::<&str>).is_err());
    }

    #[test]
    fn existing_completed_bundle_skips_transport() {
        let temporary = tempfile::tempdir().unwrap();
        let store = temporary.path().join("models");
        let (files, bodies) = fixture();
        let requests = Arc::new(AtomicUsize::new(0));
        let transport = FakeTransport {
            bodies: Arc::new(bodies),
            requests: Arc::clone(&requests),
        };
        let first = ensure_model_with(
            &store,
            &"1".repeat(64),
            &files,
            &transport,
            verify_fixture,
            |_| {},
        )
        .unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), 3);
        let second = ensure_model_with(
            &store,
            &"1".repeat(64),
            &files,
            &transport,
            verify_fixture,
            |_| {},
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(requests.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn failed_verification_never_publishes_completed_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let store = temporary.path().join("models");
        let (files, mut bodies) = fixture();
        bodies.get_mut("inference.json").unwrap().push(0);
        let transport = FakeTransport {
            bodies: Arc::new(bodies),
            requests: Arc::new(AtomicUsize::new(0)),
        };
        assert!(
            ensure_model_with(
                &store,
                &"2".repeat(64),
                &files,
                &transport,
                verify_fixture,
                |_| {}
            )
            .is_err()
        );
        assert!(!store.join("bundles").join("2".repeat(64)).exists());
        assert_eq!(
            store.join("bundles").read_dir().unwrap().count(),
            1,
            "only the store marker remains"
        );
    }

    #[test]
    fn transport_http_size_and_digest_failures_never_publish() {
        let body = b"model".to_vec();
        let file = RegisteredLiveModelFile {
            filename: "inference.onnx".to_owned(),
            source_url: "https://example.invalid/inference.onnx".to_owned(),
            sha256: sha256_hex(&body),
            bytes: body.len() as u64,
        };
        let cases = [
            FixedTransport(Err(ModelCacheError::Timeout {
                filename: file.filename.clone(),
            })),
            FixedTransport(Ok(ModelHttpResponse {
                status: 503,
                content_length: Some(body.len() as u64),
                body: body.clone(),
            })),
            FixedTransport(Ok(ModelHttpResponse {
                status: 200,
                content_length: Some((body.len() - 1) as u64),
                body: body[..body.len() - 1].to_vec(),
            })),
            FixedTransport(Ok(ModelHttpResponse {
                status: 200,
                content_length: Some((body.len() + 1) as u64),
                body: [body.as_slice(), b"x"].concat(),
            })),
            FixedTransport(Ok(ModelHttpResponse {
                status: 200,
                content_length: Some(body.len() as u64),
                body: b"other".to_vec(),
            })),
        ];
        for (index, transport) in cases.iter().enumerate() {
            let temporary = tempfile::tempdir().unwrap();
            let store = temporary.path().join("models");
            let digest = format!("{index:064x}");
            assert!(
                ensure_model_with(
                    &store,
                    &digest,
                    std::slice::from_ref(&file),
                    transport,
                    |_| Ok(()),
                    |_| {}
                )
                .is_err()
            );
            assert!(!store.join("bundles").join(digest).exists());
        }
    }

    #[test]
    fn publication_interruption_cleans_staging_and_target() {
        let temporary = tempfile::tempdir().unwrap();
        let store = temporary.path().join("models");
        let (files, bodies) = fixture();
        let transport = FakeTransport {
            bodies: Arc::new(bodies),
            requests: Arc::new(AtomicUsize::new(0)),
        };
        let digest = "4".repeat(64);
        assert!(
            ensure_model_with_publish_hook(
                &store,
                &digest,
                &files,
                &transport,
                verify_fixture,
                |_| Err(ModelCacheError::Io(io::Error::other(
                    "publication interrupted"
                ))),
                |_| {},
            )
            .is_err()
        );
        assert!(!store.join("bundles").join(digest).exists());
        assert_eq!(store.join("bundles").read_dir().unwrap().count(), 1);
    }

    #[test]
    fn recovery_removes_only_marked_owned_staging() {
        let temporary = tempfile::tempdir().unwrap();
        let bundles = temporary.path().join("bundles");
        fs::create_dir(&bundles).unwrap();
        let owned = bundles.join(format!("{STAGING_PREFIX}owned"));
        let unmarked = bundles.join(format!("{STAGING_PREFIX}operator"));
        fs::create_dir(&owned).unwrap();
        fs::write(owned.join(STAGING_MARKER), STAGING_MARKER_BYTES).unwrap();
        fs::create_dir(&unmarked).unwrap();
        recover_owned_staging(&bundles).unwrap();
        assert!(!owned.exists());
        assert!(unmarked.exists());
    }

    #[test]
    fn concurrent_ensure_publishes_once() {
        let temporary = tempfile::tempdir().unwrap();
        let store = temporary.path().join("models");
        let requests = Arc::new(AtomicUsize::new(0));
        let (files, bodies) = fixture();
        let transport = FakeTransport {
            bodies: Arc::new(bodies),
            requests: Arc::clone(&requests),
        };
        let barrier = Arc::new(Barrier::new(3));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let store = store.clone();
                let transport = transport.clone();
                let files = files.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    ensure_model_with(
                        &store,
                        &"3".repeat(64),
                        &files,
                        &transport,
                        verify_fixture,
                        |_| {},
                    )
                })
            })
            .collect();
        barrier.wait();
        for thread in threads {
            thread.join().unwrap().unwrap();
        }
        assert_eq!(requests.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn capacity_rejects_new_content_before_transport_and_reuses_existing_content() {
        let temporary = tempfile::tempdir().unwrap();
        let store = temporary.path().join("models");
        create_private_directory(&store).unwrap();
        let bundles = ensure_bundle_store(&store).unwrap();
        for index in 0..MAX_BUNDLE_COUNT {
            let object = bundles.join(format!("{index:064x}"));
            fs::create_dir(&object).unwrap();
            fs::write(object.join("inference.onnx"), b"x").unwrap();
        }
        let (files, bodies) = fixture();
        let requests = Arc::new(AtomicUsize::new(0));
        let transport = FakeTransport {
            bodies: Arc::new(bodies),
            requests: Arc::clone(&requests),
        };
        let existing = format!("{:064x}", 0);
        assert_eq!(
            ensure_model_with(
                &store,
                &existing,
                &files,
                &transport,
                verify_fixture,
                |_| {}
            )
            .unwrap(),
            bundles.join(existing)
        );
        assert!(
            ensure_model_with(
                &store,
                &"f".repeat(64),
                &files,
                &transport,
                verify_fixture,
                |_| {}
            )
            .is_err()
        );
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }
}
