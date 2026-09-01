use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Component, Path};
use std::time::Duration;

use scorepeek::catalog::CatalogStore;
use scorepeek::recognition::{CanonicalLayout, ScreenClass};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::capture_live::GamescopeLiveSessionEvent;
use crate::diagnostic_live::BoundCanonicalFrame;
use crate::diagnostic_recording::{
    CANONICAL_BYTES, DiagnosticBinding, DiagnosticPolicy, DiagnosticResource, DiagnosticRetention,
    DiagnosticRunDescriptor, DiagnosticRunStatus,
};
use crate::recognition_live::field_observer::{
    DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT, FieldObserverFinishStatus,
};
use crate::recognition_live::field_session::{
    FieldObservationSession, FieldObservationSessionPoll, FieldObservationSubmission,
};

const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RUN_BYTES: u64 = 64 * 1024;
const MAX_QOI_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RETAINED_FRAMES: usize = 8192;
const FIELD_TIMEOUT: Duration = Duration::from_secs(30);
const CANONICAL_WIDTH: u32 = 1920;
const CANONICAL_HEIGHT: u32 = 1080;
const STAGING_MARKER: &str = ".scorepeek-reevaluation-staging-v1";

#[derive(Debug, Deserialize)]
struct SourceSessionManifest {
    schema: String,
    source_kind: String,
    session_id: String,
    capture_generation: u64,
    profile_sha256: String,
    catalog_sha256: String,
    recognition_interval_ms: u64,
    processed_ticks: u64,
    busy_skips: u64,
    maximum_consecutive_busy_skips: u64,
    #[serde(default)]
    field_observation_busy_skips: u64,
    #[serde(default)]
    maximum_consecutive_field_observation_busy_skips: u64,
    completeness: String,
    capture_manifest_sha256: String,
    artifacts: Vec<SourceArtifact>,
}

#[derive(Debug, Deserialize)]
struct SourceArtifact {
    kind: String,
    path: String,
    sha256: String,
    bytes: u64,
}

#[derive(Debug, Deserialize)]
struct CaptureManifest {
    schema: String,
    start: CaptureStartReference,
    frames: Vec<CaptureFrame>,
}

#[derive(Debug, Deserialize)]
struct CaptureStartReference {
    schema: String,
    filename: String,
    file_sha256: String,
    bytes: u64,
}

#[derive(Debug, Deserialize)]
struct CaptureFrame {
    sequence: u64,
    monotonic_start_ms: u64,
    monotonic_end_ms: u64,
    filename: String,
    canonical_pixel_sha256: String,
    file_sha256: String,
    bytes: u64,
}

#[derive(Debug, Deserialize)]
struct CaptureStart {
    schema: String,
    run_id: String,
    binding: CaptureBinding,
}

#[derive(Debug, Deserialize)]
struct CaptureBinding {
    capture_generation: u64,
    capture_profile_sha256: String,
    normalizer_sha256: String,
    canonical_layout_sha256: String,
    catalog_sha256: String,
    model_sha256: String,
    runtime_sha256: String,
}

#[derive(Debug)]
struct LoadedSource {
    manifest: SourceSessionManifest,
    capture: CaptureManifest,
    binding: CaptureBinding,
}

#[derive(Debug)]
struct PublicationLease<'a> {
    output: &'a Path,
    parent: &'a Path,
    _lock: File,
}

#[derive(Debug)]
struct OutputStaging<'a> {
    path: &'a Path,
    keep: bool,
}

impl OutputStaging<'_> {
    const fn path(&self) -> &Path {
        self.path
    }
}

impl Drop for OutputStaging<'_> {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(self.path);
        }
    }
}

#[derive(Serialize)]
pub struct ReevaluationSummary {
    schema: &'static str,
    output: String,
    manifest_sha256: String,
    observations_sha256: String,
    retained_frames: u64,
    field_observations: u64,
    session_reconstructed: bool,
    temporal_domain_events_reconstructed: bool,
}

#[derive(Serialize)]
struct ReevaluationManifest<'a> {
    schema: &'static str,
    source: SourceBinding<'a>,
    evaluator: EvaluatorBinding<'a>,
    coverage: Coverage,
    observations: OutputArtifact,
    screen_counts: BTreeMap<&'static str, u64>,
    field_observations: u64,
}

#[derive(Serialize)]
struct SourceBinding<'a> {
    session_id: &'a str,
    session_sha256: &'a str,
    capture_generation: u64,
    capture_profile_sha256: &'a str,
    normalizer_sha256: &'a str,
    canonical_layout_sha256: &'a str,
    catalog_sha256: &'a str,
    model_sha256: &'a str,
    runtime_sha256: &'a str,
    completeness: &'a str,
}

#[derive(Serialize)]
struct EvaluatorBinding<'a> {
    executable_sha256: &'a str,
    canonical_layout_sha256: &'a str,
    catalog_sha256: &'a str,
    source_catalog_changed: bool,
    model_sha256: &'static str,
    runtime_sha256: &'static str,
}

