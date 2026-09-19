use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::adapter::{AdapterError, DqnLiveAdapter, MAX_SOURCE_BYTES, SourceRevision};
use super::federation::SourceSnapshot;

const DQN_ENDPOINT: &str = "https://dqn.github.io/iidxapi/infinitas/music.json";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: u32 = 0;
const MAX_CACHE_REVISIONS: usize = 64;
const MAX_CACHE_BYTES: u64 = MAX_CACHE_REVISIONS as u64 * MAX_SOURCE_BYTES as u64;
const CACHE_STAGING_PREFIX: &str = ".scorepeek-dqn-staging-";

pub(super) struct AcquiredDqn {
    pub snapshot: SourceSnapshot,
    pub content_sha256: String,
    pub record_count: usize,
}

#[derive(Debug)]
pub enum DqnAcquisitionError {
    UnexpectedStatus(u16),
    DeclaredBodyTooLarge {
        declared: u64,
        maximum: usize,
    },
    BodyTooLarge {
        actual: Option<usize>,
        maximum: usize,
    },
    Timeout,
    Transport(String),
    Adapter(AdapterError),
    CacheIo(io::Error),
    CacheConflict(PathBuf),
    CacheCapacityExceeded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CacheWritePoint {
    FilePersisted,
}

impl fmt::Display for DqnAcquisitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedStatus(status) => {
                write!(formatter, "dqn acquisition returned HTTP status {status}")
            }
            Self::DeclaredBodyTooLarge { declared, maximum } => write!(
                formatter,
                "dqn response declares {declared} bytes; maximum is {maximum}"
            ),
            Self::BodyTooLarge { actual, maximum } => match actual {
                Some(actual) => write!(
                    formatter,
                    "dqn response has {actual} bytes; maximum is {maximum}"
                ),
                None => write!(formatter, "dqn response exceeds the {maximum}-byte maximum"),
            },
            Self::Timeout => formatter.write_str("dqn acquisition timed out"),
            Self::Transport(detail) => write!(formatter, "dqn acquisition failed: {detail}"),
            Self::Adapter(error) => write!(formatter, "dqn response validation failed: {error}"),
            Self::CacheIo(error) => write!(formatter, "dqn cache write failed: {error}"),
            Self::CacheConflict(path) => write!(
                formatter,
                "dqn cache content conflicts with its digest path {}",
                path.display()
            ),
            Self::CacheCapacityExceeded => formatter.write_str("dqn cache capacity is exhausted"),
        }
    }
}

impl Error for DqnAcquisitionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Adapter(error) => Some(error),
            Self::CacheIo(error) => Some(error),
            Self::UnexpectedStatus(_)
            | Self::DeclaredBodyTooLarge { .. }
            | Self::BodyTooLarge { .. }
            | Self::Timeout
            | Self::Transport(_)
            | Self::CacheConflict(_)
            | Self::CacheCapacityExceeded => None,
        }
    }
}

impl From<AdapterError> for DqnAcquisitionError {
    fn from(error: AdapterError) -> Self {
        Self::Adapter(error)
    }
}

#[derive(Debug)]
pub(super) struct DqnHttpResponse {
    pub status: u16,
    pub content_length: Option<u64>,
    pub body: Vec<u8>,
}

pub(super) trait DqnTransport {
    fn get(&self) -> Result<DqnHttpResponse, DqnAcquisitionError>;
}

pub(super) struct UreqDqnTransport {
    agent: ureq::Agent,
}

