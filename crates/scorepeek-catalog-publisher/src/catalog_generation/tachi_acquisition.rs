use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use super::acquisition::create_private_directory;
use super::adapter::{
    AdapterError, MAX_TACHI_CHART_BYTES, MAX_TACHI_SONG_BYTES, SourceRevision, TachiLiveAdapter,
};
use super::federation::SourceSnapshot;

const TACHI_REF_ENDPOINT: &str = "https://api.github.com/repos/zkldi/Tachi/git/ref/heads/main";
const TACHI_RAW_ROOT: &str = "https://raw.githubusercontent.com/zkldi/Tachi";
const MAX_REF_BYTES: usize = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: u32 = 0;
const MAX_CACHE_REVISIONS: usize = 8;
const MAX_CACHE_BYTES: u64 = 512 * 1024 * 1024;
const CACHE_STAGING_PREFIX: &str = ".scorepeek-tachi-staging-";

pub(super) struct AcquiredTachi {
    pub snapshot: SourceSnapshot,
    pub revision: String,
    pub content_sha256: String,
    pub record_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TachiResource {
    MainRef,
    Songs,
    SingleCharts,
    DoubleCharts,
}

impl TachiResource {
    const fn path(self) -> Option<&'static str> {
        match self {
            Self::MainRef => None,
            Self::Songs => Some("db/seeds/songs-iidx.json"),
            Self::SingleCharts => Some("db/seeds/charts-iidx-sp.json"),
            Self::DoubleCharts => Some("db/seeds/charts-iidx-dp.json"),
        }
    }

    const fn maximum(self) -> usize {
        match self {
            Self::MainRef => MAX_REF_BYTES,
            Self::Songs => MAX_TACHI_SONG_BYTES,
            Self::SingleCharts | Self::DoubleCharts => MAX_TACHI_CHART_BYTES,
        }
    }
}

impl fmt::Display for TachiResource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MainRef => formatter.write_str("main ref"),
            Self::Songs => formatter.write_str("songs seed"),
            Self::SingleCharts => formatter.write_str("SP charts seed"),
            Self::DoubleCharts => formatter.write_str("DP charts seed"),
        }
    }
}

#[derive(Debug)]
pub enum TachiAcquisitionError {
    UnexpectedStatus {
        resource: TachiResource,
        status: u16,
    },
    DeclaredBodyTooLarge {
        resource: TachiResource,
        declared: u64,
        maximum: usize,
    },
    BodyTooLarge {
        resource: TachiResource,
        actual: Option<usize>,
        maximum: usize,
    },
    Timeout(TachiResource),
    Transport(TachiResource, String),
    InvalidRevisionResponse(serde_json::Error),
    InvalidRevision(String),
    Adapter(AdapterError),
    CacheIo(io::Error),
    CacheConflict(PathBuf),
    CacheCapacityExceeded,
}

impl fmt::Display for TachiAcquisitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedStatus { resource, status } => {
                write!(formatter, "Tachi {resource} returned HTTP status {status}")
            }
            Self::DeclaredBodyTooLarge {
                resource,
                declared,
                maximum,
            } => write!(
                formatter,
                "Tachi {resource} declares {declared} bytes; maximum is {maximum}"
            ),
            Self::BodyTooLarge {
                resource,
                actual,
                maximum,
            } => match actual {
                Some(actual) => write!(
                    formatter,
                    "Tachi {resource} has {actual} bytes; maximum is {maximum}"
                ),
                None => write!(
                    formatter,
                    "Tachi {resource} exceeds the {maximum}-byte maximum"
                ),
            },
            Self::Timeout(resource) => write!(formatter, "Tachi {resource} acquisition timed out"),
            Self::Transport(resource, detail) => {
                write!(formatter, "Tachi {resource} acquisition failed: {detail}")
            }
            Self::InvalidRevisionResponse(error) => {
                write!(formatter, "invalid Tachi Git ref response: {error}")
            }
            Self::InvalidRevision(detail) => write!(formatter, "invalid Tachi revision: {detail}"),
            Self::Adapter(error) => write!(formatter, "Tachi seed validation failed: {error}"),
            Self::CacheIo(error) => write!(formatter, "Tachi cache write failed: {error}"),
            Self::CacheConflict(path) => write!(
                formatter,
                "Tachi cache content conflicts with its revision path {}",
                path.display()
            ),
            Self::CacheCapacityExceeded => formatter.write_str("Tachi cache capacity is exhausted"),
        }
    }
}