#[derive(Serialize)]
struct Coverage {
    kind: &'static str,
    retained_frames: u64,
    source_processed_ticks: u64,
    source_busy_skips: u64,
    source_maximum_consecutive_busy_skips: u64,
    source_field_observation_busy_skips: u64,
    source_maximum_consecutive_field_observation_busy_skips: u64,
    source_recognition_interval_ms: u64,
    session_reconstructed: bool,
    temporal_domain_events_reconstructed: bool,
}

#[derive(Serialize)]
struct OutputArtifact {
    path: &'static str,
    sha256: String,
    bytes: u64,
    records: u64,
}

#[derive(Serialize)]
struct FrameReevaluation<'a> {
    schema: &'static str,
    sequence: u64,
    monotonic_start_ms: u64,
    monotonic_end_ms: u64,
    source_qoi_sha256: &'a str,
    canonical_pixel_sha256: &'a str,
    screen: ScreenClass,
    screen_observation: &'a scorepeek::recognition::ScreenPredicateObservation,
    field_observation: Option<Value>,
}

/// Re-runs every retained full canonical QOI through the current production recognizer.
///
/// The retained sequence is intentionally not passed through temporal or domain-event reducers:
/// foreground retention is sparse evidence, not the original recognition cadence.
#[allow(clippy::too_many_lines)]
pub fn reevaluate(
    source: &Path,
    expected_session_sha256: &str,
    output: &Path,
    catalog_root: &Path,
    bundle_root: &Path,
    executable_sha256: &str,
) -> Result<ReevaluationSummary, String> {
    let resolved_output = validate_locations(source, output)?;
    require_sha256(expected_session_sha256, "session SHA-256")?;
    require_sha256(executable_sha256, "executable SHA-256")?;
    let publication = PublicationLease::acquire(&resolved_output)?;
    let loaded = load_source(source, expected_session_sha256)?;
    let active = CatalogStore::new(catalog_root)
        .load_active()
        .map_err(|error| format!("active catalog could not be loaded: {error}"))?
        .ok_or_else(|| "active catalog is unavailable".to_owned())?;
    let canonical_layout_sha256 = CanonicalLayout::sha256();
    let descriptor = DiagnosticRunDescriptor {
        run_id: format!("reevaluate-{}", &expected_session_sha256[..16]),
        monotonic_start_ms: loaded
            .capture
            .frames
            .first()
            .map_or(0, |frame| frame.monotonic_start_ms),
        resource: DiagnosticResource {
            program: "scorepeek",
            version: env!("CARGO_PKG_VERSION"),
            build_sha256: executable_sha256.to_owned(),
        },
        binding: DiagnosticBinding {
            capture_generation: loaded.manifest.capture_generation,
            capture_profile_sha256: loaded.binding.capture_profile_sha256.clone(),
            normalizer_sha256: loaded.binding.normalizer_sha256.clone(),
            canonical_layout_sha256: canonical_layout_sha256.clone(),
            catalog_sha256: active.digest.clone(),
            model_sha256: scorepeek::recognition::LIVE_MODEL_SHA256.to_owned(),
            runtime_sha256: scorepeek::recognition::LIVE_RUNTIME_SHA256.to_owned(),
            replay: None,
        },
    };
    let disabled_diagnostics = DiagnosticPolicy {
        enabled: false,
        sample_interval_ms: 100,
        maximum_run_bytes: 1,
        retention: DiagnosticRetention::CompleteCadence,
    };
    let mut staging = Some(publication.create_staging()?);
    let parent = publication.parent;
    let result = (|| {
        let observations_path = staging
            .as_ref()
            .expect("staging remains owned until publication")
            .path()
            .join("observations.ndjson");
        let mut observations_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&observations_path)
            .map_err(|error| format!("reevaluation observations could not be created: {error}"))?;
        let mut session = FieldObservationSession::start_registered(
            source,
            descriptor,
            disabled_diagnostics,
            catalog_root,
            bundle_root,
            crate::recognition_live::text_observer_pool::RecognitionExecutionMode::Offline,
        )
        .map_err(|error| format!("production recognizer could not start: {error:?}"))?;
        let mut observations_hasher = Sha256::new();
        let mut observations_bytes = 0u64;
        let mut screen_counts = BTreeMap::new();
        let mut field_observations = 0u64;

        let evaluation = (|| {
            for frame_reference in &loaded.capture.frames {
                let pixels = read_frame(source, &loaded.manifest, frame_reference)?;
                let frame = BoundCanonicalFrame::for_replay(
                    loaded.manifest.capture_generation,
                    frame_reference.sequence,
                    frame_reference.monotonic_end_ms,
                    loaded.binding.capture_profile_sha256.clone(),
                    loaded.binding.normalizer_sha256.clone(),
                    pixels,
                )
                .map_err(|_| "retained QOI violates the canonical frame contract".to_owned())?;
                let inspected = session
                    .inspect(&frame)
                    .map_err(|error| format!("current screen recognition failed: {error:?}"))?;
                let screen = inspected.observation.screen();
                *screen_counts.entry(screen_name(screen)).or_insert(0) += 1;
                let field_observation = match inspected.field_submission {
                    FieldObservationSubmission::BusySkipped => {
                        return Err(
                            "re-evaluation unexpectedly skipped field OCR as busy".to_owned()
                        );
                    }
                    FieldObservationSubmission::NotApplicable => None,
                    FieldObservationSubmission::Rejected(error) => {
                        return Err(format!("current field observation was rejected: {error:?}"));
                    }
                    FieldObservationSubmission::Submitted(pending) => {
                        let FieldObservationSessionPoll::Ready { observation, .. } =
                            session.wait_field_observation(&pending, FIELD_TIMEOUT)
                        else {
                            return Err("current field observation did not complete".to_owned());
                        };
                        let output = observation.output().as_ref().map_err(|error| {
                            format!("current field observation failed: {error}")
                        })?;
                        field_observations += 1;
                        Some(crate::live_session_event_value(
                            None,
                            None,
                            GamescopeLiveSessionEvent::Observation {
                                screen_episode_id: 0,
                                sequence: frame_reference.sequence,
                                monotonic_start_ms: frame_reference.monotonic_start_ms,
                                monotonic_end_ms: frame_reference.monotonic_end_ms,
                                output,
                            },
                        )?)
                    }
                };
                let record = FrameReevaluation {
                    schema: "scorepeek-diagnostic-retained-frame-reevaluation-v1",
                    sequence: frame_reference.sequence,
                    monotonic_start_ms: frame_reference.monotonic_start_ms,
                    monotonic_end_ms: frame_reference.monotonic_end_ms,
                    source_qoi_sha256: &frame_reference.file_sha256,
                    canonical_pixel_sha256: &frame_reference.canonical_pixel_sha256,
                    screen,
                    screen_observation: inspected.observation.predicate(),
                    field_observation,
                };
                let mut bytes = serde_json::to_vec(&record).map_err(|error| {
                    format!("reevaluation record serialization failed: {error}")
                })?;
                bytes.push(b'\n');
                observations_file
                    .write_all(&bytes)
                    .map_err(|error| format!("reevaluation observation write failed: {error}"))?;
                observations_hasher.update(&bytes);
                observations_bytes = observations_bytes
                    .checked_add(bytes.len() as u64)
                    .ok_or_else(|| "reevaluation observation size overflow".to_owned())?;
            }
            Ok(())
        })();
        let last_monotonic_ms = loaded
            .capture
            .frames
            .last()
            .map_or(0, |frame| frame.monotonic_end_ms);
        let finish = session.finish(
            if evaluation.is_ok() {
                DiagnosticRunStatus::Success
            } else {
                DiagnosticRunStatus::Error
            },
            last_monotonic_ms,
            DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT,
        );
        evaluation?;
        if finish.field_observer.status != FieldObserverFinishStatus::Complete {
            return Err("production field observer did not finish cleanly".to_owned());
        }
        observations_file
            .sync_all()
            .map_err(|error| format!("reevaluation observations sync failed: {error}"))?;
        let observations_sha256 = encode_digest(observations_hasher.finalize());
        let retained_frames = loaded.capture.frames.len() as u64;
        let manifest = ReevaluationManifest {
            schema: "scorepeek-private-diagnostic-reevaluation-v1",
            source: SourceBinding {
                session_id: &loaded.manifest.session_id,
                session_sha256: expected_session_sha256,
                capture_generation: loaded.manifest.capture_generation,
                capture_profile_sha256: &loaded.binding.capture_profile_sha256,
                normalizer_sha256: &loaded.binding.normalizer_sha256,
                canonical_layout_sha256: &loaded.binding.canonical_layout_sha256,
                catalog_sha256: &loaded.manifest.catalog_sha256,
                model_sha256: &loaded.binding.model_sha256,
                runtime_sha256: &loaded.binding.runtime_sha256,
                completeness: &loaded.manifest.completeness,
            },
            evaluator: EvaluatorBinding {
                executable_sha256,
                canonical_layout_sha256: &canonical_layout_sha256,
                catalog_sha256: &active.digest,
                source_catalog_changed: loaded.manifest.catalog_sha256 != active.digest,
                model_sha256: scorepeek::recognition::LIVE_MODEL_SHA256,
                runtime_sha256: scorepeek::recognition::LIVE_RUNTIME_SHA256,
            },
            coverage: Coverage {
                kind: "retained_full_frame_qoi",
                retained_frames,
                source_processed_ticks: loaded.manifest.processed_ticks,
                source_busy_skips: loaded.manifest.busy_skips,
                source_maximum_consecutive_busy_skips: loaded
                    .manifest
                    .maximum_consecutive_busy_skips,
                source_field_observation_busy_skips: loaded.manifest.field_observation_busy_skips,
                source_maximum_consecutive_field_observation_busy_skips: loaded
                    .manifest
                    .maximum_consecutive_field_observation_busy_skips,
                source_recognition_interval_ms: loaded.manifest.recognition_interval_ms,
                session_reconstructed: false,
                temporal_domain_events_reconstructed: false,
            },
            observations: OutputArtifact {
                path: "observations.ndjson",
                sha256: observations_sha256.clone(),
                bytes: observations_bytes,
                records: retained_frames,
            },
            screen_counts,
            field_observations,
        };
        let mut manifest_bytes = serde_json::to_vec(&manifest)
            .map_err(|error| format!("reevaluation manifest serialization failed: {error}"))?;
        manifest_bytes.push(b'\n');
        write_private_file(
            &staging
                .as_ref()
                .expect("staging remains owned until publication")
                .path()
                .join("manifest.json"),
            &manifest_bytes,
        )?;
        File::open(
            staging
                .as_ref()
                .expect("staging remains owned until publication")
                .path(),
        )
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("reevaluation staging sync failed: {error}"))?;
        let manifest_sha256 = digest_bytes(&manifest_bytes);
        publication.publish(
            staging
                .take()
                .expect("staging is consumed exactly once by publication"),
        )?;

        Ok(ReevaluationSummary {
            schema: "scorepeek-diagnostic-reevaluation-summary-v1",
            output: output.display().to_string(),
            manifest_sha256,
            observations_sha256,
            retained_frames,
            field_observations,
            session_reconstructed: false,
            temporal_domain_events_reconstructed: false,
        })
    })();
    match result {
        Ok(summary) => Ok(summary),
        Err(error) => match staging.take() {
            None => Err(error),
            Some(staging) => rollback_output(staging, &error, &mut || sync_directory(parent)),
        },
    }
}

