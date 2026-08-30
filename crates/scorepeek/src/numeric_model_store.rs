//! Create-only private storage and atomic activation for the registered numeric model.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use crate::recognition::{NumericModelContract, RegisteredNumericRuntime};

const STORE_MARKER: &str = ".scorepeek-numeric-model-store-v1";
const STORE_MARKER_BYTES: &[u8] = b"scorepeek-owned-numeric-model-store-v1\n";
const STAGING_PREFIX: &str = ".scorepeek-numeric-staging-";
const WRITER_LOCK: &str = ".writer.lock";
const ACTIVE_POINTER: &str = "active";
const MAX_OBJECTS: usize = 8;
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Debug)]
pub enum NumericModelStoreError {
    Location(&'static str),
    Io(std::io::Error),
    InvalidManifest,
    InvalidBundle(String),
    Capacity,
    Unavailable,
}

impl std::fmt::Display for NumericModelStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Location(message) => formatter.write_str(message),
            Self::Io(error) => write!(formatter, "numeric model store I/O failed: {error}"),
            Self::InvalidManifest => formatter.write_str("numeric model manifest is invalid"),
            Self::InvalidBundle(error) => {
                write!(formatter, "numeric model bundle is invalid: {error}")
            }
            Self::Capacity => formatter.write_str("numeric model store capacity exceeded"),
            Self::Unavailable => formatter.write_str("active numeric model is unavailable"),
        }
    }
}

impl std::error::Error for NumericModelStoreError {}

impl From<std::io::Error> for NumericModelStoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// # Errors
///
/// Returns an error when neither XDG data nor home resolves to an absolute store path.
pub fn default_numeric_model_store() -> Result<PathBuf, NumericModelStoreError> {
    default_numeric_model_store_from(env::var_os("XDG_DATA_HOME"), env::var_os("HOME"))
}

fn default_numeric_model_store_from(
    xdg_data_home: Option<impl AsRef<std::ffi::OsStr>>,
    home: Option<impl AsRef<std::ffi::OsStr>>,
) -> Result<PathBuf, NumericModelStoreError> {
    let base = if let Some(configured) = xdg_data_home {
        PathBuf::from(configured.as_ref())
    } else {
        PathBuf::from(
            home.ok_or(NumericModelStoreError::Location(
                "HOME is required when XDG_DATA_HOME is unset",
            ))?
            .as_ref(),
        )
        .join(".local/share")
    };
    if !base.is_absolute() {
        return Err(NumericModelStoreError::Location(
            "numeric model store base must be absolute",
        ));
    }
    Ok(base.join("scorepeek/numeric-models"))
}

/// Installs exactly the registered manifest/model bytes and atomically activates the object.
///
/// # Errors
///
/// Returns an error for an invalid registered bundle, unavailable storage, or capacity failure.
pub fn install_registered(
    source: &Path,
    registered_manifest: &[u8],
    manifest_sha256: &str,
) -> Result<PathBuf, NumericModelStoreError> {
    let store = default_numeric_model_store()?;
    install_registered_at(&store, source, registered_manifest, manifest_sha256)
}

fn install_registered_at(
    store: &Path,
    source: &Path,
    registered_manifest: &[u8],
    manifest_sha256: &str,
) -> Result<PathBuf, NumericModelStoreError> {
    if !store.is_absolute() || !source.is_absolute() {
        return Err(NumericModelStoreError::Location(
            "numeric model paths must be absolute",
        ));
    }
    if !valid_sha256(manifest_sha256) {
        return Err(NumericModelStoreError::InvalidManifest);
    }
    let contract: NumericModelContract = serde_json::from_slice(registered_manifest)
        .map_err(|_| NumericModelStoreError::InvalidManifest)?;
    let source_manifest = read_bounded(&source.join("manifest.json"), MAX_MANIFEST_BYTES)?;
    if source_manifest != registered_manifest {
        return Err(NumericModelStoreError::InvalidManifest);
    }
    create_private_directory(store)?;
    verify_marker(store)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(store.join(WRITER_LOCK))?;
    lock.lock()?;
    let objects = store.join("objects");
    create_private_directory(&objects)?;
    recover_staging(&objects)?;
    let target = objects.join(manifest_sha256);
    if target.exists() {
        verify_bundle(&target, registered_manifest, manifest_sha256)?;
        activate(store, manifest_sha256)?;
        return Ok(target);
    }
    ensure_capacity(
        &objects,
        contract.model_bytes + registered_manifest.len() as u64,
    )?;
    let staging = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempdir_in(&objects)?;
    write_durable(&staging.path().join("manifest.json"), registered_manifest)?;
    let model_source = source.join(&contract.model_filename);
    let input = File::open(model_source)?;
    let mut model = Vec::new();
    input
        .take(contract.model_bytes + 1)
        .read_to_end(&mut model)?;
    if model.len() as u64 != contract.model_bytes {
        return Err(NumericModelStoreError::InvalidBundle(
            "model size mismatched".to_owned(),
        ));
    }
    write_durable(&staging.path().join(&contract.model_filename), &model)?;
    verify_bundle(staging.path(), registered_manifest, manifest_sha256)?;
    sync_directory(staging.path())?;
    let staging_path = staging.keep();
    fs::rename(&staging_path, &target)?;
    sync_directory(&objects)?;
    activate(store, manifest_sha256)?;
    Ok(target)
}

