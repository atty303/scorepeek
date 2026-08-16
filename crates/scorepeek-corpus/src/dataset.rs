use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tempfile::Builder;

use super::*;

const CAPTURE_CONTEXT_SCHEMA: &str = "scorepeek-capture-context-v1";
const CAPTURE_PROFILE_SCHEMA: &str = "scorepeek-capture-profile-v1";
const RECORDING_MANIFEST_SCHEMA: &str = "scorepeek-recording-v1";
const RECORDING_IMPORT_SUMMARY_SCHEMA: &str = "scorepeek-recording-import-summary-v1";
const DATASET_GENERATION_SCHEMA: &str = "scorepeek-recording-dataset-generation-v1";
const DATASET_SUMMARY_SCHEMA: &str = "scorepeek-recording-dataset-summary-v1";
const DOCUMENT_STAGING_PREFIX: &str = ".scorepeek-dataset-staging-";
pub(crate) const MAX_DATASET_DOCUMENT_BYTES: usize = 64 * 1024 * 1024;
const MAX_DATASET_STORAGE_BYTES: u64 = 2 * MAX_SOURCE_STORAGE_BYTES;
const MAX_PROFILE_STORAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PROBE_STORAGE_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_RECORDING_STORAGE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CaptureContext {
    schema: String,
    route: CaptureRoute,
    environment_id: String,
    capture_adapter_id: String,
    capture_adapter_version: String,
    settings_revision: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CaptureRoute {
    PortalPipewire,
    GamescopeDirectPipewire,
    ObsVkcapture,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CaptureProfile {
    schema: String,
    context: CaptureContext,
    observed: media::ObservedMediaContract,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RecordingManifest {
    schema: String,
    recording_sha256: String,
    recording_bytes: u64,
    fixture_id: String,
    session_id: String,
    capture_profile_sha256: String,
    source_manifest_sha256: String,
    media_probe_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RecordingImportSummary {
    pub schema: String,
    pub recording_sha256: String,
    pub recording_bytes: u64,
    pub session_id: String,
    pub capture_profile_sha256: String,
    pub source_manifest_sha256: String,
    pub media_probe_sha256: String,
    pub recording_manifest_sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DatasetObjectKind {
    CaptureProfile,
    MediaProbe,
    RecordingManifest,
    SourceManifest,
    SourceMedia,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DatasetObject {
    pub(crate) kind: DatasetObjectKind,
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
    pub(crate) recording_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecordingDatasetGeneration {
    pub(crate) schema: String,
    pub(crate) dataset_id: String,
    pub(crate) objects: Vec<DatasetObject>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DatasetSummary {
    pub schema: String,
    pub dataset_id: String,
    pub generation_sha256: String,
    pub recording_count: u64,
    pub object_count: u64,
    pub total_bytes: u64,
}

impl CorpusStore {
    /// Imports one complete game-run recording as an immutable dataset root.
    ///
    /// # Errors
    /// Returns an error if the context or media is invalid, the recording changes during import,
    /// or any private artifact cannot be published durably.
    pub fn import_recording(
        &self,
        recording_path: impl AsRef<Path>,
        capture_context_path: impl AsRef<Path>,
    ) -> Result<RecordingImportSummary, CorpusError> {
        let recording_path = recording_path.as_ref();
        validate_source_file(recording_path)?;
        let context_bytes = read_bounded_regular(
            capture_context_path.as_ref(),
            MAX_REQUEST_BYTES,
            ErrorContext::Request,
        )?;
        let context: CaptureContext = serde_json::from_slice(&context_bytes)?;
        context.validate()?;

        let expected_sha256 = digest_regular_file(recording_path, MAX_SOURCE_BYTES)?;
        let observation = media::inspect_recording(recording_path)?;
        let profile = CaptureProfile {
            schema: CAPTURE_PROFILE_SCHEMA.to_owned(),
            context,
            observed: observation.observed,
        };
        profile.validate()?;
        let profile_bytes = canonical_json(&profile)?;
        let profile_sha256 = digest_bytes(&profile_bytes);
        self.publish_dataset_document("profiles", &profile_sha256, &profile_bytes)?;

        let source_manifest = self.ingest_verified_recording(
            recording_path,
            expected_sha256.clone(),
            expected_sha256.clone(),
            profile_sha256.clone(),
            &expected_sha256,
        )?;
        let source_summary = source_manifest.summary()?;

        let probe_staging = Builder::new()
            .prefix(".scorepeek-recording-probe-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(&self.root)?;
        let probe_path = probe_staging.path().join("probe.json");
        let probe_summary = self.probe_media(&expected_sha256, &probe_path)?;
        let probe_bytes =
            read_bounded_regular(&probe_path, 64 * 1024 * 1024, ErrorContext::Replay)?;
        if digest_bytes(&probe_bytes) != probe_summary.media_probe_sha256 {
            return Err(CorpusError::InvalidMedia(
                "recording probe digest changed before publication".to_owned(),
            ));
        }
        self.publish_dataset_document("probes", &probe_summary.media_probe_sha256, &probe_bytes)?;

        let recording = RecordingManifest {
            schema: RECORDING_MANIFEST_SCHEMA.to_owned(),
            recording_sha256: expected_sha256.clone(),
            recording_bytes: source_manifest.source.bytes,
            fixture_id: expected_sha256.clone(),
            session_id: expected_sha256.clone(),
            capture_profile_sha256: profile_sha256.clone(),
            source_manifest_sha256: source_summary.source_manifest_sha256.clone(),
            media_probe_sha256: probe_summary.media_probe_sha256.clone(),
        };
        recording.validate()?;
        let recording_bytes = canonical_json(&recording)?;
        let recording_manifest_sha256 = digest_bytes(&recording_bytes);
        self.publish_named_dataset_document("recordings", &expected_sha256, &recording_bytes)?;

        Ok(RecordingImportSummary {
            schema: RECORDING_IMPORT_SUMMARY_SCHEMA.to_owned(),
            recording_sha256: expected_sha256.clone(),
            recording_bytes: source_manifest.source.bytes,
            session_id: expected_sha256,
            capture_profile_sha256: profile_sha256,
            source_manifest_sha256: source_summary.source_manifest_sha256,
            media_probe_sha256: probe_summary.media_probe_sha256,
            recording_manifest_sha256,
        })
    }

    /// Seals every imported recording into one reusable dataset generation.
    ///
    /// # Errors
    /// Returns an error if any recording or referenced object is malformed or missing.
    pub fn seal_recording_dataset(&self, dataset_id: &str) -> Result<DatasetSummary, CorpusError> {
        self.validate_dataset_store(false)?;
        validate_opaque_id(dataset_id, "dataset_id", ErrorContext::Request)?;
        let recordings_dir = self.root.join("recordings");
        validate_private_directory_mode(&recordings_dir, ErrorContext::Request)?;
        let mut objects = Vec::new();
        let mut recording_count = 0_u64;
        let mut entries = fs::read_dir(&recordings_dir)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let name = entry.file_name().into_string().map_err(|_| {
                CorpusError::InvalidRequest("recording name is not UTF-8".to_owned())
            })?;
            let recording_sha256 = name
                .strip_suffix(".json")
                .ok_or_else(|| CorpusError::InvalidRequest("invalid recording entry".to_owned()))?;
            validate_sha256(recording_sha256, "recording_sha256", ErrorContext::Request)?;
            let bytes =
                read_bounded_regular(&entry.path(), MAX_REQUEST_BYTES, ErrorContext::Request)?;
            let recording: RecordingManifest = serde_json::from_slice(&bytes)?;
            recording.validate()?;
            if recording.recording_sha256 != recording_sha256
                || canonical_json(&recording)? != bytes
            {
                return Err(CorpusError::InvalidRequest(
                    "recording manifest is not canonical or bound to its name".to_owned(),
                ));
            }
            objects.extend(self.recording_objects(&recording, &bytes)?);
            recording_count = recording_count
                .checked_add(1)
                .ok_or(CorpusError::CapacityExceeded)?;
        }
        if recording_count == 0 {
            return Err(CorpusError::InvalidRequest(
                "dataset contains no recordings".to_owned(),
            ));
        }
        objects.sort();
        objects.dedup();
        let generation = RecordingDatasetGeneration {
            schema: DATASET_GENERATION_SCHEMA.to_owned(),
            dataset_id: dataset_id.to_owned(),
            objects,
        };
        generation.validate()?;
        for object in &generation.objects {
            self.validate_dataset_object(object)?;
        }
        self.validate_recording_bindings(&generation)?;
        let bytes = canonical_json(&generation)?;
        let generation_sha256 = digest_bytes(&bytes);
        self.publish_dataset_document("dataset-generations", &generation_sha256, &bytes)?;
        generation.summary(&generation_sha256)
    }

    /// Verifies a local recording dataset generation and every referenced byte object.
    ///
    /// # Errors
    /// Returns an error for a missing, malformed, oversized, or digest-mismatched object.
    pub fn verify_recording_dataset(
        &self,
        generation_sha256: &str,
    ) -> Result<DatasetSummary, CorpusError> {
        self.validate_dataset_store(true)?;
        let generation = self.load_recording_generation(generation_sha256)?;
        for object in &generation.objects {
            self.validate_dataset_object(object)?;
        }
        self.validate_recording_bindings(&generation)?;
        generation.summary(generation_sha256)
    }

    pub(crate) fn load_recording_generation(
        &self,
        generation_sha256: &str,
    ) -> Result<RecordingDatasetGeneration, CorpusError> {
        validate_sha256(
            generation_sha256,
            "generation_sha256",
            ErrorContext::Request,
        )?;
        let path = self
            .root
            .join("dataset-generations")
            .join(format!("{generation_sha256}.json"));
        let bytes = read_bounded_regular(&path, MAX_DATASET_DOCUMENT_BYTES, ErrorContext::Request)?;
        if digest_bytes(&bytes) != generation_sha256 {
            return Err(CorpusError::InvalidRequest(
                "dataset generation digest does not match its bytes".to_owned(),
            ));
        }
        let generation: RecordingDatasetGeneration = serde_json::from_slice(&bytes)?;
        generation.validate()?;
        if canonical_json(&generation)? != bytes {
            return Err(CorpusError::InvalidRequest(
                "dataset generation is not canonical".to_owned(),
            ));
        }
        Ok(generation)
    }

    pub(crate) fn dataset_object_path(&self, object: &DatasetObject) -> PathBuf {
        match object.kind {
            DatasetObjectKind::SourceMedia => self
                .root
                .join("content")
                .join(&object.sha256)
                .join(SOURCE_FILE),
            DatasetObjectKind::SourceManifest => self
                .root
                .join("manifests")
                .join(format!("{}.json", object.recording_sha256)),
            DatasetObjectKind::CaptureProfile => self
                .root
                .join("profiles")
                .join(format!("{}.json", object.sha256)),
            DatasetObjectKind::MediaProbe => self
                .root
                .join("probes")
                .join(format!("{}.json", object.sha256)),
            DatasetObjectKind::RecordingManifest => self
                .root
                .join("recordings")
                .join(format!("{}.json", object.recording_sha256)),
        }
    }

    fn recording_objects(
        &self,
        recording: &RecordingManifest,
        recording_manifest_bytes: &[u8],
    ) -> Result<Vec<DatasetObject>, CorpusError> {
        let candidates = [
            (
                DatasetObjectKind::SourceMedia,
                recording.recording_sha256.clone(),
                recording.recording_bytes,
            ),
            (
                DatasetObjectKind::SourceManifest,
                recording.source_manifest_sha256.clone(),
                0,
            ),
            (
                DatasetObjectKind::CaptureProfile,
                recording.capture_profile_sha256.clone(),
                0,
            ),
            (
                DatasetObjectKind::MediaProbe,
                recording.media_probe_sha256.clone(),
                0,
            ),
            (
                DatasetObjectKind::RecordingManifest,
                digest_bytes(recording_manifest_bytes),
                recording_manifest_bytes.len() as u64,
            ),
        ];
        candidates
            .into_iter()
            .map(|(kind, sha256, declared_bytes)| {
                let mut object = DatasetObject {
                    kind,
                    sha256,
                    bytes: declared_bytes,
                    recording_sha256: recording.recording_sha256.clone(),
                };
                let path = self.dataset_object_path(&object);
                let metadata = path.symlink_metadata()?;
                if !metadata.is_file() {
                    return Err(CorpusError::InvalidRequest(
                        "dataset object is not a regular file".to_owned(),
                    ));
                }
                object.bytes = metadata.len();
                validate_object(&path, &object)?;
                Ok(object)
            })
            .collect()
    }

    fn validate_recording_bindings(
        &self,
        generation: &RecordingDatasetGeneration,
    ) -> Result<(), CorpusError> {
        for manifest_object in generation
            .objects
            .iter()
            .filter(|object| object.kind == DatasetObjectKind::RecordingManifest)
        {
            let recording_bytes = self.read_dataset_document(manifest_object, MAX_REQUEST_BYTES)?;
            let source_manifest = required_object(
                generation,
                &manifest_object.recording_sha256,
                DatasetObjectKind::SourceManifest,
            )?;
            let capture_profile = required_object(
                generation,
                &manifest_object.recording_sha256,
                DatasetObjectKind::CaptureProfile,
            )?;
            let media_probe = required_object(
                generation,
                &manifest_object.recording_sha256,
                DatasetObjectKind::MediaProbe,
            )?;
            let source_bytes = self.read_dataset_document(source_manifest, MAX_REQUEST_BYTES)?;
            let profile_bytes = self.read_dataset_document(capture_profile, MAX_REQUEST_BYTES)?;
            let probe_bytes =
                self.read_dataset_document(media_probe, MAX_DATASET_DOCUMENT_BYTES)?;
            validate_recording_bundle(
                generation,
                manifest_object,
                &recording_bytes,
                &source_bytes,
                &profile_bytes,
                &probe_bytes,
            )?;
        }
        Ok(())
    }

    fn validate_dataset_store(
        &self,
        require_generation_directory: bool,
    ) -> Result<(), CorpusError> {
        self.validate_root()?;
        preflight_managed_components(&self.root)?;
        validate_private_directory_mode(&self.root, ErrorContext::Request)?;
        for directory in ["content", "manifests", "profiles", "probes", "recordings"] {
            validate_private_directory_mode(&self.root.join(directory), ErrorContext::Request)?;
        }
        if require_generation_directory {
            validate_private_directory_mode(
                &self.root.join("dataset-generations"),
                ErrorContext::Request,
            )?;
        }
        Ok(())
    }

    pub(crate) fn validate_dataset_object(
        &self,
        object: &DatasetObject,
    ) -> Result<(), CorpusError> {
        let path = self.dataset_object_path(object);
        let parent = path.parent().ok_or_else(|| {
            CorpusError::InvalidRequest("dataset object has no parent directory".to_owned())
        })?;
        validate_private_directory_mode(parent, ErrorContext::Request)?;
        validate_private_file_mode(&path, ErrorContext::Request)?;
        validate_object(&path, object)
    }

    fn read_dataset_document(
        &self,
        object: &DatasetObject,
        maximum: usize,
    ) -> Result<Vec<u8>, CorpusError> {
        self.validate_dataset_object(object)?;
        read_bounded_regular(
            &self.dataset_object_path(object),
            maximum,
            ErrorContext::Request,
        )
    }

    pub(crate) fn publish_dataset_document(
        &self,
        directory: &str,
        digest: &str,
        bytes: &[u8],
    ) -> Result<(), CorpusError> {
        self.publish_named_dataset_document(directory, digest, bytes)
    }

    pub(crate) fn publish_named_dataset_document(
        &self,
        directory: &str,
        name: &str,
        bytes: &[u8],
    ) -> Result<(), CorpusError> {
        validate_sha256(name, "dataset document name", ErrorContext::Request)?;
        if bytes.len() > MAX_DATASET_DOCUMENT_BYTES {
            return Err(CorpusError::CapacityExceeded);
        }
        preflight_managed_components(&self.root)?;
        create_private_directory(&self.root)?;
        let lock = open_store_lock(&self.root, true)?;
        lock.lock()?;
        preflight_managed_components(&self.root)?;
        let document_dir = self.root.join(directory);
        create_private_directory(&document_dir)?;
        let destination = document_dir.join(format!("{name}.json"));
        match destination.symlink_metadata() {
            Ok(metadata) if metadata.is_file() => {
                if read_bounded_regular(
                    &destination,
                    MAX_DATASET_DOCUMENT_BYTES,
                    ErrorContext::Request,
                )? != bytes
                {
                    return Err(CorpusError::InvalidRequest(
                        "dataset document name is bound to different bytes".to_owned(),
                    ));
                }
                fs::set_permissions(&destination, fs::Permissions::from_mode(0o600))?;
            }
            Ok(_) => {
                return Err(CorpusError::InvalidRequest(
                    "dataset document destination is not a regular file".to_owned(),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let staging_prefix = if directory == "manifests" {
                    let content_dir = self.root.join("content");
                    create_private_directory(&content_dir)?;
                    recover_staging(&content_dir, &document_dir)?;
                    ensure_manifest_capacity(&document_dir, bytes.len())?;
                    MANIFEST_STAGING_PREFIX
                } else {
                    ensure_dataset_document_capacity(
                        &document_dir,
                        directory,
                        1,
                        bytes.len() as u64,
                    )?;
                    DOCUMENT_STAGING_PREFIX
                };
                write_atomic_file(&document_dir, &destination, bytes, staging_prefix)?;
            }
            Err(error) => return Err(error.into()),
        }
        sync_file_and_parent(&destination, &document_dir)?;
        drop(lock);
        Ok(())
    }
}

impl CaptureContext {
    pub(crate) fn validate(&self) -> Result<(), CorpusError> {
        if self.schema != CAPTURE_CONTEXT_SCHEMA {
            return Err(CorpusError::InvalidRequest(format!(
                "capture context schema must be {CAPTURE_CONTEXT_SCHEMA:?}"
            )));
        }
        for (name, value) in [
            ("environment_id", &self.environment_id),
            ("capture_adapter_id", &self.capture_adapter_id),
            ("capture_adapter_version", &self.capture_adapter_version),
            ("settings_revision", &self.settings_revision),
        ] {
            validate_token(value, name, ErrorContext::Request)?;
        }
        Ok(())
    }
}

impl CaptureProfile {
    fn validate(&self) -> Result<(), CorpusError> {
        if self.schema != CAPTURE_PROFILE_SCHEMA {
            return Err(CorpusError::InvalidRequest(
                "unsupported capture profile schema".to_owned(),
            ));
        }
        self.context.validate()?;
        self.observed.validate()
    }
}

impl RecordingManifest {
    fn validate(&self) -> Result<(), CorpusError> {
        if self.schema != RECORDING_MANIFEST_SCHEMA {
            return Err(CorpusError::InvalidRequest(
                "unsupported recording manifest schema".to_owned(),
            ));
        }
        for (name, value) in [
            ("recording_sha256", &self.recording_sha256),
            ("capture_profile_sha256", &self.capture_profile_sha256),
            ("source_manifest_sha256", &self.source_manifest_sha256),
            ("media_probe_sha256", &self.media_probe_sha256),
        ] {
            validate_sha256(value, name, ErrorContext::Request)?;
        }
        if self.recording_bytes == 0
            || self.recording_bytes > MAX_SOURCE_BYTES
            || self.fixture_id != self.recording_sha256
            || self.session_id != self.recording_sha256
        {
            return Err(CorpusError::InvalidRequest(
                "recording identity or size is invalid".to_owned(),
            ));
        }
        Ok(())
    }
}

impl RecordingDatasetGeneration {
    pub(crate) fn validate(&self) -> Result<(), CorpusError> {
        if self.schema != DATASET_GENERATION_SCHEMA {
            return Err(CorpusError::InvalidRequest(
                "unsupported recording dataset generation schema".to_owned(),
            ));
        }
        validate_opaque_id(&self.dataset_id, "dataset_id", ErrorContext::Request)?;
        if self.objects.is_empty() || self.objects.len() > MAX_SOURCE_OBJECTS * 5 {
            return Err(CorpusError::CapacityExceeded);
        }
        if self.objects.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(CorpusError::InvalidRequest(
                "dataset objects must be uniquely ordered".to_owned(),
            ));
        }
        let mut roles = BTreeMap::<&str, BTreeSet<DatasetObjectKind>>::new();
        for object in &self.objects {
            validate_sha256(&object.sha256, "object sha256", ErrorContext::Request)?;
            validate_sha256(
                &object.recording_sha256,
                "object recording sha256",
                ErrorContext::Request,
            )?;
            if object.bytes == 0 || object.bytes > MAX_SOURCE_BYTES {
                return Err(CorpusError::CapacityExceeded);
            }
            let role_maximum = match object.kind {
                DatasetObjectKind::SourceMedia => MAX_SOURCE_BYTES,
                DatasetObjectKind::SourceManifest
                | DatasetObjectKind::CaptureProfile
                | DatasetObjectKind::RecordingManifest => MAX_REQUEST_BYTES as u64,
                DatasetObjectKind::MediaProbe => MAX_DATASET_DOCUMENT_BYTES as u64,
            };
            if object.bytes > role_maximum {
                return Err(CorpusError::CapacityExceeded);
            }
            if !roles
                .entry(&object.recording_sha256)
                .or_default()
                .insert(object.kind)
            {
                return Err(CorpusError::InvalidRequest(
                    "recording generation repeats an object role".to_owned(),
                ));
            }
        }
        let required_roles = BTreeSet::from([
            DatasetObjectKind::CaptureProfile,
            DatasetObjectKind::MediaProbe,
            DatasetObjectKind::RecordingManifest,
            DatasetObjectKind::SourceManifest,
            DatasetObjectKind::SourceMedia,
        ]);
        if roles.values().any(|roles| roles != &required_roles) {
            return Err(CorpusError::InvalidRequest(
                "each recording must bind every dataset object role exactly once".to_owned(),
            ));
        }
        self.total_bytes()?;
        Ok(())
    }

    fn total_bytes(&self) -> Result<u64, CorpusError> {
        let total = self.objects.iter().try_fold(0_u64, |total, object| {
            total
                .checked_add(object.bytes)
                .ok_or(CorpusError::CapacityExceeded)
        })?;
        if total > MAX_DATASET_STORAGE_BYTES {
            return Err(CorpusError::CapacityExceeded);
        }
        Ok(total)
    }

    pub(crate) fn summary(&self, generation_sha256: &str) -> Result<DatasetSummary, CorpusError> {
        let recordings = self
            .objects
            .iter()
            .map(|object| object.recording_sha256.as_str())
            .collect::<BTreeSet<_>>();
        Ok(DatasetSummary {
            schema: DATASET_SUMMARY_SCHEMA.to_owned(),
            dataset_id: self.dataset_id.clone(),
            generation_sha256: generation_sha256.to_owned(),
            recording_count: recordings.len() as u64,
            object_count: self.objects.len() as u64,
            total_bytes: self.total_bytes()?,
        })
    }
}

pub(crate) fn validate_object(path: &Path, object: &DatasetObject) -> Result<(), CorpusError> {
    let metadata = path.symlink_metadata()?;
    if !metadata.is_file() || metadata.len() != object.bytes {
        return Err(CorpusError::InvalidRequest(
            "dataset object size or type is invalid".to_owned(),
        ));
    }
    if digest_regular_file(path, MAX_SOURCE_BYTES)? != object.sha256 {
        return Err(CorpusError::InvalidRequest(
            "dataset object digest is invalid".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn ensure_dataset_document_capacity(
    directory: &Path,
    directory_name: &str,
    added_count: usize,
    added_bytes: u64,
) -> Result<(), CorpusError> {
    recover_dataset_document_staging(directory)?;
    let (maximum_count, maximum_storage_bytes) = match directory_name {
        "profiles" => (MAX_SOURCE_OBJECTS, MAX_PROFILE_STORAGE_BYTES),
        "probes" => (MAX_SOURCE_OBJECTS, MAX_PROBE_STORAGE_BYTES),
        "recordings" => (MAX_SOURCE_OBJECTS, MAX_RECORDING_STORAGE_BYTES),
        "dataset-generations" => (MAX_GENERATIONS, MAX_GENERATION_STORAGE_BYTES),
        _ => {
            return Err(CorpusError::InvalidRequest(
                "unknown dataset document directory".to_owned(),
            ));
        }
    };
    let mut count = 0_usize;
    let mut bytes = 0_u64;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|_| {
            CorpusError::InvalidRequest("dataset store contains a non-UTF-8 entry".to_owned())
        })?;
        if name.starts_with(DOCUMENT_STAGING_PREFIX) {
            continue;
        }
        let digest = name.strip_suffix(".json").ok_or_else(|| {
            CorpusError::InvalidRequest("dataset store contains an unrecognized entry".to_owned())
        })?;
        validate_sha256(digest, "dataset document digest", ErrorContext::Request)?;
        let metadata = entry.path().symlink_metadata()?;
        if !metadata.is_file()
            || metadata.len() == 0
            || metadata.len() > MAX_DATASET_DOCUMENT_BYTES as u64
        {
            return Err(CorpusError::InvalidRequest(
                "dataset store contains an invalid document".to_owned(),
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
    if new_count > maximum_count || new_bytes > maximum_storage_bytes {
        return Err(CorpusError::CapacityExceeded);
    }
    Ok(())
}

fn recover_dataset_document_staging(directory: &Path) -> Result<(), CorpusError> {
    let mut changed = false;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|_| {
            CorpusError::InvalidRequest("dataset store contains a non-UTF-8 entry".to_owned())
        })?;
        if !name.starts_with(DOCUMENT_STAGING_PREFIX) {
            continue;
        }
        if !entry.path().symlink_metadata()?.is_file() {
            return Err(CorpusError::InvalidRequest(
                "dataset staging entry is not a regular file".to_owned(),
            ));
        }
        fs::remove_file(entry.path())?;
        changed = true;
    }
    if changed {
        File::open(directory)?.sync_all()?;
    }
    Ok(())
}

pub(crate) fn validate_recording_bundle(
    generation: &RecordingDatasetGeneration,
    manifest_object: &DatasetObject,
    recording_bytes: &[u8],
    source_manifest_bytes: &[u8],
    capture_profile_bytes: &[u8],
    media_probe_bytes: &[u8],
) -> Result<(), CorpusError> {
    let recording: RecordingManifest = serde_json::from_slice(recording_bytes)?;
    recording.validate()?;
    if canonical_json(&recording)? != recording_bytes
        || recording.recording_sha256 != manifest_object.recording_sha256
    {
        return Err(CorpusError::InvalidRequest(
            "recording manifest is not canonical or bound to its generation".to_owned(),
        ));
    }
    let expected = [
        (
            DatasetObjectKind::SourceMedia,
            recording.recording_sha256.as_str(),
            Some(recording.recording_bytes),
        ),
        (
            DatasetObjectKind::SourceManifest,
            recording.source_manifest_sha256.as_str(),
            None,
        ),
        (
            DatasetObjectKind::CaptureProfile,
            recording.capture_profile_sha256.as_str(),
            None,
        ),
        (
            DatasetObjectKind::MediaProbe,
            recording.media_probe_sha256.as_str(),
            None,
        ),
        (
            DatasetObjectKind::RecordingManifest,
            manifest_object.sha256.as_str(),
            Some(manifest_object.bytes),
        ),
    ];
    for (kind, sha256, bytes) in expected {
        let object = required_object(generation, &recording.recording_sha256, kind)?;
        if object.sha256 != sha256 || bytes.is_some_and(|bytes| object.bytes != bytes) {
            return Err(CorpusError::InvalidRequest(
                "recording generation object binding is invalid".to_owned(),
            ));
        }
    }

    let source_manifest: SourceManifest = serde_json::from_slice(source_manifest_bytes)?;
    source_manifest.validate()?;
    if canonical_json(&source_manifest)? != source_manifest_bytes
        || digest_bytes(source_manifest_bytes) != recording.source_manifest_sha256
        || source_manifest.fixture_id != recording.recording_sha256
        || source_manifest.session_id != recording.recording_sha256
        || source_manifest.capture_profile_id != recording.capture_profile_sha256
        || source_manifest.source.sha256 != recording.recording_sha256
        || source_manifest.source.bytes != recording.recording_bytes
    {
        return Err(CorpusError::InvalidRequest(
            "source manifest is not canonical or recording-bound".to_owned(),
        ));
    }

    let capture_profile: CaptureProfile = serde_json::from_slice(capture_profile_bytes)?;
    capture_profile.validate()?;
    if canonical_json(&capture_profile)? != capture_profile_bytes
        || digest_bytes(capture_profile_bytes) != recording.capture_profile_sha256
    {
        return Err(CorpusError::InvalidRequest(
            "capture profile is not canonical or recording-bound".to_owned(),
        ));
    }

    if digest_bytes(media_probe_bytes) != recording.media_probe_sha256 {
        return Err(CorpusError::InvalidRequest(
            "media probe digest is not recording-bound".to_owned(),
        ));
    }
    media::validate_recording_probe_bytes(
        media_probe_bytes,
        &recording.recording_sha256,
        &recording.source_manifest_sha256,
        &source_manifest.source,
        &recording.capture_profile_sha256,
        &capture_profile.observed,
    )?;
    Ok(())
}

pub(crate) fn required_object<'a>(
    generation: &'a RecordingDatasetGeneration,
    recording_sha256: &str,
    kind: DatasetObjectKind,
) -> Result<&'a DatasetObject, CorpusError> {
    generation
        .objects
        .iter()
        .find(|object| object.recording_sha256 == recording_sha256 && object.kind == kind)
        .ok_or_else(|| {
            CorpusError::InvalidRequest(
                "recording generation is missing a required object role".to_owned(),
            )
        })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::os::unix::fs::symlink;
    use std::process::Command;

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    struct ImportedDataset {
        _temporary: tempfile::TempDir,
        private: PathBuf,
        recording: PathBuf,
        store: CorpusStore,
        first: RecordingImportSummary,
        generation: DatasetSummary,
    }

    fn prepare_imported_dataset() -> ImportedDataset {
        let temporary = tempdir().unwrap();
        let private = temporary.path().join("private");
        fs::create_dir(&private).unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
        let recording = private.join("complete-run.mkv");
        let status = Command::new(media::find_executable("ffmpeg").unwrap())
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=320x180:r=3:d=1",
                "-c:v",
                "ffv1",
                "-pix_fmt",
                "rgb24",
                recording.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success());
        let context = private.join("capture-context.json");
        fs::write(
            &context,
            serde_json::to_vec(&json!({
                "schema": CAPTURE_CONTEXT_SCHEMA,
                "route": "portal_pipewire",
                "environment_id": "bazzite-handheld-2026-08",
                "capture_adapter_id": "portal-pipewire",
                "capture_adapter_version": "v1",
                "settings_revision": "obs-free-1080p-v1"
            }))
            .unwrap(),
        )
        .unwrap();

        let store = CorpusStore::new(private.join("store"));
        let first = store.import_recording(&recording, &context).unwrap();
        let second = store.import_recording(&recording, &context).unwrap();
        assert_eq!(first, second);

        let generation = store.seal_recording_dataset("calibration-001").unwrap();
        ImportedDataset {
            _temporary: temporary,
            private,
            recording,
            store,
            first,
            generation,
        }
    }

    fn assert_reusable_source_bytes(fixture: &ImportedDataset) {
        let ImportedDataset {
            private,
            recording,
            store,
            first,
            generation,
            ..
        } = fixture;
        assert_eq!(generation.recording_count, 1);
        assert_eq!(generation.object_count, 5);
        assert_eq!(
            generation,
            &store
                .verify_recording_dataset(&generation.generation_sha256)
                .unwrap()
        );
        assert_eq!(
            fs::read(
                private
                    .join("store/content")
                    .join(&first.recording_sha256)
                    .join(SOURCE_FILE)
            )
            .unwrap(),
            fs::read(recording).unwrap()
        );
    }

    fn assert_typed_role_substitution_is_rejected(fixture: &ImportedDataset) {
        let ImportedDataset {
            store,
            first,
            generation,
            ..
        } = fixture;
        let mut tampered_generation = store
            .load_recording_generation(&generation.generation_sha256)
            .unwrap();
        let mut oversized_generation = tampered_generation.clone();
        oversized_generation
            .objects
            .iter_mut()
            .find(|object| object.kind == DatasetObjectKind::CaptureProfile)
            .unwrap()
            .bytes = MAX_REQUEST_BYTES as u64 + 1;
        assert!(matches!(
            oversized_generation.validate(),
            Err(CorpusError::CapacityExceeded)
        ));
        let recording_object = required_object(
            &tampered_generation,
            &first.recording_sha256,
            DatasetObjectKind::RecordingManifest,
        )
        .unwrap()
        .clone();
        let source_object = required_object(
            &tampered_generation,
            &first.recording_sha256,
            DatasetObjectKind::SourceManifest,
        )
        .unwrap()
        .clone();
        let profile_object = required_object(
            &tampered_generation,
            &first.recording_sha256,
            DatasetObjectKind::CaptureProfile,
        )
        .unwrap()
        .clone();
        let probe_object = required_object(
            &tampered_generation,
            &first.recording_sha256,
            DatasetObjectKind::MediaProbe,
        )
        .unwrap()
        .clone();
        let mut recording_manifest: RecordingManifest = serde_json::from_slice(
            &fs::read(store.dataset_object_path(&recording_object)).unwrap(),
        )
        .unwrap();
        let mut unrelated_source: SourceManifest =
            serde_json::from_slice(&fs::read(store.dataset_object_path(&source_object)).unwrap())
                .unwrap();
        unrelated_source.fixture_id = "another-recording".to_owned();
        unrelated_source.session_id = "another-session".to_owned();
        let unrelated_source_bytes = canonical_json(&unrelated_source).unwrap();
        recording_manifest.source_manifest_sha256 = digest_bytes(&unrelated_source_bytes);
        let tampered_recording_bytes = canonical_json(&recording_manifest).unwrap();
        for object in &mut tampered_generation.objects {
            if object.recording_sha256 == first.recording_sha256 {
                match object.kind {
                    DatasetObjectKind::SourceManifest => {
                        object.sha256 = digest_bytes(&unrelated_source_bytes);
                        object.bytes = unrelated_source_bytes.len() as u64;
                    }
                    DatasetObjectKind::RecordingManifest => {
                        object.sha256 = digest_bytes(&tampered_recording_bytes);
                        object.bytes = tampered_recording_bytes.len() as u64;
                    }
                    _ => {}
                }
            }
        }
        tampered_generation.objects.sort();
        tampered_generation.validate().unwrap();
        let tampered_recording_object = required_object(
            &tampered_generation,
            &first.recording_sha256,
            DatasetObjectKind::RecordingManifest,
        )
        .unwrap();
        assert!(
            validate_recording_bundle(
                &tampered_generation,
                tampered_recording_object,
                &tampered_recording_bytes,
                &unrelated_source_bytes,
                &fs::read(store.dataset_object_path(&profile_object)).unwrap(),
                &fs::read(store.dataset_object_path(&probe_object)).unwrap(),
            )
            .is_err()
        );
    }

    fn assert_intermediate_symlink_is_rejected(fixture: &ImportedDataset) {
        let ImportedDataset {
            private,
            store,
            first,
            generation,
            ..
        } = fixture;
        let content_directory = private.join("store/content").join(&first.recording_sha256);
        let moved_content = private.join("moved-content");
        fs::rename(&content_directory, &moved_content).unwrap();
        symlink(&moved_content, &content_directory).unwrap();
        assert!(
            store
                .verify_recording_dataset(&generation.generation_sha256)
                .is_err()
        );
    }

    #[test]
    fn recording_import_is_idempotent_and_seals_reusable_source_bytes() {
        let fixture = prepare_imported_dataset();
        assert_reusable_source_bytes(&fixture);
        assert_typed_role_substitution_is_rejected(&fixture);
        assert_intermediate_symlink_is_rejected(&fixture);
    }

    #[test]
    fn dataset_document_capacity_is_checked_before_publication() {
        let temporary = tempdir().unwrap();
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let stale = temporary
            .path()
            .join(format!("{DOCUMENT_STAGING_PREFIX}stale"));
        fs::write(&stale, b"stale").unwrap();
        assert!(matches!(
            ensure_dataset_document_capacity(
                temporary.path(),
                "dataset-generations",
                MAX_GENERATIONS + 1,
                1,
            ),
            Err(CorpusError::CapacityExceeded)
        ));
        assert!(!stale.exists());

        let content = temporary.path().join("content");
        let manifests = temporary.path().join("manifests");
        create_private_directory(&content).unwrap();
        create_private_directory(&manifests).unwrap();
        let stale_manifest = manifests.join(format!("{MANIFEST_STAGING_PREFIX}stale"));
        fs::write(&stale_manifest, b"stale").unwrap();
        let store = CorpusStore::new(temporary.path());
        store
            .publish_named_dataset_document("manifests", &digest_bytes(b"fixture"), b"{}\n")
            .unwrap();
        assert!(!stale_manifest.exists());
    }
}
