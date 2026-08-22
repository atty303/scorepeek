use std::collections::BTreeMap;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::diagnostic_recording::{
    DEFAULT_AGGREGATE_BYTES, DiagnosticCompleteness, DiagnosticErrorType, DiagnosticRunStatus,
    MANIFEST_RESERVE_BYTES, MAX_DEGRADATIONS_PER_RUN, MAX_FACT_BYTES, MAX_FACTS_PER_RUN,
    MAX_FRAMES_PER_RUN, NORMAL_RETENTION_HOURS, PRIORITY_RETENTION_HOURS,
};
const MAX_RUNS: usize = 8_192;
const MAX_FILES_PER_RUN: usize = 50_000;
const MAX_START_BYTES: u64 = 64 * 1024;
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const STORE_LOCK_FILENAME: &str = ".scorepeek-diagnostic-store.lock";
const DELETE_STAGING_PREFIX: &str = ".scorepeek-diagnostic-delete-";
const DELETE_MARKER_FILENAME: &str = ".scorepeek-diagnostic-delete-owner-v1";
const DELETE_MARKER_STAGING_FILENAME: &str = ".scorepeek-diagnostic-delete-owner-staging-v1";

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
    facts: Vec<FactReference>,
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
        schema: "scorepeek-diagnostic-run-list-v1",
        runs: inspect_store(root)?,
    })
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
    let before = directory_identity(directory)?;
    let start_bytes = read_bounded_regular(&directory.join("run.json"), MAX_START_BYTES)?;
    let start: RunStartDocument =
        serde_json::from_slice(&start_bytes).map_err(|_| invalid_store())?;
    if start.schema != "scorepeek-private-diagnostic-run-start-v1"
        || start.run_id != run_id
        || !valid_start(&start)
        || canonical_json(&start)? != start_bytes
    {
        return Err(invalid_store());
    }
    let run_sha256 = encode_sha256(&start_bytes);
    let files = run_files(directory)?;
    let managed_bytes = files.values().try_fold(0_u64, |total, bytes| {
        total.checked_add(*bytes).ok_or_else(invalid_store)
    })?;
    if managed_bytes > start.policy.maximum_run_bytes {
        return Err(invalid_store());
    }
    let manifest_path = directory.join("manifest.json");
    let summary = match manifest_path.symlink_metadata() {
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
    if before != directory_identity(directory)? {
        return Err(invalid_store());
    }
    Ok(summary)
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
        let (anchor_path, anchor_lock) =
            open_store_anchor(root, true)?.ok_or(DiagnosticErrorType::StoreUnavailable)?;
        match anchor_lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(DiagnosticErrorType::WorkerUnavailable);
            }
            Err(std::fs::TryLockError::Error(_)) => {
                return Err(DiagnosticErrorType::StoreUnavailable);
            }
        }
        let root_inode = metadata_inode(
            &root
                .symlink_metadata()
                .map_err(|_| DiagnosticErrorType::StoreUnavailable)?,
        );
        if root_inode
            != metadata_inode(
                &root_lock
                    .metadata()
                    .map_err(|_| DiagnosticErrorType::StoreUnavailable)?,
            )
        {
            return Err(DiagnosticErrorType::StoreUnavailable);
        }
        ensure_store_lock_marker(root)?;
        recover_delete_staging(root)?;
        let mut runs = inspect_store(root).map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
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
                delete_run(root, &run)?;
                managed_bytes = managed_bytes
                    .checked_sub(run.managed_bytes)
                    .ok_or(DiagnosticErrorType::StoreUnavailable)?;
            } else if !run.priority {
                normal_candidates.push(run);
            }
        }
        let mut lease = Self {
            root: root.to_owned(),
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
    let Some((_, anchor_lock)) = open_store_anchor(root, false).map_err(|_| invalid_store())?
    else {
        return Ok((
            false,
            Some((
                root_lock.try_clone().map_err(|_| invalid_store())?,
                root_lock,
            )),
        ));
    };
    match anchor_lock.try_lock_shared() {
        Ok(()) => Ok((false, Some((root_lock, anchor_lock)))),
        Err(std::fs::TryLockError::WouldBlock) => Ok((true, None)),
        Err(std::fs::TryLockError::Error(_)) => Err(invalid_store()),
    }
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
        .symlink_metadata()
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    if !path.is_absolute() || !before.is_dir() || before.file_type().is_symlink() {
        return Err(DiagnosticErrorType::StoreUnavailable);
    }
    let file = File::open(path).map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    let opened = file
        .metadata()
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    let after = path
        .symlink_metadata()
        .map_err(|_| DiagnosticErrorType::StoreUnavailable)?;
    if !same_inode(&before, &opened) || !same_inode(&before, &after) {
        return Err(DiagnosticErrorType::StoreUnavailable);
    }
    Ok(file)
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
    let bytes = read_bounded_regular(marker, MAX_MANIFEST_BYTES)
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
            || file.bytes == 0
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
    if manifest.schema != "scorepeek-private-diagnostic-run-v1"
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

fn valid_manifest_entries(
    manifest: &RunManifestDocument,
    files: &BTreeMap<String, u64>,
    run_bytes: u64,
    manifest_bytes: u64,
    monotonic_start_ms: u64,
) -> bool {
    if manifest.frames.len() > MAX_FRAMES_PER_RUN
        || manifest.facts.len() > MAX_FACTS_PER_RUN
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
        let Some(next) = artifact_bytes.checked_add(frame.bytes) else {
            return false;
        };
        artifact_bytes = next;
    }
    for (expected_index, fact) in manifest.facts.iter().enumerate() {
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
        if *bytes == 0 {
            return Err(invalid_store());
        }
        if valid_indexed_artifact_name(name, "frame-", ".qoi") {
            frame_count = frame_count.checked_add(1).ok_or_else(invalid_store)?;
            if frame_count > MAX_FRAMES_PER_RUN {
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
    let metadata = path.symlink_metadata().map_err(|_| invalid_store())?;
    if !path.is_absolute() || !metadata.is_dir() || metadata.file_type().is_symlink() {
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
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
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
        DiagnosticBinding, DiagnosticCompleteness, DiagnosticPolicy, DiagnosticRecorder,
        DiagnosticReplayBinding, DiagnosticResource, DiagnosticRunDescriptor, DiagnosticRunStatus,
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

        let mut files = BTreeMap::from([("run.json".to_owned(), run_bytes)]);
        for index in 0..=MAX_FACTS_PER_RUN {
            files.insert(format!("fact-{index:020}.json"), 1);
        }
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
}
