use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::SourceSnapshot;
use super::acquisition::create_private_directory;
use super::adapter::{AdapterError, SourceRevision};
use super::textage_adapter::{MAX_TEXTAGE_FILE_BYTES, TextageLiveAdapter, textage_bundle_digest};

const TEXTAGE_ROOT: &str = "https://textage.cc/score";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: u32 = 0;
const MAX_CACHE_REVISIONS: usize = 64;
const MAX_CACHE_BYTES: u64 = 64 * 1024 * 1024;
const CACHE_STAGING_PREFIX: &str = ".scorepeek-textage-staging-";

#[derive(Clone, Debug)]
pub(super) struct AcquiredTextage {
    pub snapshot: SourceSnapshot,
    pub content_sha256: String,
    pub record_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TextageResource {
    Title,
    Availability,
    Chart,
}

impl TextageResource {
    const fn endpoint(self) -> &'static str {
        match self {
            Self::Title => "https://textage.cc/score/titletbl.js",
            Self::Availability => "https://textage.cc/score/actbl.js",
            Self::Chart => "https://textage.cc/score/datatbl.js",
        }
    }

    const fn filename(self) -> &'static str {
        match self {
            Self::Title => "titletbl.js",
            Self::Availability => "actbl.js",
            Self::Chart => "datatbl.js",
        }
    }

    const fn bit(self) -> u8 {
        match self {
            Self::Title => 0b001,
            Self::Availability => 0b010,
            Self::Chart => 0b100,
        }
    }
}

impl fmt::Display for TextageResource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Title => "title table",
            Self::Availability => "availability table",
            Self::Chart => "chart table",
        })
    }
}

#[derive(Debug)]
pub enum TextageAcquisitionError {
    UnexpectedStatus {
        resource: TextageResource,
        status: u16,
    },
    DeclaredBodyTooLarge {
        resource: TextageResource,
        declared: u64,
        maximum: usize,
    },
    BodyTooLarge {
        resource: TextageResource,
        actual: Option<usize>,
        maximum: usize,
    },
    Timeout(TextageResource),
    Transport(TextageResource, String),
    Adapter(AdapterError),
    CacheIo(io::Error),
    CacheConflict(PathBuf),
    CacheCapacityExceeded,
}

impl fmt::Display for TextageAcquisitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedStatus { resource, status } => {
                write!(
                    formatter,
                    "Textage {resource} returned HTTP status {status}"
                )
            }
            Self::DeclaredBodyTooLarge {
                resource,
                declared,
                maximum,
            } => write!(
                formatter,
                "Textage {resource} declares {declared} bytes; maximum is {maximum}"
            ),
            Self::BodyTooLarge {
                resource,
                actual,
                maximum,
            } => match actual {
                Some(actual) => write!(
                    formatter,
                    "Textage {resource} has {actual} bytes; maximum is {maximum}"
                ),
                None => write!(
                    formatter,
                    "Textage {resource} exceeds the {maximum}-byte maximum"
                ),
            },
            Self::Timeout(resource) => write!(formatter, "Textage {resource} timed out"),
            Self::Transport(resource, detail) => {
                write!(formatter, "Textage {resource} transport failed: {detail}")
            }
            Self::Adapter(error) => write!(formatter, "Textage validation failed: {error}"),
            Self::CacheIo(error) => write!(formatter, "Textage cache write failed: {error}"),
            Self::CacheConflict(path) => write!(
                formatter,
                "Textage cache content conflicts with its digest path {}",
                path.display()
            ),
            Self::CacheCapacityExceeded => {
                formatter.write_str("Textage cache capacity is exhausted")
            }
        }
    }
}

impl Error for TextageAcquisitionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Adapter(error) => Some(error),
            Self::CacheIo(error) => Some(error),
            Self::UnexpectedStatus { .. }
            | Self::DeclaredBodyTooLarge { .. }
            | Self::BodyTooLarge { .. }
            | Self::Timeout(_)
            | Self::Transport(_, _)
            | Self::CacheConflict(_)
            | Self::CacheCapacityExceeded => None,
        }
    }
}

