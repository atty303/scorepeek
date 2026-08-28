use std::collections::BTreeMap;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use scorepeek::capture::{UncalibratedMemoryType, UncalibratedVideoContract};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::diagnostic_recording::{
    DEFAULT_AGGREGATE_BYTES, DiagnosticCompleteness, DiagnosticErrorType, DiagnosticRetention,
    DiagnosticRunStatus, MANIFEST_RESERVE_BYTES, MAX_DEGRADATIONS_PER_RUN, MAX_FACT_BYTES,
    MAX_FACTS_PER_RUN, MAX_FRAMES_PER_RUN, NORMAL_RETENTION_HOURS, PRIORITY_RETENTION_HOURS,
};
use crate::publish_private_file;
const MAX_RUNS: usize = 8_192;
const MAX_FILES_PER_RUN: usize = 50_000;
const MAX_START_BYTES: u64 = 64 * 1024;
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SOURCE_FRAME_BYTES: u64 = 128 * 1024 * 1024;
const STORE_LOCK_FILENAME: &str = ".scorepeek-diagnostic-store.lock";
const DELETE_STAGING_PREFIX: &str = ".scorepeek-diagnostic-delete-";
const DELETE_MARKER_FILENAME: &str = ".scorepeek-diagnostic-delete-owner-v1";
const DELETE_MARKER_STAGING_FILENAME: &str = ".scorepeek-diagnostic-delete-owner-staging-v1";
const FREEZE_FILENAME: &str = ".scorepeek-diagnostic-freeze-v1.json";
const FREEZE_STAGING_FILENAME: &str = ".scorepeek-diagnostic-freeze-staging-v1.json";
const MAX_CONTROL_BYTES: u64 = 16 * 1024;
const EXPORT_MANIFEST_FILENAME: &str = "export.json";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DeleteMarkerDocument {
    schema: String,
    run_id: String,
    files: Vec<DeleteMarkerFile>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DeleteMarkerFile {
    filename: String,
    bytes: u64,
}

#[derive(Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct FreezeDocument {
    schema: String,
    run_id: String,
    run_sha256: String,
    manifest_sha256: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticControlOutcome {
    schema: &'static str,
    operation: &'static str,
    run_id: String,
    run_sha256: String,
    manifest_sha256: Option<String>,
    frozen: bool,
    managed_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticExportOutcome {
    schema: &'static str,
    operation: &'static str,
    run_id: String,
    run_sha256: String,
    manifest_sha256: String,
    file_count: usize,
    exported_bytes: u64,
}

#[derive(Serialize)]
struct ExportManifestDocument {
    schema: &'static str,
    run_id: String,
    run_sha256: String,
    manifest_sha256: String,
    files: Vec<ExportFileDocument>,
    artifact_bytes: u64,
}

#[derive(Serialize)]
struct ExportFileDocument {
    filename: String,
    sha256: String,
    bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticStoreStatus {
    schema: &'static str,
    recording_enabled_by_default: bool,
    remote_export_enabled: bool,
    writer_active: bool,
    aggregate_retention_bytes: u64,
    normal_retention_hours: u32,
    priority_retention_hours: u32,
    managed_bytes: u64,
    remaining_bytes: u64,
    run_count: usize,
    complete_count: usize,
    partial_count: usize,
    dropped_count: usize,
    priority_count: usize,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticRunList {
    schema: &'static str,
    runs: Vec<DiagnosticRunSummary>,
}

#[derive(Debug, Serialize)]
pub(crate) struct DiagnosticRunSummary {
    pub(crate) run_id: String,
    run_sha256: String,
    manifest_sha256: Option<String>,
    status: Option<DiagnosticRunStatus>,
    completeness: DiagnosticCompleteness,
    frozen: bool,
    pub(crate) priority: bool,
    pub(crate) managed_bytes: u64,
    #[serde(skip)]
    pub(crate) retention_time: SystemTime,
}

pub(crate) struct DiagnosticStoreLease {
    root: std::path::PathBuf,
    root_inode: (u64, u64),
    #[cfg_attr(not(test), allow(dead_code))]
    anchor_path: std::path::PathBuf,
    _anchor_lock: File,
    _root_lock: File,
    managed_bytes: u64,
    normal_candidates: Vec<DiagnosticRunSummary>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RunStartDocument {
    schema: String,
    run_id: String,
    monotonic_start_ms: u64,
    resource: RunResource,
    binding: RunBinding,
    policy: RunPolicy,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LegacyRunStartDocument {
    schema: String,
    run_id: String,
    monotonic_start_ms: u64,
    resource: RunResource,
    binding: RunBinding,
    policy: LegacyRunPolicy,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RunResource {
    program: String,
    version: String,
    build_sha256: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RunBinding {
    capture_generation: u64,
    capture_profile_sha256: String,
    normalizer_sha256: String,
    canonical_layout_sha256: String,
    catalog_sha256: String,
    model_sha256: String,
    runtime_sha256: String,
    replay: Option<RunReplayBinding>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RunReplayBinding {
    request_sha256: String,
    extraction_sha256: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RunPolicy {
    sample_interval_ms: u64,
    maximum_run_bytes: u64,
    aggregate_retention_bytes: u64,
    normal_retention_hours: u32,
    priority_retention_hours: u32,
    remote_export_enabled: bool,
    retention: DiagnosticRetention,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LegacyRunPolicy {
    sample_interval_ms: u64,
    maximum_run_bytes: u64,
    aggregate_retention_bytes: u64,
    normal_retention_hours: u32,
    priority_retention_hours: u32,
    remote_export_enabled: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunManifestDocument {
    schema: String,
    monotonic_end_ms: u64,
    status: DiagnosticRunStatus,
    completeness: DiagnosticCompleteness,
    dropped_count: u64,
    last_error_type: Option<DiagnosticErrorType>,
    maximum_observation_gap_ms: Option<u64>,
    result_miss_denominator_eligible: bool,
    artifact_bytes: u64,
    manifest_bytes: u64,
    total_bytes: u64,
    start: StartReference,
    frames: Vec<FrameReference>,
    facts: FactManifest,
    degradations: Vec<DegradationReference>,
    degradation_entries_dropped: u64,
    degradation_reason_counts: Vec<DegradationReasonCount>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartReference {
    schema: String,
    filename: String,
    file_sha256: String,
    bytes: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FrameReference {
    sequence: u64,
    monotonic_start_ms: u64,
    monotonic_end_ms: u64,
    filename: String,
    canonical_pixel_sha256: String,
    file_sha256: String,
    bytes: u64,
    #[serde(default)]
    source: Option<SourceFrameReference>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceFrameReference {
    filename: String,
    #[serde(default)]
    source_sequence: Option<u64>,
    #[serde(default)]
    pixel_format: Option<String>,
    #[serde(default)]
    observed_pixel_format: Option<String>,
    #[serde(default)]
    encoded_pixel_format: Option<String>,
    video: UncalibratedVideoContract,
    memory_type: UncalibratedMemoryType,
    stride: u32,
    received_monotonic_ns: u64,
    file_sha256: String,
    bytes: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FactReference {
    index: u64,
    sequence: u64,
    filename: String,
    file_sha256: String,
    bytes: u64,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FactManifest {
    Legacy(Vec<FactReference>),
    Ndjson(NdjsonReference),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NdjsonReference {
    filename: String,
    record_count: u64,
    first_sequence: Option<u64>,
    last_sequence: Option<u64>,
    file_sha256: String,
    bytes: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DegradationReference {
    reason: DiagnosticErrorType,
    affected_sequence: Option<u64>,
    first_missing_sequence: Option<u64>,
    last_missing_sequence: Option<u64>,
    known_missing_count: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DegradationReasonCount {
    reason: DiagnosticErrorType,
    count: u64,
}

/// Returns a bounded aggregate view of the application-owned diagnostic store.
///
/// # Errors
/// Returns a value-free error when the root or any managed run is invalid or changes while read.
pub fn diagnostic_store_status(root: &Path) -> Result<DiagnosticStoreStatus, String> {
    let (writer_active, _idle_guard) = diagnostic_writer_activity(root)?;
    let runs = inspect_store(root)?;
    let managed_bytes = runs.iter().try_fold(0_u64, |total, run| {
        total
            .checked_add(run.managed_bytes)
            .ok_or_else(invalid_store)
    })?;
    let remaining_bytes = DEFAULT_AGGREGATE_BYTES
        .checked_sub(managed_bytes)
        .ok_or_else(invalid_store)?;
    Ok(DiagnosticStoreStatus {
        schema: "scorepeek-diagnostic-store-status-v2",
        recording_enabled_by_default: true,
        remote_export_enabled: false,
        writer_active,
        aggregate_retention_bytes: DEFAULT_AGGREGATE_BYTES,
        normal_retention_hours: NORMAL_RETENTION_HOURS,
        priority_retention_hours: PRIORITY_RETENTION_HOURS,
        managed_bytes,
        remaining_bytes,
        run_count: runs.len(),
        complete_count: runs
            .iter()
            .filter(|run| run.completeness == DiagnosticCompleteness::Complete)
            .count(),
        partial_count: runs
            .iter()
            .filter(|run| run.completeness == DiagnosticCompleteness::Partial)
            .count(),
        dropped_count: runs
            .iter()
            .filter(|run| run.completeness == DiagnosticCompleteness::Dropped)
            .count(),
        priority_count: runs.iter().filter(|run| run.priority).count(),
    })
}

/// Lists bounded, value-free diagnostic run identities and terminal state.
///
/// # Errors
/// Returns a value-free error when the root or any managed run is invalid or changes while read.
pub fn diagnostic_run_list(root: &Path) -> Result<DiagnosticRunList, String> {
    Ok(DiagnosticRunList {
        schema: "scorepeek-diagnostic-run-list-v2",
        runs: inspect_store(root)?,
    })
}

/// Freezes one digest-confirmed run into priority retention.
///
/// # Errors
/// Returns a value-free control error when the store, confirmation, or publication is invalid.
pub fn diagnostic_freeze(
    root: &Path,
    run_id: &str,
    run_sha256: &str,
    manifest_sha256: Option<&str>,
) -> Result<DiagnosticControlOutcome, String> {
    let _lease = DiagnosticStoreLease::acquire_control(root).map_err(control_error)?;
    let run = find_confirmed_run(root, run_id, run_sha256, manifest_sha256)?;
    if !run.frozen {
        publish_freeze(root, &run).map_err(control_error)?;
    }
    let frozen = find_confirmed_run(root, run_id, run_sha256, manifest_sha256)?;
    if !frozen.frozen {
        return Err(control_error(DiagnosticErrorType::StoreUnavailable));
    }
    Ok(control_outcome("freeze", &frozen))
}

/// Deletes one digest-confirmed run under the store writer lease.
///
/// # Errors
/// Returns a value-free control error when the store, confirmation, or deletion is invalid.
pub fn diagnostic_delete(
    root: &Path,
    run_id: &str,
    run_sha256: &str,
    manifest_sha256: Option<&str>,
) -> Result<DiagnosticControlOutcome, String> {
    let _lease = DiagnosticStoreLease::acquire_control(root).map_err(control_error)?;
    let run = find_confirmed_run(root, run_id, run_sha256, manifest_sha256)?;
    let outcome = control_outcome("delete", &run);
    delete_run(root, &run).map_err(control_error)?;
    if root.join(run_id).symlink_metadata().is_ok() {
        return Err(control_error(DiagnosticErrorType::StoreUnavailable));
    }
    Ok(outcome)
}

/// Exports one complete digest-confirmed run into a new local directory.
///
/// # Errors
/// Returns a value-free control error when verification, creation, copy, or durability fails.
pub fn diagnostic_export(
    root: &Path,
    run_id: &str,
    run_sha256: &str,
    manifest_sha256: &str,
    destination: &Path,
) -> Result<DiagnosticExportOutcome, String> {
    let _lease = DiagnosticStoreLease::acquire_control(root).map_err(control_error)?;
    let run = find_confirmed_run(root, run_id, run_sha256, Some(manifest_sha256))?;
    let destination = resolve_export_destination(root, destination)
        .ok_or_else(|| "diagnostic export request is invalid".to_owned())?;
    if run.completeness != DiagnosticCompleteness::Complete || run.manifest_sha256.is_none() {
        return Err("diagnostic export request is invalid".to_owned());
    }
    export_complete_run(root, &run, &destination)
}

fn resolve_export_destination(root: &Path, destination: &Path) -> Option<PathBuf> {
    if !destination.is_absolute()
        || destination.file_name().is_none()
        || destination.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
        || destination.symlink_metadata().is_ok()
    {
        return None;
    }
    let parent = destination.parent()?;
    let parent_metadata = parent.metadata().ok()?;
    if !parent_metadata.is_dir() {
        return None;
    }
    let canonical_root = root.canonicalize().ok()?;
    let canonical_parent = parent.canonicalize().ok()?;
    let resolved = canonical_parent.join(destination.file_name()?);
    (!resolved.starts_with(canonical_root)).then_some(resolved)
}

fn export_complete_run(
    root: &Path,
    run: &DiagnosticRunSummary,
    destination: &Path,
) -> Result<DiagnosticExportOutcome, String> {
    let source = root.join(&run.run_id);
    let current = inspect_run(&source, &run.run_id)?;
    if !same_run_summary(run, &current) {
        return Err("diagnostic export source changed".to_owned());
    }
    let manifest_bytes = read_bounded_regular(&source.join("manifest.json"), MAX_MANIFEST_BYTES)?;
    let manifest: RunManifestDocument =
        serde_json::from_slice(&manifest_bytes).map_err(|_| invalid_store())?;
    let mut expected = BTreeMap::from([
        ("run.json".to_owned(), run.run_sha256.clone()),
        (
            "manifest.json".to_owned(),
            run.manifest_sha256.clone().ok_or_else(invalid_store)?,
        ),
    ]);
    for frame in &manifest.frames {
        expected.insert(frame.filename.clone(), frame.file_sha256.clone());
        if let Some(source) = &frame.source {
            expected.insert(source.filename.clone(), source.file_sha256.clone());
        }
    }
    match &manifest.facts {
        FactManifest::Legacy(facts) => {
            for fact in facts {
                expected.insert(fact.filename.clone(), fact.file_sha256.clone());
            }
        }
        FactManifest::Ndjson(facts) => {
            expected.insert(facts.filename.clone(), facts.file_sha256.clone());
        }
    }

    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder
        .create(destination)
        .map_err(|_| "diagnostic export destination creation failed".to_owned())?;
    File::open(destination.parent().ok_or_else(invalid_store)?)
        .and_then(|parent| parent.sync_all())
        .map_err(|_| "diagnostic export destination creation failed".to_owned())?;

    let files = run_files(&source)?;
    let mut exported = Vec::with_capacity(files.len());
    let mut artifact_bytes = 0_u64;
    for (filename, bytes) in files {
        let digest =
            copy_verified_file(&source.join(&filename), &destination.join(&filename), bytes)?;
        if expected
            .get(&filename)
            .is_some_and(|expected| expected != &digest)
            || (filename != FREEZE_FILENAME && !expected.contains_key(&filename))
        {
            return Err("diagnostic export source digest failed".to_owned());
        }
        artifact_bytes = artifact_bytes
            .checked_add(bytes)
            .ok_or_else(invalid_store)?;
        exported.push(ExportFileDocument {
            filename,
            sha256: digest,
            bytes,
        });
    }
    let document = ExportManifestDocument {
        schema: "scorepeek-diagnostic-local-export-v1",
        run_id: run.run_id.clone(),
        run_sha256: run.run_sha256.clone(),
        manifest_sha256: run.manifest_sha256.clone().ok_or_else(invalid_store)?,
        files: exported,
        artifact_bytes,
    };
    let export_bytes = canonical_json(&document)?;
    let exported_bytes = artifact_bytes
        .checked_add(export_bytes.len() as u64)
        .ok_or_else(invalid_store)?;
    let file_count = document
        .files
        .len()
        .checked_add(1)
        .ok_or_else(invalid_store)?;
    File::open(destination)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "diagnostic export durability failed".to_owned())?;
    publish_private_file(&destination.join(EXPORT_MANIFEST_FILENAME), &export_bytes)
        .map_err(|_| "diagnostic export manifest publication failed".to_owned())?;
    Ok(DiagnosticExportOutcome {
        schema: "scorepeek-diagnostic-export-outcome-v1",
        operation: "export",
        run_id: run.run_id.clone(),
        run_sha256: run.run_sha256.clone(),
        manifest_sha256: run.manifest_sha256.clone().ok_or_else(invalid_store)?,
        file_count,
        exported_bytes,
    })
}

fn copy_verified_file(
    source: &Path,
    destination: &Path,
    expected_bytes: u64,
) -> Result<String, String> {
    let before = source.symlink_metadata().map_err(|_| invalid_store())?;
    if !before.is_file() || before.file_type().is_symlink() || before.len() != expected_bytes {
        return Err(invalid_store());
    }
    let mut input = File::open(source).map_err(|_| invalid_store())?;
    if !same_file(&before, &input.metadata().map_err(|_| invalid_store())?) {
        return Err(invalid_store());
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)
        .map_err(|_| "diagnostic export file publication failed".to_owned())?;
    let mut hasher = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = input.read(&mut buffer).map_err(|_| invalid_store())?;
        if read == 0 {
            break;
        }
        copied = copied.checked_add(read as u64).ok_or_else(invalid_store)?;
        if copied > expected_bytes {
            return Err(invalid_store());
        }
        hasher.update(&buffer[..read]);
        output
            .write_all(&buffer[..read])
            .map_err(|_| "diagnostic export file publication failed".to_owned())?;
    }
    if copied != expected_bytes
        || !same_file(&before, &input.metadata().map_err(|_| invalid_store())?)
    {
        return Err(invalid_store());
    }
    output
        .sync_all()
        .map_err(|_| "diagnostic export file publication failed".to_owned())?;
    Ok(encode_sha256_digest(hasher.finalize()))
}

fn control_outcome(
    operation: &'static str,
    run: &DiagnosticRunSummary,
) -> DiagnosticControlOutcome {
    DiagnosticControlOutcome {
        schema: "scorepeek-diagnostic-control-outcome-v1",
        operation,
        run_id: run.run_id.clone(),
        run_sha256: run.run_sha256.clone(),
        manifest_sha256: run.manifest_sha256.clone(),
        frozen: run.frozen,
        managed_bytes: run.managed_bytes,
    }
}

fn find_confirmed_run(
    root: &Path,
    run_id: &str,
    run_sha256: &str,
    manifest_sha256: Option<&str>,
) -> Result<DiagnosticRunSummary, String> {
    if !valid_run_id(run_id)
        || !valid_sha256(run_sha256)
        || manifest_sha256.is_some_and(|digest| !valid_sha256(digest))
    {
        return Err("diagnostic control confirmation is invalid".to_owned());
    }
    let run = inspect_store(root)?
        .into_iter()
        .find(|run| run.run_id == run_id)
        .ok_or_else(|| "diagnostic control target was not found".to_owned())?;
    if run.run_sha256 != run_sha256 || run.manifest_sha256.as_deref() != manifest_sha256 {
        return Err("diagnostic control digest confirmation failed".to_owned());
    }
    Ok(run)
}

fn control_error(error: DiagnosticErrorType) -> String {
    format!(
        "diagnostic control failed: {}",
        diagnostic_error_name(error)
    )
}

fn diagnostic_error_name(error: DiagnosticErrorType) -> &'static str {
    match error {
        DiagnosticErrorType::WorkerUnavailable => "worker_unavailable",
        DiagnosticErrorType::CapacityExceeded => "capacity_exceeded",
        _ => "store_unavailable",
    }
}

pub(crate) fn inspect_store(root: &Path) -> Result<Vec<DiagnosticRunSummary>, String> {
    inspect_store_with(root, |_| {})
}

fn inspect_store_with(
    root: &Path,
    mut after_run: impl FnMut(&Path),
) -> Result<Vec<DiagnosticRunSummary>, String> {
    let before = directory_identity(root)?;
    let entries = fs::read_dir(root).map_err(|_| invalid_store())?;
    let mut runs = Vec::new();
    let mut run_identities = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| invalid_store())?;
        let file_name = entry.file_name();
        let run_id = file_name.to_str().ok_or_else(invalid_store)?;
        if run_id == STORE_LOCK_FILENAME {
            let metadata = entry
                .path()
                .symlink_metadata()
                .map_err(|_| invalid_store())?;
            if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != 0 {
                return Err(invalid_store());
            }
            continue;
        }
        if runs.len() >= MAX_RUNS {
            return Err(invalid_store());
        }
        if !valid_run_id(run_id) {
            return Err(invalid_store());
        }
        let metadata = entry.file_type().map_err(|_| invalid_store())?;
        if !metadata.is_dir() || metadata.is_symlink() {
            return Err(invalid_store());
        }
        let path = entry.path();
        run_identities.push((path.clone(), directory_identity(&path)?));
        runs.push(inspect_run(&path, run_id)?);
        after_run(&path);
    }
    if before != directory_identity(root)? {
        return Err(invalid_store());
    }
    for (path, identity) in run_identities {
        if identity != directory_identity(&path)? {
            return Err(invalid_store());
        }
    }
    let managed_bytes = runs.iter().try_fold(0_u64, |total, run| {
        total
            .checked_add(run.managed_bytes)
            .ok_or_else(invalid_store)
    })?;
    if managed_bytes > DEFAULT_AGGREGATE_BYTES {
        return Err(invalid_store());
    }
    runs.sort_by(|left, right| left.run_id.cmp(&right.run_id));
    Ok(runs)
}

pub(crate) fn inspect_run(directory: &Path, run_id: &str) -> Result<DiagnosticRunSummary, String> {
    inspect_run_with_freeze_staging(directory, run_id, false)
}

fn inspect_run_with_freeze_staging(
    directory: &Path,
    run_id: &str,
    allow_freeze_staging: bool,
) -> Result<DiagnosticRunSummary, String> {
    let before = directory_identity(directory)?;
    let start_bytes = read_bounded_regular(&directory.join("run.json"), MAX_START_BYTES)?;
    let start = parse_run_start(&start_bytes, run_id)?;
    let run_sha256 = encode_sha256(&start_bytes);
    let mut files = run_files(directory)?;
    files.remove(FREEZE_FILENAME);
    if files.remove(FREEZE_STAGING_FILENAME).is_some() && !allow_freeze_staging {
        return Err(invalid_store());
    }
    let managed_bytes = files.values().try_fold(0_u64, |total, bytes| {
        total.checked_add(*bytes).ok_or_else(invalid_store)
    })?;
    if managed_bytes > start.policy.maximum_run_bytes {
        return Err(invalid_store());
    }
    let manifest_path = directory.join("manifest.json");
    let mut summary = match manifest_path.symlink_metadata() {
        Ok(_) => {
            let manifest_bytes = read_bounded_regular(&manifest_path, MAX_MANIFEST_BYTES)?;
            let manifest: RunManifestDocument =
                serde_json::from_slice(&manifest_bytes).map_err(|_| invalid_store())?;
            validate_manifest(
                &manifest,
                &run_sha256,
                start_bytes.len() as u64,
                manifest_bytes.len() as u64,
                managed_bytes,
                start.monotonic_start_ms,
                &files,
            )?;
            let priority = matches!(
                manifest.status,
                DiagnosticRunStatus::Error | DiagnosticRunStatus::Timeout
            ) || manifest.completeness != DiagnosticCompleteness::Complete;
            DiagnosticRunSummary {
                run_id: run_id.to_owned(),
                run_sha256,
                manifest_sha256: Some(encode_sha256(&manifest_bytes)),
                status: Some(manifest.status),
                completeness: manifest.completeness,
                frozen: false,
                priority,
                managed_bytes,
                retention_time: manifest_path
                    .symlink_metadata()
                    .and_then(|metadata| metadata.modified())
                    .map_err(|_| invalid_store())?,
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            validate_partial_files(&files, start_bytes.len() as u64)?;
            DiagnosticRunSummary {
                run_id: run_id.to_owned(),
                run_sha256,
                manifest_sha256: None,
                status: None,
                completeness: DiagnosticCompleteness::Partial,
                frozen: false,
                priority: true,
                managed_bytes,
                retention_time: directory
                    .symlink_metadata()
                    .and_then(|metadata| metadata.modified())
                    .map_err(|_| invalid_store())?,
            }
        }
        Err(_) => return Err(invalid_store()),
    };
    if let Some(retention_time) = inspect_freeze(directory, &summary)? {
        summary.frozen = true;
        summary.priority = true;
        summary.retention_time = retention_time;
    }
    if before != directory_identity(directory)? {
        return Err(invalid_store());
    }
    Ok(summary)
}

fn inspect_freeze(
    directory: &Path,
    run: &DiagnosticRunSummary,
) -> Result<Option<SystemTime>, String> {
    let path = directory.join(FREEZE_FILENAME);
    match path.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(invalid_store()),
        Ok(metadata) => {
            let bytes = read_bounded_regular(&path, MAX_CONTROL_BYTES)?;
            let document: FreezeDocument =
                serde_json::from_slice(&bytes).map_err(|_| invalid_store())?;
            if document.schema != "scorepeek-diagnostic-freeze-v1"
                || document.run_id != run.run_id
                || document.run_sha256 != run.run_sha256
                || document.manifest_sha256 != run.manifest_sha256
                || canonical_json(&document)? != bytes
            {
                return Err(invalid_store());
            }
            metadata.modified().map(Some).map_err(|_| invalid_store())
        }
    }
}

impl DiagnosticStoreLease {
    #[cfg(test)]
    pub(crate) fn acquire(root: &Path, required_bytes: u64) -> Result<Self, DiagnosticErrorType> {
        Self::acquire_at_for_run(root, required_bytes, SystemTime::now(), None)
    }

    pub(crate) fn acquire_for_run(
        root: &Path,
        run_id: &str,
        required_bytes: u64,
    ) -> Result<Self, DiagnosticErrorType> {
        Self::acquire_at_for_run(root, required_bytes, SystemTime::now(), Some(run_id))
    }

    fn acquire_control(root: &Path) -> Result<Self, DiagnosticErrorType> {
        let root_lock = open_lock_directory(root)?;
        match root_lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(DiagnosticErrorType::WorkerUnavailable);
            }
            Err(std::fs::TryLockError::Error(_)) => {
                return Err(DiagnosticErrorType::StoreUnavailable);
            }
        }
        let canonical_root = canonical_locked_root(root, &root_lock)?;
        let (anchor_path, anchor_lock) = open_store_anchor(&canonical_root, true)?
            .ok_or(DiagnosticErrorType::StoreUnavailable)?;
        match anchor_lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(DiagnosticErrorType::WorkerUnavailable);
            }
            Err(std::fs::TryLockError::Error(_)) => {
                return Err(DiagnosticErrorType::StoreUnavailable);
            }
        }
        validate_locked_root(root, &canonical_root, &root_lock)?;
        let root_inode = metadata_inode(
            &root_lock
                .metadata()
                .map_err(|_| DiagnosticErrorType::StoreUnavailable)?,
        );
        ensure_store_lock_marker(&canonical_root)?;
        recover_delete_staging(&canonical_root)?;
        recover_freeze_staging(&canonical_root)?;
        let runs =
            inspect_store(&canonical_root).map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        let managed_bytes = runs
            .iter()
            .try_fold(0_u64, |total, run| total.checked_add(run.managed_bytes))
            .ok_or(DiagnosticErrorType::StoreUnavailable)?;
        Ok(Self {
            root: canonical_root,
            root_inode,
            anchor_path,
            _anchor_lock: anchor_lock,
            _root_lock: root_lock,
            managed_bytes,
            normal_candidates: Vec::new(),
        })
    }

    #[cfg(test)]
    fn acquire_at(
        root: &Path,
        required_bytes: u64,
        now: SystemTime,
    ) -> Result<Self, DiagnosticErrorType> {
        Self::acquire_at_for_run(root, required_bytes, now, None)
    }

    fn acquire_at_for_run(
        root: &Path,
        required_bytes: u64,
        now: SystemTime,
        new_run_id: Option<&str>,
    ) -> Result<Self, DiagnosticErrorType> {
        let root_lock = open_lock_directory(root)?;
        match root_lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(DiagnosticErrorType::WorkerUnavailable);
            }
            Err(std::fs::TryLockError::Error(_)) => {
                return Err(DiagnosticErrorType::StoreUnavailable);
            }
        }
        let canonical_root = canonical_locked_root(root, &root_lock)?;
        let (anchor_path, anchor_lock) = open_store_anchor(&canonical_root, true)?
            .ok_or(DiagnosticErrorType::StoreUnavailable)?;
        match anchor_lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(DiagnosticErrorType::WorkerUnavailable);
            }
            Err(std::fs::TryLockError::Error(_)) => {
                return Err(DiagnosticErrorType::StoreUnavailable);
            }
        }
        validate_locked_root(root, &canonical_root, &root_lock)?;
        let root_inode = metadata_inode(
            &root_lock
                .metadata()
                .map_err(|_| DiagnosticErrorType::StoreUnavailable)?,
        );
        ensure_store_lock_marker(&canonical_root)?;
        recover_delete_staging(&canonical_root)?;
        recover_freeze_staging(&canonical_root)?;
        let mut runs =
            inspect_store(&canonical_root).map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        if new_run_id.is_some_and(|run_id| runs.iter().any(|run| run.run_id == run_id)) {
            return Err(DiagnosticErrorType::StoreUnavailable);
        }
        let mut managed_bytes = runs
            .iter()
            .try_fold(0_u64, |total, run| total.checked_add(run.managed_bytes))
            .ok_or(DiagnosticErrorType::StoreUnavailable)?;

        runs.sort_by(|left, right| {
            left.retention_time
                .cmp(&right.retention_time)
                .then_with(|| left.run_id.cmp(&right.run_id))
        });
        let mut normal_candidates = Vec::new();
        for run in runs {
            let retention_hours = if run.priority {
                PRIORITY_RETENTION_HOURS
            } else {
                NORMAL_RETENTION_HOURS
            };
            let expired = now
                .duration_since(run.retention_time)
                .is_ok_and(|age| age >= Duration::from_secs(u64::from(retention_hours) * 3_600));
            if expired {
                delete_run(&canonical_root, &run)?;
                managed_bytes = managed_bytes
                    .checked_sub(run.managed_bytes)
                    .ok_or(DiagnosticErrorType::StoreUnavailable)?;
            } else if !run.priority {
                normal_candidates.push(run);
            }
        }
        let mut lease = Self {
            root: canonical_root,
            root_inode,
            anchor_path,
            _anchor_lock: anchor_lock,
            _root_lock: root_lock,
            managed_bytes,
            normal_candidates,
        };
        lease.reserve(required_bytes)?;
        Ok(lease)
    }

    pub(crate) fn reserve(&mut self, additional_bytes: u64) -> Result<(), DiagnosticErrorType> {
        self.validate_root()?;
        let required_total = self
            .managed_bytes
            .checked_add(additional_bytes)
            .ok_or(DiagnosticErrorType::CapacityExceeded)?;
        if required_total > DEFAULT_AGGREGATE_BYTES {
            let required_reclaim = required_total - DEFAULT_AGGREGATE_BYTES;
            let reclaimable = self
                .normal_candidates
                .iter()
                .try_fold(0_u64, |total, run| total.checked_add(run.managed_bytes));
            if reclaimable.is_none_or(|bytes| bytes < required_reclaim) {
                return Err(DiagnosticErrorType::CapacityExceeded);
            }
        }
        loop {
            if self
                .managed_bytes
                .checked_add(additional_bytes)
                .is_some_and(|total| total <= DEFAULT_AGGREGATE_BYTES)
            {
                self.managed_bytes += additional_bytes;
                return Ok(());
            }
            if self.normal_candidates.is_empty() {
                return Err(DiagnosticErrorType::CapacityExceeded);
            }
            let run = self.normal_candidates.remove(0);
            delete_run(&self.root, &run)?;
            self.managed_bytes = self
                .managed_bytes
                .checked_sub(run.managed_bytes)
                .ok_or(DiagnosticErrorType::StoreUnavailable)?;
        }
    }

    pub(crate) fn release(&mut self, bytes: u64) {
        self.managed_bytes = self
            .managed_bytes
            .checked_sub(bytes)
            .expect("only a successful reservation can be released");
    }

    fn validate_root(&self) -> Result<(), DiagnosticErrorType> {
        let metadata = self
            .root
            .symlink_metadata()
            .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        if metadata_inode(&metadata) != self.root_inode {
            return Err(DiagnosticErrorType::StoreUnavailable);
        }
        Ok(())
    }
}

fn diagnostic_writer_activity(root: &Path) -> Result<(bool, Option<(File, File)>), String> {
    let root_lock = open_lock_directory(root).map_err(|_| invalid_store())?;
    match root_lock.try_lock_shared() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Ok((true, None)),
        Err(std::fs::TryLockError::Error(_)) => return Err(invalid_store()),
    }
    let canonical_root = canonical_locked_root(root, &root_lock).map_err(|_| invalid_store())?;
    let Some((_, anchor_lock)) =
        open_store_anchor(&canonical_root, false).map_err(|_| invalid_store())?
    else {
        validate_locked_root(root, &canonical_root, &root_lock).map_err(|_| invalid_store())?;
        return Ok((
            false,
            Some((
                root_lock.try_clone().map_err(|_| invalid_store())?,
                root_lock,
            )),
        ));
    };
    match anchor_lock.try_lock_shared() {
        Ok(()) => {
            validate_locked_root(root, &canonical_root, &root_lock).map_err(|_| invalid_store())?;
            Ok((false, Some((root_lock, anchor_lock))))
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok((true, None)),
        Err(std::fs::TryLockError::Error(_)) => Err(invalid_store()),
    }
}

fn canonical_locked_root(root: &Path, root_lock: &File) -> Result<PathBuf, DiagnosticErrorType> {
    let canonical_root = root
        .canonicalize()
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    validate_locked_root(root, &canonical_root, root_lock)?;
    Ok(canonical_root)
}

fn validate_locked_root(
    root: &Path,
    canonical_root: &Path,
    root_lock: &File,
) -> Result<(), DiagnosticErrorType> {
    let raw = root
        .metadata()
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    let canonical = canonical_root
        .metadata()
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    let opened = root_lock
        .metadata()
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    if !raw.is_dir()
        || !canonical.is_dir()
        || !same_inode(&raw, &opened)
        || !same_inode(&canonical, &opened)
    {
        return Err(DiagnosticErrorType::StoreUnavailable);
    }
    Ok(())
}

fn open_store_anchor(
    root: &Path,
    create: bool,
) -> Result<Option<(std::path::PathBuf, File)>, DiagnosticErrorType> {
    let parent = root.parent().ok_or(DiagnosticErrorType::StoreUnavailable)?;
    let digest = Sha256::digest(root.as_os_str().as_bytes());
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    let path = parent.join(format!(".scorepeek-diagnostic-store-anchor-{encoded}.lock"));
    match path.symlink_metadata() {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != 0 {
                return Err(DiagnosticErrorType::StoreUnavailable);
            }
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
            if !same_inode(
                &metadata,
                &file
                    .metadata()
                    .map_err(|_| DiagnosticErrorType::StoreUnavailable)?,
            ) {
                return Err(DiagnosticErrorType::StoreUnavailable);
            }
            Ok(Some((path, file)))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !create => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let file = match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return open_store_anchor(root, false);
                }
                Err(_) => return Err(DiagnosticErrorType::StoreUnavailable),
            };
            file.sync_all()
                .and_then(|()| File::open(parent)?.sync_all())
                .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
            Ok(Some((path, file)))
        }
        Err(_) => Err(DiagnosticErrorType::StoreUnavailable),
    }
}

#[cfg(test)]
impl Drop for DiagnosticStoreLease {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.anchor_path);
    }
}

fn ensure_store_lock_marker(root: &Path) -> Result<(), DiagnosticErrorType> {
    let path = root.join(STORE_LOCK_FILENAME);
    match path.symlink_metadata() {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != 0 {
                return Err(DiagnosticErrorType::StoreUnavailable);
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .and_then(|file| file.sync_all())
            .and_then(|()| File::open(root)?.sync_all())
            .map_err(|_| DiagnosticErrorType::StoreUnavailable),
        Err(_) => Err(DiagnosticErrorType::StoreUnavailable),
    }
}

fn open_lock_directory(path: &Path) -> Result<File, DiagnosticErrorType> {
    let before = path
        .metadata()
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    if !path.is_absolute() || !before.is_dir() {
        return Err(DiagnosticErrorType::StoreUnavailable);
    }
    let file = File::open(path).map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    let opened = file
        .metadata()
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    let after = path
        .metadata()
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    if !same_inode(&before, &opened) || !same_inode(&before, &after) {
        return Err(DiagnosticErrorType::StoreUnavailable);
    }
    Ok(file)
}

fn recover_freeze_staging(root: &Path) -> Result<(), DiagnosticErrorType> {
    for entry in fs::read_dir(root).map_err(|_| DiagnosticErrorType::StoreUnavailable)? {
        let entry = entry.map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        let metadata = entry
            .path()
            .symlink_metadata()
            .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        let Some(run_id) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };
        if !valid_run_id(&run_id) || !metadata.is_dir() || metadata.file_type().is_symlink() {
            continue;
        }
        let staging = entry.path().join(FREEZE_STAGING_FILENAME);
        if staging.symlink_metadata().is_ok() {
            complete_freeze_publication(&entry.path(), &run_id)?;
        }
    }
    Ok(())
}

fn publish_freeze(root: &Path, run: &DiagnosticRunSummary) -> Result<(), DiagnosticErrorType> {
    let directory = root.join(&run.run_id);
    let document = FreezeDocument {
        schema: "scorepeek-diagnostic-freeze-v1".to_owned(),
        run_id: run.run_id.clone(),
        run_sha256: run.run_sha256.clone(),
        manifest_sha256: run.manifest_sha256.clone(),
    };
    let bytes = canonical_json(&document).map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    let staging_path = directory.join(FREEZE_STAGING_FILENAME);
    let mut staging = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staging_path)
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    staging
        .write_all(&bytes)
        .and_then(|()| staging.sync_all())
        .and_then(|()| File::open(&directory)?.sync_all())
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    complete_freeze_publication(&directory, &run.run_id)
}

fn complete_freeze_publication(directory: &Path, run_id: &str) -> Result<(), DiagnosticErrorType> {
    let staging_path = directory.join(FREEZE_STAGING_FILENAME);
    let final_path = directory.join(FREEZE_FILENAME);
    let Ok(staging) = read_freeze_document(&staging_path, run_id) else {
        match final_path.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let staging_metadata = staging_path
                    .symlink_metadata()
                    .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
                if !staging_metadata.is_file() || staging_metadata.file_type().is_symlink() {
                    return Err(DiagnosticErrorType::StoreUnavailable);
                }
                fs::remove_file(&staging_path)
                    .and_then(|()| File::open(directory)?.sync_all())
                    .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
                return Ok(());
            }
            _ => return Err(DiagnosticErrorType::StoreUnavailable),
        }
    };
    let run = inspect_run_with_freeze_staging(directory, run_id, true)
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    if staging.run_sha256 != run.run_sha256 || staging.manifest_sha256 != run.manifest_sha256 {
        return Err(DiagnosticErrorType::StoreUnavailable);
    }
    match final_path.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::hard_link(&staging_path, &final_path)
                .and_then(|()| File::open(directory)?.sync_all())
                .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        }
        Ok(final_metadata) => {
            let staging_metadata = staging_path
                .symlink_metadata()
                .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
            if !same_inode(&final_metadata, &staging_metadata)
                || read_freeze_document(&final_path, run_id)? != staging
            {
                return Err(DiagnosticErrorType::StoreUnavailable);
            }
        }
        Err(_) => return Err(DiagnosticErrorType::StoreUnavailable),
    }
    fs::remove_file(&staging_path)
        .and_then(|()| File::open(directory)?.sync_all())
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)
}