fn validate_locations(source: &Path, output: &Path) -> Result<std::path::PathBuf, String> {
    if !source.is_absolute() || !source.is_dir() {
        return Err("diagnostic source session must be an absolute directory".to_owned());
    }
    if !output.is_absolute() || output.as_os_str().is_empty() {
        return Err("reevaluation output must be an absolute path".to_owned());
    }
    let parent = output
        .parent()
        .ok_or_else(|| "reevaluation output must have a parent".to_owned())?;
    if !parent.is_dir() {
        return Err("reevaluation output parent must exist".to_owned());
    }
    let canonical_source = source
        .canonicalize()
        .map_err(|error| format!("diagnostic source session could not be resolved: {error}"))?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|error| format!("reevaluation output parent could not be resolved: {error}"))?;
    let resolved = canonical_parent.join(
        output
            .file_name()
            .ok_or_else(|| "reevaluation output must have a filename".to_owned())?,
    );
    if resolved.starts_with(canonical_source) {
        return Err("reevaluation output must be outside the source session".to_owned());
    }
    Ok(resolved)
}

impl<'a> PublicationLease<'a> {
    fn acquire(output: &'a Path) -> Result<Self, String> {
        let parent = output
            .parent()
            .ok_or_else(|| "reevaluation output must have a parent".to_owned())?;
        let key = digest_bytes(output.as_os_str().as_bytes());
        let staging_prefix = format!(".scorepeek-reevaluation-{}-", &key[..16]);
        let lock_path = parent.join(format!("{staging_prefix}lock"));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(lock_path)
            .map_err(|error| format!("reevaluation publication lock failed: {error}"))?;
        lock.lock()
            .map_err(|error| format!("reevaluation publication lock failed: {error}"))?;
        if output.symlink_metadata().is_ok() {
            if !valid_owned_staging(output) {
                return Err("reevaluation output already exists".to_owned());
            }
            fs::remove_dir_all(output).map_err(|error| {
                format!("reevaluation staging recovery failed: {error}; output may exist")
            })?;
            sync_directory(parent).map_err(|error| {
                format!("reevaluation staging recovery sync failed: {error}; output may exist")
            })?;
        }
        Ok(Self {
            output,
            parent,
            _lock: lock,
        })
    }