impl UreqDqnTransport {
    pub fn new() -> Self {
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

impl DqnTransport for UreqDqnTransport {
    fn get(&self) -> Result<DqnHttpResponse, DqnAcquisitionError> {
        let mut response = self
            .agent
            .get(DQN_ENDPOINT)
            .call()
            .map_err(map_ureq_error)?;
        let status = response.status().as_u16();
        let content_length = response.body().content_length();
        if status != 200 {
            return Ok(DqnHttpResponse {
                status,
                content_length,
                body: Vec::new(),
            });
        }
        if content_length.is_some_and(|length| length > MAX_SOURCE_BYTES as u64) {
            return Err(DqnAcquisitionError::DeclaredBodyTooLarge {
                declared: content_length.expect("checked content length"),
                maximum: MAX_SOURCE_BYTES,
            });
        }
        let body = read_bounded_body(response.body_mut().as_reader())?;
        Ok(DqnHttpResponse {
            status,
            content_length,
            body,
        })
    }
}

fn read_bounded_body(reader: impl io::Read) -> Result<Vec<u8>, DqnAcquisitionError> {
    let mut body = Vec::new();
    reader
        .take((MAX_SOURCE_BYTES + 1) as u64)
        .read_to_end(&mut body)
        .map_err(|error| map_body_io_error(&error))?;
    if body.len() > MAX_SOURCE_BYTES {
        return Err(DqnAcquisitionError::BodyTooLarge {
            actual: Some(body.len()),
            maximum: MAX_SOURCE_BYTES,
        });
    }
    Ok(body)
}

fn map_body_io_error(error: &io::Error) -> DqnAcquisitionError {
    let wrapped_timeout = error
        .get_ref()
        .and_then(|source| source.downcast_ref::<ureq::Error>())
        .is_some_and(|error| matches!(error, ureq::Error::Timeout(_)));
    if error.kind() == io::ErrorKind::TimedOut || wrapped_timeout {
        DqnAcquisitionError::Timeout
    } else {
        DqnAcquisitionError::Transport("response body read failed".to_owned())
    }
}

fn map_ureq_error(error: ureq::Error) -> DqnAcquisitionError {
    match error {
        ureq::Error::Timeout(_) => DqnAcquisitionError::Timeout,
        ureq::Error::BodyExceedsLimit(_) => DqnAcquisitionError::BodyTooLarge {
            actual: None,
            maximum: MAX_SOURCE_BYTES,
        },
        other => DqnAcquisitionError::Transport(other.to_string()),
    }
}

pub(super) fn acquire_dqn(
    transport: &impl DqnTransport,
    cache_root: &Path,
) -> Result<AcquiredDqn, DqnAcquisitionError> {
    let response = transport.get()?;
    if response.status != 200 {
        return Err(DqnAcquisitionError::UnexpectedStatus(response.status));
    }
    if let Some(declared) = response.content_length
        && declared > MAX_SOURCE_BYTES as u64
    {
        return Err(DqnAcquisitionError::DeclaredBodyTooLarge {
            declared,
            maximum: MAX_SOURCE_BYTES,
        });
    }
    if response.body.len() > MAX_SOURCE_BYTES {
        return Err(DqnAcquisitionError::BodyTooLarge {
            actual: Some(response.body.len()),
            maximum: MAX_SOURCE_BYTES,
        });
    }

    let snapshot =
        DqnLiveAdapter::parse(&response.body, SourceRevision::from_content(&response.body))?;
    let content_sha256 = snapshot.evidence().content_sha256().to_owned();
    let record_count = snapshot.evidence().record_count();
    cache_verified_bytes(cache_root, &content_sha256, &response.body)?;
    Ok(AcquiredDqn {
        snapshot,
        content_sha256,
        record_count,
    })
}

fn cache_verified_bytes(
    cache_root: &Path,
    content_sha256: &str,
    bytes: &[u8],
) -> Result<(), DqnAcquisitionError> {
    cache_verified_bytes_with(cache_root, content_sha256, bytes, |_| Ok(()))
}

fn cache_verified_bytes_with(
    cache_root: &Path,
    content_sha256: &str,
    bytes: &[u8],
    mut checkpoint: impl FnMut(CacheWritePoint) -> io::Result<()>,
) -> Result<(), DqnAcquisitionError> {
    let directory = cache_root.join("dqn");
    create_private_directory(&directory).map_err(DqnAcquisitionError::CacheIo)?;
    recover_cache_staging(&directory)?;
    let destination = directory.join(format!("{content_sha256}.json"));
    if destination.exists() {
        verify_existing_cache(&destination, &directory, bytes)?;
        return Ok(());
    }
    ensure_cache_capacity(&directory, bytes.len())?;

    let mut temporary = tempfile::Builder::new()
        .prefix(CACHE_STAGING_PREFIX)
        .tempfile_in(&directory)
        .map_err(DqnAcquisitionError::CacheIo)?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.flush())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(DqnAcquisitionError::CacheIo)?;
    temporary
        .persist_noclobber(&destination)
        .map_err(|error| DqnAcquisitionError::CacheIo(error.error))?;
    checkpoint(CacheWritePoint::FilePersisted).map_err(DqnAcquisitionError::CacheIo)?;
    File::open(&directory)
        .and_then(|directory| directory.sync_all())
        .map_err(DqnAcquisitionError::CacheIo)
}

fn recover_cache_staging(directory: &Path) -> Result<(), DqnAcquisitionError> {
    let mut removed = false;
    for entry in fs::read_dir(directory).map_err(DqnAcquisitionError::CacheIo)? {
        let entry = entry.map_err(DqnAcquisitionError::CacheIo)?;
        let is_staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(CACHE_STAGING_PREFIX));
        if !is_staging {
            continue;
        }
        let metadata = entry
            .path()
            .symlink_metadata()
            .map_err(DqnAcquisitionError::CacheIo)?;
        if !metadata.is_file() {
            return Err(DqnAcquisitionError::CacheCapacityExceeded);
        }
        fs::remove_file(entry.path()).map_err(DqnAcquisitionError::CacheIo)?;
        removed = true;
    }
    if removed {
        File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(DqnAcquisitionError::CacheIo)?;
    }
    Ok(())
}