fn read_freeze_document(path: &Path, run_id: &str) -> Result<FreezeDocument, DiagnosticErrorType> {
    let bytes = read_bounded_owned_regular(path, MAX_CONTROL_BYTES)
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    let document: FreezeDocument =
        serde_json::from_slice(&bytes).map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    if document.schema != "scorepeek-diagnostic-freeze-v1"
        || document.run_id != run_id
        || !valid_sha256(&document.run_sha256)
        || document
            .manifest_sha256
            .as_deref()
            .is_some_and(|digest| !valid_sha256(digest))
        || canonical_json(&document).map_err(|_| DiagnosticErrorType::StoreUnavailable)? != bytes
    {
        return Err(DiagnosticErrorType::StoreUnavailable);
    }
    Ok(document)
}

fn recover_delete_staging(root: &Path) -> Result<(), DiagnosticErrorType> {
    let mut recovered = 0_usize;
    for entry in fs::read_dir(root).map_err(|_| DiagnosticErrorType::StoreUnavailable)? {
        let entry = entry.map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        let Some(run_id) = name.strip_prefix(DELETE_STAGING_PREFIX) else {
            continue;
        };
        recovered = recovered
            .checked_add(1)
            .ok_or(DiagnosticErrorType::StoreUnavailable)?;
        if recovered > MAX_RUNS || !valid_run_id(run_id) {
            return Err(DiagnosticErrorType::StoreUnavailable);
        }
        let marker = entry.path().join(DELETE_MARKER_FILENAME);
        let marker_staging = entry.path().join(DELETE_MARKER_STAGING_FILENAME);
        match (marker.symlink_metadata(), marker_staging.symlink_metadata()) {
            (Err(marker_error), Err(staging_error))
                if marker_error.kind() == std::io::ErrorKind::NotFound
                    && staging_error.kind() == std::io::ErrorKind::NotFound =>
            {
                if fs::read_dir(entry.path())
                    .map_err(|_| DiagnosticErrorType::StoreUnavailable)?
                    .next()
                    .is_none()
                {
                    fs::remove_dir(entry.path())
                        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
                    File::open(root)
                        .and_then(|root| root.sync_all())
                        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
                    continue;
                }
                inspect_run(&entry.path(), run_id)
                    .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
                mark_delete_staging(&entry.path(), run_id)?;
            }
            (Ok(_), _) | (_, Ok(_)) => mark_delete_staging(&entry.path(), run_id)?,
            _ => return Err(DiagnosticErrorType::StoreUnavailable),
        }
        remove_owned_delete_staging(root, &entry.path(), run_id)?;
    }
    Ok(())
}