    fn create_staging(&self) -> Result<OutputStaging<'a>, String> {
        self.create_staging_with(|| sync_directory(self.parent))
    }

    fn create_staging_with(
        &self,
        mut sync_parent: impl FnMut() -> io::Result<()>,
    ) -> Result<OutputStaging<'a>, String> {
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(self.output)
            .map_err(|error| format!("reevaluation output reservation failed: {error}"))?;
        let staging = OutputStaging {
            path: self.output,
            keep: false,
        };
        let result = write_private_file(&self.output.join(STAGING_MARKER), b"v1\n")
            .and_then(|()| {
                sync_directory(self.output).map_err(|error| {
                    format!("reevaluation output reservation sync failed: {error}")
                })
            })
            .and_then(|()| {
                sync_parent().map_err(|error| {
                    format!("reevaluation output reservation sync failed: {error}")
                })
            });
        match result {
            Ok(()) => Ok(staging),
            Err(error) => rollback_output(staging, &error, &mut sync_parent),
        }
    }

    fn publish(self, staging: OutputStaging<'a>) -> Result<(), String> {
        let parent = self.parent;
        self.publish_with(staging, || sync_directory(parent))
    }

    fn publish_with(
        self,
        staging: OutputStaging<'a>,
        sync_parent: impl FnMut() -> io::Result<()>,
    ) -> Result<(), String> {
        let marker = self.output.join(STAGING_MARKER);
        self.publish_with_operations(staging, || fs::remove_file(&marker), sync_parent)
    }

    fn publish_with_operations(
        self,
        mut staging: OutputStaging<'a>,
        mut remove_marker: impl FnMut() -> io::Result<()>,
        mut sync_parent: impl FnMut() -> io::Result<()>,
    ) -> Result<(), String> {
        if let Err(error) = remove_marker() {
            return rollback_output(
                staging,
                &format!("reevaluation publication marker failed: {error}"),
                &mut sync_parent,
            );
        }
        if sync_directory(self.output).is_ok() && sync_parent().is_ok() {
            staging.keep = true;
            return Ok(());
        }
        if fs::remove_dir_all(self.output).is_ok() && sync_parent().is_ok() {
            staging.keep = true;
            return Err(
                "reevaluation publication durability failed; output rolled back".to_owned(),
            );
        }
        staging.keep = true;
        Err("reevaluation publication durability is uncertain; output may exist".to_owned())
    }
}