fn ensure_cache_capacity(
    directory: &Path,
    incoming_bytes: usize,
) -> Result<(), DqnAcquisitionError> {
    let mut revisions = 0_usize;
    let mut total_bytes = 0_u64;
    for entry in fs::read_dir(directory).map_err(DqnAcquisitionError::CacheIo)? {
        let entry = entry.map_err(DqnAcquisitionError::CacheIo)?;
        let metadata = entry
            .path()
            .metadata()
            .map_err(DqnAcquisitionError::CacheIo)?;
        if !metadata.is_file()
            || entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("json")
            || metadata.len() > MAX_SOURCE_BYTES as u64
        {
            return Err(DqnAcquisitionError::CacheCapacityExceeded);
        }
        revisions = revisions.saturating_add(1);
        total_bytes = total_bytes.saturating_add(metadata.len());
        if revisions >= MAX_CACHE_REVISIONS || total_bytes > MAX_CACHE_BYTES {
            return Err(DqnAcquisitionError::CacheCapacityExceeded);
        }
    }
    if total_bytes.saturating_add(incoming_bytes as u64) > MAX_CACHE_BYTES {
        return Err(DqnAcquisitionError::CacheCapacityExceeded);
    }
    Ok(())
}

fn verify_existing_cache(
    destination: &Path,
    directory: &Path,
    expected: &[u8],
) -> Result<(), DqnAcquisitionError> {
    let mut file = File::open(destination).map_err(DqnAcquisitionError::CacheIo)?;
    let length = file.metadata().map_err(DqnAcquisitionError::CacheIo)?.len();
    if length != expected.len() as u64 || length > MAX_SOURCE_BYTES as u64 {
        return Err(DqnAcquisitionError::CacheConflict(destination.to_owned()));
    }
    let mut existing = Vec::with_capacity(expected.len());
    std::io::Read::by_ref(&mut file)
        .take((MAX_SOURCE_BYTES + 1) as u64)
        .read_to_end(&mut existing)
        .map_err(DqnAcquisitionError::CacheIo)?;
    if existing != expected {
        return Err(DqnAcquisitionError::CacheConflict(destination.to_owned()));
    }
    file.sync_all().map_err(DqnAcquisitionError::CacheIo)?;
    File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(DqnAcquisitionError::CacheIo)
}

pub(super) fn create_private_directory(path: &Path) -> io::Result<()> {
    let mut missing = Vec::new();
    let mut candidate = path;
    loop {
        match candidate.metadata() {
            Ok(metadata) if metadata.is_dir() => break,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "cache ancestor is not a directory",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(candidate.to_owned());
                candidate = candidate.parent().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "directory has no existing ancestor",
                    )
                })?;
            }
            Err(error) => return Err(error),
        }
    }
    for directory in missing.into_iter().rev() {
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        sync_directory_and_parent(&directory)?;
    }
    sync_directory_and_parent(path)
}