impl Error for TachiAcquisitionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidRevisionResponse(error) => Some(error),
            Self::Adapter(error) => Some(error),
            Self::CacheIo(error) => Some(error),
            Self::UnexpectedStatus { .. }
            | Self::DeclaredBodyTooLarge { .. }
            | Self::BodyTooLarge { .. }
            | Self::Timeout(_)
            | Self::Transport(_, _)
            | Self::InvalidRevision(_)
            | Self::CacheConflict(_)
            | Self::CacheCapacityExceeded => None,
        }
    }
}

impl From<AdapterError> for TachiAcquisitionError {
    fn from(error: AdapterError) -> Self {
        Self::Adapter(error)
    }
}

#[derive(Clone, Debug)]
pub(super) struct TachiHttpResponse {
    pub status: u16,
    pub content_length: Option<u64>,
    pub body: Vec<u8>,
}

pub(super) trait TachiTransport {
    fn get_ref(&self) -> Result<TachiHttpResponse, TachiAcquisitionError>;
    fn get_seed(
        &self,
        revision: &str,
        resource: TachiResource,
    ) -> Result<TachiHttpResponse, TachiAcquisitionError>;
}

pub(super) struct UreqTachiTransport {
    agent: ureq::Agent,
}

impl UreqTachiTransport {
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

    fn get(
        &self,
        url: &str,
        resource: TachiResource,
    ) -> Result<TachiHttpResponse, TachiAcquisitionError> {
        let mut response = self
            .agent
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .call()
            .map_err(|error| map_ureq_error(resource, error))?;
        let status = response.status().as_u16();
        let content_length = response.body().content_length();
        if status != 200 {
            return Ok(TachiHttpResponse {
                status,
                content_length,
                body: Vec::new(),
            });
        }
        let maximum = resource.maximum();
        if content_length.is_some_and(|length| length > maximum as u64) {
            return Err(TachiAcquisitionError::DeclaredBodyTooLarge {
                resource,
                declared: content_length.expect("checked content length"),
                maximum,
            });
        }
        let body = read_bounded_body(response.body_mut().as_reader(), resource)?;
        Ok(TachiHttpResponse {
            status,
            content_length,
            body,
        })
    }
}

impl TachiTransport for UreqTachiTransport {
    fn get_ref(&self) -> Result<TachiHttpResponse, TachiAcquisitionError> {
        self.get(TACHI_REF_ENDPOINT, TachiResource::MainRef)
    }

    fn get_seed(
        &self,
        revision: &str,
        resource: TachiResource,
    ) -> Result<TachiHttpResponse, TachiAcquisitionError> {
        let path = resource
            .path()
            .expect("only seed resources have raw repository paths");
        self.get(&format!("{TACHI_RAW_ROOT}/{revision}/{path}"), resource)
    }
}