impl From<AdapterError> for TextageAcquisitionError {
    fn from(error: AdapterError) -> Self {
        Self::Adapter(error)
    }
}

#[derive(Clone, Debug)]
pub(super) struct TextageHttpResponse {
    pub status: u16,
    pub content_length: Option<u64>,
    pub body: Vec<u8>,
}

pub(super) trait TextageTransport {
    fn get(
        &self,
        resource: TextageResource,
    ) -> Result<TextageHttpResponse, TextageAcquisitionError>;
}

pub(super) struct UreqTextageTransport {
    agent: ureq::Agent,
}

impl UreqTextageTransport {
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

impl TextageTransport for UreqTextageTransport {
    fn get(
        &self,
        resource: TextageResource,
    ) -> Result<TextageHttpResponse, TextageAcquisitionError> {
        debug_assert!(resource.endpoint().starts_with(TEXTAGE_ROOT));
        let mut response = self
            .agent
            .get(resource.endpoint())
            .header("Accept", "application/javascript")
            .call()
            .map_err(|error| map_ureq_error(resource, error))?;
        let status = response.status().as_u16();
        let content_length = response.body().content_length();
        if status != 200 {
            return Ok(TextageHttpResponse {
                status,
                content_length,
                body: Vec::new(),
            });
        }
        if content_length.is_some_and(|length| length > MAX_TEXTAGE_FILE_BYTES as u64) {
            return Err(TextageAcquisitionError::DeclaredBodyTooLarge {
                resource,
                declared: content_length.expect("checked content length"),
                maximum: MAX_TEXTAGE_FILE_BYTES,
            });
        }
        let body = read_bounded_body(response.body_mut().as_reader(), resource)?;
        Ok(TextageHttpResponse {
            status,
            content_length,
            body,
        })
    }
}

pub(super) fn acquire_textage(
    transport: &impl TextageTransport,
    cache_root: &Path,
) -> Result<AcquiredTextage, TextageAcquisitionError> {
    let title = verified_body(
        transport.get(TextageResource::Title)?,
        TextageResource::Title,
    )?;
    let availability = verified_body(
        transport.get(TextageResource::Availability)?,
        TextageResource::Availability,
    )?;
    let chart = verified_body(
        transport.get(TextageResource::Chart)?,
        TextageResource::Chart,
    )?;
    let content_sha256 = textage_bundle_digest([
        (TextageResource::Title.filename(), title.as_slice()),
        (
            TextageResource::Availability.filename(),
            availability.as_slice(),
        ),
        (TextageResource::Chart.filename(), chart.as_slice()),
    ]);
    let snapshot = TextageLiveAdapter::parse(
        &title,
        &availability,
        &chart,
        SourceRevision::content_sha256(&content_sha256)?,
    )?;
    let record_count = snapshot.evidence().record_count();
    cache_verified_bundle(
        cache_root,
        &content_sha256,
        [
            (TextageResource::Title, title.as_slice()),
            (TextageResource::Availability, availability.as_slice()),
            (TextageResource::Chart, chart.as_slice()),
        ],
    )?;
    Ok(AcquiredTextage {
        snapshot,
        content_sha256,
        record_count,
    })
}

fn verified_body(
    response: TextageHttpResponse,
    resource: TextageResource,
) -> Result<Vec<u8>, TextageAcquisitionError> {
    if response.status != 200 {
        return Err(TextageAcquisitionError::UnexpectedStatus {
            resource,
            status: response.status,
        });
    }
    if let Some(declared) = response.content_length
        && declared > MAX_TEXTAGE_FILE_BYTES as u64
    {
        return Err(TextageAcquisitionError::DeclaredBodyTooLarge {
            resource,
            declared,
            maximum: MAX_TEXTAGE_FILE_BYTES,
        });
    }
    if response.body.len() > MAX_TEXTAGE_FILE_BYTES {
        return Err(TextageAcquisitionError::BodyTooLarge {
            resource,
            actual: Some(response.body.len()),
            maximum: MAX_TEXTAGE_FILE_BYTES,
        });
    }
    Ok(response.body)
}

fn read_bounded_body(
    reader: impl io::Read,
    resource: TextageResource,
) -> Result<Vec<u8>, TextageAcquisitionError> {
    let mut body = Vec::new();
    reader
        .take((MAX_TEXTAGE_FILE_BYTES + 1) as u64)
        .read_to_end(&mut body)
        .map_err(|error| map_body_io_error(resource, &error))?;
    if body.len() > MAX_TEXTAGE_FILE_BYTES {
        return Err(TextageAcquisitionError::BodyTooLarge {
            resource,
            actual: Some(body.len()),
            maximum: MAX_TEXTAGE_FILE_BYTES,
        });
    }
    Ok(body)
}

fn map_body_io_error(resource: TextageResource, error: &io::Error) -> TextageAcquisitionError {
    let wrapped_timeout = error
        .get_ref()
        .and_then(|source| source.downcast_ref::<ureq::Error>())
        .is_some_and(|error| matches!(error, ureq::Error::Timeout(_)));
    if error.kind() == io::ErrorKind::TimedOut || wrapped_timeout {
        TextageAcquisitionError::Timeout(resource)
    } else {
        TextageAcquisitionError::Transport(resource, "response body read failed".to_owned())
    }
}

fn map_ureq_error(resource: TextageResource, error: ureq::Error) -> TextageAcquisitionError {
    match error {
        ureq::Error::Timeout(_) => TextageAcquisitionError::Timeout(resource),
        ureq::Error::BodyExceedsLimit(_) => TextageAcquisitionError::BodyTooLarge {
            resource,
            actual: None,
            maximum: MAX_TEXTAGE_FILE_BYTES,
        },
        other => TextageAcquisitionError::Transport(resource, other.to_string()),
    }
}

fn cache_verified_bundle<'a>(
    cache_root: &Path,
    content_sha256: &str,
    files: impl IntoIterator<Item = (TextageResource, &'a [u8])>,
) -> Result<(), TextageAcquisitionError> {
    let files: Vec<_> = files.into_iter().collect();
    let directory = cache_root.join("textage");
    create_private_directory(&directory).map_err(TextageAcquisitionError::CacheIo)?;
    recover_cache_staging(&directory)?;
    let destination = directory.join(content_sha256);
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
        .map_err(TextageAcquisitionError::CacheIo)?;
    for (resource, bytes) in &files {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(staging.path().join(resource.filename()))
            .map_err(TextageAcquisitionError::CacheIo)?;
        file.write_all(bytes)
            .and_then(|()| file.flush())
            .and_then(|()| file.sync_all())
            .map_err(TextageAcquisitionError::CacheIo)?;
    }
    File::open(staging.path())
        .and_then(|directory| directory.sync_all())
        .map_err(TextageAcquisitionError::CacheIo)?;
    let staging_path = staging.keep();
    fs::rename(&staging_path, &destination).map_err(TextageAcquisitionError::CacheIo)?;
    File::open(&directory)
        .and_then(|directory| directory.sync_all())
        .map_err(TextageAcquisitionError::CacheIo)
}

fn recover_cache_staging(directory: &Path) -> Result<(), TextageAcquisitionError> {
    let mut removed = false;
    for entry in fs::read_dir(directory).map_err(TextageAcquisitionError::CacheIo)? {
        let entry = entry.map_err(TextageAcquisitionError::CacheIo)?;
        let is_staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(CACHE_STAGING_PREFIX));
        if !is_staging {
            continue;
        }
        if !entry
            .path()
            .symlink_metadata()
            .map_err(TextageAcquisitionError::CacheIo)?
            .is_dir()
        {
            return Err(TextageAcquisitionError::CacheCapacityExceeded);
        }
        fs::remove_dir_all(entry.path()).map_err(TextageAcquisitionError::CacheIo)?;
        removed = true;
    }
    if removed {
        File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(TextageAcquisitionError::CacheIo)?;
    }
    Ok(())
}