fn sync_directory_and_parent(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::os::unix::fs::symlink;

    use tempfile::TempDir;

    #[test]
    fn managed_cache_directory_follows_operator_symlinks() {
        let root = TempDir::new().unwrap();
        let target = root.path().join("target");
        let alias = root.path().join("alias");
        fs::create_dir(&target).unwrap();
        symlink(&target, &alias).unwrap();
        create_private_directory(&alias).unwrap();
    }

    use super::*;

    #[test]
    fn bounded_reader_accepts_the_limit_and_rejects_one_more_byte() {
        let exact = read_bounded_body(io::Cursor::new(vec![0; MAX_SOURCE_BYTES])).unwrap();
        assert_eq!(exact.len(), MAX_SOURCE_BYTES);

        let error = read_bounded_body(io::Cursor::new(vec![0; MAX_SOURCE_BYTES + 1])).unwrap_err();
        assert!(matches!(
            error,
            DqnAcquisitionError::BodyTooLarge {
                actual: Some(actual),
                maximum: MAX_SOURCE_BYTES
            } if actual == MAX_SOURCE_BYTES + 1
        ));
    }

    #[test]
    fn bounded_reader_preserves_timeout_classification() {
        let error = read_bounded_body(TimeoutReader).unwrap_err();
        assert!(matches!(error, DqnAcquisitionError::Timeout));
    }

    #[test]
    fn existing_cache_read_is_bounded_and_conflicting_large_file_is_rejected() {
        let root = TempDir::new().unwrap();
        let digest = "0".repeat(64);
        let directory = root.path().join("dqn");
        create_private_directory(&directory).unwrap();
        let destination = directory.join(format!("{digest}.json"));
        File::create(&destination)
            .unwrap()
            .set_len((MAX_SOURCE_BYTES + 1) as u64)
            .unwrap();

        let error = cache_verified_bytes(root.path(), &digest, b"valid").unwrap_err();
        assert!(matches!(error, DqnAcquisitionError::CacheConflict(path) if path == destination));
    }

    #[test]
    fn retry_repairs_durability_after_persist_checkpoint_failure() {
        let root = TempDir::new().unwrap();
        let digest = "1".repeat(64);
        let bytes = b"verified bytes";
        let error = cache_verified_bytes_with(root.path(), &digest, bytes, |point| {
            assert_eq!(point, CacheWritePoint::FilePersisted);
            Err(io::Error::other("synthetic fsync interruption"))
        })
        .unwrap_err();
        assert!(matches!(error, DqnAcquisitionError::CacheIo(_)));

        cache_verified_bytes(root.path(), &digest, bytes).unwrap();
        assert_eq!(
            fs::read(root.path().join("dqn").join(format!("{digest}.json"))).unwrap(),
            bytes
        );
    }

    #[test]
    fn cache_generation_limit_rejects_only_new_content() {
        let root = TempDir::new().unwrap();
        let directory = root.path().join("dqn");
        create_private_directory(&directory).unwrap();
        let existing_digest = "0".repeat(64);
        let existing_bytes = b"verified bytes";
        fs::write(
            directory.join(format!("{existing_digest}.json")),
            existing_bytes,
        )
        .unwrap();
        for index in 1..MAX_CACHE_REVISIONS {
            fs::write(directory.join(format!("{index:064x}.json")), []).unwrap();
        }

        cache_verified_bytes(root.path(), &existing_digest, existing_bytes).unwrap();
        let error = cache_verified_bytes(root.path(), &"f".repeat(64), b"new bytes").unwrap_err();
        assert!(matches!(error, DqnAcquisitionError::CacheCapacityExceeded));
    }

    #[test]
    fn cache_retry_removes_owned_incomplete_staging_file() {
        let root = TempDir::new().unwrap();
        let directory = root.path().join("dqn");
        create_private_directory(&directory).unwrap();
        let stale = directory.join(format!("{CACHE_STAGING_PREFIX}interrupted"));
        fs::write(&stale, b"partial source bytes").unwrap();
        let digest = "a".repeat(64);
        let bytes = b"verified bytes";

        cache_verified_bytes(root.path(), &digest, bytes).unwrap();

        assert!(!stale.exists());
        assert_eq!(
            fs::read(directory.join(format!("{digest}.json"))).unwrap(),
            bytes
        );
    }

    struct TimeoutReader;

    impl io::Read for TimeoutReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::TimedOut.into())
        }
    }
}