fn rollback_output<T>(
    mut staging: OutputStaging<'_>,
    cause: &str,
    sync_parent: &mut impl FnMut() -> io::Result<()>,
) -> Result<T, String> {
    if fs::remove_dir_all(staging.path).is_ok() && sync_parent().is_ok() {
        staging.keep = true;
        return Err(format!("{cause}; output rolled back"));
    }
    staging.keep = true;
    Err(format!("{cause}; output may exist"))
}

fn valid_owned_staging(path: &Path) -> bool {
    let Ok(metadata) = path.symlink_metadata() else {
        return false;
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return false;
    }
    let Ok(marker) = fs::read(path.join(STAGING_MARKER)) else {
        return false;
    };
    if marker != b"v1\n" {
        return false;
    }
    let Ok(mut entries) = fs::read_dir(path) else {
        return false;
    };
    entries.all(|entry| {
        let Ok(entry) = entry else {
            return false;
        };
        let name = entry.file_name();
        matches!(
            name.to_str(),
            Some(STAGING_MARKER | "observations.ndjson" | "manifest.json")
        ) && entry
            .path()
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
    })
}

fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

fn load_source(source: &Path, expected_session_sha256: &str) -> Result<LoadedSource, String> {
    let manifest_bytes = read_bounded(&source.join("manifest.json"), MAX_MANIFEST_BYTES)?;
    if digest_bytes(&manifest_bytes) != expected_session_sha256 {
        return Err("diagnostic session manifest digest differs".to_owned());
    }
    let manifest: SourceSessionManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("diagnostic session manifest is invalid: {error}"))?;
    validate_source_manifest(&manifest)?;
    let capture_artifact = artifact(&manifest, "capture/manifest.json")?;
    if capture_artifact.sha256 != manifest.capture_manifest_sha256 {
        return Err("capture manifest binding differs".to_owned());
    }
    let capture_bytes = read_artifact(source, capture_artifact, MAX_MANIFEST_BYTES)?;
    let capture: CaptureManifest = serde_json::from_slice(&capture_bytes)
        .map_err(|error| format!("capture manifest is invalid: {error}"))?;
    validate_capture_manifest(&capture)?;
    let run_artifact = artifact(&manifest, "capture/run.json")?;
    if capture.start.schema != "scorepeek-private-diagnostic-artifact-v1"
        || capture.start.filename != "run.json"
        || capture.start.file_sha256 != run_artifact.sha256
        || capture.start.bytes != run_artifact.bytes
    {
        return Err("capture start binding differs".to_owned());
    }
    let run_bytes = read_artifact(source, run_artifact, MAX_RUN_BYTES)?;
    let start: CaptureStart = serde_json::from_slice(&run_bytes)
        .map_err(|error| format!("capture start is invalid: {error}"))?;
    if !matches!(
        start.schema.as_str(),
        "scorepeek-private-diagnostic-capture-start-v3"
            | "scorepeek-private-diagnostic-capture-start-v4"
    ) || start.run_id != manifest.session_id
        || start.binding.capture_generation != manifest.capture_generation
        || start.binding.capture_profile_sha256 != manifest.profile_sha256
        || !valid_sha256(&start.binding.normalizer_sha256)
        || !valid_sha256(&start.binding.canonical_layout_sha256)
        || start.binding.catalog_sha256 != manifest.catalog_sha256
        || !valid_sha256(&start.binding.model_sha256)
        || !valid_sha256(&start.binding.runtime_sha256)
    {
        return Err("capture start does not match the diagnostic session".to_owned());
    }
    Ok(LoadedSource {
        manifest,
        capture,
        binding: start.binding,
    })
}

fn validate_source_manifest(manifest: &SourceSessionManifest) -> Result<(), String> {
    if !matches!(
        manifest.schema.as_str(),
        "scorepeek-private-diagnostic-session-v3" | "scorepeek-private-diagnostic-session-v4"
    ) || manifest.source_kind != "live_run"
        || manifest.session_id.is_empty()
        || manifest.capture_generation == 0
        || !valid_sha256(&manifest.profile_sha256)
        || !valid_sha256(&manifest.catalog_sha256)
        || !valid_sha256(&manifest.capture_manifest_sha256)
        || manifest.recognition_interval_ms == 0
        || !matches!(manifest.completeness.as_str(), "complete" | "partial")
        || manifest.artifacts.len() > 20_000
    {
        return Err("diagnostic session manifest contract is invalid".to_owned());
    }
    let mut paths = BTreeSet::new();
    for item in &manifest.artifacts {
        if item.kind.is_empty()
            || !safe_relative_path(&item.path)
            || !valid_sha256(&item.sha256)
            || item.bytes == 0
            || !paths.insert(item.path.as_str())
        {
            return Err("diagnostic session artifact inventory is invalid".to_owned());
        }
    }
    Ok(())
}