fn ensure_cache_capacity(
    directory: &Path,
    incoming_bytes: u64,
) -> Result<(), TextageAcquisitionError> {
    let mut revisions = 0_usize;
    let mut total_bytes = 0_u64;
    for entry in fs::read_dir(directory).map_err(TextageAcquisitionError::CacheIo)? {
        let entry = entry.map_err(TextageAcquisitionError::CacheIo)?;
        let metadata = entry
            .path()
            .symlink_metadata()
            .map_err(TextageAcquisitionError::CacheIo)?;
        if !metadata.is_dir() || !valid_digest(&entry.file_name().to_string_lossy()) {
            return Err(TextageAcquisitionError::CacheCapacityExceeded);
        }
        revisions = revisions.saturating_add(1);
        total_bytes = total_bytes.saturating_add(bundle_size(&entry.path())?);
        if revisions >= MAX_CACHE_REVISIONS || total_bytes > MAX_CACHE_BYTES {
            return Err(TextageAcquisitionError::CacheCapacityExceeded);
        }
    }
    if incoming_bytes > MAX_CACHE_BYTES
        || total_bytes.saturating_add(incoming_bytes) > MAX_CACHE_BYTES
    {
        return Err(TextageAcquisitionError::CacheCapacityExceeded);
    }
    Ok(())
}