fn delete_run(root: &Path, run: &DiagnosticRunSummary) -> Result<(), DiagnosticErrorType> {
    let source = root.join(&run.run_id);
    let current =
        inspect_run(&source, &run.run_id).map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    if !same_run_summary(run, &current) {
        return Err(DiagnosticErrorType::StoreUnavailable);
    }
    let staging = root.join(format!("{DELETE_STAGING_PREFIX}{}", run.run_id));
    match staging.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err(DiagnosticErrorType::StoreUnavailable),
    }
    fs::rename(&source, &staging).map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    File::open(root)
        .and_then(|root| root.sync_all())
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    let renamed =
        inspect_run(&staging, &run.run_id).map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    if !same_run_summary(run, &renamed) {
        return Err(DiagnosticErrorType::StoreUnavailable);
    }
    mark_delete_staging(&staging, &run.run_id)?;
    remove_owned_delete_staging(root, &staging, &run.run_id)
}

fn same_run_summary(left: &DiagnosticRunSummary, right: &DiagnosticRunSummary) -> bool {
    left.run_id == right.run_id
        && left.run_sha256 == right.run_sha256
        && left.manifest_sha256 == right.manifest_sha256
        && left.status == right.status
        && left.completeness == right.completeness
        && left.frozen == right.frozen
        && left.priority == right.priority
        && left.managed_bytes == right.managed_bytes
        && left.retention_time == right.retention_time
}