fn validate_capture_manifest(manifest: &CaptureManifest) -> Result<(), String> {
    if !matches!(
        manifest.schema.as_str(),
        "scorepeek-private-diagnostic-capture-v3" | "scorepeek-private-diagnostic-capture-v4"
    ) || manifest.frames.len() > MAX_RETAINED_FRAMES
    {
        return Err("capture manifest contract is invalid".to_owned());
    }
    let mut previous_sequence = None;
    for frame in &manifest.frames {
        if frame.filename != format!("frame-{:020}.qoi", frame.sequence)
            || frame.monotonic_end_ms < frame.monotonic_start_ms
            || previous_sequence.is_some_and(|previous| previous >= frame.sequence)
            || !valid_sha256(&frame.canonical_pixel_sha256)
            || !valid_sha256(&frame.file_sha256)
            || frame.bytes == 0
            || frame.bytes > MAX_QOI_BYTES
        {
            return Err("capture frame inventory is invalid".to_owned());
        }
        previous_sequence = Some(frame.sequence);
    }
    Ok(())
}

fn read_frame(
    source: &Path,
    session: &SourceSessionManifest,
    frame: &CaptureFrame,
) -> Result<Box<[u8]>, String> {
    let relative = format!("capture/{}", frame.filename);
    let item = artifact(session, &relative)?;
    if item.sha256 != frame.file_sha256 || item.bytes != frame.bytes {
        return Err("retained QOI inventory binding differs".to_owned());
    }
    let encoded = read_artifact(source, item, MAX_QOI_BYTES)?;
    decode_canonical_qoi(&encoded, &frame.filename, &frame.canonical_pixel_sha256)
}

fn decode_canonical_qoi(
    encoded: &[u8],
    filename: &str,
    expected_pixel_sha256: &str,
) -> Result<Box<[u8]>, String> {
    let header = qoi::decode_header(encoded)
        .map_err(|_| format!("retained QOI {filename} could not be decoded"))?;
    if header.width != CANONICAL_WIDTH
        || header.height != CANONICAL_HEIGHT
        || !header.channels.is_rgb()
    {
        return Err(format!(
            "retained QOI {filename} is not the complete canonical frame"
        ));
    }
    let mut pixels = vec![0; CANONICAL_BYTES];
    qoi::decode_to_buf(&mut pixels, encoded)
        .map_err(|_| format!("retained QOI {filename} could not be decoded"))?;
    if digest_bytes(&pixels) != expected_pixel_sha256 {
        return Err(format!("retained QOI {filename} pixel digest differs"));
    }
    Ok(pixels.into_boxed_slice())
}

fn artifact<'a>(
    manifest: &'a SourceSessionManifest,
    relative: &str,
) -> Result<&'a SourceArtifact, String> {
    manifest
        .artifacts
        .iter()
        .find(|item| item.path == relative)
        .ok_or_else(|| format!("diagnostic session artifact is missing: {relative}"))
}

fn read_artifact(source: &Path, artifact: &SourceArtifact, limit: u64) -> Result<Vec<u8>, String> {
    if artifact.bytes > limit {
        return Err(format!(
            "diagnostic artifact is oversized: {}",
            artifact.path
        ));
    }
    let bytes = read_bounded(&source.join(&artifact.path), limit)?;
    if bytes.len() as u64 != artifact.bytes || digest_bytes(&bytes) != artifact.sha256 {
        return Err(format!(
            "diagnostic artifact binding differs: {}",
            artifact.path
        ));
    }
    Ok(bytes)
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let metadata = path
        .symlink_metadata()
        .map_err(|error| format!("diagnostic artifact metadata failed: {error}"))?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > limit {
        return Err("diagnostic artifact is not a bounded regular file".to_owned());
    }
    let file = File::open(path)
        .map_err(|error| format!("diagnostic artifact could not be opened: {error}"))?;
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| "diagnostic artifact size cannot be represented".to_owned())?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("diagnostic artifact could not be read: {error}"))?;
    if bytes.len() as u64 != metadata.len() {
        return Err("diagnostic artifact changed while being read".to_owned());
    }
    Ok(bytes)
}

fn safe_relative_path(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("reevaluation manifest could not be created: {error}"))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("reevaluation manifest write failed: {error}"))
}