fn bundle_size(path: &Path) -> Result<u64, TextageAcquisitionError> {
    let mut seen = 0_u8;
    let mut total = 0_u64;
    for entry in fs::read_dir(path).map_err(TextageAcquisitionError::CacheIo)? {
        let entry = entry.map_err(TextageAcquisitionError::CacheIo)?;
        let resource = resource_for_filename(&entry.file_name().to_string_lossy())
            .ok_or(TextageAcquisitionError::CacheCapacityExceeded)?;
        if seen & resource.bit() != 0 {
            return Err(TextageAcquisitionError::CacheCapacityExceeded);
        }
        seen |= resource.bit();
        let metadata = entry
            .path()
            .symlink_metadata()
            .map_err(TextageAcquisitionError::CacheIo)?;
        if !metadata.is_file() || metadata.len() > MAX_TEXTAGE_FILE_BYTES as u64 {
            return Err(TextageAcquisitionError::CacheCapacityExceeded);
        }
        total = total.saturating_add(metadata.len());
    }
    if seen != 0b111 {
        return Err(TextageAcquisitionError::CacheCapacityExceeded);
    }
    Ok(total)
}

fn verify_existing_bundle(
    destination: &Path,
    directory: &Path,
    expected: &[(TextageResource, &[u8])],
) -> Result<(), TextageAcquisitionError> {
    if !destination
        .symlink_metadata()
        .map_err(TextageAcquisitionError::CacheIo)?
        .is_dir()
    {
        return Err(TextageAcquisitionError::CacheConflict(
            destination.to_owned(),
        ));
    }
    for (resource, bytes) in expected {
        let path = destination.join(resource.filename());
        let mut file = File::open(&path).map_err(TextageAcquisitionError::CacheIo)?;
        let metadata = file.metadata().map_err(TextageAcquisitionError::CacheIo)?;
        if metadata.len() != bytes.len() as u64 || metadata.len() > MAX_TEXTAGE_FILE_BYTES as u64 {
            return Err(TextageAcquisitionError::CacheConflict(path));
        }
        let mut existing = Vec::with_capacity(bytes.len());
        std::io::Read::by_ref(&mut file)
            .take((MAX_TEXTAGE_FILE_BYTES + 1) as u64)
            .read_to_end(&mut existing)
            .map_err(TextageAcquisitionError::CacheIo)?;
        if existing.as_slice() != *bytes {
            return Err(TextageAcquisitionError::CacheConflict(path));
        }
        file.sync_all().map_err(TextageAcquisitionError::CacheIo)?;
    }
    if bundle_size(destination)?
        != expected
            .iter()
            .map(|(_, bytes)| bytes.len() as u64)
            .sum::<u64>()
    {
        return Err(TextageAcquisitionError::CacheConflict(
            destination.to_owned(),
        ));
    }
    File::open(destination)
        .and_then(|directory| directory.sync_all())
        .and_then(|()| File::open(directory).and_then(|directory| directory.sync_all()))
        .map_err(TextageAcquisitionError::CacheIo)
}