fn mark_delete_staging(directory: &Path, run_id: &str) -> Result<(), DiagnosticErrorType> {
    let marker_path = directory.join(DELETE_MARKER_FILENAME);
    let staging_path = directory.join(DELETE_MARKER_STAGING_FILENAME);
    let marker_exists = marker_path.symlink_metadata().is_ok();
    let mut staging_exists = staging_path.symlink_metadata().is_ok();
    if !marker_exists && staging_exists && validate_delete_marker(&staging_path, run_id).is_err() {
        let metadata = staging_path
            .symlink_metadata()
            .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(DiagnosticErrorType::StoreUnavailable);
        }
        fs::remove_file(&staging_path)
            .and_then(|()| File::open(directory)?.sync_all())
            .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        inspect_run(directory, run_id).map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        staging_exists = false;
    }
    if !marker_exists && !staging_exists {
        let files = run_files(directory).map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        let marker = DeleteMarkerDocument {
            schema: "scorepeek-diagnostic-delete-staging-v1".to_owned(),
            run_id: run_id.to_owned(),
            files: files
                .into_iter()
                .map(|(filename, bytes)| DeleteMarkerFile { filename, bytes })
                .collect(),
        };
        let bytes = canonical_json(&marker).map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(DiagnosticErrorType::StoreUnavailable);
        }
        let mut staging = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staging_path)
            .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        staging
            .write_all(&bytes)
            .and_then(|()| staging.sync_all())
            .and_then(|()| File::open(directory)?.sync_all())
            .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    }

    let expected = if staging_path.symlink_metadata().is_ok() {
        validate_delete_marker(&staging_path, run_id)?
    } else {
        validate_delete_marker(&marker_path, run_id)?
    };
    validate_delete_payload_subset(directory, &expected)?;
    match marker_path.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::hard_link(&staging_path, &marker_path)
                .and_then(|()| File::open(directory)?.sync_all())
                .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        }
        Ok(marker_metadata) => {
            let staging_metadata = staging_path.symlink_metadata();
            if let Ok(staging_metadata) = staging_metadata
                && !same_inode(&marker_metadata, &staging_metadata)
            {
                return Err(DiagnosticErrorType::StoreUnavailable);
            }
            if validate_delete_marker(&marker_path, run_id)? != expected {
                return Err(DiagnosticErrorType::StoreUnavailable);
            }
        }
        Err(_) => return Err(DiagnosticErrorType::StoreUnavailable),
    }
    if staging_path.symlink_metadata().is_ok() {
        fs::remove_file(&staging_path)
            .and_then(|()| File::open(directory)?.sync_all())
            .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    }
    Ok(())
}

fn validate_delete_payload_subset(
    directory: &Path,
    expected: &BTreeMap<String, u64>,
) -> Result<(), DiagnosticErrorType> {
    for entry in fs::read_dir(directory).map_err(|_| DiagnosticErrorType::StoreUnavailable)? {
        let entry = entry.map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        if name == DELETE_MARKER_FILENAME || name == DELETE_MARKER_STAGING_FILENAME {
            continue;
        }
        let metadata = entry
            .path()
            .symlink_metadata()
            .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || expected.get(&name) != Some(&metadata.len())
        {
            return Err(DiagnosticErrorType::StoreUnavailable);
        }
    }
    Ok(())
}

fn validate_delete_marker(
    marker: &Path,
    run_id: &str,
) -> Result<BTreeMap<String, u64>, DiagnosticErrorType> {
    let bytes = read_bounded_owned_regular(marker, MAX_MANIFEST_BYTES)
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    let document: DeleteMarkerDocument =
        serde_json::from_slice(&bytes).map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    if document.schema != "scorepeek-diagnostic-delete-staging-v1"
        || document.run_id != run_id
        || canonical_json(&document).map_err(|_| DiagnosticErrorType::StoreUnavailable)? != bytes
        || document.files.is_empty()
        || document.files.len() > MAX_FILES_PER_RUN
    {
        return Err(DiagnosticErrorType::StoreUnavailable);
    }
    let mut files = BTreeMap::new();
    for file in document.files {
        if file.filename == DELETE_MARKER_FILENAME
            || file.filename == DELETE_MARKER_STAGING_FILENAME
            || (file.bytes == 0 && file.filename != "facts.ndjson")
            || files.insert(file.filename, file.bytes).is_some()
        {
            return Err(DiagnosticErrorType::StoreUnavailable);
        }
    }
    Ok(files)
}

fn remove_owned_delete_staging(
    root: &Path,
    directory: &Path,
    run_id: &str,
) -> Result<(), DiagnosticErrorType> {
    let metadata = directory
        .symlink_metadata()
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(DiagnosticErrorType::StoreUnavailable);
    }
    let expected = validate_delete_marker(&directory.join(DELETE_MARKER_FILENAME), run_id)?;
    let mut marker = None;
    let mut payloads = Vec::new();
    for entry in fs::read_dir(directory).map_err(|_| DiagnosticErrorType::StoreUnavailable)? {
        let entry = entry.map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        if entry.file_name() == DELETE_MARKER_FILENAME {
            marker = Some(entry.path());
            continue;
        }
        if entry.file_name() == DELETE_MARKER_STAGING_FILENAME {
            return Err(DiagnosticErrorType::StoreUnavailable);
        }
        let metadata = entry
            .path()
            .symlink_metadata()
            .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || expected.get(&name) != Some(&metadata.len())
        {
            return Err(DiagnosticErrorType::StoreUnavailable);
        }
        payloads.push(entry.path());
    }
    for payload in payloads {
        fs::remove_file(payload).map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    }
    File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    fs::remove_file(marker.ok_or(DiagnosticErrorType::StoreUnavailable)?)
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    fs::remove_dir(directory).map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    File::open(root)
        .and_then(|root| root.sync_all())
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)
}

fn validate_manifest(
    manifest: &RunManifestDocument,
    run_sha256: &str,
    run_bytes: u64,
    manifest_bytes: u64,
    managed_bytes: u64,
    monotonic_start_ms: u64,
    files: &BTreeMap<String, u64>,
) -> Result<(), String> {
    if !matches!(
        manifest.schema.as_str(),
        "scorepeek-private-diagnostic-run-v1"
            | "scorepeek-private-diagnostic-run-v2"
            | "scorepeek-private-diagnostic-capture-v3"
    ) || (manifest.schema == "scorepeek-private-diagnostic-run-v1"
        && manifest.frames.iter().any(|frame| frame.source.is_some()))
        || manifest.start.schema != "scorepeek-private-diagnostic-artifact-v1"
        || manifest.start.filename != "run.json"
        || manifest.start.file_sha256 != run_sha256
        || manifest.start.bytes != run_bytes
        || manifest.manifest_bytes != manifest_bytes
        || manifest.total_bytes != managed_bytes
        || manifest.artifact_bytes.checked_add(manifest.manifest_bytes)
            != Some(manifest.total_bytes)
        || manifest.monotonic_end_ms < monotonic_start_ms
        || manifest.maximum_observation_gap_ms.is_none()
        || manifest.result_miss_denominator_eligible
        || !valid_manifest_outcome(manifest)
        || !valid_manifest_entries(
            manifest,
            files,
            run_bytes,
            manifest_bytes,
            monotonic_start_ms,
        )
    {
        return Err(invalid_store());
    }
    Ok(())
}

fn valid_manifest_outcome(manifest: &RunManifestDocument) -> bool {
    let mut counts = [0_u64; DiagnosticErrorType::COUNT];
    let mut seen = [false; DiagnosticErrorType::COUNT];
    for entry in &manifest.degradation_reason_counts {
        let index = entry.reason.index();
        if entry.count == 0 || seen[index] {
            return false;
        }
        seen[index] = true;
        counts[index] = entry.count;
    }
    let reason_total = counts
        .iter()
        .try_fold(0_u64, |total, count| total.checked_add(*count));
    let last_error_is_counted = manifest
        .last_error_type
        .is_none_or(|error| counts[error.index()] > 0);
    match manifest.completeness {
        DiagnosticCompleteness::Complete => {
            manifest.dropped_count == 0
                && manifest.last_error_type.is_none()
                && manifest.degradations.is_empty()
                && manifest.degradation_entries_dropped == 0
                && manifest.degradation_reason_counts.is_empty()
        }
        DiagnosticCompleteness::Partial => {
            manifest.dropped_count > 0
                && manifest.last_error_type.is_some()
                && reason_total == Some(manifest.dropped_count)
                && last_error_is_counted
                && manifest.degradations.len() <= MAX_DEGRADATIONS_PER_RUN
                && (manifest.degradations.len() as u64)
                    .checked_add(manifest.degradation_entries_dropped)
                    .is_some_and(|entries| entries <= manifest.dropped_count)
                && manifest
                    .degradations
                    .iter()
                    .all(|entry| valid_degradation(entry) && counts[entry.reason.index()] > 0)
        }
        DiagnosticCompleteness::Dropped => false,
    }
}

