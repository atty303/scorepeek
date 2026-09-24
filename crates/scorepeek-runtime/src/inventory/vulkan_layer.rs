use std::env;
use std::ffi::OsStr;
use std::fmt::{self, Write as _};
use std::fs::{self, File};
use std::io::{Cursor, Read as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest as _, Sha256};

const MANIFEST_NAME: &str = "VkLayer_SCOREPEEK_capture.json";
const LIBRARY_NAME: &str = "libscorepeek_vulkan_capture.so";
const LIBRARY_RELATIVE_PATH: &str = "../../scorepeek/vulkan-layer/libscorepeek_vulkan_capture.so";
const LAYER_NAME: &str = "VK_LAYER_SCOREPEEK_capture";
const SUPPORTED_FORMAT_VERSIONS: &[&str] = &[
    "1.0.0", "1.0.1", "1.1.0", "1.1.1", "1.1.2", "1.2.0", "1.2.1",
];
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_LIBRARY_BYTES: u64 = 128 * 1024 * 1024;
const EMBEDDED_ARCHIVE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/scorepeek-vulkan-layer.zip"));

#[derive(Debug)]
pub(crate) enum VulkanLayerError {
    Location(&'static str),
    Payload(String),
    Filesystem {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for VulkanLayerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Location(message) => formatter.write_str(message),
            Self::Payload(message) => {
                write!(formatter, "embedded Vulkan layer is invalid: {message}")
            }
            Self::Filesystem {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "Vulkan layer {operation} failed for {}: {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for VulkanLayerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Filesystem { source, .. } => Some(source),
            Self::Location(_) | Self::Payload(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallOutcome {
    Installed,
    Updated,
    Unchanged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UninstallOutcome {
    Uninstalled,
    NotInstalled,
}

use scorepeek_frontend_api::{
    VulkanLayerReport as InstallationReport, VulkanLayerStatus as InstallationStatus,
};

#[derive(Clone, Debug)]
struct InstallationPaths {
    manifest: PathBuf,
    library: PathBuf,
    lock_directory: PathBuf,
}

impl InstallationPaths {
    fn discover() -> Result<Self, VulkanLayerError> {
        Self::from_environment(
            env::var_os("XDG_DATA_HOME").as_deref(),
            env::var_os("HOME").as_deref(),
        )
    }

    fn from_environment(
        xdg_data_home: Option<&OsStr>,
        home: Option<&OsStr>,
    ) -> Result<Self, VulkanLayerError> {
        let data_home = if let Some(value) = xdg_data_home {
            absolute_directory(PathBuf::from(value), "XDG_DATA_HOME")?
        } else {
            let home = home.ok_or(VulkanLayerError::Location(
                "HOME is required when XDG_DATA_HOME is unset",
            ))?;
            absolute_directory(PathBuf::from(home), "HOME")?.join(".local/share")
        };
        Ok(Self::from_data_home(&data_home))
    }

    fn from_data_home(data_home: &Path) -> Self {
        let lock_directory = data_home.join("scorepeek/vulkan-layer");
        Self {
            manifest: data_home
                .join("vulkan/explicit_layer.d")
                .join(MANIFEST_NAME),
            library: lock_directory.join(LIBRARY_NAME),
            lock_directory,
        }
    }
}

struct EmbeddedPayload {
    manifest: Vec<u8>,
    library: Vec<u8>,
}

impl EmbeddedPayload {
    fn load() -> Result<Self, VulkanLayerError> {
        let mut archive = zip::ZipArchive::new(Cursor::new(EMBEDDED_ARCHIVE))
            .map_err(|error| VulkanLayerError::Payload(error.to_string()))?;
        let mut names = archive.file_names().map(str::to_owned).collect::<Vec<_>>();
        names.sort_unstable();
        let mut expected = vec![MANIFEST_NAME.to_owned(), LIBRARY_NAME.to_owned()];
        expected.sort_unstable();
        if names != expected {
            return Err(VulkanLayerError::Payload(format!(
                "archive entries are {names:?}, expected {expected:?}"
            )));
        }

        let manifest = read_archive_entry(&mut archive, MANIFEST_NAME, MAX_MANIFEST_BYTES)?;
        validate_manifest(&manifest).map_err(VulkanLayerError::Payload)?;
        let library = read_archive_entry(&mut archive, LIBRARY_NAME, MAX_LIBRARY_BYTES)?;
        Ok(Self { manifest, library })
    }
}

pub(crate) fn install() -> Result<InstallOutcome, VulkanLayerError> {
    install_at(&InstallationPaths::discover()?)
}

fn install_at(paths: &InstallationPaths) -> Result<InstallOutcome, VulkanLayerError> {
    install_at_with_checkpoint(paths, || {})
}

fn install_at_with_checkpoint(
    paths: &InstallationPaths,
    after_library: impl FnOnce(),
) -> Result<InstallOutcome, VulkanLayerError> {
    let payload = EmbeddedPayload::load()?;
    ensure_directory(&paths.lock_directory)?;
    let _lock = acquire_lock(paths, LockKind::Exclusive)?;
    install_locked(paths, &payload, after_library)
}

fn install_locked(
    paths: &InstallationPaths,
    payload: &EmbeddedPayload,
    after_library: impl FnOnce(),
) -> Result<InstallOutcome, VulkanLayerError> {
    let previous = inspect_at(paths, payload);
    if previous.status == InstallationStatus::MatchesEmbedded {
        return Ok(InstallOutcome::Unchanged);
    }

    ensure_parent(&paths.library)?;
    ensure_parent(&paths.manifest)?;
    if !regular_file_matches(&paths.library, &payload.library) {
        atomic_replace(&paths.library, &payload.library)?;
    }
    after_library();
    if !regular_file_matches(&paths.manifest, &payload.manifest) {
        atomic_replace(&paths.manifest, &payload.manifest)?;
    }

    Ok(if previous.status == InstallationStatus::NotInstalled {
        InstallOutcome::Installed
    } else {
        InstallOutcome::Updated
    })
}

pub(crate) fn uninstall() -> Result<UninstallOutcome, VulkanLayerError> {
    uninstall_at(&InstallationPaths::discover()?)
}

fn uninstall_at(paths: &InstallationPaths) -> Result<UninstallOutcome, VulkanLayerError> {
    if !managed_path_exists(&paths.lock_directory)? {
        if !managed_path_exists(&paths.manifest)? {
            return Ok(UninstallOutcome::NotInstalled);
        }
        ensure_directory(&paths.lock_directory)?;
    }
    let _lock = acquire_lock(paths, LockKind::Exclusive)?;
    let manifest_removed = remove_managed_file(&paths.manifest)?;
    let library_removed = remove_managed_file(&paths.library)?;
    Ok(if manifest_removed || library_removed {
        UninstallOutcome::Uninstalled
    } else {
        UninstallOutcome::NotInstalled
    })
}

pub(crate) fn inspect() -> InstallationReport {
    let paths = match InstallationPaths::discover() {
        Ok(paths) => paths,
        Err(error) => {
            return invalid_report(
                InstallationPaths::from_data_home(Path::new("<unresolved-XDG-DATA-HOME>")),
                None,
                error.to_string(),
            );
        }
    };
    match EmbeddedPayload::load() {
        Ok(payload) => inspect_with_lock(&paths, &payload),
        Err(error) => invalid_report(paths, None, error.to_string()),
    }
}

fn inspect_with_lock(paths: &InstallationPaths, payload: &EmbeddedPayload) -> InstallationReport {
    match acquire_existing_lock(paths, LockKind::Shared) {
        Ok(Some(_lock)) => inspect_at(paths, payload),
        Ok(None) => {
            let report = inspect_at(paths, payload);
            match acquire_existing_lock(paths, LockKind::Shared) {
                Ok(Some(_lock)) => inspect_at(paths, payload),
                Ok(None) => report,
                Err(error) => invalid_report(paths.clone(), Some(payload), error.to_string()),
            }
        }
        Err(error) => invalid_report(paths.clone(), Some(payload), error.to_string()),
    }
}

fn inspect_at(paths: &InstallationPaths, payload: &EmbeddedPayload) -> InstallationReport {
    let embedded_manifest_sha256 = sha256_hex(&payload.manifest);
    let embedded_library_sha256 = sha256_hex(&payload.library);
    let manifest = read_installed_file(&paths.manifest, MAX_MANIFEST_BYTES);
    let library = read_installed_file(&paths.library, MAX_LIBRARY_BYTES);

    let (status, installed_manifest_sha256, installed_library_sha256, reason) =
        match (manifest, library) {
            (InstalledFile::Missing, InstalledFile::Missing) => {
                (InstallationStatus::NotInstalled, None, None, None)
            }
            (InstalledFile::Bytes(manifest), InstalledFile::Bytes(library)) => {
                let manifest_sha256 = sha256_hex(&manifest);
                let library_sha256 = sha256_hex(&library);
                match validate_manifest(&manifest) {
                    Ok(()) => {
                        let status = if manifest_sha256 == embedded_manifest_sha256
                            && library_sha256 == embedded_library_sha256
                        {
                            InstallationStatus::MatchesEmbedded
                        } else {
                            InstallationStatus::DifferentPayload
                        };
                        (status, Some(manifest_sha256), Some(library_sha256), None)
                    }
                    Err(reason) => (
                        InstallationStatus::Invalid,
                        Some(manifest_sha256),
                        Some(library_sha256),
                        Some(reason),
                    ),
                }
            }
            (manifest, library) => {
                let manifest_sha256 = manifest.digest();
                let library_sha256 = library.digest();
                let reason = [
                    manifest.invalid_reason("manifest"),
                    library.invalid_reason("library"),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("; ");
                (
                    InstallationStatus::Invalid,
                    manifest_sha256,
                    library_sha256,
                    Some(reason),
                )
            }
        };

    InstallationReport {
        status,
        manifest_path: paths.manifest.clone(),
        library_path: paths.library.clone(),
        embedded_manifest_sha256: Some(embedded_manifest_sha256),
        embedded_library_sha256: Some(embedded_library_sha256),
        installed_manifest_sha256,
        installed_library_sha256,
        reason,
    }
}

fn invalid_report(
    paths: InstallationPaths,
    payload: Option<&EmbeddedPayload>,
    reason: String,
) -> InstallationReport {
    InstallationReport {
        status: InstallationStatus::Invalid,
        manifest_path: paths.manifest,
        library_path: paths.library,
        embedded_manifest_sha256: payload.map(|payload| sha256_hex(&payload.manifest)),
        embedded_library_sha256: payload.map(|payload| sha256_hex(&payload.library)),
        installed_manifest_sha256: None,
        installed_library_sha256: None,
        reason: Some(reason),
    }
}

enum InstalledFile {
    Missing,
    Bytes(Vec<u8>),
    Invalid(String),
}

impl InstalledFile {
    fn digest(&self) -> Option<String> {
        match self {
            Self::Bytes(bytes) => Some(sha256_hex(bytes)),
            Self::Missing | Self::Invalid(_) => None,
        }
    }

    fn invalid_reason(&self, label: &str) -> Option<String> {
        match self {
            Self::Missing => Some(format!("{label} is missing")),
            Self::Invalid(reason) => Some(format!("{label} is invalid: {reason}")),
            Self::Bytes(_) => None,
        }
    }
}

fn read_installed_file(path: &Path, maximum_bytes: u64) -> InstalledFile {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return InstalledFile::Missing;
        }
        Err(error) => return InstalledFile::Invalid(error.to_string()),
    };
    if !metadata.file_type().is_file() {
        return InstalledFile::Invalid("path is not a regular file".to_owned());
    }
    if metadata.len() > maximum_bytes {
        return InstalledFile::Invalid(format!(
            "file size {} exceeds limit {maximum_bytes}",
            metadata.len()
        ));
    }
    match fs::read(path) {
        Ok(bytes) => InstalledFile::Bytes(bytes),
        Err(error) => InstalledFile::Invalid(error.to_string()),
    }
}

#[derive(Deserialize)]
struct LayerManifestDocument {
    file_format_version: String,
    layer: LayerManifestEntry,
}

#[derive(Deserialize)]
struct LayerManifestEntry {
    name: String,
    #[serde(rename = "type")]
    layer_type: String,
    library_path: String,
    api_version: String,
    implementation_version: String,
    #[serde(rename = "description")]
    _description: String,
}

fn validate_manifest(bytes: &[u8]) -> Result<(), String> {
    let document: LayerManifestDocument = serde_json::from_slice(bytes)
        .map_err(|error| format!("manifest JSON is invalid: {error}"))?;
    if !SUPPORTED_FORMAT_VERSIONS.contains(&document.file_format_version.as_str()) {
        return Err(format!(
            "manifest file_format_version {} is unsupported",
            document.file_format_version
        ));
    }
    if document.layer.name != LAYER_NAME {
        return Err(format!("manifest layer name must be {LAYER_NAME}"));
    }
    if document.layer.layer_type != "GLOBAL" {
        return Err("manifest layer type must be GLOBAL".to_owned());
    }
    if document.layer.library_path != LIBRARY_RELATIVE_PATH {
        return Err(format!(
            "manifest library_path must be {LIBRARY_RELATIVE_PATH}"
        ));
    }
    validate_dotted_version(&document.layer.api_version, "api_version")?;
    document
        .layer
        .implementation_version
        .parse::<u32>()
        .map_err(|_| "manifest implementation_version must be an unsigned integer".to_owned())?;
    Ok(())
}

fn validate_dotted_version(version: &str, field: &str) -> Result<(), String> {
    let components = version.split('.').collect::<Vec<_>>();
    if components.len() != 3
        || components
            .iter()
            .any(|component| component.parse::<u32>().is_err())
    {
        return Err(format!(
            "manifest {field} must be a major.minor.patch version"
        ));
    }
    Ok(())
}

fn read_archive_entry(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
    maximum_bytes: u64,
) -> Result<Vec<u8>, VulkanLayerError> {
    let mut entry = archive
        .by_name(name)
        .map_err(|error| VulkanLayerError::Payload(error.to_string()))?;
    if entry.compression() != zip::CompressionMethod::Deflated {
        return Err(VulkanLayerError::Payload(format!(
            "{name} is not deflate-compressed"
        )));
    }
    if entry.size() > maximum_bytes {
        return Err(VulkanLayerError::Payload(format!(
            "{name} exceeds the {maximum_bytes}-byte limit"
        )));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or_default());
    entry
        .read_to_end(&mut bytes)
        .map_err(|error| VulkanLayerError::Payload(error.to_string()))?;
    Ok(bytes)
}

fn absolute_directory(path: PathBuf, name: &'static str) -> Result<PathBuf, VulkanLayerError> {
    if path.as_os_str().is_empty() || !path.is_absolute() {
        return Err(VulkanLayerError::Location(match name {
            "XDG_DATA_HOME" => "XDG_DATA_HOME must be an absolute, non-empty path",
            "HOME" => "HOME must be an absolute, non-empty path",
            _ => "data directory must be an absolute, non-empty path",
        }));
    }
    Ok(path)
}

#[derive(Clone, Copy)]
enum LockKind {
    Shared,
    Exclusive,
}

fn acquire_lock(paths: &InstallationPaths, kind: LockKind) -> Result<File, VulkanLayerError> {
    let lock = File::open(&paths.lock_directory)
        .map_err(|source| filesystem_error("lock directory open", &paths.lock_directory, source))?;
    match kind {
        LockKind::Shared => lock.lock_shared(),
        LockKind::Exclusive => lock.lock(),
    }
    .map_err(|source| filesystem_error("lock acquisition", &paths.lock_directory, source))?;
    Ok(lock)
}

fn acquire_existing_lock(
    paths: &InstallationPaths,
    kind: LockKind,
) -> Result<Option<File>, VulkanLayerError> {
    match File::open(&paths.lock_directory) {
        Ok(lock) => {
            match kind {
                LockKind::Shared => lock.lock_shared(),
                LockKind::Exclusive => lock.lock(),
            }
            .map_err(|source| {
                filesystem_error("lock acquisition", &paths.lock_directory, source)
            })?;
            Ok(Some(lock))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(filesystem_error(
            "lock directory open",
            &paths.lock_directory,
            source,
        )),
    }
}

fn managed_path_exists(path: &Path) -> Result<bool, VulkanLayerError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(filesystem_error("path inspection", path, source)),
    }
}

fn ensure_directory(path: &Path) -> Result<(), VulkanLayerError> {
    fs::create_dir_all(path).map_err(|source| filesystem_error("directory creation", path, source))
}

fn ensure_parent(path: &Path) -> Result<(), VulkanLayerError> {
    let parent = path.parent().ok_or(VulkanLayerError::Location(
        "Vulkan layer path has no parent",
    ))?;
    ensure_directory(parent)
}

fn regular_file_matches(path: &Path, expected: &[u8]) -> bool {
    matches!(
        read_installed_file(path, u64::try_from(expected.len()).unwrap_or(u64::MAX)),
        InstalledFile::Bytes(bytes) if bytes == expected
    )
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), VulkanLayerError> {
    let parent = path.parent().ok_or(VulkanLayerError::Location(
        "Vulkan layer path has no parent",
    ))?;
    let mut staging = tempfile::Builder::new()
        .prefix(".scorepeek-vulkan-layer-")
        .tempfile_in(parent)
        .map_err(|source| filesystem_error("staging", parent, source))?;
    staging
        .as_file_mut()
        .write_all(bytes)
        .map_err(|source| filesystem_error("staging write", staging.path(), source))?;
    staging
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o644))
        .map_err(|source| filesystem_error("staging permissions", staging.path(), source))?;
    staging
        .as_file_mut()
        .sync_all()
        .map_err(|source| filesystem_error("staging sync", staging.path(), source))?;
    staging
        .persist(path)
        .map_err(|error| filesystem_error("atomic rename", path, error.error))?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| filesystem_error("directory sync", parent, source))
}

fn remove_managed_file(path: &Path) -> Result<bool, VulkanLayerError> {
    match fs::remove_file(path) {
        Ok(()) => {
            let parent = path.parent().ok_or(VulkanLayerError::Location(
                "Vulkan layer path has no parent",
            ))?;
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|source| filesystem_error("directory sync", parent, source))?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(filesystem_error("removal", path, source)),
    }
}

fn filesystem_error(
    operation: &'static str,
    path: &Path,
    source: std::io::Error,
) -> VulkanLayerError {
    VulkanLayerError::Filesystem {
        operation,
        path: path.to_owned(),
        source,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn data_home_maps_to_loader_and_scorepeek_locations() {
        let paths = InstallationPaths::from_environment(
            Some(OsStr::new("/data")),
            Some(OsStr::new("/ignored")),
        )
        .unwrap();
        assert_eq!(
            paths.manifest,
            Path::new("/data/vulkan/explicit_layer.d/VkLayer_SCOREPEEK_capture.json")
        );
        assert_eq!(
            paths.library,
            Path::new("/data/scorepeek/vulkan-layer/libscorepeek_vulkan_capture.so")
        );

        let fallback =
            InstallationPaths::from_environment(None, Some(OsStr::new("/home/player"))).unwrap();
        assert_eq!(
            fallback.manifest,
            Path::new(
                "/home/player/.local/share/vulkan/explicit_layer.d/VkLayer_SCOREPEEK_capture.json"
            )
        );
    }

    #[test]
    fn install_inspect_update_and_uninstall_follow_the_fixed_contract() {
        let root = tempfile::tempdir().unwrap();
        let paths = InstallationPaths::from_data_home(root.path());
        let payload = EmbeddedPayload::load().unwrap();

        assert_eq!(
            inspect_at(&paths, &payload).status,
            InstallationStatus::NotInstalled
        );
        assert_eq!(install_at(&paths).unwrap(), InstallOutcome::Installed);
        assert_eq!(
            inspect_at(&paths, &payload).status,
            InstallationStatus::MatchesEmbedded
        );
        assert_eq!(install_at(&paths).unwrap(), InstallOutcome::Unchanged);

        fs::write(&paths.library, b"different valid payload").unwrap();
        assert_eq!(
            inspect_at(&paths, &payload).status,
            InstallationStatus::DifferentPayload
        );
        assert_eq!(install_at(&paths).unwrap(), InstallOutcome::Updated);
        assert_eq!(fs::read(&paths.library).unwrap(), payload.library);

        let explicit_layer_directory = paths.manifest.parent().unwrap().to_owned();
        assert_eq!(uninstall_at(&paths).unwrap(), UninstallOutcome::Uninstalled);
        assert!(!paths.manifest.exists());
        assert!(!paths.library.exists());
        assert!(explicit_layer_directory.is_dir());
        assert_eq!(
            uninstall_at(&paths).unwrap(),
            UninstallOutcome::NotInstalled
        );
    }

    #[test]
    fn incomplete_or_malformed_installations_are_invalid() {
        let root = tempfile::tempdir().unwrap();
        let paths = InstallationPaths::from_data_home(root.path());
        let payload = EmbeddedPayload::load().unwrap();
        ensure_parent(&paths.manifest).unwrap();
        fs::write(&paths.manifest, &payload.manifest).unwrap();
        assert_eq!(
            inspect_at(&paths, &payload).status,
            InstallationStatus::Invalid
        );
        ensure_parent(&paths.library).unwrap();
        fs::write(&paths.library, &payload.library).unwrap();

        let mut valid_but_different: serde_json::Value =
            serde_json::from_slice(&payload.manifest).unwrap();
        valid_but_different["layer"]["description"] = "another build".into();
        let valid_but_different_bytes = serde_json::to_vec(&valid_but_different).unwrap();
        validate_manifest(&valid_but_different_bytes).unwrap();
        fs::write(&paths.manifest, valid_but_different_bytes).unwrap();
        assert_eq!(
            inspect_at(&paths, &payload).status,
            InstallationStatus::DifferentPayload
        );

        valid_but_different["layer"]
            .as_object_mut()
            .unwrap()
            .remove("api_version");
        fs::write(
            &paths.manifest,
            serde_json::to_vec(&valid_but_different).unwrap(),
        )
        .unwrap();
        assert_eq!(
            inspect_at(&paths, &payload).status,
            InstallationStatus::Invalid
        );

        fs::write(&paths.manifest, b"{}").unwrap();
        assert_eq!(
            inspect_at(&paths, &payload).status,
            InstallationStatus::Invalid
        );
    }

    #[test]
    fn uninstall_stops_before_library_when_manifest_removal_fails() {
        let root = tempfile::tempdir().unwrap();
        let paths = InstallationPaths::from_data_home(root.path());
        ensure_parent(&paths.manifest).unwrap();
        ensure_parent(&paths.library).unwrap();
        fs::create_dir(&paths.manifest).unwrap();
        fs::write(&paths.library, b"library").unwrap();

        assert!(uninstall_at(&paths).is_err());
        assert!(paths.library.is_file());
    }

    #[test]
    fn uninstall_waits_for_the_complete_install_transaction() {
        let root = tempfile::tempdir().unwrap();
        let paths = InstallationPaths::from_data_home(root.path());
        let install_paths = paths.clone();
        let uninstall_paths = paths.clone();
        let (library_ready_tx, library_ready_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let installer = std::thread::spawn(move || {
            install_at_with_checkpoint(&install_paths, || {
                library_ready_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
            })
        });
        library_ready_rx.recv().unwrap();

        let (attempting_tx, attempting_rx) = mpsc::channel();
        let (uninstalled_tx, uninstalled_rx) = mpsc::channel();
        let uninstaller = std::thread::spawn(move || {
            attempting_tx.send(()).unwrap();
            let result = uninstall_at(&uninstall_paths);
            uninstalled_tx.send(result).unwrap();
        });
        attempting_rx.recv().unwrap();
        assert!(
            uninstalled_rx
                .recv_timeout(Duration::from_millis(50))
                .is_err()
        );

        resume_tx.send(()).unwrap();
        assert_eq!(
            installer.join().unwrap().unwrap(),
            InstallOutcome::Installed
        );
        assert_eq!(
            uninstalled_rx.recv().unwrap().unwrap(),
            UninstallOutcome::Uninstalled
        );
        uninstaller.join().unwrap();
        assert!(!paths.manifest.exists());
        assert!(!paths.library.exists());
    }
}