pub(super) fn acquire_tachi(
    transport: &impl TachiTransport,
    cache_root: &Path,
) -> Result<AcquiredTachi, TachiAcquisitionError> {
    let reference = verified_body(transport.get_ref()?, TachiResource::MainRef)?;
    let revision = parse_revision(&reference)?;
    let songs = verified_body(
        transport.get_seed(&revision, TachiResource::Songs)?,
        TachiResource::Songs,
    )?;
    let single_charts = verified_body(
        transport.get_seed(&revision, TachiResource::SingleCharts)?,
        TachiResource::SingleCharts,
    )?;
    let double_charts = verified_body(
        transport.get_seed(&revision, TachiResource::DoubleCharts)?,
        TachiResource::DoubleCharts,
    )?;
    let snapshot = TachiLiveAdapter::parse(
        &songs,
        &single_charts,
        &double_charts,
        SourceRevision::git_commit(&revision)?,
    )?;
    let content_sha256 = snapshot.evidence().content_sha256().to_owned();
    let record_count = snapshot.evidence().record_count();
    cache_verified_bundle(
        cache_root,
        &revision,
        &content_sha256,
        [
            (TachiResource::Songs, songs.as_slice()),
            (TachiResource::SingleCharts, single_charts.as_slice()),
            (TachiResource::DoubleCharts, double_charts.as_slice()),
        ],
    )?;
    Ok(AcquiredTachi {
        snapshot,
        revision,
        content_sha256,
        record_count,
    })
}

fn verified_body(
    response: TachiHttpResponse,
    resource: TachiResource,
) -> Result<Vec<u8>, TachiAcquisitionError> {
    if response.status != 200 {
        return Err(TachiAcquisitionError::UnexpectedStatus {
            resource,
            status: response.status,
        });
    }
    let maximum = resource.maximum();
    if let Some(declared) = response.content_length
        && declared > maximum as u64
    {
        return Err(TachiAcquisitionError::DeclaredBodyTooLarge {
            resource,
            declared,
            maximum,
        });
    }
    if response.body.len() > maximum {
        return Err(TachiAcquisitionError::BodyTooLarge {
            resource,
            actual: Some(response.body.len()),
            maximum,
        });
    }
    Ok(response.body)
}

fn read_bounded_body(
    reader: impl io::Read,
    resource: TachiResource,
) -> Result<Vec<u8>, TachiAcquisitionError> {
    let maximum = resource.maximum();
    let mut body = Vec::new();
    reader
        .take((maximum + 1) as u64)
        .read_to_end(&mut body)
        .map_err(|error| map_body_io_error(resource, &error))?;
    if body.len() > maximum {
        return Err(TachiAcquisitionError::BodyTooLarge {
            resource,
            actual: Some(body.len()),
            maximum,
        });
    }
    Ok(body)
}

fn map_body_io_error(resource: TachiResource, error: &io::Error) -> TachiAcquisitionError {
    let wrapped_timeout = error
        .get_ref()
        .and_then(|source| source.downcast_ref::<ureq::Error>())
        .is_some_and(|error| matches!(error, ureq::Error::Timeout(_)));
    if error.kind() == io::ErrorKind::TimedOut || wrapped_timeout {
        TachiAcquisitionError::Timeout(resource)
    } else {
        TachiAcquisitionError::Transport(resource, "response body read failed".to_owned())
    }
}

