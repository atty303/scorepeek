use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::Builder;

#[allow(
    dead_code,
    reason = "removed video and v2 conversion authoring has no exported or CLI route"
)]
mod frame_corpus;
mod media;
mod music_list;
mod music_select_motion;
mod temporal_evaluation;
pub use frame_corpus::{
    CorpusReplaySummary, DiagnosticImportSummary, DiagnosticVerificationSummary,
    NumericDatasetAuthoringSummary, NumericSentinelAuthoringSummary, ReviewApplySummary,
    apply_review, author_numeric_dataset, author_numeric_sentinel, import_diagnostic,
    inspect_review, replay_corpus, verify_diagnostic,
};
pub use media::{CanonicalFrameExtractionSummary, FrameExtractionSummary, MediaProbeSummary};
pub use music_list::{
    MusicListMotionReviewApplySummary, MusicListMotionReviewPlanSummary, MusicListMotionSummary,
    MusicListRowObservationSummary, apply_music_list_motion_review,
    inspect_music_list_row_observation_draft, measure_music_list_motion,
    plan_music_list_motion_review, verify_music_list_motion,
    verify_music_list_row_observation_draft,
};
pub use music_select_motion::{
    MusicSelectCorrectnessEvaluationSummary, MusicSelectDwellEvaluationSummary,
    MusicSelectDwellPolicy, MusicSelectMotionReviewApplySummary, MusicSelectMotionReviewSummary,
    MusicSelectTemporalCandidatePolicy, apply_music_select_motion_review,
    evaluate_music_select_correctness, evaluate_music_select_dwell,
    plan_music_select_motion_review,
};
pub use temporal_evaluation::{
    TemporalEvaluationPolicy, TemporalEvaluationSummary, evaluate_temporal_corpus,
};

const INGEST_REQUEST_SCHEMA: &str = "scorepeek-private-corpus-ingest-v2";
const INGEST_SUMMARY_SCHEMA: &str = "scorepeek-private-corpus-ingest-summary-v2";
const SOURCE_MANIFEST_SCHEMA: &str = "scorepeek-private-corpus-source-v2";
const GENERATION_SCHEMA: &str = "scorepeek-private-corpus-generation-v1";
const GENERATION_SUMMARY_SCHEMA: &str = "scorepeek-private-corpus-generation-summary-v1";
const REPLAY_INDEX_SCHEMA: &str = "scorepeek-private-corpus-replay-v2";
const REPLAY_SUITE_SCHEMA: &str = "scorepeek-private-corpus-replay-suite-v2";
const REPLAY_SUITE_SUMMARY_SCHEMA: &str = "scorepeek-private-corpus-replay-suite-summary-v2";
const COMPLETE_LABEL_SCHEMA: &str = "scorepeek-private-complete-label-v1";
const COMPLETE_LABEL_SUMMARY_SCHEMA: &str = "scorepeek-private-complete-label-summary-v1";
const INDEX_PLAN_SCHEMA: &str = "scorepeek-private-corpus-index-plan-v2";
const INDEX_SUMMARY_SCHEMA: &str = "scorepeek-private-corpus-index-summary-v2";
const SYNTHETIC_TITLE_REQUEST_SCHEMA: &str = "scorepeek-synthetic-title-request-v1";
const SYNTHETIC_TITLE_MANIFEST_SCHEMA: &str = "scorepeek-synthetic-title-set-v1";
const SYNTHETIC_TITLE_SUMMARY_SCHEMA: &str = "scorepeek-synthetic-title-summary-v1";
const CANONICAL_FRAME_CONTRACT_ID: &str = "scorepeek-canonical-rgb8-1920x1080-v1";
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
const MAX_REPLAY_INDEX_STORAGE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_LABEL_BYTES: usize = 64 * 1024;
const MAX_LABEL_OBJECTS: usize = 250_000;
const MAX_LABEL_STORAGE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const SOURCE_FILE: &str = "source.media";
const EXTERNAL_SOURCE_FILE: &str = "source.external.json";
const EXTERNAL_SOURCE_SCHEMA: &str = "scorepeek-private-external-source-v1";
const SOURCE_STAGING_PREFIX: &str = ".corpus-source-staging-";
const MANIFEST_STAGING_PREFIX: &str = ".corpus-manifest-staging-";
const GENERATION_STAGING_PREFIX: &str = ".corpus-generation-staging-";
const LABEL_STAGING_PREFIX: &str = ".corpus-label-staging-";
const INDEX_STAGING_PREFIX: &str = ".corpus-index-staging-";
const SYNTHETIC_WIDTH: usize = 512;
const SYNTHETIC_HEIGHT: usize = 96;
const MAX_SYNTHETIC_SAMPLES: usize = 256;

