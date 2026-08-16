use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::Builder;

const INGEST_REQUEST_SCHEMA: &str = "scorepeek-private-corpus-ingest-v1";
const INGEST_SUMMARY_SCHEMA: &str = "scorepeek-private-corpus-ingest-summary-v1";
const SOURCE_MANIFEST_SCHEMA: &str = "scorepeek-private-corpus-source-v1";
const GENERATION_SCHEMA: &str = "scorepeek-private-corpus-generation-v1";
const GENERATION_SUMMARY_SCHEMA: &str = "scorepeek-private-corpus-generation-summary-v1";
const REPLAY_INDEX_SCHEMA: &str = "scorepeek-private-corpus-replay-v1";
const REPLAY_SUITE_SCHEMA: &str = "scorepeek-private-corpus-replay-suite-v1";
const REPLAY_SUITE_SUMMARY_SCHEMA: &str = "scorepeek-private-corpus-replay-suite-summary-v1";
const COMPLETE_LABEL_SCHEMA: &str = "scorepeek-private-complete-label-v1";
const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_SOURCE_OBJECTS: usize = 1_024;
const MAX_SOURCE_STORAGE_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
const MAX_MANIFEST_STORAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_GENERATION_BYTES: usize = 256 * 1024;
const MAX_GENERATIONS: usize = 128;
const MAX_GENERATION_STORAGE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_REPLAY_INDEX_BYTES: usize = 32 * 1024 * 1024;
const MAX_REPLAY_INDEXES: usize = 1_024;
const MAX_REPLAY_FRAMES: usize = 250_000;
const MAX_LABEL_BYTES: usize = 64 * 1024;
const MAX_LABEL_OBJECTS: usize = 250_000;
const MAX_LABEL_STORAGE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const SOURCE_FILE: &str = "source.media";
const SOURCE_STAGING_PREFIX: &str = ".corpus-source-staging-";
const MANIFEST_STAGING_PREFIX: &str = ".corpus-manifest-staging-";
const GENERATION_STAGING_PREFIX: &str = ".corpus-generation-staging-";

#[derive(Debug)]
pub enum CorpusError {
    Io(io::Error),
    Json(serde_json::Error),
    InvalidRequest(String),
    InvalidReplay(String),
    FixtureConflict,
    CapacityExceeded,
}

impl fmt::Display for CorpusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "private corpus I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "private corpus JSON failed: {error}"),
            Self::InvalidRequest(detail) => write!(formatter, "invalid ingest request: {detail}"),
            Self::InvalidReplay(detail) => write!(formatter, "invalid replay index: {detail}"),
            Self::FixtureConflict => {
                formatter.write_str("fixture ID is already bound to different source metadata")
            }
            Self::CapacityExceeded => formatter.write_str("private corpus capacity is exhausted"),
        }
    }
}

impl Error for CorpusError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::InvalidRequest(_)
            | Self::InvalidReplay(_)
            | Self::FixtureConflict
            | Self::CapacityExceeded => None,
        }
    }
}

impl From<io::Error> for CorpusError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for CorpusError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CorpusProfile {
    WindowsSemanticReference {
        recording_profile_id: String,
    },
    LinuxCaptureCalibration {
        capture_profile_id: String,
        normalizer_profile_id: String,
        layout_profile_id: String,
    },
}

impl CorpusProfile {
    fn validate(&self, context: ErrorContext) -> Result<(), CorpusError> {
        match self {
            Self::WindowsSemanticReference {
                recording_profile_id,
            } => validate_token(recording_profile_id, "recording_profile_id", context),
            Self::LinuxCaptureCalibration {
                capture_profile_id,
                normalizer_profile_id,
                layout_profile_id,
            } => {
                validate_token(capture_profile_id, "capture_profile_id", context)?;
                validate_token(normalizer_profile_id, "normalizer_profile_id", context)?;
                validate_token(layout_profile_id, "layout_profile_id", context)
            }
        }
    }