fn map_ureq_error(resource: TachiResource, error: ureq::Error) -> TachiAcquisitionError {
    match error {
        ureq::Error::Timeout(_) => TachiAcquisitionError::Timeout(resource),
        ureq::Error::BodyExceedsLimit(_) => TachiAcquisitionError::BodyTooLarge {
            resource,
            actual: None,
            maximum: resource.maximum(),
        },
        other => TachiAcquisitionError::Transport(resource, other.to_string()),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitRefResponse {
    #[serde(rename = "node_id")]
    _node_id: serde::de::IgnoredAny,
    object: GitRefObject,
    #[serde(rename = "ref")]
    reference: String,
    #[serde(rename = "url")]
    _url: serde::de::IgnoredAny,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitRefObject {
    sha: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "url")]
    _url: serde::de::IgnoredAny,
}

fn parse_revision(bytes: &[u8]) -> Result<String, TachiAcquisitionError> {
    let response: GitRefResponse =
        serde_json::from_slice(bytes).map_err(TachiAcquisitionError::InvalidRevisionResponse)?;
    if response.reference != "refs/heads/main" || response.object.kind != "commit" {
        return Err(TachiAcquisitionError::InvalidRevision(
            "expected the main branch to reference a commit".to_owned(),
        ));
    }
    let revision = SourceRevision::git_commit(response.object.sha)?;
    match revision {
        SourceRevision::GitCommit(revision) => Ok(revision),
        SourceRevision::ContentSha256(_) => unreachable!("constructed a Git revision"),
    }
}

fn cache_verified_bundle<'a>(
    cache_root: &Path,
    revision: &str,
    content_sha256: &str,
    files: impl IntoIterator<Item = (TachiResource, &'a [u8])>,
) -> Result<(), TachiAcquisitionError> {
    let files: Vec<_> = files.into_iter().collect();
    let directory = cache_root.join("tachi");
    create_private_directory(&directory).map_err(TachiAcquisitionError::CacheIo)?;
    recover_cache_staging(&directory)?;
    let destination = directory.join(format!("{revision}-{content_sha256}"));
    if destination.exists() {
        verify_existing_bundle(&destination, &directory, &files)?;
        return Ok(());
    }
    let incoming_bytes = files
        .iter()
        .map(|(_, bytes)| bytes.len() as u64)
        .sum::<u64>();
    ensure_cache_capacity(&directory, incoming_bytes)?;

    let staging = tempfile::Builder::new()
        .prefix(CACHE_STAGING_PREFIX)
        .tempdir_in(&directory)
        .map_err(TachiAcquisitionError::CacheIo)?;
    for (resource, bytes) in &files {
        let path = staging.path().join(cache_filename(*resource));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(TachiAcquisitionError::CacheIo)?;
        file.write_all(bytes)
            .and_then(|()| file.flush())
            .and_then(|()| file.sync_all())
            .map_err(TachiAcquisitionError::CacheIo)?;
    }
    File::open(staging.path())
        .and_then(|directory| directory.sync_all())
        .map_err(TachiAcquisitionError::CacheIo)?;
    let staging_path = staging.keep();
    fs::rename(&staging_path, &destination).map_err(TachiAcquisitionError::CacheIo)?;
    File::open(&directory)
        .and_then(|directory| directory.sync_all())
        .map_err(TachiAcquisitionError::CacheIo)
}

fn recover_cache_staging(directory: &Path) -> Result<(), TachiAcquisitionError> {
    let mut removed = false;
    for entry in fs::read_dir(directory).map_err(TachiAcquisitionError::CacheIo)? {
        let entry = entry.map_err(TachiAcquisitionError::CacheIo)?;
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
            .map_err(TachiAcquisitionError::CacheIo)?;
        if !metadata.is_dir() {
            return Err(TachiAcquisitionError::CacheCapacityExceeded);
        }
        fs::remove_dir_all(entry.path()).map_err(TachiAcquisitionError::CacheIo)?;
        removed = true;
    }
    if removed {
        File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(TachiAcquisitionError::CacheIo)?;
    }
    Ok(())
}

fn ensure_cache_capacity(
    directory: &Path,
    incoming_bytes: u64,
) -> Result<(), TachiAcquisitionError> {
    let mut revisions = 0_usize;
    let mut total_bytes = 0_u64;
    for entry in fs::read_dir(directory).map_err(TachiAcquisitionError::CacheIo)? {
        let entry = entry.map_err(TachiAcquisitionError::CacheIo)?;
        let path = entry.path();
        let metadata = path.metadata().map_err(TachiAcquisitionError::CacheIo)?;
        if !metadata.is_dir() || !valid_generation_name(&entry.file_name().to_string_lossy()) {
            return Err(TachiAcquisitionError::CacheCapacityExceeded);
        }
        revisions = revisions.saturating_add(1);
        total_bytes = total_bytes.saturating_add(bundle_size(&path)?);
        if revisions >= MAX_CACHE_REVISIONS || total_bytes > MAX_CACHE_BYTES {
            return Err(TachiAcquisitionError::CacheCapacityExceeded);
        }
    }
    if incoming_bytes > MAX_CACHE_BYTES
        || total_bytes.saturating_add(incoming_bytes) > MAX_CACHE_BYTES
    {
        return Err(TachiAcquisitionError::CacheCapacityExceeded);
    }
    Ok(())
}

fn bundle_size(path: &Path) -> Result<u64, TachiAcquisitionError> {
    let mut seen = 0_u8;
    let mut total = 0_u64;
    for entry in fs::read_dir(path).map_err(TachiAcquisitionError::CacheIo)? {
        let entry = entry.map_err(TachiAcquisitionError::CacheIo)?;
        let resource = resource_for_filename(&entry.file_name().to_string_lossy())
            .ok_or(TachiAcquisitionError::CacheCapacityExceeded)?;
        let bit = resource_bit(resource);
        if seen & bit != 0 {
            return Err(TachiAcquisitionError::CacheCapacityExceeded);
        }
        seen |= bit;
        let metadata = entry
            .path()
            .metadata()
            .map_err(TachiAcquisitionError::CacheIo)?;
        if !metadata.is_file() || metadata.len() > resource.maximum() as u64 {
            return Err(TachiAcquisitionError::CacheCapacityExceeded);
        }
        total = total.saturating_add(metadata.len());
    }
    if seen != 0b111 {
        return Err(TachiAcquisitionError::CacheCapacityExceeded);
    }
    Ok(total)
}

fn verify_existing_bundle(
    destination: &Path,
    directory: &Path,
    expected: &[(TachiResource, &[u8])],
) -> Result<(), TachiAcquisitionError> {
    if !destination
        .metadata()
        .map_err(TachiAcquisitionError::CacheIo)?
        .is_dir()
    {
        return Err(TachiAcquisitionError::CacheConflict(destination.to_owned()));
    }
    for (resource, bytes) in expected {
        let path = destination.join(cache_filename(*resource));
        let mut file = File::open(&path).map_err(TachiAcquisitionError::CacheIo)?;
        let metadata = file.metadata().map_err(TachiAcquisitionError::CacheIo)?;
        if metadata.len() != bytes.len() as u64 || metadata.len() > resource.maximum() as u64 {
            return Err(TachiAcquisitionError::CacheConflict(path));
        }
        let mut existing = Vec::with_capacity(bytes.len());
        std::io::Read::by_ref(&mut file)
            .take((resource.maximum() + 1) as u64)
            .read_to_end(&mut existing)
            .map_err(TachiAcquisitionError::CacheIo)?;
        if existing.as_slice() != *bytes {
            return Err(TachiAcquisitionError::CacheConflict(path));
        }
        file.sync_all().map_err(TachiAcquisitionError::CacheIo)?;
    }
    if bundle_size(destination)?
        != expected
            .iter()
            .map(|(_, bytes)| bytes.len() as u64)
            .sum::<u64>()
    {
        return Err(TachiAcquisitionError::CacheConflict(destination.to_owned()));
    }
    File::open(destination)
        .and_then(|directory| directory.sync_all())
        .and_then(|()| File::open(directory).and_then(|directory| directory.sync_all()))
        .map_err(TachiAcquisitionError::CacheIo)
}

fn valid_generation_name(name: &str) -> bool {
    name.len() == 105
        && name.as_bytes().get(40) == Some(&b'-')
        && name.bytes().enumerate().all(|(index, byte)| {
            index == 40 || byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
        })
}

const fn cache_filename(resource: TachiResource) -> &'static str {
    match resource {
        TachiResource::Songs => "songs-iidx.json",
        TachiResource::SingleCharts => "charts-iidx-sp.json",
        TachiResource::DoubleCharts => "charts-iidx-dp.json",
        TachiResource::MainRef => unreachable!(),
    }
}