/// Resolves and verifies the active registered object without mutation or fallback.
///
/// # Errors
///
/// Returns an error when the active pointer or its registered model cannot be verified.
pub fn active_registered(
    registered_manifest: &[u8],
    manifest_sha256: &str,
) -> Result<RegisteredNumericRuntime, NumericModelStoreError> {
    if !valid_sha256(manifest_sha256) {
        return Err(NumericModelStoreError::InvalidManifest);
    }
    let store = default_numeric_model_store()?;
    let pointer = read_bounded(&store.join(ACTIVE_POINTER), 65)?;
    if pointer != format!("{manifest_sha256}\n").as_bytes() {
        return Err(NumericModelStoreError::Unavailable);
    }
    let target = store.join("objects").join(manifest_sha256);
    RegisteredNumericRuntime::load(&target, registered_manifest, manifest_sha256)
        .map_err(|error| NumericModelStoreError::InvalidBundle(error.to_string()))
}

fn verify_bundle(
    path: &Path,
    manifest: &[u8],
    manifest_sha256: &str,
) -> Result<(), NumericModelStoreError> {
    RegisteredNumericRuntime::load(path, manifest, manifest_sha256)
        .map(|_| ())
        .map_err(|error| NumericModelStoreError::InvalidBundle(error.to_string()))
}

fn ensure_capacity(objects: &Path, incoming: u64) -> Result<(), NumericModelStoreError> {
    let mut count = 0;
    let mut bytes = incoming;
    for entry in fs::read_dir(objects)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGING_PREFIX)
        {
            continue;
        }
        let metadata = entry.metadata()?;
        if !metadata.is_dir() {
            return Err(NumericModelStoreError::Capacity);
        }
        count += 1;
        for file in fs::read_dir(entry.path())? {
            let metadata = file?.metadata()?;
            if !metadata.is_file() {
                return Err(NumericModelStoreError::Capacity);
            }
            bytes = bytes
                .checked_add(metadata.len())
                .ok_or(NumericModelStoreError::Capacity)?;
        }
    }
    if count >= MAX_OBJECTS || bytes > MAX_TOTAL_BYTES {
        return Err(NumericModelStoreError::Capacity);
    }
    Ok(())
}

fn recover_staging(objects: &Path) -> Result<(), NumericModelStoreError> {
    for entry in fs::read_dir(objects)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGING_PREFIX)
        {
            fs::remove_dir_all(entry.path())?;
        }
    }
    sync_directory(objects)?;
    Ok(())
}

fn activate(store: &Path, digest: &str) -> Result<(), NumericModelStoreError> {
    let mut staging = tempfile::Builder::new()
        .prefix(".active-staging-")
        .tempfile_in(store)?;
    staging
        .as_file_mut()
        .write_all(format!("{digest}\n").as_bytes())?;
    staging.as_file_mut().sync_all()?;
    staging
        .persist(store.join(ACTIVE_POINTER))
        .map_err(|error| error.error)?;
    sync_directory(store)?;
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<(), NumericModelStoreError> {
    if path.exists() {
        if !path.metadata()?.is_dir() {
            return Err(NumericModelStoreError::Location(
                "numeric model store is not a directory",
            ));
        }
        return Ok(());
    }
    let parent = path.parent().ok_or(NumericModelStoreError::Location(
        "numeric model store path has no parent",
    ))?;
    if !parent.exists() {
        create_private_directory(parent)?;
    }
    fs::DirBuilder::new().mode(0o700).create(path)?;
    sync_directory(parent)?;
    Ok(())
}

fn verify_marker(store: &Path) -> Result<(), NumericModelStoreError> {
    let marker = store.join(STORE_MARKER);
    if marker.exists() {
        if read_bounded(&marker, STORE_MARKER_BYTES.len() as u64)? != STORE_MARKER_BYTES {
            return Err(NumericModelStoreError::Location(
                "numeric model store marker is invalid",
            ));
        }
    } else {
        write_durable(&marker, STORE_MARKER_BYTES)?;
        sync_directory(store)?;
    }
    Ok(())
}

fn write_durable(path: &Path, bytes: &[u8]) -> Result<(), NumericModelStoreError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>, NumericModelStoreError> {
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(NumericModelStoreError::InvalidManifest);
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(maximum + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(NumericModelStoreError::InvalidManifest);
    }
    Ok(bytes)
}

fn sync_directory(path: &Path) -> Result<(), NumericModelStoreError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_store_uses_xdg_data_or_home() {
        assert_eq!(
            default_numeric_model_store_from(Some("/data"), Some("/home/test")).unwrap(),
            Path::new("/data/scorepeek/numeric-models")
        );
        assert_eq!(
            default_numeric_model_store_from(None::<&str>, Some("/home/test")).unwrap(),
            Path::new("/home/test/.local/share/scorepeek/numeric-models")
        );
        assert!(default_numeric_model_store_from(Some("relative"), Some("/home/test")).is_err());
    }
}