    const fn role(&self) -> CorpusRole {
        match self {
            Self::WindowsSemanticReference { .. } => CorpusRole::WindowsSemanticReference,
            Self::LinuxCaptureCalibration { .. } => CorpusRole::LinuxCaptureCalibration,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorpusRole {
    WindowsSemanticReference,
    LinuxCaptureCalibration,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct IngestRequest {
    schema: String,
    fixture_id: String,
    session_id: String,
    profile: CorpusProfile,
}

impl IngestRequest {
    fn validate(&self) -> Result<(), CorpusError> {
        if self.schema != INGEST_REQUEST_SCHEMA {
            return Err(CorpusError::InvalidRequest(format!(
                "schema must be {INGEST_REQUEST_SCHEMA:?}"
            )));
        }
        validate_opaque_id(&self.fixture_id, "fixture_id", ErrorContext::Request)?;
        validate_opaque_id(&self.session_id, "session_id", ErrorContext::Request)?;
        self.profile.validate(ErrorContext::Request)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentRef {
    pub sha256: String,
    pub bytes: u64,
}

impl ContentRef {
    fn validate(&self, context: ErrorContext) -> Result<(), CorpusError> {
        validate_sha256(&self.sha256, "sha256", context)?;
        if self.bytes == 0 || self.bytes > MAX_SOURCE_BYTES {
            return Err(context.error("source byte length is outside the admitted range"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceManifest {
    pub schema: String,
    pub fixture_id: String,
    pub session_id: String,
    pub profile: CorpusProfile,
    pub source: ContentRef,
}

impl SourceManifest {
    /// Produces the aggregate-only CLI result and canonical manifest digest.
    ///
    /// # Errors
    ///
    /// Returns an error if this in-memory manifest is invalid or cannot be encoded.
    pub fn summary(&self) -> Result<IngestSummary, CorpusError> {
        self.validate()?;
        Ok(IngestSummary {
            schema: INGEST_SUMMARY_SCHEMA.to_owned(),
            fixture_id: self.fixture_id.clone(),
            corpus_role: self.profile.role(),
            source_sha256: self.source.sha256.clone(),
            source_bytes: self.source.bytes,
            source_manifest_sha256: digest_bytes(&canonical_json(self)?),
        })
    }

    fn validate(&self) -> Result<(), CorpusError> {
        if self.schema != SOURCE_MANIFEST_SCHEMA {
            return Err(CorpusError::InvalidRequest(format!(
                "source manifest schema must be {SOURCE_MANIFEST_SCHEMA:?}"
            )));
        }
        validate_opaque_id(&self.fixture_id, "fixture_id", ErrorContext::Request)?;
        validate_opaque_id(&self.session_id, "session_id", ErrorContext::Request)?;
        self.profile.validate(ErrorContext::Request)?;
        self.source.validate(ErrorContext::Request)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IngestSummary {
    pub schema: String,
    pub fixture_id: String,
    pub corpus_role: CorpusRole,
    pub source_sha256: String,
    pub source_bytes: u64,
    pub source_manifest_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusGeneration {
    pub schema: String,
    pub generation_id: String,
    pub sources: Vec<GenerationSource>,
}

impl CorpusGeneration {
    fn validate(&self) -> Result<(), CorpusError> {
        if self.schema != GENERATION_SCHEMA {
            return Err(CorpusError::InvalidReplay(format!(
                "generation schema must be {GENERATION_SCHEMA:?}"
            )));
        }
        validate_opaque_id(&self.generation_id, "generation_id", ErrorContext::Replay)?;
        if self.sources.is_empty() || self.sources.len() > MAX_SOURCE_OBJECTS {
            return Err(CorpusError::InvalidReplay(
                "generation source count is outside the admitted range".to_owned(),
            ));
        }
        let mut previous = None;
        for source in &self.sources {
            validate_opaque_id(&source.fixture_id, "fixture_id", ErrorContext::Replay)?;
            validate_sha256(
                &source.source_manifest_sha256,
                "source_manifest_sha256",
                ErrorContext::Replay,
            )?;
            if previous.is_some_and(|value| value >= source.fixture_id.as_str()) {
                return Err(CorpusError::InvalidReplay(
                    "generation sources must be uniquely ordered by fixture_id".to_owned(),
                ));
            }
            previous = Some(source.fixture_id.as_str());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationSource {
    pub fixture_id: String,
    pub source_manifest_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GenerationSummary {
    pub schema: String,
    pub generation_id: String,
    pub corpus_generation_sha256: String,
    pub source_count: u64,
}

#[derive(Clone, Debug)]
pub struct CorpusStore {
    root: PathBuf,
}

impl CorpusStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Copies immutable source media into the bounded private content store and binds an opaque
    /// fixture ID to a deterministic manifest.
    ///
    /// Identical content and metadata are idempotent. Reusing a fixture ID for different metadata
    /// fails without changing that binding.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed metadata, non-regular or oversized source media, conflicting
    /// fixture IDs, capacity exhaustion, or failed durable storage operations.
    pub fn ingest(
        &self,
        source_path: impl AsRef<Path>,
        request_path: impl AsRef<Path>,
    ) -> Result<SourceManifest, CorpusError> {
        self.validate_root()?;
        let request = read_ingest_request(request_path.as_ref())?;
        validate_source_file(source_path.as_ref())?;

        preflight_managed_components(&self.root)?;
        create_private_directory(&self.root)?;
        let lock = open_store_lock(&self.root, true)?;
        lock.lock()?;
        preflight_managed_components(&self.root)?;
        let content_dir = self.root.join("content");
        let manifest_dir = self.root.join("manifests");
        let label_dir = self.root.join("labels");
        create_private_directory(&content_dir)?;
        create_private_directory(&manifest_dir)?;
        create_private_directory(&label_dir)?;
        recover_staging(&content_dir, &manifest_dir)?;

        let staging = Builder::new()
            .prefix(SOURCE_STAGING_PREFIX)
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(&content_dir)?;
        let staged_source = staging.path().join(SOURCE_FILE);
        let source = copy_source(source_path.as_ref(), &staged_source)?;
        File::open(&staged_source)?.sync_all()?;
        File::open(staging.path())?.sync_all()?;

        let manifest = SourceManifest {
            schema: SOURCE_MANIFEST_SCHEMA.to_owned(),
            fixture_id: request.fixture_id,
            session_id: request.session_id,
            profile: request.profile,
            source,
        };
        let manifest_bytes = canonical_json(&manifest)?;
        let manifest_path = manifest_dir.join(format!("{}.json", manifest.fixture_id));
        let manifest_exists = match manifest_path.symlink_metadata() {
            Ok(_) => {
                if read_bounded_regular(&manifest_path, MAX_REQUEST_BYTES, ErrorContext::Request)?
                    != manifest_bytes
                {
                    return Err(CorpusError::FixtureConflict);
                }
                fs::set_permissions(&manifest_path, fs::Permissions::from_mode(0o600))?;
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };

        let destination = content_dir.join(&manifest.source.sha256);
        let destination_exists = match destination.symlink_metadata() {
            Ok(metadata) if metadata.is_dir() => true,
            Ok(_) => {
                return Err(CorpusError::InvalidRequest(
                    "content-addressed destination is not a directory".to_owned(),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        if !destination_exists {
            ensure_capacity(&content_dir, manifest.source.bytes)?;
        }
        if !manifest_exists {
            ensure_manifest_capacity(&manifest_dir, manifest_bytes.len())?;
        }

        if destination_exists {
            set_stored_source_permissions(&destination)?;
            validate_stored_source(&destination, &manifest.source)?;
            sync_stored_source_and_parent(&destination, &content_dir)?;
            staging.close()?;
            File::open(&content_dir)?.sync_all()?;
        } else {
            let staging_path = staging.keep();
            fs::rename(staging_path, &destination)?;
            sync_stored_source_and_parent(&destination, &content_dir)?;
        }

        if !manifest_exists {
            write_atomic_file(
                &manifest_dir,
                &manifest_path,
                &manifest_bytes,
                MANIFEST_STAGING_PREFIX,
            )?;
        }
        sync_file_and_parent(&manifest_path, &manifest_dir)?;
        drop(lock);
        Ok(manifest)
    }

    /// Seals every source binding currently present in this private store into one immutable
    /// content-addressed corpus generation.
    ///
    /// # Errors
    ///
    /// Returns an error if the generation ID is invalid, the source store is malformed, a source
    /// object no longer matches its canonical manifest, or durable publication fails.
    pub fn seal_generation(&self, generation_id: &str) -> Result<GenerationSummary, CorpusError> {
        self.validate_root()?;
        validate_opaque_id(generation_id, "generation_id", ErrorContext::Request)?;
        preflight_managed_components(&self.root)?;
        validate_private_directory_mode(&self.root, ErrorContext::Request)?;
        let content_dir = self.root.join("content");
        let manifest_dir = self.root.join("manifests");
        validate_private_directory_mode(&content_dir, ErrorContext::Request)?;
        validate_private_directory_mode(&manifest_dir, ErrorContext::Request)?;
        let lock = open_store_lock(&self.root, false)?;
        lock.lock()?;
        preflight_managed_components(&self.root)?;
        recover_staging(&content_dir, &manifest_dir)?;

        let generation_dir = self.root.join("generations");
        create_private_directory(&generation_dir)?;
        recover_generation_staging(&generation_dir)?;
        let sources = read_generation_sources(&manifest_dir, &content_dir)?;
        let generation = CorpusGeneration {
            schema: GENERATION_SCHEMA.to_owned(),
            generation_id: generation_id.to_owned(),
            sources,
        };
        generation
            .validate()
            .map_err(|_| CorpusError::InvalidRequest("generation is invalid".to_owned()))?;
        let bytes = canonical_json(&generation)?;
        if bytes.len() > MAX_GENERATION_BYTES {
            return Err(CorpusError::CapacityExceeded);
        }
        let digest = digest_bytes(&bytes);
        let destination = generation_dir.join(format!("{digest}.json"));
        match destination.symlink_metadata() {
            Ok(_) => {
                if read_bounded_regular(&destination, MAX_GENERATION_BYTES, ErrorContext::Request)?
                    != bytes
                {
                    return Err(CorpusError::InvalidRequest(
                        "generation digest is bound to different bytes".to_owned(),
                    ));
                }
                fs::set_permissions(&destination, fs::Permissions::from_mode(0o600))?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                ensure_generation_capacity(&generation_dir, bytes.len())?;
                write_atomic_file(
                    &generation_dir,
                    &destination,
                    &bytes,
                    GENERATION_STAGING_PREFIX,
                )?;
            }
            Err(error) => return Err(error.into()),
        }
        sync_file_and_parent(&destination, &generation_dir)?;
        drop(lock);
        Ok(GenerationSummary {
            schema: GENERATION_SUMMARY_SCHEMA.to_owned(),
            generation_id: generation.generation_id,
            corpus_generation_sha256: digest,
            source_count: generation.sources.len() as u64,
        })
    }

    /// Validates a complete replay suite against the canonical source manifests and immutable
    /// media in this private store.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed suite metadata, missing or mismatched source manifests,
    /// damaged source content, or any grouped split crossing anywhere in the suite.
    pub fn validate_replay_suite(
        &self,
        suite_path: impl AsRef<Path>,
    ) -> Result<ReplaySuiteSummary, CorpusError> {
        self.validate_root()?;
        validate_private_directory_mode(&self.root, ErrorContext::Replay)?;
        validate_private_directory_mode(&self.root.join("content"), ErrorContext::Replay)?;
        validate_private_directory_mode(&self.root.join("manifests"), ErrorContext::Replay)?;
        validate_private_directory_mode(&self.root.join("generations"), ErrorContext::Replay)?;
        validate_private_directory_mode(&self.root.join("labels"), ErrorContext::Replay)?;
        validate_label_store(&self.root.join("labels"))?;
        let suite = ReplaySuite::read_from(suite_path)?;
        suite.validate_against(self)
    }

    fn validate_root(&self) -> Result<(), CorpusError> {
        if self.root.as_os_str().is_empty() || !self.root.is_absolute() {
            return Err(CorpusError::InvalidRequest(
                "private store root must be an absolute, non-empty path".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayIndex {
    pub schema: String,
    pub fixture_id: String,
    pub session_id: String,
    pub profile: CorpusProfile,
    pub source: ContentRef,
    pub source_manifest_sha256: String,
    pub extractor: ExtractorIdentity,
    pub source_time_base: TimeBase,
    pub frames: Vec<ReplayFrame>,
}

impl ReplayIndex {
    fn validate(&self) -> Result<(), CorpusError> {
        if self.schema != REPLAY_INDEX_SCHEMA {
            return Err(CorpusError::InvalidReplay(format!(
                "schema must be {REPLAY_INDEX_SCHEMA:?}"
            )));
        }
        validate_opaque_id(&self.fixture_id, "fixture_id", ErrorContext::Replay)?;
        validate_opaque_id(&self.session_id, "session_id", ErrorContext::Replay)?;
        self.profile.validate(ErrorContext::Replay)?;
        self.source.validate(ErrorContext::Replay)?;
        validate_sha256(
            &self.source_manifest_sha256,
            "source_manifest_sha256",
            ErrorContext::Replay,
        )?;
        self.extractor.validate()?;
        self.source_time_base.validate()?;
        if self.frames.is_empty() || self.frames.len() > MAX_REPLAY_FRAMES {
            return Err(CorpusError::InvalidReplay(
                "frame count is outside the admitted range".to_owned(),
            ));
        }

        let mut frame_ids = BTreeSet::new();
        let mut previous_decode_index = None;
        for frame in &self.frames {
            frame.validate()?;
            if !frame_ids.insert(&frame.frame_id) {
                return Err(CorpusError::InvalidReplay(
                    "frame_id values must be unique".to_owned(),
                ));
            }
            if previous_decode_index.is_some_and(|previous| frame.decode_index <= previous) {
                return Err(CorpusError::InvalidReplay(
                    "frames must be strictly ordered by decode_index".to_owned(),
                ));
            }
            previous_decode_index = Some(frame.decode_index);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaySuite {
    pub schema: String,
    pub suite_id: String,
    pub corpus_generation_sha256: String,
    pub indexes: Vec<ReplayIndex>,
}

impl ReplaySuite {
    fn read_from(path: impl AsRef<Path>) -> Result<Self, CorpusError> {
        let bytes =
            read_bounded_regular(path.as_ref(), MAX_REPLAY_INDEX_BYTES, ErrorContext::Replay)?;
        if bytes.len() > MAX_REPLAY_INDEX_BYTES {
            return Err(CorpusError::InvalidReplay(
                "suite exceeds the size limit".to_owned(),
            ));
        }
        let suite: Self = serde_json::from_slice(&bytes)?;
        suite.validate_structure()?;
        Ok(suite)
    }

    fn validate_structure(&self) -> Result<(), CorpusError> {
        if self.schema != REPLAY_SUITE_SCHEMA {
            return Err(CorpusError::InvalidReplay(format!(
                "suite schema must be {REPLAY_SUITE_SCHEMA:?}"
            )));
        }
        validate_opaque_id(&self.suite_id, "suite_id", ErrorContext::Replay)?;
        validate_sha256(
            &self.corpus_generation_sha256,
            "corpus_generation_sha256",
            ErrorContext::Replay,
        )?;
        if self.indexes.is_empty() || self.indexes.len() > MAX_REPLAY_INDEXES {
            return Err(CorpusError::InvalidReplay(
                "replay index count is outside the admitted range".to_owned(),
            ));
        }
        let mut total_frames = 0_usize;
        let mut previous_fixture = None;
        for index in &self.indexes {
            index.validate()?;
            if previous_fixture.is_some_and(|value| value >= index.fixture_id.as_str()) {
                return Err(CorpusError::InvalidReplay(
                    "replay indexes must be uniquely ordered by fixture_id".to_owned(),
                ));
            }
            previous_fixture = Some(index.fixture_id.as_str());
            total_frames = total_frames
                .checked_add(index.frames.len())
                .ok_or_else(|| CorpusError::InvalidReplay("frame count overflow".to_owned()))?;
            if total_frames > MAX_REPLAY_FRAMES {
                return Err(CorpusError::InvalidReplay(
                    "suite frame count exceeds the admitted range".to_owned(),
                ));
            }
        }
        Ok(())
    }

    fn validate_against(&self, store: &CorpusStore) -> Result<ReplaySuiteSummary, CorpusError> {
        self.validate_structure()?;
        let generation = load_generation(store, &self.corpus_generation_sha256)?;
        let generation_sources: BTreeMap<_, _> = generation
            .sources
            .iter()
            .map(|source| {
                (
                    source.fixture_id.as_str(),
                    source.source_manifest_sha256.as_str(),
                )
            })
            .collect();
        if self.indexes.len() != generation_sources.len() {
            return Err(CorpusError::InvalidReplay(
                "replay suite does not completely cover its corpus generation".to_owned(),
            ));
        }
        let mut fixture_ids = BTreeSet::new();
        let mut assignments = SplitAssignments::default();

        for index in &self.indexes {
            if !fixture_ids.insert(&index.fixture_id) {
                return Err(CorpusError::InvalidReplay(
                    "fixture_id values must be unique across a replay suite".to_owned(),
                ));
            }
            if generation_sources.get(index.fixture_id.as_str()).copied()
                != Some(index.source_manifest_sha256.as_str())
            {
                return Err(CorpusError::InvalidReplay(
                    "replay index is not a member of its corpus generation".to_owned(),
                ));
            }
            validate_source_binding(store, index)?;
            for frame in &index.frames {
                validate_complete_label(store, frame)?;
                assignments.record(index, frame)?;
            }
        }

        Ok(ReplaySuiteSummary {
            schema: REPLAY_SUITE_SUMMARY_SCHEMA.to_owned(),
            suite_id: self.suite_id.clone(),
            corpus_generation_sha256: self.corpus_generation_sha256.clone(),
            replay_suite_sha256: digest_bytes(&canonical_json(self)?),
            index_count: self.indexes.len() as u64,
            frame_count: assignments.frame_count,
            split_counts: assignments.split_counts,
        })
    }
}

#[derive(Default)]
struct SplitAssignments {
    frame_ids: BTreeSet<String>,
    sessions: BTreeMap<String, CorpusSplit>,
    profiles: BTreeMap<CorpusProfile, CorpusSplit>,
    episodes: BTreeMap<String, CorpusSplit>,
    session_hashes: BTreeMap<String, CorpusSplit>,
    plays: BTreeMap<String, CorpusSplit>,
    titles: BTreeMap<String, CorpusSplit>,
    frame_digests: BTreeMap<String, CorpusSplit>,
    split_counts: BTreeMap<CorpusSplit, u64>,
    frame_count: u64,
}

impl SplitAssignments {
    fn record(&mut self, index: &ReplayIndex, frame: &ReplayFrame) -> Result<(), CorpusError> {
        if !self.frame_ids.insert(frame.frame_id.clone()) {
            return Err(CorpusError::InvalidReplay(
                "frame_id values must be unique across a replay suite".to_owned(),
            ));
        }
        require_one_split(
            &mut self.sessions,
            index.session_id.clone(),
            frame.split,
            "session ID",
        )?;
        require_one_split(
            &mut self.profiles,
            index.profile.clone(),
            frame.split,
            "capture profile",
        )?;
        require_one_split(
            &mut self.episodes,
            frame.episode_id.clone(),
            frame.split,
            "episode",
        )?;
        require_one_split(
            &mut self.session_hashes,
            frame.groups.session_sha256.clone(),
            frame.split,
            "session hash",
        )?;
        require_one_split(
            &mut self.plays,
            frame.groups.play_sha256.clone(),
            frame.split,
            "play",
        )?;
        require_one_split(
            &mut self.titles,
            frame.groups.title_sha256.clone(),
            frame.split,
            "title",
        )?;
        require_one_split(
            &mut self.frame_digests,
            frame.frame_sha256.clone(),
            frame.split,
            "identical frame",
        )?;
        *self.split_counts.entry(frame.split).or_insert(0_u64) += 1;
        self.frame_count += 1;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractorIdentity {
    pub tool_id: String,
    pub tool_version: String,
    pub extractor_manifest_sha256: String,
    pub parameters_sha256: String,
}

impl ExtractorIdentity {
    fn validate(&self) -> Result<(), CorpusError> {
        validate_token(&self.tool_id, "extractor tool_id", ErrorContext::Replay)?;
        validate_token(
            &self.tool_version,
            "extractor tool_version",
            ErrorContext::Replay,
        )?;
        validate_sha256(
            &self.extractor_manifest_sha256,
            "extractor_manifest_sha256",
            ErrorContext::Replay,
        )?;
        validate_sha256(
            &self.parameters_sha256,
            "extractor parameters_sha256",
            ErrorContext::Replay,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimeBase {
    pub numerator: u32,
    pub denominator: u32,
}

impl TimeBase {
    fn validate(self) -> Result<(), CorpusError> {
        if self.numerator == 0 || self.denominator == 0 {
            return Err(CorpusError::InvalidReplay(
                "source_time_base values must be positive".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayFrame {
    pub frame_id: String,
    pub source_pts: i64,
    pub decode_index: u64,
    pub frame_sha256: String,
    pub episode_id: String,
    pub screen_class: ScreenClass,
    pub split: CorpusSplit,
    pub groups: SplitGroups,
    pub annotation_revision: String,
    pub labels_sha256: String,
}

impl ReplayFrame {
    fn validate(&self) -> Result<(), CorpusError> {
        validate_opaque_id(&self.frame_id, "frame_id", ErrorContext::Replay)?;
        validate_opaque_id(&self.episode_id, "episode_id", ErrorContext::Replay)?;
        validate_sha256(&self.frame_sha256, "frame_sha256", ErrorContext::Replay)?;
        self.groups.validate()?;
        validate_token(
            &self.annotation_revision,
            "annotation_revision",
            ErrorContext::Replay,
        )?;
        validate_sha256(&self.labels_sha256, "labels_sha256", ErrorContext::Replay)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenClass {
    Result,
    MusicSelect,
    Transition,
    Negative,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum LabelState<T> {
    Known { value: T },
    Unknown { reason: String },
    NotApplicable,
}

impl<T> LabelState<T> {
    fn validate(
        &self,
        field: &str,
        validate_known: impl FnOnce(&T) -> bool,
    ) -> Result<(), CorpusError> {
        match self {
            Self::Known { value } if validate_known(value) => Ok(()),
            Self::Known { .. } => Err(CorpusError::InvalidReplay(format!(
                "complete-label {field} has an invalid known value"
            ))),
            Self::Unknown { reason } if valid_label_text(reason) => Ok(()),
            Self::Unknown { .. } => Err(CorpusError::InvalidReplay(format!(
                "complete-label {field} has an invalid unknown reason"
            ))),
            Self::NotApplicable => Ok(()),
        }
    }

    fn require_applicable(&self, field: &str) -> Result<(), CorpusError> {
        if matches!(self, Self::NotApplicable) {
            return Err(CorpusError::InvalidReplay(format!(
                "complete-label {field} is mandatory for this shape"
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaySide {
    OnePlayer,
    TwoPlayer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayMode {
    SinglePlay,
    DoublePlay,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayType {
    Single,
    Double,
    DoubleBattle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    Beginner,
    Normal,
    Hyper,
    Another,
    Leggendaria,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case", deny_unknown_fields)]
pub enum CompleteLabel {
    Result {
        schema: String,
        frame_id: String,
        annotation_revision: String,
        screen_state: LabelState<bool>,
        savable: LabelState<bool>,
        playside: LabelState<PlaySide>,
        play_mode: LabelState<PlayMode>,
        play_type: LabelState<PlayType>,
        song_id: LabelState<String>,
        difficulty: LabelState<Difficulty>,
        level: LabelState<u8>,
        notes: LabelState<u32>,
        current_score: LabelState<u32>,
    },
    MusicSelect {
        schema: String,
        frame_id: String,
        annotation_revision: String,
        screen_state: LabelState<bool>,
        play_mode: LabelState<PlayMode>,
        song_id: LabelState<String>,
        selected_difficulty: LabelState<Difficulty>,
        selected_level: LabelState<u8>,
    },
    NonRecognition {
        schema: String,
        frame_id: String,
        annotation_revision: String,
        screen_class: NonRecognitionClass,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NonRecognitionClass {
    Transition,
    Negative,
    Unknown,
}

impl CompleteLabel {
    fn validate_contents(&self) -> Result<(), CorpusError> {
        let (schema, frame_id, annotation_revision) = match self {
            Self::Result {
                schema,
                frame_id,
                annotation_revision,
                screen_state,
                savable,
                playside,
                play_mode,
                play_type,
                song_id,
                difficulty,
                level,
                notes,
                current_score,
            } => {
                validate_required_label(screen_state, "result.screen_state", |value| *value)?;
                validate_required_label(savable, "result.savable", |_| true)?;
                validate_required_label(playside, "result.playside", |_| true)?;
                validate_required_label(play_mode, "result.play_mode", |_| true)?;
                validate_required_label(play_type, "result.play_type", |_| true)?;
                validate_required_label(song_id, "result.song_id", |value| {
                    valid_label_text(value)
                })?;
                validate_required_label(difficulty, "result.difficulty", |_| true)?;
                validate_required_label(level, "result.level", |value| (1..=12).contains(value))?;
                validate_required_label(notes, "result.notes", |value| *value > 0)?;
                validate_required_label(current_score, "result.current_score", |_| true)?;
                validate_result_cross_fields(play_mode, play_type, notes, current_score)?;
                (schema, frame_id, annotation_revision)
            }
            Self::MusicSelect {
                schema,
                frame_id,
                annotation_revision,
                screen_state,
                play_mode,
                song_id,
                selected_difficulty,
                selected_level,
            } => {
                validate_required_label(screen_state, "music_select.screen_state", |value| *value)?;
                validate_required_label(play_mode, "music_select.play_mode", |_| true)?;
                validate_required_label(song_id, "music_select.song_id", |value| {
                    valid_label_text(value)
                })?;
                validate_required_label(
                    selected_difficulty,
                    "music_select.selected_difficulty",
                    |_| true,
                )?;
                validate_required_label(selected_level, "music_select.selected_level", |value| {
                    (1..=12).contains(value)
                })?;
                (schema, frame_id, annotation_revision)
            }
            Self::NonRecognition {
                schema,
                frame_id,
                annotation_revision,
                screen_class: _,
            } => (schema, frame_id, annotation_revision),
        };
        if schema != COMPLETE_LABEL_SCHEMA {
            return Err(CorpusError::InvalidReplay(format!(
                "complete-label schema must be {COMPLETE_LABEL_SCHEMA:?}"
            )));
        }
        validate_opaque_id(frame_id, "complete-label frame_id", ErrorContext::Replay)?;
        validate_token(
            annotation_revision,
            "complete-label annotation_revision",
            ErrorContext::Replay,
        )
    }

    fn validate_for(&self, frame: &ReplayFrame) -> Result<(), CorpusError> {
        self.validate_contents()?;
        let (frame_id, annotation_revision) = match self {
            Self::Result {
                frame_id,
                annotation_revision,
                ..
            } => {
                require_screen_class(frame, ScreenClass::Result)?;
                (frame_id, annotation_revision)
            }
            Self::MusicSelect {
                frame_id,
                annotation_revision,
                ..
            } => {
                require_screen_class(frame, ScreenClass::MusicSelect)?;
                (frame_id, annotation_revision)
            }
            Self::NonRecognition {
                frame_id,
                annotation_revision,
                screen_class,
                ..
            } => {
                let expected = match screen_class {
                    NonRecognitionClass::Transition => ScreenClass::Transition,
                    NonRecognitionClass::Negative => ScreenClass::Negative,
                    NonRecognitionClass::Unknown => ScreenClass::Unknown,
                };
                require_screen_class(frame, expected)?;
                (frame_id, annotation_revision)
            }
        };
        if frame_id != &frame.frame_id || annotation_revision != &frame.annotation_revision {
            return Err(CorpusError::InvalidReplay(
                "complete-label identity does not match its replay frame".to_owned(),
            ));
        }
        Ok(())
    }
}

fn validate_result_cross_fields(
    play_mode: &LabelState<PlayMode>,
    play_type: &LabelState<PlayType>,
    notes: &LabelState<u32>,
    current_score: &LabelState<u32>,
) -> Result<(), CorpusError> {
    if let (LabelState::Known { value: mode }, LabelState::Known { value: kind }) =
        (play_mode, play_type)
    {
        let compatible = matches!(
            (mode, kind),
            (PlayMode::SinglePlay, PlayType::Single)
                | (
                    PlayMode::DoublePlay,
                    PlayType::Double | PlayType::DoubleBattle
                )
        );
        if !compatible {
            return Err(CorpusError::InvalidReplay(
                "complete-label result play_mode and play_type are inconsistent".to_owned(),
            ));
        }
    }
    if let (LabelState::Known { value: note_count }, LabelState::Known { value: score }) =
        (notes, current_score)
        && u64::from(*score) > 2 * u64::from(*note_count)
    {
        return Err(CorpusError::InvalidReplay(
            "complete-label result current_score exceeds twice the note count".to_owned(),
        ));
    }
    Ok(())
}

fn validate_required_label<T>(
    state: &LabelState<T>,
    field: &str,
    validate_known: impl FnOnce(&T) -> bool,
) -> Result<(), CorpusError> {
    state.validate(field, validate_known)?;
    state.require_applicable(field)
}

fn require_screen_class(frame: &ReplayFrame, expected: ScreenClass) -> Result<(), CorpusError> {
    if frame.screen_class != expected {
        return Err(CorpusError::InvalidReplay(
            "complete-label shape does not match screen_class".to_owned(),
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorpusSplit {
    Train,
    Validation,
    Holdout,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitGroups {
    pub session_sha256: String,
    pub play_sha256: String,
    pub title_sha256: String,
}

impl SplitGroups {
    fn validate(&self) -> Result<(), CorpusError> {
        validate_sha256(
            &self.session_sha256,
            "session group digest",
            ErrorContext::Replay,
        )?;
        validate_sha256(&self.play_sha256, "play group digest", ErrorContext::Replay)?;
        validate_sha256(
            &self.title_sha256,
            "title group digest",
            ErrorContext::Replay,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplaySuiteSummary {
    pub schema: String,
    pub suite_id: String,
    pub corpus_generation_sha256: String,
    pub replay_suite_sha256: String,
    pub index_count: u64,
    pub frame_count: u64,
    pub split_counts: BTreeMap<CorpusSplit, u64>,
}

#[derive(Clone, Copy)]
enum ErrorContext {
    Request,
    Replay,
}

impl ErrorContext {
    fn error(self, detail: impl Into<String>) -> CorpusError {
        match self {
            Self::Request => CorpusError::InvalidRequest(detail.into()),
            Self::Replay => CorpusError::InvalidReplay(detail.into()),
        }
    }
}

fn read_ingest_request(path: &Path) -> Result<IngestRequest, CorpusError> {
    let bytes = read_bounded_regular(path, MAX_REQUEST_BYTES, ErrorContext::Request)?;
    let request: IngestRequest = serde_json::from_slice(&bytes)?;
    request.validate()?;
    Ok(request)
}

fn validate_source_file(path: &Path) -> Result<(), CorpusError> {
    let metadata = path.symlink_metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_SOURCE_BYTES {
        return Err(CorpusError::InvalidRequest(
            "source must be a non-empty bounded regular file".to_owned(),
        ));
    }
    Ok(())
}

fn copy_source(path: &Path, destination: &Path) -> Result<ContentRef, CorpusError> {
    let mut source = File::open(path)?;
    if !source.metadata()?.is_file() {
        return Err(CorpusError::InvalidRequest(
            "source must remain a regular file while ingesting".to_owned(),
        ));
    }
    let mut stored = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or(CorpusError::CapacityExceeded)?;
        if bytes > MAX_SOURCE_BYTES {
            return Err(CorpusError::InvalidRequest(
                "source exceeds the per-object size limit while reading".to_owned(),
            ));
        }
        hasher.update(&buffer[..read]);
        stored.write_all(&buffer[..read])?;
    }
    if bytes == 0 {
        return Err(CorpusError::InvalidRequest(
            "source became empty while ingesting".to_owned(),
        ));
    }
    stored.flush()?;
    Ok(ContentRef {
        sha256: encode_digest(hasher.finalize()),
        bytes,
    })
}

fn ensure_capacity(content_dir: &Path, added_bytes: u64) -> Result<(), CorpusError> {
    let mut count = 0_usize;
    let mut bytes = 0_u64;
    for entry in fs::read_dir(content_dir)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            return Err(CorpusError::InvalidRequest(
                "content store contains a non-UTF-8 entry".to_owned(),
            ));
        };
        if name.starts_with(SOURCE_STAGING_PREFIX) {
            continue;
        }
        if !is_sha256(&name) || !entry.path().symlink_metadata()?.is_dir() {
            return Err(CorpusError::InvalidRequest(
                "content store contains an unrecognized entry".to_owned(),
            ));
        }
        let source = entry.path().join(SOURCE_FILE);
        let metadata = source.symlink_metadata()?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_SOURCE_BYTES {
            return Err(CorpusError::InvalidRequest(
                "content store contains an invalid source object".to_owned(),
            ));
        }
        count = count.checked_add(1).ok_or(CorpusError::CapacityExceeded)?;
        bytes = bytes
            .checked_add(metadata.len())
            .ok_or(CorpusError::CapacityExceeded)?;
    }
    let new_count = count.checked_add(1).ok_or(CorpusError::CapacityExceeded)?;
    let new_bytes = bytes
        .checked_add(added_bytes)
        .ok_or(CorpusError::CapacityExceeded)?;
    if new_count > MAX_SOURCE_OBJECTS || new_bytes > MAX_SOURCE_STORAGE_BYTES {
        return Err(CorpusError::CapacityExceeded);
    }
    Ok(())
}

fn ensure_manifest_capacity(manifest_dir: &Path, added_bytes: usize) -> Result<(), CorpusError> {
    let mut count = 0_usize;
    let mut bytes = 0_u64;
    for entry in fs::read_dir(manifest_dir)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            return Err(CorpusError::InvalidRequest(
                "manifest store contains a non-UTF-8 entry".to_owned(),
            ));
        };
        if name.starts_with(MANIFEST_STAGING_PREFIX) {
            continue;
        }
        let Some(fixture_id) = name.strip_suffix(".json") else {
            return Err(CorpusError::InvalidRequest(
                "manifest store contains an unrecognized entry".to_owned(),
            ));
        };
        validate_opaque_id(fixture_id, "stored fixture ID", ErrorContext::Request)?;
        let metadata = entry.path().symlink_metadata()?;
        if !metadata.is_file() || metadata.len() > MAX_REQUEST_BYTES as u64 {
            return Err(CorpusError::InvalidRequest(
                "manifest store contains an invalid file".to_owned(),
            ));
        }
        count = count.checked_add(1).ok_or(CorpusError::CapacityExceeded)?;
        bytes = bytes
            .checked_add(metadata.len())
            .ok_or(CorpusError::CapacityExceeded)?;
    }
    let new_count = count.checked_add(1).ok_or(CorpusError::CapacityExceeded)?;
    let added_bytes = u64::try_from(added_bytes).map_err(|_| CorpusError::CapacityExceeded)?;
    let new_bytes = bytes
        .checked_add(added_bytes)
        .ok_or(CorpusError::CapacityExceeded)?;
    if new_count > MAX_SOURCE_OBJECTS || new_bytes > MAX_MANIFEST_STORAGE_BYTES {
        return Err(CorpusError::CapacityExceeded);
    }
    Ok(())
}

fn ensure_generation_capacity(
    generation_dir: &Path,
    added_bytes: usize,
) -> Result<(), CorpusError> {
    let mut count = 0_usize;
    let mut bytes = 0_u64;
    for entry in fs::read_dir(generation_dir)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            return Err(CorpusError::InvalidRequest(
                "generation store contains a non-UTF-8 entry".to_owned(),
            ));
        };
        if name.starts_with(GENERATION_STAGING_PREFIX) {
            continue;
        }
        let Some(digest) = name.strip_suffix(".json") else {
            return Err(CorpusError::InvalidRequest(
                "generation store contains an unrecognized entry".to_owned(),
            ));
        };
        if !is_sha256(digest) {
            return Err(CorpusError::InvalidRequest(
                "generation store contains an invalid digest name".to_owned(),
            ));
        }
        let metadata = entry.path().symlink_metadata()?;
        if !metadata.is_file() || metadata.len() > MAX_GENERATION_BYTES as u64 {
            return Err(CorpusError::InvalidRequest(
                "generation store contains an invalid file".to_owned(),
            ));
        }
        count = count.checked_add(1).ok_or(CorpusError::CapacityExceeded)?;
        bytes = bytes
            .checked_add(metadata.len())
            .ok_or(CorpusError::CapacityExceeded)?;
    }
    let new_count = count.checked_add(1).ok_or(CorpusError::CapacityExceeded)?;
    let added_bytes = u64::try_from(added_bytes).map_err(|_| CorpusError::CapacityExceeded)?;
    let new_bytes = bytes
        .checked_add(added_bytes)
        .ok_or(CorpusError::CapacityExceeded)?;
    if new_count > MAX_GENERATIONS || new_bytes > MAX_GENERATION_STORAGE_BYTES {
        return Err(CorpusError::CapacityExceeded);
    }
    Ok(())
}

fn validate_stored_source(directory: &Path, expected: &ContentRef) -> Result<(), CorpusError> {
    if !directory.symlink_metadata()?.is_dir() {
        return Err(CorpusError::InvalidRequest(
            "content-addressed destination is not a directory".to_owned(),
        ));
    }
    validate_private_directory_mode(directory, ErrorContext::Request)?;
    let source = directory.join(SOURCE_FILE);
    let metadata = source.symlink_metadata()?;
    if !metadata.is_file() || metadata.len() != expected.bytes {
        return Err(CorpusError::InvalidRequest(
            "stored source does not match its manifest".to_owned(),
        ));
    }
    if digest_regular_file(&source, MAX_SOURCE_BYTES)? != expected.sha256 {
        return Err(CorpusError::InvalidRequest(
            "stored source digest does not match its content-addressed path".to_owned(),
        ));
    }
    validate_private_file_mode(&source, ErrorContext::Request)?;
    Ok(())
}

fn set_stored_source_permissions(directory: &Path) -> io::Result<()> {
    if !directory.symlink_metadata()?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "stored source directory is not a directory",
        ));
    }
    let source = directory.join(SOURCE_FILE);
    if !source.symlink_metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "stored source entry is not a regular file",
        ));
    }
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    fs::set_permissions(source, fs::Permissions::from_mode(0o600))
}

fn write_atomic_file(
    directory: &Path,
    path: &Path,
    bytes: &[u8],
    staging_prefix: &str,
) -> Result<(), CorpusError> {
    let mut temporary = Builder::new()
        .prefix(staging_prefix)
        .tempfile_in(directory)?;
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    temporary.write_all(bytes)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    File::open(directory)?.sync_all()?;
    Ok(())
}

fn sync_stored_source_and_parent(directory: &Path, content_dir: &Path) -> io::Result<()> {
    File::open(directory.join(SOURCE_FILE))?.sync_all()?;
    File::open(directory)?.sync_all()?;
    File::open(content_dir)?.sync_all()
}

fn sync_file_and_parent(path: &Path, directory: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()?;
    File::open(directory)?.sync_all()
}

fn recover_staging(content_dir: &Path, manifest_dir: &Path) -> Result<(), CorpusError> {
    let mut changed_content = false;
    for entry in fs::read_dir(content_dir)? {
        let entry = entry?;
        let is_staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(SOURCE_STAGING_PREFIX));
        if !is_staging {
            continue;
        }
        if !entry.path().symlink_metadata()?.is_dir() {
            return Err(CorpusError::InvalidRequest(
                "source staging entry is not a directory".to_owned(),
            ));
        }
        fs::remove_dir_all(entry.path())?;
        changed_content = true;
    }
    if changed_content {
        File::open(content_dir)?.sync_all()?;
    }

    let mut changed_manifests = false;
    for entry in fs::read_dir(manifest_dir)? {
        let entry = entry?;
        let is_staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(MANIFEST_STAGING_PREFIX));
        if !is_staging {
            continue;
        }
        if !entry.path().symlink_metadata()?.is_file() {
            return Err(CorpusError::InvalidRequest(
                "manifest staging entry is not a file".to_owned(),
            ));
        }
        fs::remove_file(entry.path())?;
        changed_manifests = true;
    }
    if changed_manifests {
        File::open(manifest_dir)?.sync_all()?;
    }
    Ok(())
}

fn recover_generation_staging(generation_dir: &Path) -> Result<(), CorpusError> {
    let mut changed = false;
    for entry in fs::read_dir(generation_dir)? {
        let entry = entry?;
        let is_staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(GENERATION_STAGING_PREFIX));
        if !is_staging {
            continue;
        }
        if !entry.path().symlink_metadata()?.is_file() {
            return Err(CorpusError::InvalidRequest(
                "generation staging entry is not a file".to_owned(),
            ));
        }
        fs::remove_file(entry.path())?;
        changed = true;
    }
    if changed {
        File::open(generation_dir)?.sync_all()?;
    }
    Ok(())
}

fn preflight_managed_components(root: &Path) -> Result<(), CorpusError> {
    match root.symlink_metadata() {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(CorpusError::InvalidRequest(
                "private store root is not a directory".to_owned(),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }

    for name in ["content", "manifests", "generations", "labels"] {
        match root.join(name).symlink_metadata() {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(CorpusError::InvalidRequest(format!(
                    "managed store component {name:?} is not a directory"
                )));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    match root.join("corpus-ingest.lock").symlink_metadata() {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(CorpusError::InvalidRequest(
            "corpus writer lock is not a regular file".to_owned(),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    let mut missing = Vec::new();
    let mut candidate = path;
    loop {
        match candidate.symlink_metadata() {
            Ok(metadata) if metadata.is_dir() => break,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "corpus ancestor is not a directory",
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
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        sync_directory_and_parent(&directory)?;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    sync_directory_and_parent(path)
}

fn open_store_lock(root: &Path, create: bool) -> Result<File, CorpusError> {
    let path = root.join("corpus-ingest.lock");
    let exists = match path.symlink_metadata() {
        Ok(metadata) if metadata.is_file() => true,
        Ok(_) => {
            return Err(CorpusError::InvalidRequest(
                "corpus writer lock is not a regular file".to_owned(),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && create => false,
        Err(error) => return Err(error.into()),
    };
    let lock = if exists {
        OpenOptions::new().read(true).write(true).open(&path)?
    } else {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?
    };
    if !path.symlink_metadata()?.is_file() || !lock.metadata()?.is_file() {
        return Err(CorpusError::InvalidRequest(
            "corpus writer lock changed while opening".to_owned(),
        ));
    }
    lock.set_permissions(fs::Permissions::from_mode(0o600))?;
    lock.sync_all()?;
    File::open(root)?.sync_all()?;
    Ok(lock)
}

fn sync_directory_and_parent(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn validate_private_directory_mode(path: &Path, context: ErrorContext) -> Result<(), CorpusError> {
    let metadata = path.symlink_metadata()?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o777 != 0o700 {
        return Err(context.error("private store directory must be a mode 0700 directory"));
    }
    Ok(())
}

fn validate_private_file_mode(path: &Path, context: ErrorContext) -> Result<(), CorpusError> {
    let metadata = path.symlink_metadata()?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o777 != 0o600 {
        return Err(context.error("private store file must be a mode 0600 regular file"));
    }
    Ok(())
}

fn read_bounded_regular(
    path: &Path,
    maximum: usize,
    context: ErrorContext,
) -> Result<Vec<u8>, CorpusError> {
    let metadata = path.symlink_metadata()?;
    if !metadata.is_file() || metadata.len() > maximum as u64 {
        return Err(context.error("metadata input is not a bounded regular file"));
    }
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| context.error("metadata input size is not representable"))?;
    let mut bytes = Vec::with_capacity(capacity);
    File::open(path)?
        .take((maximum + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(context.error("metadata input exceeds the size limit while reading"));
    }
    Ok(bytes)
}

fn digest_regular_file(path: &Path, maximum: u64) -> Result<String, CorpusError> {
    let metadata = path.symlink_metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum {
        return Err(CorpusError::InvalidRequest(
            "stored object is not a bounded regular file".to_owned(),
        ));
    }
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or(CorpusError::CapacityExceeded)?;
        if bytes > maximum {
            return Err(CorpusError::InvalidRequest(
                "stored object exceeds the size limit while reading".to_owned(),
            ));
        }
        hasher.update(&buffer[..read]);
    }
    Ok(encode_digest(hasher.finalize()))
}

fn canonical_json(value: &impl Serialize) -> Result<Vec<u8>, CorpusError> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn digest_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    encode_digest(hasher.finalize())
}

fn encode_digest(digest: impl IntoIterator<Item = u8>) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn validate_opaque_id(value: &str, name: &str, context: ErrorContext) -> Result<(), CorpusError> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        });
    if !valid {
        return Err(context.error(format!(
            "{name} must be a 1-64 byte lowercase opaque identifier"
        )));
    }
    Ok(())
}

fn validate_token(value: &str, name: &str, context: ErrorContext) -> Result<(), CorpusError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'+'));
    if !valid {
        return Err(context.error(format!("{name} is not a bounded portable token")));
    }
    Ok(())
}

fn validate_sha256(value: &str, name: &str, context: ErrorContext) -> Result<(), CorpusError> {
    if !is_sha256(value) {
        return Err(context.error(format!(
            "{name} must be 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

fn valid_label_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.chars().any(char::is_control)
        && value.trim() == value
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn validate_source_binding(store: &CorpusStore, index: &ReplayIndex) -> Result<(), CorpusError> {
    let manifest_path = store
        .root
        .join("manifests")
        .join(format!("{}.json", index.fixture_id));
    let bytes = read_bounded_regular(&manifest_path, MAX_REQUEST_BYTES, ErrorContext::Replay)?;
    validate_private_file_mode(&manifest_path, ErrorContext::Replay)?;
    let manifest: SourceManifest = serde_json::from_slice(&bytes)?;
    manifest
        .validate()
        .map_err(|_| CorpusError::InvalidReplay("stored source manifest is invalid".to_owned()))?;
    let canonical = canonical_json(&manifest)?;
    if bytes != canonical {
        return Err(CorpusError::InvalidReplay(
            "stored source manifest is not canonical".to_owned(),
        ));
    }
    if digest_bytes(&canonical) != index.source_manifest_sha256 {
        return Err(CorpusError::InvalidReplay(
            "source_manifest_sha256 does not match the stored manifest".to_owned(),
        ));
    }
    if manifest.fixture_id != index.fixture_id
        || manifest.session_id != index.session_id
        || manifest.profile != index.profile
        || manifest.source != index.source
    {
        return Err(CorpusError::InvalidReplay(
            "replay index does not match its stored source manifest".to_owned(),
        ));
    }
    let destination = store.root.join("content").join(&manifest.source.sha256);
    validate_stored_source(&destination, &manifest.source)
        .map_err(|_| CorpusError::InvalidReplay("stored source object is invalid".to_owned()))
}

fn validate_complete_label(store: &CorpusStore, frame: &ReplayFrame) -> Result<(), CorpusError> {
    let label_dir = store.root.join("labels");
    let path = label_dir.join(format!("{}.json", frame.labels_sha256));
    read_complete_label_object(&path, &frame.labels_sha256)?.validate_for(frame)
}

fn validate_label_store(label_dir: &Path) -> Result<(), CorpusError> {
    let mut count = 0_usize;
    let mut total = 0_u64;
    for entry in fs::read_dir(label_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(CorpusError::InvalidReplay(
                "complete-label store contains a non-UTF-8 entry".to_owned(),
            ));
        };
        let digest = name.strip_suffix(".json").ok_or_else(|| {
            CorpusError::InvalidReplay(
                "complete-label store contains an unrecognized entry".to_owned(),
            )
        })?;
        validate_sha256(digest, "complete-label filename", ErrorContext::Replay)?;
        let metadata = entry.path().symlink_metadata()?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_LABEL_BYTES as u64 {
            return Err(CorpusError::InvalidReplay(
                "complete-label store contains an invalid object".to_owned(),
            ));
        }
        count = count.checked_add(1).ok_or(CorpusError::CapacityExceeded)?;
        total = total
            .checked_add(metadata.len())
            .ok_or(CorpusError::CapacityExceeded)?;
        if count > MAX_LABEL_OBJECTS || total > MAX_LABEL_STORAGE_BYTES {
            return Err(CorpusError::CapacityExceeded);
        }
        read_complete_label_object(&entry.path(), digest)?;
    }
    Ok(())
}

fn read_complete_label_object(
    path: &Path,
    expected_digest: &str,
) -> Result<CompleteLabel, CorpusError> {
    validate_private_file_mode(path, ErrorContext::Replay)?;
    let bytes = read_bounded_regular(path, MAX_LABEL_BYTES, ErrorContext::Replay)?;
    if digest_bytes(&bytes) != expected_digest {
        return Err(CorpusError::InvalidReplay(
            "complete-label digest does not match its path".to_owned(),
        ));
    }
    let label: CompleteLabel = serde_json::from_slice(&bytes)?;
    label.validate_contents()?;
    if canonical_json(&label)? != bytes {
        return Err(CorpusError::InvalidReplay(
            "complete-label document is not canonical".to_owned(),
        ));
    }
    Ok(label)
}

fn read_generation_sources(
    manifest_dir: &Path,
    content_dir: &Path,
) -> Result<Vec<GenerationSource>, CorpusError> {
    let mut entries = fs::read_dir(manifest_dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    let mut sources = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(CorpusError::InvalidRequest(
                "manifest store contains a non-UTF-8 entry".to_owned(),
            ));
        };
        if name.starts_with(MANIFEST_STAGING_PREFIX) {
            continue;
        }
        let fixture_id = name.strip_suffix(".json").ok_or_else(|| {
            CorpusError::InvalidRequest("manifest store contains an unrecognized entry".to_owned())
        })?;
        validate_opaque_id(fixture_id, "stored fixture ID", ErrorContext::Request)?;
        validate_private_file_mode(&entry.path(), ErrorContext::Request)?;
        let bytes = read_bounded_regular(&entry.path(), MAX_REQUEST_BYTES, ErrorContext::Request)?;
        let manifest: SourceManifest = serde_json::from_slice(&bytes)?;
        manifest.validate()?;
        if manifest.fixture_id != fixture_id || canonical_json(&manifest)? != bytes {
            return Err(CorpusError::InvalidRequest(
                "stored source manifest is not canonical or filename-bound".to_owned(),
            ));
        }
        validate_stored_source(&content_dir.join(&manifest.source.sha256), &manifest.source)?;
        sources.push(GenerationSource {
            fixture_id: manifest.fixture_id,
            source_manifest_sha256: digest_bytes(&bytes),
        });
    }
    if sources.is_empty() || sources.len() > MAX_SOURCE_OBJECTS {
        return Err(CorpusError::InvalidRequest(
            "source manifest count is outside the admitted range".to_owned(),
        ));
    }
    Ok(sources)
}

fn load_generation(
    store: &CorpusStore,
    expected_digest: &str,
) -> Result<CorpusGeneration, CorpusError> {
    let path = store
        .root
        .join("generations")
        .join(format!("{expected_digest}.json"));
    validate_private_file_mode(&path, ErrorContext::Replay)?;
    let bytes = read_bounded_regular(&path, MAX_GENERATION_BYTES, ErrorContext::Replay)?;
    if digest_bytes(&bytes) != expected_digest {
        return Err(CorpusError::InvalidReplay(
            "corpus generation digest does not match its path".to_owned(),
        ));
    }
    let generation: CorpusGeneration = serde_json::from_slice(&bytes)?;
    generation.validate()?;
    if canonical_json(&generation)? != bytes {
        return Err(CorpusError::InvalidReplay(
            "corpus generation is not canonical".to_owned(),
        ));
    }
    Ok(generation)
}

fn require_one_split<K: Ord>(
    assignments: &mut BTreeMap<K, CorpusSplit>,
    group: K,
    split: CorpusSplit,
    group_name: &str,
) -> Result<(), CorpusError> {
    match assignments.insert(group, split) {
        Some(existing) if existing != split => Err(CorpusError::InvalidReplay(format!(
            "{group_name} group crosses split boundaries"
        ))),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    use serde_json::json;
    use tempfile::tempdir;

    use super::{CorpusError, CorpusSplit, CorpusStore, SourceManifest, digest_bytes};

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const D: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    const E: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

    #[test]
    fn ingest_is_private_content_addressed_and_idempotent() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let source = temporary.path().join("source.bin");
        let request = temporary.path().join("request.json");
        fs::write(&source, b"synthetic media bytes").unwrap();
        fs::write(
            &request,
            serde_json::to_vec(&json!({
                "schema": "scorepeek-private-corpus-ingest-v1",
                "fixture_id": "fixture-001",
                "session_id": "session-001",
                "profile": {
                    "kind": "windows_semantic_reference",
                    "recording_profile_id": "windows-vm-fhd-v1"
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let store = CorpusStore::new(&root);
        let first = store.ingest(&source, &request).unwrap();
        let second = store.ingest(&source, &request).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.source.bytes, 21);
        let stored = root
            .join("content")
            .join(&first.source.sha256)
            .join("source.media");
        assert_eq!(fs::read(&stored).unwrap(), b"synthetic media bytes");
        assert_eq!(root.metadata().unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(
            stored.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            root.join("manifests/fixture-001.json")
                .metadata()
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn fixture_id_cannot_be_rebound() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let source = temporary.path().join("source.bin");
        let request = temporary.path().join("request.json");
        fs::write(&source, b"first source").unwrap();
        write_request(&request, "windows-vm-fhd-v1");
        let store = CorpusStore::new(&root);
        store.ingest(&source, &request).unwrap();

        fs::write(&source, b"second source").unwrap();
        let error = store.ingest(&source, &request).unwrap_err();
        assert!(matches!(error, CorpusError::FixtureConflict));
        assert_eq!(fs::read_dir(root.join("content")).unwrap().count(), 1);
    }

    #[test]
    fn replay_suite_binds_the_stored_manifest_and_is_canonical() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        let manifest = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-001",
            "session-001",
            "windows-vm-fhd-v1",
            b"synthetic replay source",
        );
        let generation = store.seal_generation("generation-001").unwrap();
        let value = replay_suite_value(
            &generation.corpus_generation_sha256,
            &[replay_index_value(&manifest, "train", C, &root)],
        );
        let suite = temporary.path().join("suite.json");
        fs::write(&suite, serde_json::to_vec(&value).unwrap()).unwrap();
        let first_summary = store.validate_replay_suite(&suite).unwrap();
        fs::write(&suite, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        let second_summary = store.validate_replay_suite(&suite).unwrap();
        assert_eq!(first_summary, second_summary);
        assert_eq!(
            first_summary.schema,
            "scorepeek-private-corpus-replay-suite-summary-v1"
        );
        assert_eq!(
            first_summary.corpus_generation_sha256,
            generation.corpus_generation_sha256
        );
        assert_eq!(first_summary.frame_count, 2);
        assert_eq!(first_summary.index_count, 1);
        assert_eq!(first_summary.split_counts[&CorpusSplit::Train], 2);

        let mut mismatched = value;
        mismatched["indexes"][0]["source_manifest_sha256"] = json!(D);
        fs::write(&suite, serde_json::to_vec(&mismatched).unwrap()).unwrap();
        let error = store.validate_replay_suite(&suite).unwrap_err();
        assert!(error.to_string().contains("not a member"));
    }

    #[test]
    fn replay_suite_rejects_session_episode_and_profile_split_leaks() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        let manifest = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-001",
            "session-001",
            "windows-vm-fhd-v1",
            b"synthetic replay source",
        );
        let generation = store.seal_generation("generation-001").unwrap();
        let mut index = replay_index_value(&manifest, "train", C, &root);
        index["frames"][1]["split"] = json!("holdout");
        index["frames"][1]["groups"] = json!({
            "session_sha256": D,
            "play_sha256": E,
            "title_sha256": A
        });
        let suite_value = replay_suite_value(&generation.corpus_generation_sha256, &[index]);
        let suite = temporary.path().join("suite.json");
        fs::write(&suite, serde_json::to_vec(&suite_value).unwrap()).unwrap();
        let error = store.validate_replay_suite(&suite).unwrap_err();
        assert!(error.to_string().contains("session ID group crosses"));
    }

    #[test]
    fn replay_suite_rejects_cross_index_title_leaks() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        let first = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-001",
            "session-001",
            "windows-vm-fhd-v1",
            b"first replay source",
        );
        let second = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-002",
            "session-002",
            "windows-vm-fhd-v2",
            b"second replay source",
        );
        let generation = store.seal_generation("generation-001").unwrap();
        let subset = replay_suite_value(
            &generation.corpus_generation_sha256,
            &[replay_index_value(&first, "train", C, &root)],
        );
        let suite = temporary.path().join("suite.json");
        fs::write(&suite, serde_json::to_vec(&subset).unwrap()).unwrap();
        let error = store.validate_replay_suite(&suite).unwrap_err();
        assert!(error.to_string().contains("does not completely cover"));

        let suite_value = replay_suite_value(
            &generation.corpus_generation_sha256,
            &[
                replay_index_value(&first, "train", C, &root),
                replay_index_value(&second, "holdout", C, &root),
            ],
        );
        fs::write(&suite, serde_json::to_vec(&suite_value).unwrap()).unwrap();
        let error = store.validate_replay_suite(&suite).unwrap_err();
        assert!(error.to_string().contains("title group crosses"));
    }

    #[test]
    fn replay_suite_rejects_cross_session_episode_leaks() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        let first = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-001",
            "session-001",
            "windows-vm-fhd-v1",
            b"first replay source",
        );
        let second = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-002",
            "session-002",
            "windows-vm-fhd-v2",
            b"second replay source",
        );
        let generation = store.seal_generation("generation-001").unwrap();
        let mut first_index = replay_index_value(&first, "train", C, &root);
        let mut second_index = replay_index_value(&second, "holdout", D, &root);
        for frame in first_index["frames"].as_array_mut().unwrap() {
            frame["episode_id"] = json!("shared-episode");
        }
        for frame in second_index["frames"].as_array_mut().unwrap() {
            frame["episode_id"] = json!("shared-episode");
        }
        let suite_value = replay_suite_value(
            &generation.corpus_generation_sha256,
            &[first_index, second_index],
        );
        let suite = temporary.path().join("suite.json");
        fs::write(&suite, serde_json::to_vec(&suite_value).unwrap()).unwrap();
        let error = store.validate_replay_suite(&suite).unwrap_err();
        assert!(error.to_string().contains("episode group crosses"));
    }

    #[test]
    fn replay_suite_rejects_noncanonical_index_order() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        let first = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-001",
            "session-001",
            "windows-vm-fhd-v1",
            b"first replay source",
        );
        let second = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-002",
            "session-002",
            "windows-vm-fhd-v2",
            b"second replay source",
        );
        let generation = store.seal_generation("generation-001").unwrap();
        let suite_value = replay_suite_value(
            &generation.corpus_generation_sha256,
            &[
                replay_index_value(&second, "train", D, &root),
                replay_index_value(&first, "train", C, &root),
            ],
        );
        let suite = temporary.path().join("suite.json");
        fs::write(&suite, serde_json::to_vec(&suite_value).unwrap()).unwrap();
        let error = store.validate_replay_suite(&suite).unwrap_err();
        assert!(error.to_string().contains("uniquely ordered"));
    }

    #[test]
    fn replay_suite_rejects_noncanonical_digest_and_decode_order() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        let manifest = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-001",
            "session-001",
            "windows-vm-fhd-v1",
            b"synthetic replay source",
        );
        let generation = store.seal_generation("generation-001").unwrap();
        let mut value = replay_suite_value(
            &generation.corpus_generation_sha256,
            &[replay_index_value(&manifest, "train", C, &root)],
        );
        value["indexes"][0]["frames"][0]["frame_sha256"] = json!(A.to_uppercase());
        let suite = temporary.path().join("suite.json");
        fs::write(&suite, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(store.validate_replay_suite(&suite).is_err());

        let mut value = replay_suite_value(
            &generation.corpus_generation_sha256,
            &[replay_index_value(&manifest, "train", C, &root)],
        );
        value["indexes"][0]["frames"][1]["decode_index"] = json!(0);
        fs::write(&suite, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(store.validate_replay_suite(&suite).is_err());
    }

    #[test]
    fn replay_suite_rejects_a_complete_label_for_another_shape() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        let manifest = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-001",
            "session-001",
            "windows-vm-fhd-v1",
            b"synthetic replay source",
        );
        let generation = store.seal_generation("generation-001").unwrap();
        let mut index = replay_index_value(&manifest, "train", C, &root);
        let frame_id = index["frames"][0]["frame_id"].as_str().unwrap();
        let wrong_shape = write_label(
            &root,
            json!({
                "shape": "music_select",
                "schema": "scorepeek-private-complete-label-v1",
                "frame_id": frame_id,
                "annotation_revision": "labels-v1",
                "screen_state": { "state": "known", "value": true },
                "play_mode": { "state": "known", "value": "single_play" },
                "song_id": { "state": "unknown", "reason": "ambiguous title" },
                "selected_difficulty": { "state": "known", "value": "another" },
                "selected_level": { "state": "known", "value": 12 }
            }),
        );
        index["frames"][0]["labels_sha256"] = json!(wrong_shape);
        let suite_value = replay_suite_value(&generation.corpus_generation_sha256, &[index]);
        let suite = temporary.path().join("suite.json");
        fs::write(&suite, serde_json::to_vec(&suite_value).unwrap()).unwrap();

        let error = store.validate_replay_suite(&suite).unwrap_err();
        assert!(error.to_string().contains("shape does not match"));
    }

    #[test]
    fn replay_suite_rejects_inconsistent_result_label_fields() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        let manifest = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-001",
            "session-001",
            "windows-vm-fhd-v1",
            b"synthetic replay source",
        );
        let generation = store.seal_generation("generation-001").unwrap();
        let mut index = replay_index_value(&manifest, "train", C, &root);
        let frame_id = index["frames"][0]["frame_id"].as_str().unwrap();
        let mut label = json!({
            "shape": "result",
            "schema": "scorepeek-private-complete-label-v1",
            "frame_id": frame_id,
            "annotation_revision": "labels-v1",
            "screen_state": { "state": "known", "value": true },
            "savable": { "state": "known", "value": true },
            "playside": { "state": "known", "value": "one_player" },
            "play_mode": { "state": "known", "value": "single_play" },
            "play_type": { "state": "known", "value": "double_battle" },
            "song_id": { "state": "known", "value": "synthetic-song-001" },
            "difficulty": { "state": "known", "value": "another" },
            "level": { "state": "known", "value": 12 },
            "notes": { "state": "known", "value": 1000 },
            "current_score": { "state": "known", "value": 3000 }
        });
        let inconsistent = write_label(&root, label.clone());
        index["frames"][0]["labels_sha256"] = json!(inconsistent);
        let suite_value =
            replay_suite_value(&generation.corpus_generation_sha256, &[index.clone()]);
        let suite = temporary.path().join("suite.json");
        fs::write(&suite, serde_json::to_vec(&suite_value).unwrap()).unwrap();

        let error = store.validate_replay_suite(&suite).unwrap_err();
        assert!(error.to_string().contains("inconsistent"));

        fs::remove_file(root.join("labels").join(format!("{inconsistent}.json"))).unwrap();
        label["play_type"]["value"] = json!("single");
        let excessive_score = write_label(&root, label);
        index["frames"][0]["labels_sha256"] = json!(excessive_score);
        let suite_value = replay_suite_value(&generation.corpus_generation_sha256, &[index]);
        fs::write(&suite, serde_json::to_vec(&suite_value).unwrap()).unwrap();

        let error = store.validate_replay_suite(&suite).unwrap_err();
        assert!(error.to_string().contains("exceeds twice"));
    }

    #[test]
    fn replay_suite_rejects_an_invalid_unreferenced_label_object() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        let manifest = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-001",
            "session-001",
            "windows-vm-fhd-v1",
            b"synthetic replay source",
        );
        let generation = store.seal_generation("generation-001").unwrap();
        let index = replay_index_value(&manifest, "train", C, &root);
        let suite_value = replay_suite_value(&generation.corpus_generation_sha256, &[index]);
        let suite = temporary.path().join("suite.json");
        fs::write(&suite, serde_json::to_vec(&suite_value).unwrap()).unwrap();
        let unreferenced = root.join("labels").join(format!("{A}.json"));
        fs::write(&unreferenced, b"{}\n").unwrap();
        fs::set_permissions(&unreferenced, fs::Permissions::from_mode(0o600)).unwrap();

        let error = store.validate_replay_suite(&suite).unwrap_err();
        assert!(error.to_string().contains("digest does not match"));
    }

    #[test]
    fn digest_is_stable() {
        assert_eq!(
            digest_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn metadata_store_limits_reject_new_bindings_and_generations() {
        let temporary = tempdir().unwrap();
        let manifests = temporary.path().join("manifests");
        let generations = temporary.path().join("generations");
        fs::create_dir(&manifests).unwrap();
        fs::create_dir(&generations).unwrap();
        for index in 0..super::MAX_SOURCE_OBJECTS {
            fs::write(manifests.join(format!("fixture-{index:04}.json")), b"{}\n").unwrap();
        }
        assert!(matches!(
            super::ensure_manifest_capacity(&manifests, 3),
            Err(CorpusError::CapacityExceeded)
        ));

        for index in 0..super::MAX_GENERATIONS {
            fs::write(generations.join(format!("{index:064x}.json")), b"{}\n").unwrap();
        }
        assert!(matches!(
            super::ensure_generation_capacity(&generations, 3),
            Err(CorpusError::CapacityExceeded)
        ));
    }

    #[test]
    fn manifest_capacity_failure_does_not_publish_an_orphan_source() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        ingest_fixture(
            temporary.path(),
            &store,
            "fixture-0000",
            "session-0000",
            "windows-vm-fhd-v1",
            b"first source",
        );
        let manifests = root.join("manifests");
        for index in 1..super::MAX_SOURCE_OBJECTS {
            fs::write(manifests.join(format!("fixture-{index:04}.json")), b"{}\n").unwrap();
        }
        let source = temporary.path().join("overflow.media");
        let request = temporary.path().join("overflow.json");
        fs::write(&source, b"unbound source").unwrap();
        write_request_for(
            &request,
            "fixture-overflow",
            "session-overflow",
            "windows-vm-fhd-v1",
        );
        let before = fs::read_dir(root.join("content")).unwrap().count();
        assert!(matches!(
            store.ingest(source, request),
            Err(CorpusError::CapacityExceeded)
        ));
        assert_eq!(fs::read_dir(root.join("content")).unwrap().count(), before);
    }

    #[test]
    fn ingest_rejects_symlinked_store_components_and_lock() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("source.media");
        let request = temporary.path().join("request.json");
        fs::write(&source, b"synthetic source").unwrap();
        write_request_for(&request, "fixture-001", "session-001", "windows-vm-fhd-v1");

        let target = temporary.path().join("target");
        let alias = temporary.path().join("alias");
        fs::create_dir(&target).unwrap();
        symlink(&target, &alias).unwrap();
        assert!(CorpusStore::new(&alias).ingest(&source, &request).is_err());

        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        let manifest = store.ingest(&source, &request).unwrap();
        let stored_source = root
            .join("content")
            .join(&manifest.source.sha256)
            .join("source.media");
        let outside_media = temporary.path().join("outside-media");
        fs::write(&outside_media, b"outside").unwrap();
        fs::set_permissions(&outside_media, fs::Permissions::from_mode(0o644)).unwrap();
        fs::remove_file(&stored_source).unwrap();
        symlink(&outside_media, &stored_source).unwrap();
        assert!(store.ingest(&source, &request).is_err());
        assert_eq!(
            outside_media.metadata().unwrap().permissions().mode() & 0o777,
            0o644
        );

        fs::remove_file(root.join("corpus-ingest.lock")).unwrap();
        let sentinel = temporary.path().join("sentinel");
        fs::write(&sentinel, b"sentinel").unwrap();
        symlink(&sentinel, root.join("corpus-ingest.lock")).unwrap();
        assert!(store.ingest(&source, &request).is_err());
    }

    #[test]
    fn managed_component_preflight_rejects_without_mutating_the_store() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        ingest_fixture(
            temporary.path(),
            &store,
            "fixture-001",
            "session-001",
            "windows-vm-fhd-v1",
            b"first source",
        );
        let outside = temporary.path().join("outside-generations");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, root.join("generations")).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        let content_count = fs::read_dir(root.join("content")).unwrap().count();
        let manifest_count = fs::read_dir(root.join("manifests")).unwrap().count();

        let source = temporary.path().join("fixture-002.media");
        let request = temporary.path().join("fixture-002.json");
        fs::write(&source, b"second source").unwrap();
        write_request_for(&request, "fixture-002", "session-002", "windows-vm-fhd-v1");
        assert!(store.ingest(&source, &request).is_err());

        assert_eq!(root.metadata().unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(
            fs::read_dir(root.join("content")).unwrap().count(),
            content_count
        );
        assert_eq!(
            fs::read_dir(root.join("manifests")).unwrap().count(),
            manifest_count
        );
        assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
    }

    fn write_request(path: &std::path::Path, profile: &str) {
        write_request_for(path, "fixture-001", "session-001", profile);
    }

    fn write_request_for(
        path: &std::path::Path,
        fixture_id: &str,
        session_id: &str,
        profile: &str,
    ) {
        fs::write(
            path,
            serde_json::to_vec(&json!({
                "schema": "scorepeek-private-corpus-ingest-v1",
                "fixture_id": fixture_id,
                "session_id": session_id,
                "profile": {
                    "kind": "windows_semantic_reference",
                    "recording_profile_id": profile
                }
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn ingest_fixture(
        directory: &std::path::Path,
        store: &CorpusStore,
        fixture_id: &str,
        session_id: &str,
        profile: &str,
        bytes: &[u8],
    ) -> SourceManifest {
        let source = directory.join(format!("{fixture_id}.media"));
        let request = directory.join(format!("{fixture_id}.json"));
        fs::write(&source, bytes).unwrap();
        fs::write(
            &request,
            serde_json::to_vec(&json!({
                "schema": "scorepeek-private-corpus-ingest-v1",
                "fixture_id": fixture_id,
                "session_id": session_id,
                "profile": {
                    "kind": "windows_semantic_reference",
                    "recording_profile_id": profile
                }
            }))
            .unwrap(),
        )
        .unwrap();
        store.ingest(source, request).unwrap()
    }

    fn replay_suite_value(
        corpus_generation_sha256: &str,
        indexes: &[serde_json::Value],
    ) -> serde_json::Value {
        json!({
            "schema": "scorepeek-private-corpus-replay-suite-v1",
            "suite_id": "suite-001",
            "corpus_generation_sha256": corpus_generation_sha256,
            "indexes": indexes
        })
    }

    fn replay_index_value(
        manifest: &SourceManifest,
        split: &str,
        title_sha256: &str,
        root: &std::path::Path,
    ) -> serde_json::Value {
        let fixture_id = &manifest.fixture_id;
        let first_frame = format!("{fixture_id}-frame-001");
        let second_frame = format!("{fixture_id}-frame-002");
        let episode = format!("{fixture_id}-episode-001");
        let (first_digest, second_digest, session_digest, play_digest) =
            if fixture_id == "fixture-001" {
                (A, B, A, B)
            } else {
                (D, E, D, E)
            };
        let first_label = write_label(
            root,
            json!({
                "shape": "result",
                "schema": "scorepeek-private-complete-label-v1",
                "frame_id": first_frame,
                "annotation_revision": "labels-v1",
                "screen_state": { "state": "known", "value": true },
                "savable": { "state": "known", "value": true },
                "playside": { "state": "known", "value": "one_player" },
                "play_mode": { "state": "known", "value": "single_play" },
                "play_type": { "state": "known", "value": "single" },
                "song_id": { "state": "known", "value": "synthetic-song-001" },
                "difficulty": { "state": "known", "value": "another" },
                "level": { "state": "known", "value": 12 },
                "notes": { "state": "known", "value": 1000 },
                "current_score": { "state": "known", "value": 1800 }
            }),
        );
        let second_label = write_label(
            root,
            json!({
                "shape": "non_recognition",
                "schema": "scorepeek-private-complete-label-v1",
                "frame_id": second_frame,
                "annotation_revision": "labels-v1",
                "screen_class": "transition"
            }),
        );
        json!({
            "schema": "scorepeek-private-corpus-replay-v1",
            "fixture_id": fixture_id,
            "session_id": manifest.session_id,
            "profile": manifest.profile,
            "source": manifest.source,
            "source_manifest_sha256": manifest.summary().unwrap().source_manifest_sha256,
            "extractor": {
                "tool_id": "ffmpeg",
                "tool_version": "8.0.0",
                "extractor_manifest_sha256": A,
                "parameters_sha256": B
            },
            "source_time_base": { "numerator": 1, "denominator": 60000 },
            "frames": [
                {
                    "frame_id": first_frame,
                    "source_pts": 1001,
                    "decode_index": 0,
                    "frame_sha256": first_digest,
                    "episode_id": episode,
                    "screen_class": "result",
                    "split": split,
                    "groups": {
                        "session_sha256": session_digest,
                        "play_sha256": play_digest,
                        "title_sha256": title_sha256
                    },
                    "annotation_revision": "labels-v1",
                    "labels_sha256": first_label
                },
                {
                    "frame_id": second_frame,
                    "source_pts": 2002,
                    "decode_index": 1,
                    "frame_sha256": second_digest,
                    "episode_id": episode,
                    "screen_class": "transition",
                    "split": split,
                    "groups": {
                        "session_sha256": session_digest,
                        "play_sha256": play_digest,
                        "title_sha256": title_sha256
                    },
                    "annotation_revision": "labels-v1",
                    "labels_sha256": second_label
                }
            ]
        })
    }

    fn write_label(root: &std::path::Path, value: serde_json::Value) -> String {
        let label: super::CompleteLabel = serde_json::from_value(value).unwrap();
        let bytes = super::canonical_json(&label).unwrap();
        let digest = digest_bytes(&bytes);
        let path = root.join("labels").join(format!("{digest}.json"));
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        digest
    }
}