fn valid_digest(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn resource_for_filename(name: &str) -> Option<TextageResource> {
    match name {
        "titletbl.js" => Some(TextageResource::Title),
        "actbl.js" => Some(TextageResource::Availability),
        "datatbl.js" => Some(TextageResource::Chart),
        _ => None,
    }
}

#[cfg(test)]
mod tests {

    use tempfile::TempDir;

    use super::*;

    struct FakeTransport {
        responses: std::collections::BTreeMap<TextageResource, TextageHttpResponse>,
    }

    impl TextageTransport for FakeTransport {
        fn get(
            &self,
            resource: TextageResource,
        ) -> Result<TextageHttpResponse, TextageAcquisitionError> {
            Ok(self.responses[&resource].clone())
        }
    }

    #[test]
    fn caches_a_verified_private_bundle_and_reuses_identical_content() {
        let root = TempDir::new().unwrap();
        let transport = fixture_transport();
        let first = acquire_textage(&transport, root.path()).unwrap();
        let second = acquire_textage(&transport, root.path()).unwrap();
        assert_eq!(first.content_sha256, second.content_sha256);
        let directory = root.path().join("textage").join(first.content_sha256);
        assert!(directory.is_dir());
        for resource in [
            TextageResource::Title,
            TextageResource::Availability,
            TextageResource::Chart,
        ] {
            assert!(directory.join(resource.filename()).is_file());
        }
    }

    #[test]
    fn rejects_status_and_declared_or_actual_size_before_parsing() {
        let mut transport = fixture_transport();
        transport
            .responses
            .get_mut(&TextageResource::Title)
            .unwrap()
            .status = 503;
        assert!(matches!(
            acquire_textage(&transport, TempDir::new().unwrap().path()),
            Err(TextageAcquisitionError::UnexpectedStatus { status: 503, .. })
        ));

        let oversized = TextageHttpResponse {
            status: 200,
            content_length: Some((MAX_TEXTAGE_FILE_BYTES + 1) as u64),
            body: Vec::new(),
        };
        assert!(matches!(
            verified_body(oversized, TextageResource::Chart),
            Err(TextageAcquisitionError::DeclaredBodyTooLarge { .. })
        ));
    }

    fn fixture_transport() -> FakeTransport {
        let title = br#"VERINDEX=0;IDINDEX=1;OPTINDEX=2;GENREINDEX=3;ARTISTINDEX=4;TITLEINDEX=5;SUBTITLEINDEX=6;SS=0;titletbl={'alpha':[1,10,0,"GENRE","ARTIST","ALPHA"]};"#.to_vec();
        let availability = br#"pspver="version";A=10,B=11,C=12,D=13,E=14,F=15;actbl={'alpha':[1,0,0,1,7,4,7,8,7,A,7,0,0,0,0,4,7,8,7,A,7,0,0]};"#.to_vec();
        let chart = br#"datatbl={'alpha':[0,100,200,300,400,0,0,210,310,410,0,"120"]};"#.to_vec();
        FakeTransport {
            responses: [
                (TextageResource::Title, title),
                (TextageResource::Availability, availability),
                (TextageResource::Chart, chart),
            ]
            .into_iter()
            .map(|(resource, body)| {
                (
                    resource,
                    TextageHttpResponse {
                        status: 200,
                        content_length: Some(body.len() as u64),
                        body,
                    },
                )
            })
            .collect(),
        }
    }
}