fn resource_for_filename(name: &str) -> Option<TachiResource> {
    match name {
        "songs-iidx.json" => Some(TachiResource::Songs),
        "charts-iidx-sp.json" => Some(TachiResource::SingleCharts),
        "charts-iidx-dp.json" => Some(TachiResource::DoubleCharts),
        _ => None,
    }
}

const fn resource_bit(resource: TachiResource) -> u8 {
    match resource {
        TachiResource::Songs => 0b001,
        TachiResource::SingleCharts => 0b010,
        TachiResource::DoubleCharts => 0b100,
        TachiResource::MainRef => 0,
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn revision_response_is_strict_and_requires_a_commit_ref() {
        let revision = "0123456789abcdef0123456789abcdef01234567";
        let bytes = format!(
            r#"{{"ref":"refs/heads/main","node_id":"node","url":"url","object":{{"sha":"{revision}","type":"commit","url":"url"}}}}"#
        );
        assert_eq!(parse_revision(bytes.as_bytes()).unwrap(), revision);

        let drifted = bytes.replace("\"node_id\":\"node\"", "\"extra\":true");
        assert!(matches!(
            parse_revision(drifted.as_bytes()),
            Err(TachiAcquisitionError::InvalidRevisionResponse(_))
        ));
    }

    #[test]
    fn bounded_reader_accepts_each_limit_and_preserves_timeouts() {
        let resource = TachiResource::MainRef;
        assert_eq!(
            read_bounded_body(io::Cursor::new(vec![0; resource.maximum()]), resource)
                .unwrap()
                .len(),
            resource.maximum()
        );
        assert!(matches!(
            read_bounded_body(
                io::Cursor::new(vec![0; resource.maximum() + 1]),
                resource
            ),
            Err(TachiAcquisitionError::BodyTooLarge { actual: Some(actual), .. })
                if actual == resource.maximum() + 1
        ));
        assert!(matches!(
            read_bounded_body(TimeoutReader, resource),
            Err(TachiAcquisitionError::Timeout(TachiResource::MainRef))
        ));
    }

    #[test]
    fn cache_reuses_identical_content_and_rejects_conflicts() {
        let root = TempDir::new().unwrap();
        let revision = "0".repeat(40);
        let digest = "1".repeat(64);
        let files = [
            (TachiResource::Songs, b"songs".as_slice()),
            (TachiResource::SingleCharts, b"sp".as_slice()),
            (TachiResource::DoubleCharts, b"dp".as_slice()),
        ];
        cache_verified_bundle(root.path(), &revision, &digest, files).unwrap();
        cache_verified_bundle(root.path(), &revision, &digest, files).unwrap();

        let conflicting = [
            (TachiResource::Songs, b"changed".as_slice()),
            (TachiResource::SingleCharts, b"sp".as_slice()),
            (TachiResource::DoubleCharts, b"dp".as_slice()),
        ];
        assert!(matches!(
            cache_verified_bundle(root.path(), &revision, &digest, conflicting),
            Err(TachiAcquisitionError::CacheConflict(_))
        ));
    }

    #[test]
    fn cache_retry_removes_only_owned_staging_directory() {
        let root = TempDir::new().unwrap();
        let directory = root.path().join("tachi");
        create_private_directory(&directory).unwrap();
        let stale = directory.join(format!("{CACHE_STAGING_PREFIX}interrupted"));
        fs::create_dir(&stale).unwrap();
        fs::write(stale.join("partial"), b"partial").unwrap();
        let files = [
            (TachiResource::Songs, b"songs".as_slice()),
            (TachiResource::SingleCharts, b"sp".as_slice()),
            (TachiResource::DoubleCharts, b"dp".as_slice()),
        ];
        cache_verified_bundle(root.path(), &"2".repeat(40), &"3".repeat(64), files).unwrap();
        assert!(!stale.exists());
    }

    struct TimeoutReader;

    impl io::Read for TimeoutReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::TimedOut.into())
        }
    }
}