fn valid_degradation(entry: &DegradationReference) -> bool {
    let _ = entry.reason;
    match (
        entry.affected_sequence,
        entry.first_missing_sequence,
        entry.last_missing_sequence,
        entry.known_missing_count,
    ) {
        (Some(_) | None, None, None, 0) => true,
        (None, Some(first), Some(last), count) => {
            first <= last && count == last.saturating_sub(first).saturating_add(1)
        }
        (None, None, None, count) => count > 0,
        _ => false,
    }
}

#[allow(clippy::too_many_lines)]
fn valid_manifest_entries(
    manifest: &RunManifestDocument,
    files: &BTreeMap<String, u64>,
    run_bytes: u64,
    manifest_bytes: u64,
    monotonic_start_ms: u64,
) -> bool {
    let fact_count = match &manifest.facts {
        FactManifest::Legacy(facts) => facts.len() as u64,
        FactManifest::Ndjson(facts) => facts.record_count,
    };
    if manifest.frames.len() > MAX_FRAMES_PER_RUN
        || fact_count > MAX_FACTS_PER_RUN as u64
        || manifest.degradations.len() > MAX_DEGRADATIONS_PER_RUN
        || manifest.degradation_reason_counts.len() > DiagnosticErrorType::COUNT
    {
        return false;
    }
    let mut expected = BTreeMap::from([
        ("run.json".to_owned(), run_bytes),
        ("manifest.json".to_owned(), manifest_bytes),
    ]);
    let mut previous_sequence = None;
    let mut artifact_bytes = run_bytes;
    for frame in &manifest.frames {
        if frame.monotonic_start_ms < monotonic_start_ms
            || frame.monotonic_end_ms < frame.monotonic_start_ms
            || frame.monotonic_end_ms > manifest.monotonic_end_ms
            || frame.bytes == 0
            || !valid_sha256(&frame.canonical_pixel_sha256)
            || !valid_sha256(&frame.file_sha256)
            || frame.filename != format!("frame-{:020}.qoi", frame.sequence)
            || previous_sequence.is_some_and(|previous| previous >= frame.sequence)
            || expected
                .insert(frame.filename.clone(), frame.bytes)
                .is_some()
        {
            return false;
        }
        previous_sequence = Some(frame.sequence);
        let source_bytes = frame.source.as_ref().map_or(0, |source| source.bytes);
        if frame.source.as_ref().is_some_and(|source| {
            let _ = (
                source.source_sequence,
                source.memory_type,
                source.received_monotonic_ns,
            );
            let minimum_stride = source.video.width.checked_mul(4);
            let expected_bytes =
                u64::from(source.stride).checked_mul(u64::from(source.video.height));
            let legacy = manifest.schema != "scorepeek-private-diagnostic-capture-v3";
            let legacy_source_invalid = source.filename
                != format!("source-{:020}.bgrx", frame.sequence)
                || source.pixel_format.as_deref() != Some("bgrx");
            let qoi_source_invalid =
                source.filename != format!("source-{:020}.qoi", frame.sequence);
            let source_contract_invalid = if legacy {
                legacy_source_invalid
            } else {
                qoi_source_invalid
                    || source.observed_pixel_format.as_deref() != Some("bgrx")
                    || source.encoded_pixel_format.as_deref() != Some("rgb8")
            };
            source_contract_invalid
                || source.video.width == 0
                || source.video.height == 0
                || minimum_stride.is_none_or(|minimum| source.stride < minimum)
                || (legacy && expected_bytes != Some(source.bytes))
                || source.bytes == 0
                || source.bytes > MAX_SOURCE_FRAME_BYTES
                || !valid_sha256(&source.file_sha256)
                || expected
                    .insert(source.filename.clone(), source.bytes)
                    .is_some()
        }) {
            return false;
        }
        let Some(next) = artifact_bytes
            .checked_add(frame.bytes)
            .and_then(|bytes| bytes.checked_add(source_bytes))
        else {
            return false;
        };
        artifact_bytes = next;
    }
    match &manifest.facts {
        FactManifest::Legacy(facts) => {
            for (expected_index, fact) in facts.iter().enumerate() {
                if fact.index != expected_index as u64
                    || fact.bytes == 0
                    || fact.bytes > MAX_START_BYTES
                    || !valid_sha256(&fact.file_sha256)
                    || fact.filename != format!("fact-{:020}.json", fact.index)
                    || expected.insert(fact.filename.clone(), fact.bytes).is_some()
                {
                    return false;
                }
                let _ = fact.sequence;
                let Some(next) = artifact_bytes.checked_add(fact.bytes) else {
                    return false;
                };
                artifact_bytes = next;
            }
        }
        FactManifest::Ndjson(facts) => {
            if manifest.schema != "scorepeek-private-diagnostic-capture-v3"
                || facts.filename != "facts.ndjson"
                || !valid_sha256(&facts.file_sha256)
                || facts.bytes > (MAX_FACTS_PER_RUN as u64 * MAX_FACT_BYTES as u64)
                || (facts.record_count == 0
                    && (facts.first_sequence.is_some() || facts.last_sequence.is_some()))
                || (facts.record_count > 0
                    && facts
                        .first_sequence
                        .zip(facts.last_sequence)
                        .is_none_or(|(first, last)| first > last))
                || expected
                    .insert(facts.filename.clone(), facts.bytes)
                    .is_some()
            {
                return false;
            }
            let Some(next) = artifact_bytes.checked_add(facts.bytes) else {
                return false;
            };
            artifact_bytes = next;
        }
    }
    artifact_bytes == manifest.artifact_bytes && &expected == files
}

fn validate_partial_files(files: &BTreeMap<String, u64>, run_bytes: u64) -> Result<(), String> {
    if files.get("run.json") != Some(&run_bytes) || files.len() > MAX_FILES_PER_RUN {
        return Err(invalid_store());
    }
    let mut frame_count = 0_usize;
    let mut fact_count = 0_usize;
    for (name, bytes) in files {
        if name == "run.json" {
            continue;
        }
        if *bytes == 0 && name != "facts.ndjson" {
            return Err(invalid_store());
        }
        if valid_indexed_artifact_name(name, "frame-", ".qoi") {
            frame_count = frame_count.checked_add(1).ok_or_else(invalid_store)?;
            if frame_count > MAX_FRAMES_PER_RUN {
                return Err(invalid_store());
            }
        } else if valid_indexed_artifact_name(name, "source-", ".bgrx")
            || valid_indexed_artifact_name(name, "source-", ".qoi")
        {
            if *bytes > MAX_SOURCE_FRAME_BYTES {
                return Err(invalid_store());
            }
        } else if name == "facts.ndjson" {
            if *bytes > MAX_FACTS_PER_RUN as u64 * MAX_FACT_BYTES as u64 {
                return Err(invalid_store());
            }
        } else if valid_indexed_artifact_name(name, "fact-", ".json") {
            fact_count = fact_count.checked_add(1).ok_or_else(invalid_store)?;
            if fact_count > MAX_FACTS_PER_RUN || *bytes > MAX_FACT_BYTES as u64 {
                return Err(invalid_store());
            }
        } else {
            return Err(invalid_store());
        }
    }
    Ok(())
}

fn valid_indexed_artifact_name(name: &str, prefix: &str, suffix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(suffix))
        .is_some_and(|digits| {
            digits.len() == 20 && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn valid_start(start: &RunStartDocument) -> bool {
    let binding = &start.binding;
    let policy = &start.policy;
    let _ = start.monotonic_start_ms;
    start.resource.program == "scorepeek"
        && valid_version(&start.resource.version)
        && valid_sha256(&start.resource.build_sha256)
        && binding.capture_generation > 0
        && [
            &binding.capture_profile_sha256,
            &binding.normalizer_sha256,
            &binding.canonical_layout_sha256,
            &binding.catalog_sha256,
            &binding.model_sha256,
            &binding.runtime_sha256,
        ]
        .into_iter()
        .all(|digest| valid_sha256(digest))
        && binding.replay.as_ref().is_none_or(|replay| {
            valid_sha256(&replay.request_sha256) && valid_sha256(&replay.extraction_sha256)
        })
        && policy.sample_interval_ms > 0
        && policy.maximum_run_bytes > MANIFEST_RESERVE_BYTES
        && policy.maximum_run_bytes <= DEFAULT_AGGREGATE_BYTES
        && policy.aggregate_retention_bytes == DEFAULT_AGGREGATE_BYTES
        && policy.normal_retention_hours == NORMAL_RETENTION_HOURS
        && policy.priority_retention_hours == PRIORITY_RETENTION_HOURS
        && !policy.remote_export_enabled
}

fn parse_run_start(bytes: &[u8], run_id: &str) -> Result<RunStartDocument, String> {
    if let Ok(start) = serde_json::from_slice::<RunStartDocument>(bytes)
        && matches!(
            start.schema.as_str(),
            "scorepeek-private-diagnostic-run-start-v2"
                | "scorepeek-private-diagnostic-capture-start-v3"
        )
        && start.run_id == run_id
        && valid_start(&start)
        && canonical_json(&start)? == bytes
    {
        return Ok(start);
    }
    let legacy: LegacyRunStartDocument =
        serde_json::from_slice(bytes).map_err(|_| invalid_store())?;
    if legacy.schema != "scorepeek-private-diagnostic-run-start-v1"
        || legacy.run_id != run_id
        || canonical_json(&legacy)? != bytes
    {
        return Err(invalid_store());
    }
    let start = RunStartDocument {
        schema: "scorepeek-private-diagnostic-run-start-v2".to_owned(),
        run_id: legacy.run_id,
        monotonic_start_ms: legacy.monotonic_start_ms,
        resource: legacy.resource,
        binding: legacy.binding,
        policy: RunPolicy {
            sample_interval_ms: legacy.policy.sample_interval_ms,
            maximum_run_bytes: legacy.policy.maximum_run_bytes,
            aggregate_retention_bytes: legacy.policy.aggregate_retention_bytes,
            normal_retention_hours: legacy.policy.normal_retention_hours,
            priority_retention_hours: legacy.policy.priority_retention_hours,
            remote_export_enabled: legacy.policy.remote_export_enabled,
            retention: DiagnosticRetention::CompleteCadence,
        },
    };
    valid_start(&start)
        .then_some(start)
        .ok_or_else(invalid_store)
}

fn run_files(directory: &Path) -> Result<BTreeMap<String, u64>, String> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(directory).map_err(|_| invalid_store())? {
        if files.len() >= MAX_FILES_PER_RUN {
            return Err(invalid_store());
        }
        let entry = entry.map_err(|_| invalid_store())?;
        let metadata = entry
            .path()
            .symlink_metadata()
            .map_err(|_| invalid_store())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(invalid_store());
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid_store())?;
        if files.insert(name, metadata.len()).is_some() {
            return Err(invalid_store());
        }
    }
    Ok(files)
}

fn read_bounded_regular(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let before = path.metadata().map_err(|_| invalid_store())?;
    if !before.is_file() || before.len() > maximum {
        return Err(invalid_store());
    }
    let mut file = File::open(path).map_err(|_| invalid_store())?;
    let opened = file.metadata().map_err(|_| invalid_store())?;
    if !same_file(&before, &opened) {
        return Err(invalid_store());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid_store())?;
    let after = file.metadata().map_err(|_| invalid_store())?;
    if bytes.len() as u64 != before.len()
        || bytes.len() as u64 > maximum
        || !same_file(&before, &after)
    {
        return Err(invalid_store());
    }
    Ok(bytes)
}

fn read_bounded_owned_regular(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let before = path.symlink_metadata().map_err(|_| invalid_store())?;
    if !before.is_file() || before.file_type().is_symlink() || before.len() > maximum {
        return Err(invalid_store());
    }
    let mut file = File::open(path).map_err(|_| invalid_store())?;
    let opened = file.metadata().map_err(|_| invalid_store())?;
    if !same_file(&before, &opened) {
        return Err(invalid_store());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid_store())?;
    let after = file.metadata().map_err(|_| invalid_store())?;
    if bytes.len() as u64 != before.len()
        || bytes.len() as u64 > maximum
        || !same_file(&before, &after)
    {
        return Err(invalid_store());
    }
    Ok(bytes)
}

fn directory_identity(path: &Path) -> Result<(u64, u64, i64, i64), String> {
    let metadata = path.metadata().map_err(|_| invalid_store())?;
    if !path.is_absolute() || !metadata.is_dir() {
        return Err(invalid_store());
    }
    Ok((
        metadata.dev(),
        metadata.ino(),
        metadata.mtime(),
        metadata.mtime_nsec(),
    ))
}

fn same_file(left: &Metadata, right: &Metadata) -> bool {
    same_inode(left, right)
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
}

fn same_inode(left: &Metadata, right: &Metadata) -> bool {
    metadata_inode(left) == metadata_inode(right)
}