fn require_sha256(value: &str, label: &str) -> Result<(), String> {
    valid_sha256(value)
        .then_some(())
        .ok_or_else(|| format!("{label} is invalid"))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn digest_bytes(bytes: &[u8]) -> String {
    encode_digest(Sha256::digest(bytes))
}

fn encode_digest(bytes: impl AsRef<[u8]>) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in bytes.as_ref() {
        use std::fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

const fn screen_name(screen: ScreenClass) -> &'static str {
    match screen {
        ScreenClass::Result => "result",
        ScreenClass::MusicSelect => "music_select",
        ScreenClass::ModeSelect => "mode_select",
        ScreenClass::DecideTransition => "decide_transition",
        ScreenClass::Play => "play",
        ScreenClass::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::os::unix::fs::{MetadataExt as _, symlink};
    use std::path::Path;

    use serde_json::json;

    use super::{
        PublicationLease, decode_canonical_qoi, digest_bytes, load_source, read_frame,
        rollback_output, validate_locations,
    };

    struct Fixture {
        root: tempfile::TempDir,
        session_sha256: String,
    }

    #[test]
    fn retained_complete_canonical_qoi_is_accepted() {
        let fixture = fixture(1920, 1080);
        let loaded = load_source(fixture.root.path(), &fixture.session_sha256).unwrap();
        let pixels = read_frame(
            fixture.root.path(),
            &loaded.manifest,
            &loaded.capture.frames[0],
        )
        .unwrap();
        assert_eq!(pixels.len(), 1920 * 1080 * 3);
    }

    #[test]
    fn retained_crop_is_not_treated_as_a_complete_frame() {
        let fixture = fixture(200, 100);
        let loaded = load_source(fixture.root.path(), &fixture.session_sha256).unwrap();
        let error = read_frame(
            fixture.root.path(),
            &loaded.manifest,
            &loaded.capture.frames[0],
        )
        .unwrap_err();
        assert!(error.contains("not the complete canonical frame"));
    }

    #[test]
    fn oversized_qoi_header_is_rejected_before_pixel_allocation() {
        let mut encoded = b"qoif".to_vec();
        encoded.extend_from_slice(&20_000u32.to_be_bytes());
        encoded.extend_from_slice(&20_000u32.to_be_bytes());
        encoded.extend_from_slice(&[3, 0]);
        let error = decode_canonical_qoi(&encoded, "oversized.qoi", &"0".repeat(64)).unwrap_err();
        assert!(error.contains("not the complete canonical frame"));
    }

    #[test]
    fn source_manifest_digest_is_mandatory() {
        let fixture = fixture(1920, 1080);
        let error = load_source(fixture.root.path(), &"f".repeat(64)).unwrap_err();
        assert_eq!(error, "diagnostic session manifest digest differs");
    }

    #[test]
    fn output_cannot_be_inside_the_source_or_its_symlink_alias() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        fs::create_dir(&source).unwrap();
        let direct = validate_locations(&source, &source.join("evaluation")).unwrap_err();
        assert!(direct.contains("outside the source session"));

        let alias = root.path().join("source-alias");
        symlink(&source, &alias).unwrap();
        let aliased = validate_locations(&source, &alias.join("evaluation")).unwrap_err();
        assert!(aliased.contains("outside the source session"));
    }

    #[test]
    fn publication_does_not_replace_a_destination_created_after_reservation() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("evaluation");
        let lease = PublicationLease::acquire(&output).unwrap();
        fs::create_dir(&output).unwrap();
        let inode = output.metadata().unwrap().ino();
        let error = lease.create_staging().unwrap_err();
        assert!(error.contains("reservation failed"));
        assert_eq!(output.metadata().unwrap().ino(), inode);
    }

    #[test]
    fn parent_sync_failure_rolls_back_the_published_output() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("evaluation");
        let lease = PublicationLease::acquire(&output).unwrap();
        let staging = lease.create_staging().unwrap();
        let sync_calls = Cell::new(0);
        let error = lease
            .publish_with(staging, || {
                sync_calls.set(sync_calls.get() + 1);
                if sync_calls.get() == 1 {
                    Err(std::io::Error::other("injected sync failure"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert!(error.contains("rolled back"));
        assert!(!output.exists());
        assert_eq!(sync_calls.get(), 2);
    }

    #[test]
    fn marker_removal_failure_reports_rollback_or_uncertainty() {
        for recovery_sync_succeeds in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let output = root.path().join("evaluation");
            let lease = PublicationLease::acquire(&output).unwrap();
            let staging = lease.create_staging().unwrap();
            let error = lease
                .publish_with_operations(
                    staging,
                    || Err(std::io::Error::other("injected marker removal failure")),
                    || {
                        recovery_sync_succeeds
                            .then_some(())
                            .ok_or_else(|| std::io::Error::other("injected cleanup sync failure"))
                    },
                )
                .unwrap_err();
            assert!(error.contains("publication marker failed"));
            if recovery_sync_succeeds {
                assert!(error.contains("output rolled back"));
            } else {
                assert!(error.contains("output may exist"));
            }
            assert!(!output.exists());
        }
    }

    #[test]
    fn reservation_parent_sync_failure_reports_rollback_or_uncertainty() {
        for recovery_sync_succeeds in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let output = root.path().join("evaluation");
            let lease = PublicationLease::acquire(&output).unwrap();
            let sync_calls = Cell::new(0);
            let error = lease
                .create_staging_with(|| {
                    sync_calls.set(sync_calls.get() + 1);
                    if sync_calls.get() == 1 || !recovery_sync_succeeds {
                        Err(std::io::Error::other("injected sync failure"))
                    } else {
                        Ok(())
                    }
                })
                .unwrap_err();
            if recovery_sync_succeeds {
                assert!(error.contains("output rolled back"));
            } else {
                assert!(error.contains("output may exist"));
            }
            assert!(!output.exists());
            assert_eq!(sync_calls.get(), 2);
        }
    }

    #[test]
    fn post_reservation_failure_reports_rollback_or_uncertainty() {
        for recovery_sync_succeeds in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let output = root.path().join("evaluation");
            let lease = PublicationLease::acquire(&output).unwrap();
            let staging = lease.create_staging().unwrap();
            let result: Result<(), String> =
                rollback_output(staging, "injected evaluation failure", &mut || {
                    recovery_sync_succeeds
                        .then_some(())
                        .ok_or_else(|| std::io::Error::other("injected cleanup sync failure"))
                });
            let error = result.unwrap_err();
            if recovery_sync_succeeds {
                assert!(error.contains("output rolled back"));
            } else {
                assert!(error.contains("output may exist"));
            }
            assert!(!output.exists());
        }
    }

    #[test]
    fn next_writer_recovers_only_marker_bound_staging() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("evaluation");
        let first = PublicationLease::acquire(&output).unwrap();
        let mut staging = first.create_staging().unwrap();
        staging.keep = true;
        drop(staging);
        drop(first);

        let second = PublicationLease::acquire(&output).unwrap();
        assert!(!output.exists());

        fs::create_dir(&output).unwrap();
        drop(second);
        let error = PublicationLease::acquire(&output).unwrap_err();
        assert!(error.contains("already exists"));
        assert!(output.exists());
    }

    fn fixture(width: u32, height: u32) -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let capture = root.path().join("capture");
        fs::create_dir(&capture).unwrap();
        let pixel_count = usize::try_from(width * height * 3).unwrap();
        let pixels = vec![0x24; pixel_count];
        let encoded = qoi::encode_to_vec(&pixels, width, height).unwrap();
        let frame_name = "frame-00000000000000000001.qoi";
        write(&capture.join(frame_name), &encoded);

        let profile_sha256 = "a".repeat(64);
        let catalog_sha256 = "b".repeat(64);
        let run = serde_json::to_vec(&json!({
            "schema": "scorepeek-private-diagnostic-capture-start-v3",
            "run_id": "session-1",
            "binding": {
                "capture_generation": 7,
                "capture_profile_sha256": profile_sha256,
                "normalizer_sha256": "c".repeat(64),
                "canonical_layout_sha256": "d".repeat(64),
                "catalog_sha256": catalog_sha256,
                "model_sha256": "e".repeat(64),
                "runtime_sha256": "f".repeat(64)
            }
        }))
        .unwrap();
        write(&capture.join("run.json"), &run);
        let frame_sha256 = digest_bytes(&encoded);
        let pixel_sha256 = digest_bytes(&pixels);
        let run_sha256 = digest_bytes(&run);
        let capture_manifest = serde_json::to_vec(&json!({
            "schema": "scorepeek-private-diagnostic-capture-v3",
            "start": {
                "schema": "scorepeek-private-diagnostic-artifact-v1",
                "filename": "run.json",
                "file_sha256": run_sha256,
                "bytes": run.len()
            },
            "frames": [{
                "sequence": 1,
                "monotonic_start_ms": 100,
                "monotonic_end_ms": 101,
                "filename": frame_name,
                "canonical_pixel_sha256": pixel_sha256,
                "file_sha256": frame_sha256,
                "bytes": encoded.len()
            }]
        }))
        .unwrap();
        write(&capture.join("manifest.json"), &capture_manifest);
        let capture_manifest_sha256 = digest_bytes(&capture_manifest);
        let artifacts = vec![
            artifact("capture/manifest.json", &capture_manifest),
            artifact("capture/run.json", &run),
            artifact(&format!("capture/{frame_name}"), &encoded),
        ];
        let session_manifest = serde_json::to_vec(&json!({
            "schema": "scorepeek-private-diagnostic-session-v3",
            "source_kind": "live_run",
            "session_id": "session-1",
            "capture_generation": 7,
            "profile_sha256": profile_sha256,
            "catalog_sha256": catalog_sha256,
            "recognition_interval_ms": 100,
            "processed_ticks": 20,
            "busy_skips": 2,
            "maximum_consecutive_busy_skips": 1,
            "completeness": "complete",
            "capture_manifest_sha256": capture_manifest_sha256,
            "artifacts": artifacts
        }))
        .unwrap();
        write(&root.path().join("manifest.json"), &session_manifest);
        Fixture {
            root,
            session_sha256: digest_bytes(&session_manifest),
        }
    }

    fn artifact(path: &str, bytes: &[u8]) -> serde_json::Value {
        json!({
            "kind": "capture",
            "path": path,
            "sha256": digest_bytes(bytes),
            "bytes": bytes.len()
        })
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
    }
}