#[derive(Debug)]
pub enum CorpusError {
    Io(io::Error),
    Json(serde_json::Error),
    InvalidRequest(String),
    InvalidReplay(String),
    InvalidMedia(String),
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
            Self::InvalidMedia(detail) => write!(formatter, "invalid private media: {detail}"),
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
            | Self::InvalidMedia(_)
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalFrameBinding {
    pub normalizer_artifact_sha256: String,
    pub canonical_frame_contract_id: String,
    pub canonical_layout_sha256: String,
}

impl CanonicalFrameBinding {
    fn validate(&self, context: ErrorContext) -> Result<(), CorpusError> {
        validate_sha256(
            &self.normalizer_artifact_sha256,
            "normalizer_artifact_sha256",
            context,
        )?;
        if self.canonical_frame_contract_id != CANONICAL_FRAME_CONTRACT_ID {
            return Err(context.error(format!(
                "canonical frame contract must be {CANONICAL_FRAME_CONTRACT_ID}"
            )));
        }
        validate_sha256(
            &self.canonical_layout_sha256,
            "canonical_layout_sha256",
            context,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct IngestRequest {
    schema: String,
    fixture_id: String,
    session_id: String,
    capture_profile_id: String,
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
        validate_token(
            &self.capture_profile_id,
            "capture_profile_id",
            ErrorContext::Request,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentRef {
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExternalSourceLocator {
    schema: String,
    source: ContentRef,
    path: String,
}

impl ExternalSourceLocator {
    fn validate(&self) -> Result<PathBuf, CorpusError> {
        if self.schema != EXTERNAL_SOURCE_SCHEMA {
            return Err(CorpusError::InvalidRequest(
                "unsupported external source locator schema".to_owned(),
            ));
        }
        self.source.validate(ErrorContext::Request)?;
        let path = PathBuf::from(&self.path);
        if !path.is_absolute() || self.path.is_empty() {
            return Err(CorpusError::InvalidRequest(
                "external source path must be absolute".to_owned(),
            ));
        }
        Ok(path)
    }
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
    pub capture_profile_id: String,
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
            capture_profile_sha256: digest_bytes(self.capture_profile_id.as_bytes()),
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
        validate_token(
            &self.capture_profile_id,
            "capture_profile_id",
            ErrorContext::Request,
        )?;
        self.source.validate(ErrorContext::Request)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IngestSummary {
    pub schema: String,
    pub fixture_id: String,
    pub capture_profile_sha256: String,
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

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayIndexPlan {
    schema: String,
    fixture_id: String,
    source_manifest_sha256: String,
    extractor: ExtractorIdentity,
    canonical_frame: CanonicalFrameBinding,
    source_time_base: TimeBase,
    frames: Vec<ReplayIndexPlanFrame>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayIndexPlanFrame {
    frame_id: String,
    source_pts: i64,
    decode_index: u64,
    frame_sha256: String,
    episode_sha256: String,
    screen_class: ScreenClass,
    split: CorpusSplit,
    groups: SplitGroups,
    annotation_revision: String,
    labels_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplayIndexSummary {
    pub schema: String,
    pub fixture_id: String,
    pub replay_index_sha256: String,
    pub frame_count: u64,
    pub episode_count: u64,
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

    #[allow(dead_code)]
    fn register_external_source(
        &self,
        source_path: &Path,
        source: &ContentRef,
    ) -> Result<PathBuf, CorpusError> {
        self.validate_root()?;
        source.validate(ErrorContext::Request)?;
        let canonical_path = fs::canonicalize(source_path)?;
        validate_external_source_file(&canonical_path, source, false)?;
        let path = canonical_path.to_str().ok_or_else(|| {
            CorpusError::InvalidRequest("external source path must be UTF-8".to_owned())
        })?;
        let locator = ExternalSourceLocator {
            schema: EXTERNAL_SOURCE_SCHEMA.to_owned(),
            source: source.clone(),
            path: path.to_owned(),
        };
        let locator_bytes = canonical_json(&locator)?;

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

        let destination = content_dir.join(&source.sha256);
        match destination.symlink_metadata() {
            Ok(entry_metadata) => {
                let metadata = destination.metadata()?;
                if !metadata.is_dir() {
                    return Err(CorpusError::InvalidRequest(
                        "content-addressed destination is not a directory".to_owned(),
                    ));
                }
                let stored_media = destination.join(SOURCE_FILE);
                if stored_media.exists() {
                    let resolved = resolve_stored_source_path(&destination, source)?;
                    drop(lock);
                    return Ok(resolved);
                }
                let (existing, existing_path) = read_external_source_locator(&destination, source)?;
                if existing == locator
                    && existing_path == canonical_path
                    && validate_external_source_file(&canonical_path, source, false).is_ok()
                {
                    drop(lock);
                    return Ok(canonical_path);
                }
                if entry_metadata.file_type().is_symlink() {
                    return Err(CorpusError::InvalidRequest(
                        "symlinked content destination cannot be updated".to_owned(),
                    ));
                }
                replace_atomic_file(
                    &destination,
                    &destination.join(EXTERNAL_SOURCE_FILE),
                    &locator_bytes,
                    SOURCE_STAGING_PREFIX,
                )?;
                sync_stored_source_and_parent(&destination, &content_dir)?;
                drop(lock);
                return Ok(canonical_path);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        ensure_capacity(&content_dir, source.bytes)?;
        let staging = Builder::new()
            .prefix(SOURCE_STAGING_PREFIX)
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(&content_dir)?;
        let locator_path = staging.path().join(EXTERNAL_SOURCE_FILE);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&locator_path)?;
        file.write_all(&locator_bytes)?;
        file.flush()?;
        file.sync_all()?;
        File::open(staging.path())?.sync_all()?;
        let staging_path = staging.keep();
        fs::rename(staging_path, &destination)?;
        sync_stored_source_and_parent(&destination, &content_dir)?;
        drop(lock);
        Ok(canonical_path)
    }

    fn resolve_source_path(&self, source: &ContentRef) -> Result<PathBuf, CorpusError> {
        resolve_stored_source_path(&self.root.join("content").join(&source.sha256), source)
    }

    fn open_verified_source(&self, source: &ContentRef) -> Result<File, CorpusError> {
        let path = resolve_stored_source_path_unverified(
            &self.root.join("content").join(&source.sha256),
            source,
        )?;
        let mut file = File::open(path)?;
        verify_open_source(&mut file, source)?;
        Ok(file)
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
        let request = read_ingest_request(request_path.as_ref())?;
        self.ingest_bound(source_path.as_ref(), None, |_| Ok(request), |_| Ok(()))
            .map(|(manifest, ())| manifest)
    }

    #[allow(dead_code)]
    fn ingest_verified_recording_with<T>(
        &self,
        source_path: &Path,
        capture_profile_id: String,
        expected_source: &ContentRef,
        inspect_staged_source: impl FnOnce(&Path) -> Result<T, CorpusError>,
    ) -> Result<(SourceManifest, T), CorpusError> {
        self.ingest_bound(
            source_path,
            Some(expected_source),
            move |source| {
                Ok(IngestRequest {
                    schema: INGEST_REQUEST_SCHEMA.to_owned(),
                    fixture_id: source.sha256.clone(),
                    session_id: source.sha256.clone(),
                    capture_profile_id,
                })
            },
            inspect_staged_source,
        )
    }

    fn ingest_bound<T>(
        &self,
        source_path: &Path,
        expected_source: Option<&ContentRef>,
        request_for_source: impl FnOnce(&ContentRef) -> Result<IngestRequest, CorpusError>,
        inspect_staged_source: impl FnOnce(&Path) -> Result<T, CorpusError>,
    ) -> Result<(SourceManifest, T), CorpusError> {
        self.validate_root()?;
        validate_source_file(source_path)?;

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
        let source = copy_source(source_path, &staged_source, expected_source)?;
        let request = request_for_source(&source)?;
        request.validate()?;
        let staged_inspection = inspect_staged_source(&staged_source)?;
        File::open(&staged_source)?.sync_all()?;
        File::open(staging.path())?.sync_all()?;

        let manifest = SourceManifest {
            schema: SOURCE_MANIFEST_SCHEMA.to_owned(),
            fixture_id: request.fixture_id,
            session_id: request.session_id,
            capture_profile_id: request.capture_profile_id,
            source,
        };
        let manifest_bytes = canonical_json(&manifest)?;
        let manifest_path = manifest_dir.join(format!("{}.json", manifest.fixture_id));
        let manifest_exists = match manifest_path.metadata() {
            Ok(_) => {
                if read_bounded_regular(&manifest_path, MAX_REQUEST_BYTES, ErrorContext::Request)?
                    != manifest_bytes
                {
                    return Err(CorpusError::FixtureConflict);
                }
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };

        let destination = content_dir.join(&manifest.source.sha256);
        let destination_exists = match destination.metadata() {
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
        Ok((manifest, staged_inspection))
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
        validate_directory(&self.root, ErrorContext::Request)?;
        let content_dir = self.root.join("content");
        let manifest_dir = self.root.join("manifests");
        validate_directory(&content_dir, ErrorContext::Request)?;
        validate_directory(&manifest_dir, ErrorContext::Request)?;
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

    /// Validates and durably publishes one canonical complete-label document in the private
    /// content-addressed label store.
    ///
    /// The returned summary contains only opaque identifiers, a non-personal shape class, and
    /// content evidence. Complete field values are never returned.
    ///
    /// # Errors
    ///
    /// Returns an error if the label is malformed, the private store is unavailable or damaged,
    /// the label capacity is exhausted, or durable publication fails.
    pub fn author_complete_label(
        &self,
        label_path: impl AsRef<Path>,
    ) -> Result<CompleteLabelSummary, CorpusError> {
        self.validate_root()?;
        let input =
            read_bounded_regular(label_path.as_ref(), MAX_LABEL_BYTES, ErrorContext::Replay)?;
        let label: CompleteLabel = serde_json::from_slice(&input)?;
        label.validate_contents()?;
        let bytes = canonical_json(&label)?;
        if bytes.len() > MAX_LABEL_BYTES {
            return Err(CorpusError::CapacityExceeded);
        }
        let (frame_id, annotation_revision, shape) = label.summary_fields();

        preflight_managed_components(&self.root)?;
        validate_directory(&self.root, ErrorContext::Replay)?;
        let label_dir = self.root.join("labels");
        validate_directory(&label_dir, ErrorContext::Replay)?;
        let lock = open_store_lock(&self.root, false)?;
        lock.lock()?;
        preflight_managed_components(&self.root)?;
        validate_directory(&label_dir, ErrorContext::Replay)?;
        recover_label_staging(&label_dir)?;
        validate_label_store(&label_dir)?;

        let digest = digest_bytes(&bytes);
        let destination = label_dir.join(format!("{digest}.json"));
        match destination.symlink_metadata() {
            Ok(_) => {
                if read_bounded_regular(&destination, MAX_LABEL_BYTES, ErrorContext::Replay)?
                    != bytes
                {
                    return Err(CorpusError::InvalidReplay(
                        "complete-label digest is bound to different bytes".to_owned(),
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                ensure_label_capacity(&label_dir, bytes.len())?;
                write_atomic_file(&label_dir, &destination, &bytes, LABEL_STAGING_PREFIX)?;
            }
            Err(error) => return Err(error.into()),
        }
        sync_file_and_parent(&destination, &label_dir)?;
        drop(lock);

        Ok(CompleteLabelSummary {
            schema: COMPLETE_LABEL_SUMMARY_SCHEMA.to_owned(),
            frame_id: frame_id.to_owned(),
            annotation_revision: annotation_revision.to_owned(),
            shape,
            labels_sha256: digest,
            label_bytes: bytes.len() as u64,
        })
    }

    /// Generates and durably publishes one canonical replay index from strict frame metadata.
    ///
    /// Episode IDs are the plan's opaque episode-group SHA-256 values. Reusing an episode group
    /// after another group has begun is rejected so an episode always denotes one contiguous
    /// decode interval.
    ///
    /// # Errors
    ///
    /// Returns an error if the plan, selected source manifest, frame labels, decode order, or episode
    /// grouping is invalid, or if bounded durable publication fails.
    pub fn generate_replay_index(
        &self,
        plan_path: impl AsRef<Path>,
    ) -> Result<ReplayIndexSummary, CorpusError> {
        self.validate_root()?;
        let plan = ReplayIndexPlan::read_from(plan_path)?;
        preflight_managed_components(&self.root)?;
        validate_directory(&self.root, ErrorContext::Replay)?;
        let lock = open_store_lock(&self.root, false)?;
        lock.lock()?;
        preflight_managed_components(&self.root)?;
        for name in ["content", "manifests", "labels"] {
            validate_directory(&self.root.join(name), ErrorContext::Replay)?;
        }
        let manifest = load_source_manifest(self, &plan.fixture_id, &plan.source_manifest_sha256)?;
        let index = plan.into_replay_index(manifest);
        index.validate()?;
        for frame in &index.frames {
            validate_complete_label(self, frame)?;
        }

        let bytes = canonical_json(&index)?;
        if bytes.len() > MAX_REPLAY_INDEX_BYTES {
            return Err(CorpusError::CapacityExceeded);
        }
        let digest = digest_bytes(&bytes);
        let index_dir = self.root.join("indexes");
        create_private_directory(&index_dir)?;
        recover_index_staging(&index_dir)?;
        let destination = index_dir.join(format!("{digest}.json"));
        match destination.symlink_metadata() {
            Ok(_) => {
                if read_bounded_regular(&destination, MAX_REPLAY_INDEX_BYTES, ErrorContext::Replay)?
                    != bytes
                {
                    return Err(CorpusError::InvalidReplay(
                        "replay-index digest is bound to different bytes".to_owned(),
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                ensure_index_capacity(&index_dir, bytes.len())?;
                write_atomic_file(&index_dir, &destination, &bytes, INDEX_STAGING_PREFIX)?;
            }
            Err(error) => return Err(error.into()),
        }
        sync_file_and_parent(&destination, &index_dir)?;
        drop(lock);

        let episodes = index
            .frames
            .iter()
            .map(|frame| frame.episode_id.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        Ok(ReplayIndexSummary {
            schema: INDEX_SUMMARY_SCHEMA.to_owned(),
            fixture_id: index.fixture_id,
            replay_index_sha256: digest,
            frame_count: index.frames.len() as u64,
            episode_count: episodes as u64,
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
        validate_directory(&self.root, ErrorContext::Replay)?;
        validate_directory(&self.root.join("content"), ErrorContext::Replay)?;
        validate_directory(&self.root.join("manifests"), ErrorContext::Replay)?;
        validate_directory(&self.root.join("generations"), ErrorContext::Replay)?;
        validate_directory(&self.root.join("labels"), ErrorContext::Replay)?;
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

impl ReplayIndexPlan {
    fn read_from(path: impl AsRef<Path>) -> Result<Self, CorpusError> {
        let bytes =
            read_bounded_regular(path.as_ref(), MAX_REPLAY_INDEX_BYTES, ErrorContext::Replay)?;
        let plan: Self = serde_json::from_slice(&bytes)?;
        plan.validate()?;
        Ok(plan)
    }

    fn validate(&self) -> Result<(), CorpusError> {
        if self.schema != INDEX_PLAN_SCHEMA {
            return Err(CorpusError::InvalidReplay(format!(
                "index-plan schema must be {INDEX_PLAN_SCHEMA:?}"
            )));
        }
        validate_opaque_id(&self.fixture_id, "fixture_id", ErrorContext::Replay)?;
        validate_sha256(
            &self.source_manifest_sha256,
            "source_manifest_sha256",
            ErrorContext::Replay,
        )?;
        self.extractor.validate()?;
        self.canonical_frame.validate(ErrorContext::Replay)?;
        self.source_time_base.validate()?;
        if self.frames.is_empty() || self.frames.len() > MAX_REPLAY_FRAMES {
            return Err(CorpusError::InvalidReplay(
                "index-plan frame count is outside the admitted range".to_owned(),
            ));
        }

        let mut completed_episodes = BTreeSet::new();
        let mut current_episode = None;
        for frame in &self.frames {
            frame.validate()?;
            if current_episode != Some(frame.episode_sha256.as_str()) {
                if let Some(previous) = current_episode {
                    completed_episodes.insert(previous);
                }
                if completed_episodes.contains(frame.episode_sha256.as_str()) {
                    return Err(CorpusError::InvalidReplay(
                        "episode group is not contiguous in decode order".to_owned(),
                    ));
                }
                current_episode = Some(frame.episode_sha256.as_str());
            }
        }
        Ok(())
    }

    fn into_replay_index(self, manifest: SourceManifest) -> ReplayIndex {
        ReplayIndex {
            schema: REPLAY_INDEX_SCHEMA.to_owned(),
            fixture_id: manifest.fixture_id,
            session_id: manifest.session_id,
            capture_profile_id: manifest.capture_profile_id,
            source: manifest.source,
            source_manifest_sha256: self.source_manifest_sha256,
            extractor: self.extractor,
            canonical_frame: self.canonical_frame,
            source_time_base: self.source_time_base,
            frames: self
                .frames
                .into_iter()
                .map(ReplayIndexPlanFrame::into_replay_frame)
                .collect(),
        }
    }
}

impl ReplayIndexPlanFrame {
    fn validate(&self) -> Result<(), CorpusError> {
        validate_sha256(&self.episode_sha256, "episode_sha256", ErrorContext::Replay)?;
        self.clone().into_replay_frame().validate()
    }

    fn into_replay_frame(self) -> ReplayFrame {
        ReplayFrame {
            frame_id: self.frame_id,
            source_pts: self.source_pts,
            decode_index: self.decode_index,
            frame_sha256: self.frame_sha256,
            episode_id: self.episode_sha256,
            screen_class: self.screen_class,
            split: self.split,
            groups: self.groups,
            annotation_revision: self.annotation_revision,
            labels_sha256: self.labels_sha256,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayIndex {
    pub schema: String,
    pub fixture_id: String,
    pub session_id: String,
    pub capture_profile_id: String,
    pub source: ContentRef,
    pub source_manifest_sha256: String,
    pub extractor: ExtractorIdentity,
    pub canonical_frame: CanonicalFrameBinding,
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
        validate_token(
            &self.capture_profile_id,
            "capture_profile_id",
            ErrorContext::Replay,
        )?;
        self.source.validate(ErrorContext::Replay)?;
        validate_sha256(
            &self.source_manifest_sha256,
            "source_manifest_sha256",
            ErrorContext::Replay,
        )?;
        self.extractor.validate()?;
        self.canonical_frame.validate(ErrorContext::Replay)?;
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
    pub split_contract: SplitContract,
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
        let mut canonical_contract = None;
        let mut canonical_layout = None;
        let mut normalizer_profiles = BTreeMap::new();
        for index in &self.indexes {
            index.validate()?;
            if previous_fixture.is_some_and(|value| value >= index.fixture_id.as_str()) {
                return Err(CorpusError::InvalidReplay(
                    "replay indexes must be uniquely ordered by fixture_id".to_owned(),
                ));
            }
            previous_fixture = Some(index.fixture_id.as_str());
            if canonical_contract.is_some_and(|value| {
                value != index.canonical_frame.canonical_frame_contract_id.as_str()
            }) {
                return Err(CorpusError::InvalidReplay(
                    "canonical frame contract differs within the replay suite".to_owned(),
                ));
            }
            if canonical_layout.is_some_and(|value| {
                value != index.canonical_frame.canonical_layout_sha256.as_str()
            }) {
                return Err(CorpusError::InvalidReplay(
                    "canonical layout differs within the replay suite".to_owned(),
                ));
            }
            canonical_contract = Some(index.canonical_frame.canonical_frame_contract_id.as_str());
            canonical_layout = Some(index.canonical_frame.canonical_layout_sha256.as_str());
            let normalizer = index.canonical_frame.normalizer_artifact_sha256.as_str();
            if normalizer_profiles
                .get(normalizer)
                .is_some_and(|profile| *profile != index.capture_profile_id.as_str())
            {
                return Err(CorpusError::InvalidReplay(
                    "normalizer artifact is bound to multiple capture profiles".to_owned(),
                ));
            }
            normalizer_profiles.insert(normalizer, index.capture_profile_id.as_str());
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
        let mut assignments = SplitAssignments::new(self.split_contract);

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
            split_contract: self.split_contract,
            index_count: self.indexes.len() as u64,
            frame_count: assignments.frame_count,
            split_counts: assignments.split_counts,
        })
    }
}

struct SplitAssignments {
    frame_ids: BTreeSet<String>,
    sessions: BTreeMap<String, CorpusSplit>,
    profiles: ProfileAssignments,
    episodes: BTreeMap<String, CorpusSplit>,
    session_hashes: BTreeMap<String, CorpusSplit>,
    plays: BTreeMap<String, CorpusSplit>,
    titles: BTreeMap<String, CorpusSplit>,
    frame_digests: BTreeMap<String, CorpusSplit>,
    split_counts: BTreeMap<CorpusSplit, u64>,
    frame_count: u64,
}

impl SplitAssignments {
    fn new(contract: SplitContract) -> Self {
        Self {
            frame_ids: BTreeSet::new(),
            sessions: BTreeMap::new(),
            profiles: match contract {
                SplitContract::InProfile => ProfileAssignments::Shared,
                SplitContract::ProfileDisjoint => ProfileAssignments::Disjoint(BTreeMap::new()),
            },
            episodes: BTreeMap::new(),
            session_hashes: BTreeMap::new(),
            plays: BTreeMap::new(),
            titles: BTreeMap::new(),
            frame_digests: BTreeMap::new(),
            split_counts: BTreeMap::new(),
            frame_count: 0,
        }
    }

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
        if let ProfileAssignments::Disjoint(assignments) = &mut self.profiles {
            require_one_split(
                assignments,
                index.capture_profile_id.clone(),
                frame.split,
                "capture profile",
            )?;
        }
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

enum ProfileAssignments {
    Shared,
    Disjoint(BTreeMap<String, CorpusSplit>),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelShape {
    Result,
    MusicSelect,
    NonRecognition,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CompleteLabelSummary {
    pub schema: String,
    pub frame_id: String,
    pub annotation_revision: String,
    pub shape: LabelShape,
    pub labels_sha256: String,
    pub label_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NonRecognitionClass {
    Transition,
    Negative,
    Unknown,
}

impl CompleteLabel {
    fn summary_fields(&self) -> (&str, &str, LabelShape) {
        match self {
            Self::Result {
                frame_id,
                annotation_revision,
                ..
            } => (frame_id, annotation_revision, LabelShape::Result),
            Self::MusicSelect {
                frame_id,
                annotation_revision,
                ..
            } => (frame_id, annotation_revision, LabelShape::MusicSelect),
            Self::NonRecognition {
                frame_id,
                annotation_revision,
                ..
            } => (frame_id, annotation_revision, LabelShape::NonRecognition),
        }
    }

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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitContract {
    InProfile,
    ProfileDisjoint,
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
    pub split_contract: SplitContract,
    pub index_count: u64,
    pub frame_count: u64,
    pub split_counts: BTreeMap<CorpusSplit, u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct SyntheticTitleRequest {
    schema: String,
    set_id: String,
    seed_sha256: String,
    sample_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SyntheticTitleManifest {
    schema: String,
    set_id: String,
    renderer_id: String,
    seed_sha256: String,
    width: usize,
    height: usize,
    pixel_format: String,
    samples: Vec<SyntheticTitleSample>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SyntheticTitleSample {
    sample_id: String,
    file_name: String,
    generated_text: String,
    content_sha256: String,
    bytes: u64,
    style: SyntheticTitleStyle,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SyntheticTitleStyle {
    glyph_scale: u8,
    letter_spacing: u8,
    shadow_offset: u8,
    noise_pixels: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SyntheticTitleSummary {
    pub schema: String,
    pub set_id: String,
    pub manifest_sha256: String,
    pub sample_count: u64,
    pub total_sample_bytes: u64,
}

/// Renders a deterministic, catalog-independent RGB8 title-crop set using only scorepeek's
/// procedural 5x7 glyphs and seed-derived ASCII n-grams.
///
/// The output directory must be an absent absolute path. The renderer never accepts caller text,
/// fonts, images, or catalog data, so every pixel and label is derived from the versioned renderer
/// and the request seed.
///
/// # Errors
///
/// Returns an error for an invalid request, an existing or relative output path, or a failed
/// durable write. A newly created output directory can remain incomplete if an I/O failure occurs.
pub fn render_synthetic_title_set(
    request_path: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
) -> Result<SyntheticTitleSummary, CorpusError> {
    let request_bytes = read_bounded_regular(
        request_path.as_ref(),
        MAX_REQUEST_BYTES,
        ErrorContext::Request,
    )?;
    let request: SyntheticTitleRequest = serde_json::from_slice(&request_bytes)?;
    request.validate()?;
    let output = output_path.as_ref();
    if !output.is_absolute() || output.as_os_str().is_empty() {
        return Err(CorpusError::InvalidRequest(
            "synthetic output must be an absolute, non-empty path".to_owned(),
        ));
    }
    let parent = output.parent().ok_or_else(|| {
        CorpusError::InvalidRequest("synthetic output has no parent directory".to_owned())
    })?;
    if !parent.metadata()?.is_dir() {
        return Err(CorpusError::InvalidRequest(
            "synthetic output parent must be a directory".to_owned(),
        ));
    }
    match output.symlink_metadata() {
        Ok(_) => {
            return Err(CorpusError::InvalidRequest(
                "synthetic output path already exists".to_owned(),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs::DirBuilder::new().mode(0o755).create(output)?;

    let mut samples = Vec::with_capacity(request.sample_count);
    let mut total_sample_bytes = 0_u64;
    for index in 0..request.sample_count {
        let seed = synthetic_sample_seed(&request.seed_sha256, index);
        let generated_text = synthetic_text(&seed);
        let style = SyntheticTitleStyle {
            glyph_scale: 3 + seed[17] % 3,
            letter_spacing: 1 + seed[18] % 4,
            shadow_offset: 1 + seed[19] % 3,
            noise_pixels: 128 + u16::from(seed[20]),
        };
        let bytes = render_synthetic_sample(&generated_text, &seed, &style);
        let file_name = format!("sample-{index:04}.ppm");
        write_redistributable_file(&output.join(&file_name), &bytes)?;
        total_sample_bytes = total_sample_bytes
            .checked_add(bytes.len() as u64)
            .ok_or(CorpusError::CapacityExceeded)?;
        samples.push(SyntheticTitleSample {
            sample_id: format!("sample-{index:04}"),
            file_name,
            generated_text,
            content_sha256: digest_bytes(&bytes),
            bytes: bytes.len() as u64,
            style,
        });
    }
    let manifest = SyntheticTitleManifest {
        schema: SYNTHETIC_TITLE_MANIFEST_SCHEMA.to_owned(),
        set_id: request.set_id.clone(),
        renderer_id: "scorepeek-procedural-5x7-v1".to_owned(),
        seed_sha256: request.seed_sha256,
        width: SYNTHETIC_WIDTH,
        height: SYNTHETIC_HEIGHT,
        pixel_format: "rgb8-p6-ppm".to_owned(),
        samples,
    };
    let manifest_bytes = canonical_json(&manifest)?;
    let manifest_sha256 = digest_bytes(&manifest_bytes);
    write_redistributable_file(&output.join("manifest.json"), &manifest_bytes)?;
    File::open(output)?.sync_all()?;
    File::open(parent)?.sync_all()?;

    Ok(SyntheticTitleSummary {
        schema: SYNTHETIC_TITLE_SUMMARY_SCHEMA.to_owned(),
        set_id: request.set_id,
        manifest_sha256,
        sample_count: manifest.samples.len() as u64,
        total_sample_bytes,
    })
}

impl SyntheticTitleRequest {
    fn validate(&self) -> Result<(), CorpusError> {
        if self.schema != SYNTHETIC_TITLE_REQUEST_SCHEMA {
            return Err(CorpusError::InvalidRequest(format!(
                "synthetic-title schema must be {SYNTHETIC_TITLE_REQUEST_SCHEMA:?}"
            )));
        }
        validate_opaque_id(&self.set_id, "set_id", ErrorContext::Request)?;
        validate_sha256(&self.seed_sha256, "seed_sha256", ErrorContext::Request)?;
        if self.sample_count == 0 || self.sample_count > MAX_SYNTHETIC_SAMPLES {
            return Err(CorpusError::InvalidRequest(
                "synthetic sample_count is outside the admitted range".to_owned(),
            ));
        }
        Ok(())
    }
}

fn synthetic_sample_seed(seed: &str, index: usize) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"scorepeek-procedural-5x7-v1\0");
    hasher.update(seed.as_bytes());
    hasher.update(
        u64::try_from(index)
            .expect("synthetic sample bound fits u64")
            .to_be_bytes(),
    );
    hasher.finalize().into()
}

fn synthetic_text(seed: &[u8; 32]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let length = 6 + usize::from(seed[0] % 11);
    (0..length)
        .map(|index| ALPHABET[usize::from(seed[1 + index]) % ALPHABET.len()] as char)
        .collect()
}

fn render_synthetic_sample(text: &str, seed: &[u8; 32], style: &SyntheticTitleStyle) -> Vec<u8> {
    let mut pixels = vec![0_u8; SYNTHETIC_WIDTH * SYNTHETIC_HEIGHT * 3];
    for y in 0..SYNTHETIC_HEIGHT {
        for x in 0..SYNTHETIC_WIDTH {
            let offset = (y * SYNTHETIC_WIDTH + x) * 3;
            pixels[offset] = seed[21] / 8
                + u8::try_from((x * usize::from(seed[22] % 24)) / SYNTHETIC_WIDTH)
                    .expect("horizontal gradient is below 24");
            pixels[offset + 1] = seed[23] / 8
                + u8::try_from((y * usize::from(seed[24] % 24)) / SYNTHETIC_HEIGHT)
                    .expect("vertical gradient is below 24");
            pixels[offset + 2] = seed[25] / 8
                + u8::try_from(
                    ((x + y) * usize::from(seed[26] % 16)) / (SYNTHETIC_WIDTH + SYNTHETIC_HEIGHT),
                )
                .expect("diagonal gradient is below 16");
        }
    }

    let scale = usize::from(style.glyph_scale);
    let spacing = usize::from(style.letter_spacing);
    let glyph_width = 5 * scale;
    let text_width = text
        .len()
        .saturating_mul(glyph_width + spacing)
        .saturating_sub(spacing);
    let start_x = SYNTHETIC_WIDTH.saturating_sub(text_width) / 2;
    let start_y = (SYNTHETIC_HEIGHT - 7 * scale) / 2;
    let shadow = usize::from(style.shadow_offset);
    for (index, character) in text.chars().enumerate() {
        let x = start_x + index * (glyph_width + spacing);
        let glyph = procedural_glyph(character);
        draw_glyph(
            &mut pixels,
            x + shadow,
            start_y + shadow,
            scale,
            glyph,
            [8, 8, 12],
        );
        let tint = seed[(index + 3) % seed.len()] % 48;
        draw_glyph(
            &mut pixels,
            x,
            start_y,
            scale,
            glyph,
            [207 + tint, 207 + tint / 2, 255 - tint / 2],
        );
    }

    let mut state = u64::from_be_bytes(seed[..8].try_into().expect("fixed seed slice"));
    for _ in 0..style.noise_pixels {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let pixel_count = u64::try_from(SYNTHETIC_WIDTH * SYNTHETIC_HEIGHT)
            .expect("synthetic dimensions fit u64");
        let pixel = usize::try_from(state % pixel_count).expect("pixel index fits usize");
        let channel = usize::try_from((state >> 32) % 3).expect("channel index is below 3");
        let offset = pixel * 3 + channel;
        let intensity = u8::try_from((state >> 40) % 24).expect("noise intensity is below 24");
        pixels[offset] = pixels[offset].saturating_add(intensity);
    }

    let mut ppm = format!("P6\n{SYNTHETIC_WIDTH} {SYNTHETIC_HEIGHT}\n255\n").into_bytes();
    ppm.extend_from_slice(&pixels);
    ppm
}

fn draw_glyph(pixels: &mut [u8], x: usize, y: usize, scale: usize, rows: [u8; 7], color: [u8; 3]) {
    for (row, bits) in rows.into_iter().enumerate() {
        for column in 0..5 {
            if bits & (1 << (4 - column)) == 0 {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    let pixel_x = x + column * scale + dx;
                    let pixel_y = y + row * scale + dy;
                    if pixel_x >= SYNTHETIC_WIDTH || pixel_y >= SYNTHETIC_HEIGHT {
                        continue;
                    }
                    let offset = (pixel_y * SYNTHETIC_WIDTH + pixel_x) * 3;
                    pixels[offset..offset + 3].copy_from_slice(&color);
                }
            }
        }
    }
}

fn procedural_glyph(character: char) -> [u8; 7] {
    match character {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'J' => [7, 2, 2, 2, 2, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 25, 21, 19, 17, 17, 17],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        _ => [31, 17, 2, 4, 8, 0, 8],
    }
}

fn write_redistributable_file(path: &Path, bytes: &[u8]) -> Result<(), CorpusError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(path)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
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
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_SOURCE_BYTES {
        return Err(CorpusError::InvalidRequest(
            "source must be a non-empty bounded regular file".to_owned(),
        ));
    }
    Ok(())
}

fn copy_source(
    path: &Path,
    destination: &Path,
    expected: Option<&ContentRef>,
) -> Result<ContentRef, CorpusError> {
    if let Some(expected) = expected {
        expected.validate(ErrorContext::Request)?;
    }
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
    let sha256 = encode_digest(hasher.finalize());
    if let Some(expected) = expected {
        if bytes != expected.bytes || sha256 != expected.sha256 {
            return Err(CorpusError::InvalidRequest(
                "source changed while copying into the content store".to_owned(),
            ));
        }
        return Ok(expected.clone());
    }
    Ok(ContentRef { sha256, bytes })
}

fn ensure_capacity(content_dir: &Path, added_bytes: u64) -> Result<(), CorpusError> {
    ensure_content_capacity(content_dir, 1, added_bytes)
}

fn ensure_content_capacity(
    content_dir: &Path,
    added_count: usize,
    added_bytes: u64,
) -> Result<(), CorpusError> {
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
        if !is_sha256(&name) || !entry.path().metadata()?.is_dir() {
            return Err(CorpusError::InvalidRequest(
                "content store contains an unrecognized entry".to_owned(),
            ));
        }
        let source_bytes = stored_source_logical_bytes(&entry.path(), &name)?;
        count = count.checked_add(1).ok_or(CorpusError::CapacityExceeded)?;
        bytes = bytes
            .checked_add(source_bytes)
            .ok_or(CorpusError::CapacityExceeded)?;
    }
    let new_count = count
        .checked_add(added_count)
        .ok_or(CorpusError::CapacityExceeded)?;
    let new_bytes = bytes
        .checked_add(added_bytes)
        .ok_or(CorpusError::CapacityExceeded)?;
    if new_count > MAX_SOURCE_OBJECTS || new_bytes > MAX_SOURCE_STORAGE_BYTES {
        return Err(CorpusError::CapacityExceeded);
    }
    Ok(())
}

fn stored_source_logical_bytes(
    directory: &Path,
    expected_sha256: &str,
) -> Result<u64, CorpusError> {
    validate_directory(directory, ErrorContext::Request)?;
    let source = directory.join(SOURCE_FILE);
    match source.metadata() {
        Ok(metadata)
            if metadata.is_file() && metadata.len() > 0 && metadata.len() <= MAX_SOURCE_BYTES =>
        {
            return Ok(metadata.len());
        }
        Ok(_) => {
            return Err(CorpusError::InvalidRequest(
                "content store contains an invalid source object".to_owned(),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let locator_path = directory.join(EXTERNAL_SOURCE_FILE);
    validate_regular_file(&locator_path, ErrorContext::Request)?;
    let bytes = read_bounded_regular(&locator_path, MAX_REQUEST_BYTES, ErrorContext::Request)?;
    let locator: ExternalSourceLocator = serde_json::from_slice(&bytes)?;
    locator.validate()?;
    if locator.source.sha256 != expected_sha256 || canonical_json(&locator)? != bytes {
        return Err(CorpusError::InvalidRequest(
            "content store contains an invalid external source locator".to_owned(),
        ));
    }
    Ok(locator.source.bytes)
}

fn ensure_manifest_capacity(manifest_dir: &Path, added_bytes: usize) -> Result<(), CorpusError> {
    ensure_manifest_capacity_additions(manifest_dir, 1, added_bytes as u64)
}

fn ensure_manifest_capacity_additions(
    manifest_dir: &Path,
    added_count: usize,
    added_bytes: u64,
) -> Result<(), CorpusError> {
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
        let metadata = entry.path().metadata()?;
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
    let new_count = count
        .checked_add(added_count)
        .ok_or(CorpusError::CapacityExceeded)?;
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
        let metadata = entry.path().metadata()?;
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

fn ensure_label_capacity(label_dir: &Path, added_bytes: usize) -> Result<(), CorpusError> {
    let mut count = 0_usize;
    let mut total = 0_u64;
    for entry in fs::read_dir(label_dir)? {
        let entry = entry?;
        count = count.checked_add(1).ok_or(CorpusError::CapacityExceeded)?;
        total = total
            .checked_add(entry.path().metadata()?.len())
            .ok_or(CorpusError::CapacityExceeded)?;
    }
    if count >= MAX_LABEL_OBJECTS
        || total
            .checked_add(added_bytes as u64)
            .is_none_or(|value| value > MAX_LABEL_STORAGE_BYTES)
    {
        return Err(CorpusError::CapacityExceeded);
    }
    Ok(())
}

fn ensure_index_capacity(index_dir: &Path, added_bytes: usize) -> Result<(), CorpusError> {
    let mut count = 0_usize;
    let mut total = 0_u64;
    for entry in fs::read_dir(index_dir)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            return Err(CorpusError::InvalidReplay(
                "replay-index store contains a non-UTF-8 entry".to_owned(),
            ));
        };
        if name.starts_with(INDEX_STAGING_PREFIX) {
            continue;
        }
        let Some(digest) = name.strip_suffix(".json") else {
            return Err(CorpusError::InvalidReplay(
                "replay-index store contains an unrecognized entry".to_owned(),
            ));
        };
        validate_sha256(digest, "replay-index filename", ErrorContext::Replay)?;
        let metadata = entry.path().metadata()?;
        if !metadata.is_file()
            || metadata.len() == 0
            || metadata.len() > MAX_REPLAY_INDEX_BYTES as u64
        {
            return Err(CorpusError::InvalidReplay(
                "replay-index store contains an invalid object".to_owned(),
            ));
        }
        count = count.checked_add(1).ok_or(CorpusError::CapacityExceeded)?;
        total = total
            .checked_add(metadata.len())
            .ok_or(CorpusError::CapacityExceeded)?;
    }
    let added_bytes = u64::try_from(added_bytes).map_err(|_| CorpusError::CapacityExceeded)?;
    if count >= MAX_REPLAY_INDEXES
        || total
            .checked_add(added_bytes)
            .is_none_or(|value| value > MAX_REPLAY_INDEX_STORAGE_BYTES)
    {
        return Err(CorpusError::CapacityExceeded);
    }
    Ok(())
}

fn validate_stored_source(directory: &Path, expected: &ContentRef) -> Result<(), CorpusError> {
    resolve_stored_source_path(directory, expected).map(|_| ())
}

fn resolve_stored_source_path(
    directory: &Path,
    expected: &ContentRef,
) -> Result<PathBuf, CorpusError> {
    let path = resolve_stored_source_path_unverified(directory, expected)?;
    let mut file = File::open(&path)?;
    verify_open_source(&mut file, expected)?;
    Ok(path)
}

fn resolve_stored_source_path_unverified(
    directory: &Path,
    expected: &ContentRef,
) -> Result<PathBuf, CorpusError> {
    if !directory.metadata()?.is_dir() {
        return Err(CorpusError::InvalidRequest(
            "content-addressed destination is not a directory".to_owned(),
        ));
    }
    validate_directory(directory, ErrorContext::Request)?;
    let source = directory.join(SOURCE_FILE);
    match source.metadata() {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.len() != expected.bytes {
                return Err(CorpusError::InvalidRequest(
                    "stored source does not match its manifest".to_owned(),
                ));
            }
            validate_regular_file(&source, ErrorContext::Request)?;
            return Ok(source);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let (_, path) = read_external_source_locator(directory, expected)?;
    validate_external_source_file(&path, expected, false)?;
    Ok(path)
}

fn verify_open_source(file: &mut File, expected: &ContentRef) -> Result<(), CorpusError> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() != expected.bytes {
        return Err(CorpusError::InvalidRequest(
            "opened source does not match its content binding".to_owned(),
        ));
    }
    if digest_open_file(file, MAX_SOURCE_BYTES)?.0 != expected.sha256 {
        return Err(CorpusError::InvalidRequest(
            "opened source digest does not match its content binding".to_owned(),
        ));
    }
    Ok(())
}

fn read_external_source_locator(
    directory: &Path,
    expected: &ContentRef,
) -> Result<(ExternalSourceLocator, PathBuf), CorpusError> {
    let locator_path = directory.join(EXTERNAL_SOURCE_FILE);
    validate_regular_file(&locator_path, ErrorContext::Request)?;
    let bytes = read_bounded_regular(&locator_path, MAX_REQUEST_BYTES, ErrorContext::Request)?;
    let locator: ExternalSourceLocator = serde_json::from_slice(&bytes)?;
    let path = locator.validate()?;
    if locator.source != *expected || canonical_json(&locator)? != bytes {
        return Err(CorpusError::InvalidRequest(
            "external source locator is not canonical or content-bound".to_owned(),
        ));
    }
    Ok((locator, path))
}

fn validate_external_source_file(
    path: &Path,
    expected: &ContentRef,
    verify_digest: bool,
) -> Result<(), CorpusError> {
    if !path.is_absolute() {
        return Err(CorpusError::InvalidRequest(
            "external source path must be absolute".to_owned(),
        ));
    }
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() != expected.bytes {
        return Err(CorpusError::InvalidRequest(
            "external source does not match its locator".to_owned(),
        ));
    }
    if verify_digest && digest_regular_file(path, MAX_SOURCE_BYTES)? != expected.sha256 {
        return Err(CorpusError::InvalidRequest(
            "external source digest does not match its locator".to_owned(),
        ));
    }
    Ok(())
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
    temporary.write_all(bytes)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    File::open(directory)?.sync_all()?;
    Ok(())
}

#[allow(dead_code)]
fn replace_atomic_file(
    directory: &Path,
    path: &Path,
    bytes: &[u8],
    staging_prefix: &str,
) -> Result<(), CorpusError> {
    let mut temporary = Builder::new()
        .prefix(staging_prefix)
        .permissions(fs::Permissions::from_mode(0o600))
        .tempfile_in(directory)?;
    temporary.write_all(bytes)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    File::open(directory)?.sync_all()?;
    Ok(())
}

fn sync_stored_source_and_parent(directory: &Path, content_dir: &Path) -> io::Result<()> {
    let source = directory.join(SOURCE_FILE);
    match source.metadata() {
        Ok(metadata) if metadata.is_file() => File::open(source)?.sync_all()?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            File::open(directory.join(EXTERNAL_SOURCE_FILE))?.sync_all()?;
        }
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "stored source entry is not a regular file",
            ));
        }
        Err(error) => return Err(error),
    }
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

fn recover_label_staging(label_dir: &Path) -> Result<(), CorpusError> {
    let mut changed = false;
    for entry in fs::read_dir(label_dir)? {
        let entry = entry?;
        let is_staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(LABEL_STAGING_PREFIX));
        if !is_staging {
            continue;
        }
        if !entry.path().symlink_metadata()?.is_file() {
            return Err(CorpusError::InvalidReplay(
                "complete-label staging entry is not a file".to_owned(),
            ));
        }
        fs::remove_file(entry.path())?;
        changed = true;
    }
    if changed {
        File::open(label_dir)?.sync_all()?;
    }
    Ok(())
}

fn recover_index_staging(index_dir: &Path) -> Result<(), CorpusError> {
    let mut changed = false;
    for entry in fs::read_dir(index_dir)? {
        let entry = entry?;
        let is_staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(INDEX_STAGING_PREFIX));
        if !is_staging {
            continue;
        }
        if !entry.path().symlink_metadata()?.is_file() {
            return Err(CorpusError::InvalidReplay(
                "replay-index staging entry is not a file".to_owned(),
            ));
        }
        fs::remove_file(entry.path())?;
        changed = true;
    }
    if changed {
        File::open(index_dir)?.sync_all()?;
    }
    Ok(())
}

fn preflight_managed_components(root: &Path) -> Result<(), CorpusError> {
    match root.metadata() {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(CorpusError::InvalidRequest(
                "private store root is not a directory".to_owned(),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }

    for name in [
        "content",
        "manifests",
        "generations",
        "labels",
        "indexes",
        "profiles",
        "probes",
        "recordings",
        "dataset-generations",
    ] {
        match root.join(name).metadata() {
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
        match candidate.metadata() {
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
        sync_directory_and_parent(&directory)?;
    }
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

fn validate_directory(path: &Path, context: ErrorContext) -> Result<(), CorpusError> {
    let metadata = path.metadata()?;
    if !metadata.is_dir() {
        return Err(context.error("private store path must be a directory"));
    }
    Ok(())
}

fn validate_regular_file(path: &Path, context: ErrorContext) -> Result<(), CorpusError> {
    let metadata = path.metadata()?;
    if !metadata.is_file() {
        return Err(context.error("private store path must be a regular file"));
    }
    Ok(())
}

fn read_bounded_regular(
    path: &Path,
    maximum: usize,
    context: ErrorContext,
) -> Result<Vec<u8>, CorpusError> {
    let metadata = path.metadata()?;
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
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum {
        return Err(CorpusError::InvalidRequest(
            "stored object is not a bounded regular file".to_owned(),
        ));
    }
    let mut file = File::open(path)?;
    Ok(digest_open_file(&mut file, maximum)?.0)
}

fn digest_open_file(file: &mut File, maximum: u64) -> Result<(String, u64), CorpusError> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum {
        return Err(CorpusError::InvalidRequest(
            "opened object is not a bounded regular file".to_owned(),
        ));
    }
    file.seek(SeekFrom::Start(0))?;
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
    if bytes != metadata.len() {
        return Err(CorpusError::InvalidRequest(
            "opened object size changed while hashing".to_owned(),
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    Ok((encode_digest(hasher.finalize()), bytes))
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

fn load_source_manifest(
    store: &CorpusStore,
    fixture_id: &str,
    expected_digest: &str,
) -> Result<SourceManifest, CorpusError> {
    let path = store
        .root
        .join("manifests")
        .join(format!("{fixture_id}.json"));
    validate_regular_file(&path, ErrorContext::Replay)?;
    let bytes = read_bounded_regular(&path, MAX_REQUEST_BYTES, ErrorContext::Replay)?;
    if digest_bytes(&bytes) != expected_digest {
        return Err(CorpusError::InvalidReplay(
            "source_manifest_sha256 does not match the stored manifest".to_owned(),
        ));
    }
    let manifest: SourceManifest = serde_json::from_slice(&bytes)?;
    manifest
        .validate()
        .map_err(|_| CorpusError::InvalidReplay("stored source manifest is invalid".to_owned()))?;
    if manifest.fixture_id != fixture_id || canonical_json(&manifest)? != bytes {
        return Err(CorpusError::InvalidReplay(
            "stored source manifest is not canonical or filename-bound".to_owned(),
        ));
    }
    Ok(manifest)
}

fn validate_source_binding(store: &CorpusStore, index: &ReplayIndex) -> Result<(), CorpusError> {
    let manifest_path = store
        .root
        .join("manifests")
        .join(format!("{}.json", index.fixture_id));
    let bytes = read_bounded_regular(&manifest_path, MAX_REQUEST_BYTES, ErrorContext::Replay)?;
    validate_regular_file(&manifest_path, ErrorContext::Replay)?;
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
        || manifest.capture_profile_id != index.capture_profile_id
        || manifest.source != index.source
    {
        return Err(CorpusError::InvalidReplay(
            "replay index does not match its stored source manifest".to_owned(),
        ));
    }
    store
        .resolve_source_path(&manifest.source)
        .map(|_| ())
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
        let metadata = entry.path().metadata()?;
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
    validate_regular_file(path, ErrorContext::Replay)?;
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
        validate_regular_file(&entry.path(), ErrorContext::Request)?;
        let bytes = read_bounded_regular(&entry.path(), MAX_REQUEST_BYTES, ErrorContext::Request)?;
        let manifest: SourceManifest = serde_json::from_slice(&bytes)?;
        manifest.validate()?;
        if manifest.fixture_id != fixture_id || canonical_json(&manifest)? != bytes {
            return Err(CorpusError::InvalidRequest(
                "stored source manifest is not canonical or filename-bound".to_owned(),
            ));
        }
        resolve_stored_source_path_unverified(
            &content_dir.join(&manifest.source.sha256),
            &manifest.source,
        )?;
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
    validate_regular_file(&path, ErrorContext::Replay)?;
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
    use std::fs::{self, File};
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    use serde_json::json;
    use tempfile::tempdir;

    use super::{
        ContentRef, CorpusError, CorpusSplit, CorpusStore, EXTERNAL_SOURCE_FILE,
        LABEL_STAGING_PREFIX, LabelShape, SourceManifest, digest_bytes, render_synthetic_title_set,
        verify_open_source,
    };

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const D: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    const E: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

    #[test]
    fn verified_open_source_is_not_reopened_by_path() {
        let temporary = tempdir().unwrap();
        let source_path = temporary.path().join("source.media");
        let old_path = temporary.path().join("opened.media");
        fs::write(&source_path, b"aaaa").unwrap();
        let expected = ContentRef {
            sha256: digest_bytes(b"aaaa"),
            bytes: 4,
        };
        let mut source = File::open(&source_path).unwrap();
        verify_open_source(&mut source, &expected).unwrap();

        fs::rename(&source_path, &old_path).unwrap();
        fs::write(&source_path, b"bbbb").unwrap();
        verify_open_source(&mut source, &expected).unwrap();

        fs::write(&old_path, b"cccc").unwrap();
        assert!(verify_open_source(&mut source, &expected).is_err());
    }

    #[test]
    fn ingest_is_content_addressed_and_does_not_enforce_permissions() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let source = temporary.path().join("source.bin");
        let request = temporary.path().join("request.json");
        fs::write(&source, b"synthetic media bytes").unwrap();
        fs::write(
            &request,
            serde_json::to_vec(&json!({
                "schema": "scorepeek-private-corpus-ingest-v2",
                "fixture_id": "fixture-001",
                "session_id": "session-001",
                "capture_profile_id": "capture-profile-a"
            }))
            .unwrap(),
        )
        .unwrap();

        let store = CorpusStore::new(&root);
        let first = store.ingest(&source, &request).unwrap();
        let stored = root
            .join("content")
            .join(&first.source.sha256)
            .join("source.media");
        let manifest = root.join("manifests/fixture-001.json");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&stored, fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(&manifest, fs::Permissions::from_mode(0o644)).unwrap();
        let second = store.ingest(&source, &request).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.source.bytes, 21);
        assert_eq!(fs::read(&stored).unwrap(), b"synthetic media bytes");
        assert_eq!(root.metadata().unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(
            stored.metadata().unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(
            manifest.metadata().unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn ingest_binds_only_the_observed_capture_profile() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let source = temporary.path().join("source.bin");
        let request = temporary.path().join("request.json");
        fs::write(&source, b"synthetic observed media").unwrap();
        fs::write(
            &request,
            serde_json::to_vec(&json!({
                "schema": "scorepeek-private-corpus-ingest-v2",
                "fixture_id": "fixture-observed-001",
                "session_id": "session-observed-001",
                "capture_profile_id": "capture-profile-a"
            }))
            .unwrap(),
        )
        .unwrap();

        let manifest = CorpusStore::new(root).ingest(source, request).unwrap();
        let value = serde_json::to_value(manifest).unwrap();
        assert_eq!(value["capture_profile_id"], "capture-profile-a");
        assert!(value.get("profile").is_none());
        assert!(value.get("normalizer_artifact_sha256").is_none());
        assert!(value.get("layout_profile_id").is_none());
    }

    #[test]
    fn ingest_rejects_the_removed_profile_tuple() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("source.bin");
        let request = temporary.path().join("request.json");
        fs::write(&source, b"synthetic observed media").unwrap();
        fs::write(
            &request,
            serde_json::to_vec(&json!({
                "schema": "scorepeek-private-corpus-ingest-v1",
                "fixture_id": "fixture-observed-001",
                "session_id": "session-observed-001",
                "profile": {
                    "capture_profile_id": "capture-profile-a",
                    "normalizer_artifact_sha256": A,
                    "layout_profile_id": "layout-v1"
                }
            }))
            .unwrap(),
        )
        .unwrap();

        assert!(
            CorpusStore::new(temporary.path().join("private-corpus"))
                .ingest(source, request)
                .is_err()
        );
    }

    #[test]
    fn fixture_id_cannot_be_rebound() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let source = temporary.path().join("source.bin");
        let request = temporary.path().join("request.json");
        fs::write(&source, b"first source").unwrap();
        write_request(&request, "capture-profile-a");
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
            "capture-profile-a",
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
            "scorepeek-private-corpus-replay-suite-summary-v2"
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
    fn replay_suite_rejects_session_and_episode_split_leaks() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        let manifest = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-001",
            "session-001",
            "capture-profile-a",
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
    fn split_contract_distinguishes_in_profile_and_profile_disjoint_evaluation() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        let first = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-001",
            "session-001",
            "capture-profile-a",
            b"first replay source",
        );
        let second = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-002",
            "session-002",
            "capture-profile-a",
            b"second replay source",
        );
        let generation = store.seal_generation("generation-001").unwrap();
        let mut suite_value = replay_suite_value(
            &generation.corpus_generation_sha256,
            &[
                replay_index_value(&first, "train", C, &root),
                replay_index_value(&second, "holdout", D, &root),
            ],
        );
        let suite = temporary.path().join("suite.json");
        fs::write(&suite, serde_json::to_vec(&suite_value).unwrap()).unwrap();

        let summary = store.validate_replay_suite(&suite).unwrap();
        assert_eq!(summary.split_contract, super::SplitContract::InProfile);

        suite_value["indexes"][1]["canonical_frame"]["canonical_layout_sha256"] = json!(E);
        fs::write(&suite, serde_json::to_vec(&suite_value).unwrap()).unwrap();
        let error = store.validate_replay_suite(&suite).unwrap_err();
        assert!(error.to_string().contains("canonical layout differs"));
        suite_value["indexes"][1]["canonical_frame"]["canonical_layout_sha256"] = json!(C);

        suite_value["indexes"][1]["canonical_frame"]["canonical_frame_contract_id"] =
            json!("scorepeek-canonical-rgb10-3840x2160-v1");
        fs::write(&suite, serde_json::to_vec(&suite_value).unwrap()).unwrap();
        let error = store.validate_replay_suite(&suite).unwrap_err();
        assert!(error.to_string().contains("canonical frame contract"));
        suite_value["indexes"][1]["canonical_frame"]["canonical_frame_contract_id"] =
            json!("scorepeek-canonical-rgb8-1920x1080-v1");

        suite_value["split_contract"] = json!("profile_disjoint");
        fs::write(&suite, serde_json::to_vec(&suite_value).unwrap()).unwrap();
        let error = store.validate_replay_suite(&suite).unwrap_err();
        assert!(error.to_string().contains("capture profile group crosses"));
    }

    #[test]
    fn replay_suite_rejects_a_normalizer_shared_across_capture_profiles() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        let first = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-001",
            "session-001",
            "capture-profile-a",
            b"first replay source",
        );
        let second = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-002",
            "session-002",
            "capture-profile-b",
            b"second replay source",
        );
        let generation = store.seal_generation("generation-001").unwrap();
        let first_index = replay_index_value(&first, "train", C, &root);
        let mut second_index = replay_index_value(&second, "holdout", D, &root);
        second_index["canonical_frame"]["normalizer_artifact_sha256"] = json!(A);
        let suite_value = replay_suite_value(
            &generation.corpus_generation_sha256,
            &[first_index, second_index],
        );
        let suite = temporary.path().join("suite.json");
        fs::write(&suite, serde_json::to_vec(&suite_value).unwrap()).unwrap();

        let error = store.validate_replay_suite(&suite).unwrap_err();
        assert!(error.to_string().contains("normalizer artifact"));
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
            "capture-profile-a",
            b"first replay source",
        );
        let second = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-002",
            "session-002",
            "capture-profile-b",
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
            "capture-profile-a",
            b"first replay source",
        );
        let second = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-002",
            "session-002",
            "capture-profile-b",
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
            "capture-profile-a",
            b"first replay source",
        );
        let second = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-002",
            "session-002",
            "capture-profile-b",
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
            "capture-profile-a",
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
            "capture-profile-a",
            b"synthetic replay source",
        );
        let generation = store.seal_generation("generation-001").unwrap();
        let mut index = replay_index_value(&manifest, "train", C, &root);
        let frame_id = index["frames"][0]["frame_id"].as_str().unwrap();
        let wrong_shape = write_label(
            &root,
            &json!({
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
            "capture-profile-a",
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
        let inconsistent = write_unchecked_label(&root, label.clone());
        index["frames"][0]["labels_sha256"] = json!(inconsistent);
        let suite_value =
            replay_suite_value(&generation.corpus_generation_sha256, &[index.clone()]);
        let suite = temporary.path().join("suite.json");
        fs::write(&suite, serde_json::to_vec(&suite_value).unwrap()).unwrap();

        let error = store.validate_replay_suite(&suite).unwrap_err();
        assert!(error.to_string().contains("inconsistent"));

        fs::remove_file(root.join("labels").join(format!("{inconsistent}.json"))).unwrap();
        label["play_type"]["value"] = json!("single");
        let excessive_score = write_unchecked_label(&root, label);
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
            "capture-profile-a",
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
    fn complete_label_authoring_is_canonical_private_and_idempotent() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        ingest_fixture(
            temporary.path(),
            &store,
            "fixture-001",
            "session-001",
            "capture-profile-a",
            b"synthetic replay source",
        );
        let request = temporary.path().join("complete-label.json");
        fs::write(
            &request,
            serde_json::to_vec_pretty(&json!({
                "shape": "result",
                "schema": "scorepeek-private-complete-label-v1",
                "frame_id": "fixture-001-frame-001",
                "annotation_revision": "labels-v1",
                "screen_state": { "state": "known", "value": true },
                "savable": { "state": "known", "value": true },
                "playside": { "state": "known", "value": "one_player" },
                "play_mode": { "state": "known", "value": "single_play" },
                "play_type": { "state": "known", "value": "single" },
                "song_id": { "state": "known", "value": "private-song-001" },
                "difficulty": { "state": "known", "value": "another" },
                "level": { "state": "known", "value": 12 },
                "notes": { "state": "known", "value": 1000 },
                "current_score": { "state": "known", "value": 1800 }
            }))
            .unwrap(),
        )
        .unwrap();

        let first = store.author_complete_label(&request).unwrap();
        let interrupted = root
            .join("labels")
            .join(format!("{LABEL_STAGING_PREFIX}interrupted"));
        fs::write(&interrupted, b"partial").unwrap();
        fs::set_permissions(&interrupted, fs::Permissions::from_mode(0o600)).unwrap();
        let second = store.author_complete_label(&request).unwrap();

        assert_eq!(first, second);
        assert!(!interrupted.exists());
        assert_eq!(first.schema, "scorepeek-private-complete-label-summary-v1");
        assert_eq!(first.frame_id, "fixture-001-frame-001");
        assert_eq!(first.annotation_revision, "labels-v1");
        assert_eq!(first.shape, LabelShape::Result);
        let stored = root
            .join("labels")
            .join(format!("{}.json", first.labels_sha256));
        let stored_bytes = fs::read(&stored).unwrap();
        assert_eq!(stored_bytes.len() as u64, first.label_bytes);
        assert!(stored_bytes.ends_with(b"\n"));
        assert_eq!(digest_bytes(&stored_bytes), first.labels_sha256);
        let summary_json = serde_json::to_string(&first).unwrap();
        assert!(!summary_json.contains("private-song-001"));
        assert_eq!(fs::read_dir(root.join("labels")).unwrap().count(), 1);
    }

    #[test]
    fn replay_index_generation_is_canonical_and_idempotent() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        let manifest = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-001",
            "session-001",
            "capture-profile-a",
            b"synthetic replay source",
        );
        let plan = temporary.path().join("index-plan.json");
        fs::write(
            &plan,
            serde_json::to_vec_pretty(&replay_index_plan_value(&manifest, &root)).unwrap(),
        )
        .unwrap();

        let first = store.generate_replay_index(&plan).unwrap();
        let second = store.generate_replay_index(&plan).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.schema, "scorepeek-private-corpus-index-summary-v2");
        assert_eq!(first.fixture_id, "fixture-001");
        assert_eq!(first.frame_count, 2);
        assert_eq!(first.episode_count, 1);
        let stored = root
            .join("indexes")
            .join(format!("{}.json", first.replay_index_sha256));
        let bytes = fs::read(stored).unwrap();
        assert!(bytes.ends_with(b"\n"));
        assert_eq!(digest_bytes(&bytes), first.replay_index_sha256);
        let index: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(index["capture_profile_id"], "capture-profile-a");
        assert_eq!(
            index["canonical_frame"]["canonical_frame_contract_id"],
            "scorepeek-canonical-rgb8-1920x1080-v1"
        );
        assert_eq!(index["canonical_frame"]["canonical_layout_sha256"], C);
        assert!(index.get("profile").is_none());
        assert_eq!(index["frames"][0]["episode_id"], json!(C));
        assert_eq!(index["frames"][1]["episode_id"], json!(C));

        let mut unsupported_plan = replay_index_plan_value(&manifest, &root);
        unsupported_plan["canonical_frame"]["canonical_frame_contract_id"] =
            json!("scorepeek-canonical-rgb10-3840x2160-v1");
        fs::write(&plan, serde_json::to_vec(&unsupported_plan).unwrap()).unwrap();
        let error = store.generate_replay_index(&plan).unwrap_err();
        assert!(error.to_string().contains("canonical frame contract"));

        let generation = store.seal_generation("generation-001").unwrap();
        let suite = temporary.path().join("suite.json");
        fs::write(
            &suite,
            serde_json::to_vec(&replay_suite_value(
                &generation.corpus_generation_sha256,
                &[index],
            ))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(store.validate_replay_suite(suite).unwrap().frame_count, 2);
    }

    #[test]
    fn replay_index_generation_rejects_a_discontiguous_episode() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        let manifest = ingest_fixture(
            temporary.path(),
            &store,
            "fixture-001",
            "session-001",
            "capture-profile-a",
            b"synthetic replay source",
        );
        let mut value = replay_index_plan_value(&manifest, &root);
        let mut middle = value["frames"][0].clone();
        middle["frame_id"] = json!("fixture-001-frame-middle");
        middle["decode_index"] = json!(1);
        middle["source_pts"] = json!(1500);
        middle["frame_sha256"] = json!(E);
        middle["episode_sha256"] = json!(D);
        let middle_label = write_label(
            &root,
            &json!({
                "shape": "non_recognition",
                "schema": "scorepeek-private-complete-label-v1",
                "frame_id": "fixture-001-frame-middle",
                "annotation_revision": "labels-v1",
                "screen_class": "transition"
            }),
        );
        middle["screen_class"] = json!("transition");
        middle["labels_sha256"] = json!(middle_label);
        value["frames"][1]["decode_index"] = json!(2);
        value["frames"].as_array_mut().unwrap().insert(1, middle);
        let plan = temporary.path().join("index-plan.json");
        fs::write(&plan, serde_json::to_vec(&value).unwrap()).unwrap();

        let error = store.generate_replay_index(&plan).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("episode group is not contiguous")
        );
        assert!(!root.join("indexes").exists());
    }

    #[test]
    fn synthetic_title_rendering_is_seed_only_and_byte_deterministic() {
        let temporary = tempdir().unwrap();
        let request = temporary.path().join("synthetic-request.json");
        fs::write(
            &request,
            serde_json::to_vec_pretty(&json!({
                "schema": "scorepeek-synthetic-title-request-v1",
                "set_id": "synthetic-set-001",
                "seed_sha256": A,
                "sample_count": 3
            }))
            .unwrap(),
        )
        .unwrap();

        let first_dir = temporary.path().join("render-a");
        let second_dir = temporary.path().join("render-b");
        let first = render_synthetic_title_set(&request, &first_dir).unwrap();
        let second = render_synthetic_title_set(&request, &second_dir).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.schema, "scorepeek-synthetic-title-summary-v1");
        assert_eq!(first.sample_count, 3);
        assert_eq!(
            fs::read(first_dir.join("manifest.json")).unwrap(),
            fs::read(second_dir.join("manifest.json")).unwrap()
        );
        for sample in ["sample-0000.ppm", "sample-0001.ppm", "sample-0002.ppm"] {
            assert_eq!(
                fs::read(first_dir.join(sample)).unwrap(),
                fs::read(second_dir.join(sample)).unwrap()
            );
        }

        let mut forbidden =
            serde_json::from_slice::<serde_json::Value>(&fs::read(&request).unwrap()).unwrap();
        forbidden["training_text"] = json!("external catalog title");
        fs::write(&request, serde_json::to_vec(&forbidden).unwrap()).unwrap();
        let error =
            render_synthetic_title_set(&request, temporary.path().join("render-c")).unwrap_err();
        assert!(error.to_string().contains("unknown field"));
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
            "capture-profile-a",
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
            "capture-profile-a",
        );
        let before = fs::read_dir(root.join("content")).unwrap().count();
        assert!(matches!(
            store.ingest(source, request),
            Err(CorpusError::CapacityExceeded)
        ));
        assert_eq!(fs::read_dir(root.join("content")).unwrap().count(), before);
    }

    #[test]
    fn ingest_follows_operator_root_symlink_and_rejects_changed_content_and_lock() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("source.media");
        let request = temporary.path().join("request.json");
        fs::write(&source, b"synthetic source").unwrap();
        write_request_for(&request, "fixture-001", "session-001", "capture-profile-a");

        let target = temporary.path().join("target");
        let alias = temporary.path().join("alias");
        fs::create_dir(&target).unwrap();
        symlink(&target, &alias).unwrap();
        CorpusStore::new(&alias).ingest(&source, &request).unwrap();

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
    fn managed_component_preflight_follows_operator_directory_symlinks() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("private-corpus");
        let store = CorpusStore::new(&root);
        ingest_fixture(
            temporary.path(),
            &store,
            "fixture-001",
            "session-001",
            "capture-profile-a",
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
        write_request_for(&request, "fixture-002", "session-002", "capture-profile-a");
        store.ingest(&source, &request).unwrap();

        assert_eq!(root.metadata().unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(
            fs::read_dir(root.join("content")).unwrap().count(),
            content_count + 1
        );
        assert_eq!(
            fs::read_dir(root.join("manifests")).unwrap().count(),
            manifest_count + 1
        );
        assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
    }

    #[test]
    fn symlinked_content_destination_is_reused_but_not_updated() {
        let temporary = tempdir().unwrap();
        let store = CorpusStore::new(temporary.path().join("store"));
        let first_path = temporary.path().join("first.media");
        let second_path = temporary.path().join("second.media");
        let bytes = b"same external source";
        fs::write(&first_path, bytes).unwrap();
        fs::write(&second_path, bytes).unwrap();
        let source = ContentRef {
            sha256: digest_bytes(bytes),
            bytes: bytes.len() as u64,
        };
        let first_path = fs::canonicalize(first_path).unwrap();
        let second_path = fs::canonicalize(second_path).unwrap();
        store
            .register_external_source(&first_path, &source)
            .unwrap();
        let destination = store.root.join("content").join(&source.sha256);
        let moved = temporary.path().join("moved-content");
        fs::rename(&destination, &moved).unwrap();
        symlink(&moved, &destination).unwrap();
        let locator_before = fs::read(moved.join(EXTERNAL_SOURCE_FILE)).unwrap();

        assert!(
            store
                .register_external_source(&second_path, &source)
                .is_err()
        );
        assert!(
            destination
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read(moved.join(EXTERNAL_SOURCE_FILE)).unwrap(),
            locator_before
        );
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
                "schema": "scorepeek-private-corpus-ingest-v2",
                "fixture_id": fixture_id,
                "session_id": session_id,
                "capture_profile_id": profile
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
                "schema": "scorepeek-private-corpus-ingest-v2",
                "fixture_id": fixture_id,
                "session_id": session_id,
                "capture_profile_id": profile
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
            "schema": "scorepeek-private-corpus-replay-suite-v2",
            "suite_id": "suite-001",
            "corpus_generation_sha256": corpus_generation_sha256,
            "split_contract": "in_profile",
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
            &json!({
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
            &json!({
                "shape": "non_recognition",
                "schema": "scorepeek-private-complete-label-v1",
                "frame_id": second_frame,
                "annotation_revision": "labels-v1",
                "screen_class": "transition"
            }),
        );
        json!({
            "schema": "scorepeek-private-corpus-replay-v2",
            "fixture_id": fixture_id,
            "session_id": manifest.session_id,
            "capture_profile_id": manifest.capture_profile_id,
            "source": manifest.source,
            "source_manifest_sha256": manifest.summary().unwrap().source_manifest_sha256,
            "extractor": {
                "tool_id": "ffmpeg",
                "tool_version": "8.0.0",
                "extractor_manifest_sha256": A,
                "parameters_sha256": B
            },
            "canonical_frame": {
                "normalizer_artifact_sha256": if fixture_id == "fixture-001" { A } else { B },
                "canonical_frame_contract_id": "scorepeek-canonical-rgb8-1920x1080-v1",
                "canonical_layout_sha256": C
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

    fn replay_index_plan_value(
        manifest: &SourceManifest,
        root: &std::path::Path,
    ) -> serde_json::Value {
        let index = replay_index_value(manifest, "train", C, root);
        let frames = index["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|frame| {
                let mut frame = frame.clone();
                frame.as_object_mut().unwrap().remove("episode_id");
                frame["episode_sha256"] = json!(C);
                frame
            })
            .collect::<Vec<_>>();
        json!({
            "schema": "scorepeek-private-corpus-index-plan-v2",
            "fixture_id": manifest.fixture_id,
            "source_manifest_sha256": manifest.summary().unwrap().source_manifest_sha256,
            "extractor": index["extractor"],
            "canonical_frame": index["canonical_frame"],
            "source_time_base": index["source_time_base"],
            "frames": frames
        })
    }

    fn write_label(root: &std::path::Path, value: &serde_json::Value) -> String {
        let temporary = tempdir().unwrap();
        let path = temporary.path().join("complete-label.json");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        CorpusStore::new(root)
            .author_complete_label(path)
            .unwrap()
            .labels_sha256
    }

    fn write_unchecked_label(root: &std::path::Path, value: serde_json::Value) -> String {
        let label: super::CompleteLabel = serde_json::from_value(value).unwrap();
        let bytes = super::canonical_json(&label).unwrap();
        let digest = digest_bytes(&bytes);
        let path = root.join("labels").join(format!("{digest}.json"));
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        digest
    }
}