fn metadata_inode(metadata: &Metadata) -> (u64, u64) {
    (metadata.dev(), metadata.ino())
}

fn valid_run_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_version(value: &str) -> bool {
    if value.is_empty() || value.len() > 64 {
        return false;
    }
    let (without_build, build) = match value.split_once('+') {
        Some((version, build))
            if !build.contains('+') && valid_semver_identifiers(build, false) =>
        {
            (version, Some(build))
        }
        Some(_) => return false,
        None => (value, None),
    };
    let (core, prerelease) = match without_build.split_once('-') {
        Some((core, prerelease)) if valid_semver_identifiers(prerelease, true) => {
            (core, Some(prerelease))
        }
        Some(_) => return false,
        None => (without_build, None),
    };
    let mut components = core.split('.');
    let valid_core = (0..3).all(|_| components.next().is_some_and(valid_semver_number))
        && components.next().is_none();
    let _ = (build, prerelease);
    valid_core
}

fn valid_semver_number(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'))
}

fn valid_semver_identifiers(value: &str, reject_numeric_leading_zero: bool) -> bool {
    value.split('.').all(|identifier| {
        !identifier.is_empty()
            && identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && (!reject_numeric_leading_zero
                || !identifier.bytes().all(|byte| byte.is_ascii_digit())
                || valid_semver_number(identifier))
    })
}

fn encode_sha256(bytes: &[u8]) -> String {
    encode_sha256_digest(Sha256::digest(bytes))
}

fn encode_sha256_digest(digest: impl IntoIterator<Item = u8>) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn canonical_json(value: &impl Serialize) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| invalid_store())?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn invalid_store() -> String {
    "diagnostic store is invalid or changed while reading".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic_recording::{
        CANONICAL_BYTES, DiagnosticBinding, DiagnosticCompleteness, DiagnosticFrameInput,
        DiagnosticPolicy, DiagnosticRecorder, DiagnosticReplayBinding, DiagnosticResource,
        DiagnosticRunDescriptor, DiagnosticRunStatus,
    };

    fn descriptor(run_id: &str) -> DiagnosticRunDescriptor {
        DiagnosticRunDescriptor {
            run_id: run_id.to_owned(),
            monotonic_start_ms: 0,
            resource: DiagnosticResource {
                program: "scorepeek",
                version: env!("CARGO_PKG_VERSION"),
                build_sha256: "1".repeat(64),
            },
            binding: DiagnosticBinding {
                capture_generation: 1,
                capture_profile_sha256: "2".repeat(64),
                normalizer_sha256: "3".repeat(64),
                canonical_layout_sha256: "4".repeat(64),
                catalog_sha256: "5".repeat(64),
                model_sha256: "6".repeat(64),
                runtime_sha256: "7".repeat(64),
                replay: Some(DiagnosticReplayBinding {
                    request_sha256: "8".repeat(64),
                    extraction_sha256: "9".repeat(64),
                }),
            },
        }
    }

    #[test]
    fn lists_complete_and_recoverable_partial_runs_without_values_or_paths() {
        let root = tempfile::tempdir().unwrap();
        let complete = DiagnosticRecorder::start(
            root.path(),
            &descriptor("complete-run"),
            DiagnosticPolicy::default(),
        );
        assert_eq!(
            complete
                .finish(DiagnosticRunStatus::Success, 1_000)
                .completeness,
            Some(DiagnosticCompleteness::Complete)
        );
        let partial = DiagnosticRecorder::start(
            root.path(),
            &descriptor("partial-run"),
            DiagnosticPolicy::default(),
        );
        drop(partial);

        let list = diagnostic_run_list(root.path()).unwrap();
        assert_eq!(list.runs.len(), 2);
        assert_eq!(list.runs[0].run_id, "complete-run");
        assert_eq!(list.runs[0].completeness, DiagnosticCompleteness::Complete);
        assert!(!list.runs[0].priority);
        assert_eq!(list.runs[1].run_id, "partial-run");
        assert_eq!(list.runs[1].completeness, DiagnosticCompleteness::Partial);
        assert!(list.runs[1].priority);
        assert!(list.runs[1].manifest_sha256.is_none());

        let encoded = serde_json::to_string(&list).unwrap();
        assert!(!encoded.contains(root.path().to_str().unwrap()));
        assert!(!encoded.contains("request_sha256"));
        assert!(!encoded.contains("extraction_sha256"));

        let status = diagnostic_store_status(root.path()).unwrap();
        assert_eq!(status.run_count, 2);
        assert_eq!(status.complete_count, 1);
        assert_eq!(status.partial_count, 1);
        assert_eq!(status.priority_count, 1);
        assert!(status.managed_bytes > 0);
    }

    #[test]
    fn rejects_unmanaged_entries_and_run_symlinks() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("unexpected"), b"not a run\n").unwrap();
        assert!(diagnostic_store_status(root.path()).is_err());

        let second = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(root.path(), second.path().join("linked-run")).unwrap();
        assert!(diagnostic_run_list(second.path()).is_err());
    }

    #[test]
    fn completed_run_rejects_extra_bytes_and_partial_run_requires_valid_start() {
        let root = tempfile::tempdir().unwrap();
        let complete = DiagnosticRecorder::start(
            root.path(),
            &descriptor("complete-run"),
            DiagnosticPolicy::default(),
        );
        let _ = complete.finish(DiagnosticRunStatus::Success, 1_000);
        fs::write(root.path().join("complete-run/extra"), b"").unwrap();
        assert!(diagnostic_run_list(root.path()).is_err());

        let other = tempfile::tempdir().unwrap();
        fs::create_dir(other.path().join("partial-run")).unwrap();
        fs::write(other.path().join("partial-run/run.json"), b"{}\n").unwrap();
        assert!(diagnostic_store_status(other.path()).is_err());
    }

    #[test]
    fn completed_manifest_is_typed_and_exactly_covers_directory_entries() {
        let root = tempfile::tempdir().unwrap();
        let complete = DiagnosticRecorder::start(
            root.path(),
            &descriptor("complete-run"),
            DiagnosticPolicy::default(),
        );
        let _ = complete.finish(DiagnosticRunStatus::Success, 1_000);
        let manifest_path = root.path().join("complete-run/manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest["frames"] = serde_json::json!([{}]);
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(diagnostic_run_list(root.path()).is_err());
    }

    #[test]
    fn rejects_per_run_and_aggregate_capacity_overflow() {
        let root = tempfile::tempdir().unwrap();
        let partial = DiagnosticRecorder::start(
            root.path(),
            &descriptor("oversized-run"),
            DiagnosticPolicy::default(),
        );
        drop(partial);
        File::create(
            root.path()
                .join("oversized-run/frame-00000000000000000001.qoi"),
        )
        .unwrap()
        .set_len(DEFAULT_AGGREGATE_BYTES)
        .unwrap();
        assert!(diagnostic_store_status(root.path()).is_err());

        let aggregate = tempfile::tempdir().unwrap();
        for run_id in ["first-run", "second-run"] {
            let partial = DiagnosticRecorder::start(
                aggregate.path(),
                &descriptor(run_id),
                DiagnosticPolicy::default(),
            );
            drop(partial);
            File::create(
                aggregate
                    .path()
                    .join(run_id)
                    .join("frame-00000000000000000001.qoi"),
            )
            .unwrap()
            .set_len(5 * 1024 * 1024 * 1024)
            .unwrap();
        }
        assert!(diagnostic_run_list(aggregate.path()).is_err());
    }

    #[test]
    fn accepts_an_older_producer_version_under_the_same_schema() {
        let root = tempfile::tempdir().unwrap();
        let partial = DiagnosticRecorder::start(
            root.path(),
            &descriptor("older-run"),
            DiagnosticPolicy::default(),
        );
        drop(partial);
        let start_path = root.path().join("older-run/run.json");
        let mut start: RunStartDocument =
            serde_json::from_slice(&fs::read(&start_path).unwrap()).unwrap();
        start.resource.version = "0.0.0-old".to_owned();
        fs::write(&start_path, canonical_json(&start).unwrap()).unwrap();
        let list = diagnostic_run_list(root.path()).unwrap();
        assert_eq!(list.runs.len(), 1);
        assert_eq!(list.runs[0].run_id, "older-run");
    }

    #[test]
    fn legacy_v1_without_retention_remains_readable_and_does_not_block_a_new_run() {
        let root = tempfile::tempdir().unwrap();
        let legacy = DiagnosticRecorder::start(
            root.path(),
            &descriptor("legacy-run"),
            DiagnosticPolicy::default(),
        );
        drop(legacy);
        let start_path = root.path().join("legacy-run/run.json");
        let start: RunStartDocument =
            serde_json::from_slice(&fs::read(&start_path).unwrap()).unwrap();
        let legacy_start = LegacyRunStartDocument {
            schema: "scorepeek-private-diagnostic-run-start-v1".to_owned(),
            run_id: start.run_id,
            monotonic_start_ms: start.monotonic_start_ms,
            resource: start.resource,
            binding: start.binding,
            policy: LegacyRunPolicy {
                sample_interval_ms: start.policy.sample_interval_ms,
                maximum_run_bytes: start.policy.maximum_run_bytes,
                aggregate_retention_bytes: start.policy.aggregate_retention_bytes,
                normal_retention_hours: start.policy.normal_retention_hours,
                priority_retention_hours: start.policy.priority_retention_hours,
                remote_export_enabled: start.policy.remote_export_enabled,
            },
        };
        let legacy_bytes = canonical_json(&legacy_start).unwrap();
        fs::write(&start_path, &legacy_bytes).unwrap();
        assert!(parse_run_start(&legacy_bytes, "legacy-run").is_ok());

        assert_eq!(diagnostic_run_list(root.path()).unwrap().runs.len(), 1);
        let recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("new-run"),
            DiagnosticPolicy::default(),
        );
        assert_eq!(
            recorder
                .finish(DiagnosticRunStatus::Success, 1_000)
                .completeness,
            Some(DiagnosticCompleteness::Complete)
        );
        assert_eq!(diagnostic_run_list(root.path()).unwrap().runs.len(), 2);
    }

    #[test]
    fn producer_version_requires_semver_syntax() {
        for valid in [
            "0.0.0",
            "1.2.3-old.1",
            "1.2.3+build.7",
            "1.2.3-rc.1+build-7",
        ] {
            assert!(valid_version(valid), "expected valid SemVer: {valid}");
        }
        for invalid in [
            ".",
            "---",
            "1..2",
            "+1",
            "1+2+3",
            "1.2",
            "01.2.3",
            "1.2.3-01",
            "1.2.3+",
            "1.2.3-alpha..1",
        ] {
            assert!(
                !valid_version(invalid),
                "expected invalid SemVer: {invalid}"
            );
        }
    }

    #[test]
    fn partial_runs_enforce_writer_artifact_bounds() {
        let run_bytes = 1_u64;
        let mut files = BTreeMap::from([("run.json".to_owned(), run_bytes)]);
        for index in 0..=MAX_FRAMES_PER_RUN {
            files.insert(format!("frame-{index:020}.qoi"), 1);
        }
        assert!(validate_partial_files(&files, run_bytes).is_err());

        let files = BTreeMap::from([
            ("run.json".to_owned(), run_bytes),
            (
                "facts.ndjson".to_owned(),
                MAX_FACTS_PER_RUN as u64 * MAX_FACT_BYTES as u64 + 1,
            ),
        ]);
        assert!(validate_partial_files(&files, run_bytes).is_err());

        let files = BTreeMap::from([
            ("run.json".to_owned(), run_bytes),
            (
                "fact-00000000000000000000.json".to_owned(),
                MAX_FACT_BYTES as u64 + 1,
            ),
        ]);
        assert!(validate_partial_files(&files, run_bytes).is_err());
    }

    #[test]
    fn rejects_a_run_changed_after_its_individual_inspection() {
        let root = tempfile::tempdir().unwrap();
        for run_id in ["first-run", "second-run"] {
            let partial = DiagnosticRecorder::start(
                root.path(),
                &descriptor(run_id),
                DiagnosticPolicy::default(),
            );
            drop(partial);
        }
        let mut changed = false;
        let result = inspect_store_with(root.path(), |run| {
            if !changed {
                fs::write(run.join("frame-00000000000000000001.qoi"), b"changed\n").unwrap();
                changed = true;
            }
        });
        assert!(result.is_err());
    }

    #[test]
    fn store_lease_excludes_a_second_writer_and_status_observes_activity() {
        let root = tempfile::tempdir().unwrap();
        let lease = DiagnosticStoreLease::acquire(root.path(), 0).unwrap();
        assert_eq!(
            DiagnosticStoreLease::acquire(root.path(), 0).err(),
            Some(DiagnosticErrorType::WorkerUnavailable)
        );
        let active = diagnostic_store_status(root.path()).unwrap();
        assert!(active.writer_active);
        drop(lease);
        let idle = diagnostic_store_status(root.path()).unwrap();
        assert!(!idle.writer_active);
    }

    #[test]
    fn root_lease_survives_lock_marker_rebinding_and_covers_first_writer() {
        let root = tempfile::tempdir().unwrap();
        let root_lock = open_lock_directory(root.path()).unwrap();
        root_lock.try_lock().unwrap();
        assert!(diagnostic_store_status(root.path()).unwrap().writer_active);
        assert!(open_store_anchor(root.path(), false).unwrap().is_none());
        drop(root_lock);

        let lease = DiagnosticStoreLease::acquire(root.path(), 0).unwrap();
        fs::remove_file(root.path().join(STORE_LOCK_FILENAME)).unwrap();
        File::create(root.path().join(STORE_LOCK_FILENAME)).unwrap();
        assert_eq!(
            DiagnosticStoreLease::acquire(root.path(), 0).err(),
            Some(DiagnosticErrorType::WorkerUnavailable)
        );
        drop(lease);
    }

    #[test]
    fn parent_anchor_blocks_a_second_writer_after_root_rebinding() {
        let root = tempfile::tempdir().unwrap();
        let moved = root.path().with_extension("moved-for-lock-test");
        let lease = DiagnosticStoreLease::acquire(root.path(), 0).unwrap();
        fs::rename(root.path(), &moved).unwrap();
        fs::create_dir(root.path()).unwrap();
        assert_eq!(
            DiagnosticStoreLease::acquire(root.path(), 0).err(),
            Some(DiagnosticErrorType::WorkerUnavailable)
        );
        drop(lease);
        fs::remove_dir(root.path()).unwrap();
        fs::rename(moved, root.path()).unwrap();
    }

    #[test]
    fn canonical_parent_anchor_blocks_dotdot_alias_after_root_rebinding() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("store");
        let alias_parent = base.path().join("alias-parent");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&alias_parent).unwrap();
        let alias = alias_parent.join("..").join("store");
        let moved = base.path().join("moved-store");

        let lease = DiagnosticStoreLease::acquire(&alias, 0).unwrap();
        fs::rename(&root, &moved).unwrap();
        fs::create_dir(&root).unwrap();
        assert_eq!(
            DiagnosticStoreLease::acquire(&root, 0).err(),
            Some(DiagnosticErrorType::WorkerUnavailable)
        );
        drop(lease);
    }

    #[test]
    fn canonical_parent_anchor_blocks_intermediate_symlink_alias_after_root_rebinding() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("store");
        fs::create_dir(&root).unwrap();
        let parent_link = base.path().join("parent-link");
        std::os::unix::fs::symlink(base.path(), &parent_link).unwrap();
        let alias = parent_link.join("store");
        let moved = base.path().join("moved-store");

        let lease = DiagnosticStoreLease::acquire(&alias, 0).unwrap();
        fs::rename(&root, &moved).unwrap();
        fs::create_dir(&root).unwrap();
        assert_eq!(
            DiagnosticStoreLease::acquire(&root, 0).err(),
            Some(DiagnosticErrorType::WorkerUnavailable)
        );
        drop(lease);
    }

    #[test]
    fn retention_expires_runs_and_capacity_removes_only_normal_runs() {
        let expired = tempfile::tempdir().unwrap();
        let recorder = DiagnosticRecorder::start(
            expired.path(),
            &descriptor("expired-normal"),
            DiagnosticPolicy::default(),
        );
        let _ = recorder.finish(DiagnosticRunStatus::Success, 1_000);
        let retention_time = inspect_store(expired.path()).unwrap()[0].retention_time;
        let future =
            retention_time + Duration::from_secs(u64::from(NORMAL_RETENTION_HOURS) * 3_600);
        let _lease = DiagnosticStoreLease::acquire_at(expired.path(), 0, future).unwrap();
        assert!(!expired.path().join("expired-normal").exists());

        let capacity = tempfile::tempdir().unwrap();
        let normal = DiagnosticRecorder::start(
            capacity.path(),
            &descriptor("normal-run"),
            DiagnosticPolicy::default(),
        );
        let _ = normal.finish(DiagnosticRunStatus::Success, 1_000);
        let partial = DiagnosticRecorder::start(
            capacity.path(),
            &descriptor("priority-run"),
            DiagnosticPolicy::default(),
        );
        drop(partial);
        let runs = inspect_store(capacity.path()).unwrap();
        let normal_bytes = runs
            .iter()
            .find(|run| run.run_id == "normal-run")
            .unwrap()
            .managed_bytes;
        let current_bytes = runs.iter().map(|run| run.managed_bytes).sum::<u64>();
        File::create(
            capacity
                .path()
                .join("priority-run/frame-00000000000000000001.qoi"),
        )
        .unwrap()
        .set_len(DEFAULT_AGGREGATE_BYTES - current_bytes)
        .unwrap();
        assert_eq!(
            DiagnosticStoreLease::acquire_at(capacity.path(), normal_bytes + 1, SystemTime::now(),)
                .err(),
            Some(DiagnosticErrorType::CapacityExceeded)
        );
        assert!(capacity.path().join("normal-run").exists());
        let _lease =
            DiagnosticStoreLease::acquire_at(capacity.path(), normal_bytes, SystemTime::now())
                .unwrap();
        assert!(!capacity.path().join("normal-run").exists());
        assert!(capacity.path().join("priority-run").exists());
    }

    #[test]
    fn retention_recovers_only_valid_owned_delete_staging() {
        let root = tempfile::tempdir().unwrap();
        let recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("staged-run"),
            DiagnosticPolicy::default(),
        );
        let _ = recorder.finish(DiagnosticRunStatus::Success, 1_000);
        let staging = root
            .path()
            .join(format!("{DELETE_STAGING_PREFIX}staged-run"));
        fs::rename(root.path().join("staged-run"), &staging).unwrap();
        let lease = DiagnosticStoreLease::acquire(root.path(), 0).unwrap();
        assert!(!staging.exists());

        drop(lease);
        let recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("partly-deleted-run"),
            DiagnosticPolicy::default(),
        );
        let _ = recorder.finish(DiagnosticRunStatus::Success, 1_000);
        let partial_staging = root
            .path()
            .join(format!("{DELETE_STAGING_PREFIX}partly-deleted-run"));
        fs::rename(root.path().join("partly-deleted-run"), &partial_staging).unwrap();
        mark_delete_staging(&partial_staging, "partly-deleted-run").unwrap();
        fs::remove_file(partial_staging.join("manifest.json")).unwrap();
        let lease = DiagnosticStoreLease::acquire(root.path(), 0).unwrap();
        assert!(!partial_staging.exists());

        drop(lease);
        let empty_tombstone = root
            .path()
            .join(format!("{DELETE_STAGING_PREFIX}empty-run"));
        fs::create_dir(&empty_tombstone).unwrap();
        let lease = DiagnosticStoreLease::acquire(root.path(), 0).unwrap();
        assert!(!empty_tombstone.exists());

        drop(lease);
        let invalid = root
            .path()
            .join(format!("{DELETE_STAGING_PREFIX}invalid-run"));
        fs::create_dir(&invalid).unwrap();
        fs::write(invalid.join("unowned"), b"preserve\n").unwrap();
        assert_eq!(
            DiagnosticStoreLease::acquire(root.path(), 0).err(),
            Some(DiagnosticErrorType::StoreUnavailable)
        );
        assert!(invalid.join("unowned").exists());
    }

    #[test]
    fn retention_recovers_marker_publication_before_and_after_link() {
        for publication_point in 0..3 {
            let root = tempfile::tempdir().unwrap();
            let run_id = match publication_point {
                0 => "partial-marker",
                1 => "staged-marker",
                _ => "linked-marker",
            };
            let recorder = DiagnosticRecorder::start(
                root.path(),
                &descriptor(run_id),
                DiagnosticPolicy::default(),
            );
            let _ = recorder.finish(DiagnosticRunStatus::Success, 1_000);
            let staging = root.path().join(format!("{DELETE_STAGING_PREFIX}{run_id}"));
            fs::rename(root.path().join(run_id), &staging).unwrap();
            let marker = DeleteMarkerDocument {
                schema: "scorepeek-diagnostic-delete-staging-v1".to_owned(),
                run_id: run_id.to_owned(),
                files: run_files(&staging)
                    .unwrap()
                    .into_iter()
                    .map(|(filename, bytes)| DeleteMarkerFile { filename, bytes })
                    .collect(),
            };
            let marker_staging = staging.join(DELETE_MARKER_STAGING_FILENAME);
            if publication_point == 0 {
                fs::write(&marker_staging, b"{\"schema\":").unwrap();
            } else {
                fs::write(&marker_staging, canonical_json(&marker).unwrap()).unwrap();
            }
            if publication_point == 2 {
                fs::hard_link(&marker_staging, staging.join(DELETE_MARKER_FILENAME)).unwrap();
            }

            let lease = DiagnosticStoreLease::acquire(root.path(), 0).unwrap();
            assert!(!staging.exists());
            drop(lease);
        }
    }

    #[test]
    fn marker_bound_staging_rejects_unknown_entries_before_deleting() {
        let root = tempfile::tempdir().unwrap();
        let recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("marked-run"),
            DiagnosticPolicy::default(),
        );
        let _ = recorder.finish(DiagnosticRunStatus::Success, 1_000);
        let staging = root
            .path()
            .join(format!("{DELETE_STAGING_PREFIX}marked-run"));
        fs::rename(root.path().join("marked-run"), &staging).unwrap();
        mark_delete_staging(&staging, "marked-run").unwrap();
        fs::write(staging.join("unowned"), b"preserve\n").unwrap();

        assert_eq!(
            DiagnosticStoreLease::acquire(root.path(), 0).err(),
            Some(DiagnosticErrorType::StoreUnavailable)
        );
        assert!(staging.join("manifest.json").exists());
        assert!(staging.join("unowned").exists());
    }

    #[test]
    fn delete_recovery_preserves_a_valid_document_staging_symlink() {
        let root = tempfile::tempdir().unwrap();
        let recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("delete-symlink-run"),
            DiagnosticPolicy::default(),
        );
        let _ = recorder.finish(DiagnosticRunStatus::Success, 1_000);
        let staging = root
            .path()
            .join(format!("{DELETE_STAGING_PREFIX}delete-symlink-run"));
        fs::rename(root.path().join("delete-symlink-run"), &staging).unwrap();
        let document = DeleteMarkerDocument {
            schema: "scorepeek-diagnostic-delete-staging-v1".to_owned(),
            run_id: "delete-symlink-run".to_owned(),
            files: run_files(&staging)
                .unwrap()
                .into_iter()
                .map(|(filename, bytes)| DeleteMarkerFile { filename, bytes })
                .collect(),
        };
        let external = tempfile::NamedTempFile::new().unwrap();
        fs::write(external.path(), canonical_json(&document).unwrap()).unwrap();
        let marker_staging = staging.join(DELETE_MARKER_STAGING_FILENAME);
        std::os::unix::fs::symlink(external.path(), &marker_staging).unwrap();

        assert_eq!(
            DiagnosticStoreLease::acquire(root.path(), 0).err(),
            Some(DiagnosticErrorType::StoreUnavailable)
        );
        assert!(
            marker_staging
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(staging.join("manifest.json").is_file());
        assert!(external.path().is_file());
    }

    #[test]
    fn exact_start_capacity_does_not_evict_a_normal_run_for_manifest_reserve() {
        let probe = tempfile::tempdir().unwrap();
        let probe_run = DiagnosticRecorder::start(
            probe.path(),
            &descriptor("new-run"),
            DiagnosticPolicy::default(),
        );
        drop(probe_run);
        let new_start_bytes = fs::metadata(probe.path().join("new-run/run.json"))
            .unwrap()
            .len();

        let root = tempfile::tempdir().unwrap();
        let normal = DiagnosticRecorder::start(
            root.path(),
            &descriptor("normal-run"),
            DiagnosticPolicy::default(),
        );
        let _ = normal.finish(DiagnosticRunStatus::Success, 1_000);
        let normal_bytes = inspect_store(root.path()).unwrap()[0].managed_bytes;
        let priority = DiagnosticRecorder::start(
            root.path(),
            &descriptor("priority-run"),
            DiagnosticPolicy::default(),
        );
        drop(priority);
        let priority_start_bytes = fs::metadata(root.path().join("priority-run/run.json"))
            .unwrap()
            .len();
        let free_bytes = new_start_bytes + 128;
        let sparse_bytes = DEFAULT_AGGREGATE_BYTES
            .checked_sub(normal_bytes + priority_start_bytes + free_bytes)
            .unwrap();
        File::create(
            root.path()
                .join("priority-run/frame-00000000000000000001.qoi"),
        )
        .unwrap()
        .set_len(sparse_bytes)
        .unwrap();

        let new_run = DiagnosticRecorder::start(
            root.path(),
            &descriptor("new-run"),
            DiagnosticPolicy::default(),
        );
        drop(new_run);
        assert!(root.path().join("normal-run").exists());
        assert!(root.path().join("new-run/run.json").exists());
    }

    #[test]
    fn run_id_collision_does_not_evict_normal_evidence() {
        let root = tempfile::tempdir().unwrap();
        let normal = DiagnosticRecorder::start(
            root.path(),
            &descriptor("normal-run"),
            DiagnosticPolicy::default(),
        );
        let _ = normal.finish(DiagnosticRunStatus::Success, 1_000);
        let normal_bytes = inspect_store(root.path()).unwrap()[0].managed_bytes;
        let collision = DiagnosticRecorder::start(
            root.path(),
            &descriptor("collision-run"),
            DiagnosticPolicy::default(),
        );
        drop(collision);
        let collision_bytes = fs::metadata(root.path().join("collision-run/run.json"))
            .unwrap()
            .len();
        File::create(
            root.path()
                .join("collision-run/frame-00000000000000000001.qoi"),
        )
        .unwrap()
        .set_len(DEFAULT_AGGREGATE_BYTES - normal_bytes - collision_bytes)
        .unwrap();
        let before = inspect_store(root.path()).unwrap();

        let attempted = DiagnosticRecorder::start(
            root.path(),
            &descriptor("collision-run"),
            DiagnosticPolicy::default(),
        );
        assert!(matches!(
            attempted,
            DiagnosticRecorder::Degraded(crate::diagnostic_recording::DiagnosticDegradation {
                error_type: DiagnosticErrorType::StoreUnavailable
            })
        ));
        let after = inspect_store(root.path()).unwrap();
        assert_eq!(before.len(), after.len());
        assert!(root.path().join("normal-run").exists());
    }

    #[test]
    fn failed_publication_reservation_can_be_released_exactly() {
        let root = tempfile::tempdir().unwrap();
        let mut lease = DiagnosticStoreLease::acquire(root.path(), 0).unwrap();
        let initial = lease.managed_bytes;
        lease.reserve(7).unwrap();
        assert_eq!(lease.managed_bytes, initial + 7);
        lease.release(7);
        assert_eq!(lease.managed_bytes, initial);
    }

    #[test]
    fn retention_preserves_a_valid_run_replaced_after_inventory() {
        let root = tempfile::tempdir().unwrap();
        let recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("changed-run"),
            DiagnosticPolicy::default(),
        );
        let _ = recorder.finish(DiagnosticRunStatus::Success, 1_000);
        let original = inspect_store(root.path()).unwrap().remove(0);
        let start_path = root.path().join("changed-run/run.json");
        let mut start: RunStartDocument =
            serde_json::from_slice(&fs::read(&start_path).unwrap()).unwrap();
        start.resource.version = "0.0.0-replaced".to_owned();
        fs::write(&start_path, canonical_json(&start).unwrap()).unwrap();

        assert_eq!(
            delete_run(root.path(), &original).err(),
            Some(DiagnosticErrorType::StoreUnavailable)
        );
        assert!(root.path().join("changed-run").exists());
    }

    #[test]
    fn priority_capacity_prevents_a_new_run_without_changing_the_store() {
        let root = tempfile::tempdir().unwrap();
        let partial = DiagnosticRecorder::start(
            root.path(),
            &descriptor("priority-run"),
            DiagnosticPolicy::default(),
        );
        drop(partial);
        let run_bytes = fs::metadata(root.path().join("priority-run/run.json"))
            .unwrap()
            .len();
        File::create(
            root.path()
                .join("priority-run/frame-00000000000000000001.qoi"),
        )
        .unwrap()
        .set_len(DEFAULT_AGGREGATE_BYTES - run_bytes)
        .unwrap();

        let blocked = DiagnosticRecorder::start(
            root.path(),
            &descriptor("blocked-run"),
            DiagnosticPolicy::default(),
        );
        let outcome = blocked.finish(DiagnosticRunStatus::Cancel, 0);
        assert_eq!(
            outcome.error_type,
            Some(DiagnosticErrorType::CapacityExceeded)
        );
        assert!(!root.path().join("blocked-run").exists());
        let status = diagnostic_store_status(root.path()).unwrap();
        assert_eq!(status.managed_bytes, DEFAULT_AGGREGATE_BYTES);
        assert_eq!(status.priority_count, 1);
    }

    #[test]
    fn freeze_is_digest_confirmed_idempotent_and_priority() {
        let root = tempfile::tempdir().unwrap();
        let recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("freeze-run"),
            DiagnosticPolicy::default(),
        );
        let _ = recorder.finish(DiagnosticRunStatus::Success, 1_000);
        let run = inspect_store(root.path()).unwrap().remove(0);
        assert!(!run.priority);
        assert!(!run.frozen);

        assert!(
            diagnostic_freeze(
                root.path(),
                "freeze-run",
                &"0".repeat(64),
                run.manifest_sha256.as_deref(),
            )
            .is_err()
        );
        assert!(
            !root
                .path()
                .join("freeze-run")
                .join(FREEZE_FILENAME)
                .exists()
        );

        let first = diagnostic_freeze(
            root.path(),
            "freeze-run",
            &run.run_sha256,
            run.manifest_sha256.as_deref(),
        )
        .unwrap();
        let second = diagnostic_freeze(
            root.path(),
            "freeze-run",
            &run.run_sha256,
            run.manifest_sha256.as_deref(),
        )
        .unwrap();
        assert!(first.frozen && second.frozen);
        let frozen = inspect_store(root.path()).unwrap().remove(0);
        assert!(frozen.frozen);
        assert!(frozen.priority);
        assert_eq!(frozen.managed_bytes, run.managed_bytes);
    }

    #[test]
    fn delete_requires_exact_current_digests() {
        let root = tempfile::tempdir().unwrap();
        let recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("delete-run"),
            DiagnosticPolicy::default(),
        );
        let _ = recorder.finish(DiagnosticRunStatus::Success, 1_000);
        let run = inspect_store(root.path()).unwrap().remove(0);
        assert!(diagnostic_delete(root.path(), "delete-run", &run.run_sha256, None,).is_err());
        assert!(root.path().join("delete-run").exists());

        let outcome = diagnostic_delete(
            root.path(),
            "delete-run",
            &run.run_sha256,
            run.manifest_sha256.as_deref(),
        )
        .unwrap();
        assert_eq!(outcome.operation, "delete");
        assert!(!root.path().join("delete-run").exists());
    }

    #[test]
    fn partial_run_uses_explicit_no_manifest_confirmation() {
        let root = tempfile::tempdir().unwrap();
        let recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("partial-control"),
            DiagnosticPolicy::default(),
        );
        drop(recorder);
        let run = inspect_store(root.path()).unwrap().remove(0);
        assert!(run.manifest_sha256.is_none());
        let frozen =
            diagnostic_freeze(root.path(), "partial-control", &run.run_sha256, None).unwrap();
        assert!(frozen.frozen);
        diagnostic_delete(root.path(), "partial-control", &run.run_sha256, None).unwrap();
        assert!(!root.path().join("partial-control").exists());
    }

    #[test]
    fn export_is_complete_verified_and_create_only() {
        let root = tempfile::tempdir().unwrap();
        let export_parent = tempfile::tempdir().unwrap();
        let mut recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("export-run"),
            DiagnosticPolicy::default(),
        );
        let pixels = vec![7; CANONICAL_BYTES];
        assert!(matches!(
            recorder.record_frame(DiagnosticFrameInput {
                sequence: 1,
                monotonic_start_ms: 0,
                monotonic_end_ms: 16,
                pixels: &pixels,
                source: None,
            }),
            crate::diagnostic_recording::DiagnosticRecordOutcome::Recorded
        ));
        let _ = recorder.finish(DiagnosticRunStatus::Success, 1_000);
        let run = inspect_store(root.path()).unwrap().remove(0);
        let destination = export_parent.path().join("exported-run");
        let outcome = diagnostic_export(
            root.path(),
            "export-run",
            &run.run_sha256,
            run.manifest_sha256.as_deref().unwrap(),
            &destination,
        )
        .unwrap();
        assert_eq!(outcome.operation, "export");
        assert!(destination.join(EXPORT_MANIFEST_FILENAME).is_file());
        assert!(destination.join("run.json").is_file());
        assert!(destination.join("manifest.json").is_file());
        assert!(
            diagnostic_export(
                root.path(),
                "export-run",
                &run.run_sha256,
                run.manifest_sha256.as_deref().unwrap(),
                &destination,
            )
            .is_err()
        );

        let frame = root
            .path()
            .join("export-run/frame-00000000000000000001.qoi");
        let mut corrupted = fs::read(&frame).unwrap();
        let last = corrupted.len() - 1;
        corrupted[last] ^= 1;
        fs::write(&frame, corrupted).unwrap();
        assert!(
            diagnostic_export(
                root.path(),
                "export-run",
                &run.run_sha256,
                run.manifest_sha256.as_deref().unwrap(),
                &export_parent.path().join("corrupt-export"),
            )
            .is_err()
        );
    }

    #[test]
    fn export_rejects_a_manifest_bearing_partial_run_before_claiming_destination() {
        let root = tempfile::tempdir().unwrap();
        let export_parent = tempfile::tempdir().unwrap();
        let mut recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("partial-export-run"),
            DiagnosticPolicy::default(),
        );
        let pixels = vec![7; CANONICAL_BYTES];
        let _ = recorder.record_frame(DiagnosticFrameInput {
            sequence: 1,
            monotonic_start_ms: 0,
            monotonic_end_ms: 16,
            pixels: &pixels,
            source: None,
        });
        let _ = recorder.record_frame(DiagnosticFrameInput {
            sequence: 3,
            monotonic_start_ms: 32,
            monotonic_end_ms: 48,
            pixels: &pixels,
            source: None,
        });
        let outcome = recorder.finish(DiagnosticRunStatus::Success, 1_000);
        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Partial));
        let run = inspect_store(root.path()).unwrap().remove(0);
        assert!(run.manifest_sha256.is_some());
        let destination = export_parent.path().join("partial-export");
        assert!(
            diagnostic_export(
                root.path(),
                "partial-export-run",
                &run.run_sha256,
                run.manifest_sha256.as_deref().unwrap(),
                &destination,
            )
            .is_err()
        );
        assert!(!destination.exists());
    }

    #[test]
    fn export_rejects_destinations_resolving_inside_store() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("store");
        fs::create_dir(&root).unwrap();
        let recorder = DiagnosticRecorder::start(
            &root,
            &descriptor("inside-export-run"),
            DiagnosticPolicy::default(),
        );
        let _ = recorder.finish(DiagnosticRunStatus::Success, 1_000);
        let run = inspect_store(&root).unwrap().remove(0);
        let manifest_sha256 = run.manifest_sha256.as_deref().unwrap();

        let alias_parent = base.path().join("alias-parent");
        fs::create_dir(&alias_parent).unwrap();
        let root_alias = alias_parent.join("..").join("store");
        let direct_destination = root.join("direct-inside-export");
        assert!(
            diagnostic_export(
                &root_alias,
                "inside-export-run",
                &run.run_sha256,
                manifest_sha256,
                &direct_destination,
            )
            .is_err()
        );
        assert!(!direct_destination.exists());

        let nested = root.join("nested");
        fs::create_dir(&nested).unwrap();
        let store_link = base.path().join("store-link");
        std::os::unix::fs::symlink(&root, &store_link).unwrap();
        let symlink_destination = store_link.join("nested/symlink-inside-export");
        assert!(
            diagnostic_export(
                &root,
                "inside-export-run",
                &run.run_sha256,
                manifest_sha256,
                &symlink_destination,
            )
            .is_err()
        );
        assert!(!nested.join("symlink-inside-export").exists());
    }

    #[test]
    fn next_writer_recovers_interrupted_freeze_publication() {
        let root = tempfile::tempdir().unwrap();
        let recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("recover-freeze"),
            DiagnosticPolicy::default(),
        );
        let _ = recorder.finish(DiagnosticRunStatus::Success, 1_000);
        let run = inspect_store(root.path()).unwrap().remove(0);
        let document = FreezeDocument {
            schema: "scorepeek-diagnostic-freeze-v1".to_owned(),
            run_id: run.run_id.clone(),
            run_sha256: run.run_sha256.clone(),
            manifest_sha256: run.manifest_sha256.clone(),
        };
        fs::write(
            root.path()
                .join("recover-freeze")
                .join(FREEZE_STAGING_FILENAME),
            canonical_json(&document).unwrap(),
        )
        .unwrap();

        let lease = DiagnosticStoreLease::acquire_control(root.path()).unwrap();
        let frozen = inspect_store(root.path()).unwrap().remove(0);
        assert!(frozen.frozen);
        drop(lease);
    }

    #[test]
    fn freeze_recovery_preserves_a_valid_document_staging_symlink() {
        let root = tempfile::tempdir().unwrap();
        let recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("freeze-symlink-run"),
            DiagnosticPolicy::default(),
        );
        let _ = recorder.finish(DiagnosticRunStatus::Success, 1_000);
        let run = inspect_store(root.path()).unwrap().remove(0);
        let external = tempfile::NamedTempFile::new().unwrap();
        let document = FreezeDocument {
            schema: "scorepeek-diagnostic-freeze-v1".to_owned(),
            run_id: run.run_id.clone(),
            run_sha256: run.run_sha256.clone(),
            manifest_sha256: run.manifest_sha256.clone(),
        };
        fs::write(external.path(), canonical_json(&document).unwrap()).unwrap();
        let staging = root
            .path()
            .join("freeze-symlink-run")
            .join(FREEZE_STAGING_FILENAME);
        std::os::unix::fs::symlink(external.path(), &staging).unwrap();

        assert!(
            diagnostic_freeze(
                root.path(),
                "freeze-symlink-run",
                &run.run_sha256,
                run.manifest_sha256.as_deref(),
            )
            .is_err()
        );
        assert!(staging.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(external.path().is_file());
    }
}
