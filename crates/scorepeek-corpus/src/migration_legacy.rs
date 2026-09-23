//! Removable readers for private v2/v4 canonical artifacts. No live crate owns compatibility.
//!
//! The operator supplies a v2 or v4 recording and its v4 capture-session document.
//! The source remains read-only; the converted v5 recording is published in a new directory.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const V4_RECORDING: &str = "scorepeek-canonical-session-recording-v4";
const V2_RECORDING: &str = "scorepeek-canonical-session-recording-v2";
const V4_SESSION: &str = "scorepeek-private-capture-session-v4";
const V5_RECORDING: &str = "scorepeek-canonical-session-recording-v5";
const V6_LABEL: &str = "scorepeek-private-session-regression-label-v6";
const V2_LABEL: &str = "scorepeek-private-canonical-regression-label-v2";
const FRAME_CONTRACT: &str = "scorepeek-canonical-rgb8-1920x1080-v1";
const MAX_DOCUMENT: u64 = 16 * 1024 * 1024;
const MAX_TICKS: usize = 250_000;
const MAX_SEGMENTS: usize = 20_000;

#[derive(Debug)]
pub enum MigrationError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Recording(crate::canonical::RecordingError),
    Invalid(&'static str),
    MissingObject(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyActiveSuite {
    schema: String,
    generation_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacySuite {
    schema: String,
    previous_generation_sha256: Option<String>,
    entries: Vec<LegacySuiteEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacySuiteEntry {
    session_sha256: String,
    label_sha256: String,
}

/// Converts one label that is included in the old active suite, preserving its reviewed truth.
/// The old store is read-only and no current runtime schema is needed.
///
/// # Errors
/// Rejects a missing active binding, changed content-addressed bytes, or a mismatched session.
pub fn migrate_reviewed_label(
    old_store: &Path,
    old_session_sha256: &str,
    new_session_sha256: &str,
    new_recording_root: &Path,
) -> Result<crate::store::RegressionLabel, MigrationError> {
    if !is_sha256(old_session_sha256) || !is_sha256(new_session_sha256) {
        return Err(MigrationError::Invalid(
            "reviewed label session digest is invalid",
        ));
    }
    let old_path = old_store
        .join("sessions")
        .join(format!("{old_session_sha256}.json"));
    if digest_file(&old_path)?.0 != old_session_sha256 {
        return Err(MigrationError::Invalid(
            "old reviewed session digest differs",
        ));
    }
    let old_session: V4Session = serde_json::from_slice(&read_document(&old_path)?)?;
    let new_manifest: V5BindingManifest = serde_json::from_slice(&read_document(
        &new_recording_root.join("canonical-manifest.json"),
    )?)?;
    if !new_manifest.valid()
        || old_session.schema != V4_SESSION
        || old_session.source_session_id != new_manifest.session_id
        || !reviewed_frames_match_recording(&old_session, &new_manifest, new_recording_root)?
    {
        return Err(MigrationError::Invalid("reviewed session identity differs"));
    }
    let active: LegacyActiveSuite =
        serde_json::from_slice(&read_document(&old_store.join("active-suite.json"))?)?;
    if active.schema != "scorepeek-private-regression-suite-active-v1"
        || !is_sha256(&active.generation_sha256)
    {
        return Err(MigrationError::Invalid("old active suite is invalid"));
    }
    let suite_path = old_store
        .join("suites")
        .join(format!("{}.json", active.generation_sha256));
    if digest_file(&suite_path)?.0 != active.generation_sha256 {
        return Err(MigrationError::Invalid("old active suite digest differs"));
    }
    let suite: LegacySuite = serde_json::from_slice(&read_document(&suite_path)?)?;
    if suite.schema != "scorepeek-private-regression-suite-v1" || suite.entries.len() > 1024 {
        return Err(MigrationError::Invalid("old suite contract differs"));
    }
    let _ = suite.previous_generation_sha256;
    let matches = suite
        .entries
        .iter()
        .filter(|entry| entry.session_sha256 == old_session_sha256)
        .collect::<Vec<_>>();
    let [entry] = matches.as_slice() else {
        return Err(MigrationError::Invalid(
            "old session is not uniquely active",
        ));
    };
    if !is_sha256(&entry.label_sha256) {
        return Err(MigrationError::Invalid("old label digest is invalid"));
    }
    let label_path = old_store
        .join("labels")
        .join(format!("{}.json", entry.label_sha256));
    if digest_file(&label_path)?.0 != entry.label_sha256 {
        return Err(MigrationError::Invalid("old reviewed label digest differs"));
    }
    let mut value: Value = serde_json::from_slice(&read_document(&label_path)?)?;
    if value.get("schema").and_then(Value::as_str) != Some(V6_LABEL)
        || value.get("session_sha256").and_then(Value::as_str) != Some(old_session_sha256)
        || value.get("disposition").and_then(Value::as_str) != Some("include")
    {
        return Err(MigrationError::Invalid(
            "old reviewed label binding differs",
        ));
    }
    value["schema"] = json!(V2_LABEL);
    value["session_sha256"] = json!(new_session_sha256);
    let label: crate::store::RegressionLabel = serde_json::from_value(value)?;
    Ok(label)
}

fn reviewed_frames_match_recording(
    old_session: &V4Session,
    new_manifest: &V5BindingManifest,
    new_recording_root: &Path,
) -> Result<bool, MigrationError> {
    let frames = &old_session.canonical_frames;
    let mut cursor: usize = 0;
    for segment in &new_manifest.segments {
        let Ok(count) = usize::try_from(segment.frames) else {
            return Ok(false);
        };
        let Some(group) = frames.get(cursor..cursor.saturating_add(count)) else {
            return Ok(false);
        };
        if group.first().map(|frame| frame.sequence) != Some(segment.first_sequence)
            || group.last().map(|frame| frame.sequence) != Some(segment.last_sequence)
            || group
                .iter()
                .any(|frame| frame.artifact_sha256 != segment.sha256)
            || group
                .windows(2)
                .any(|pair| pair[0].sequence >= pair[1].sequence)
        {
            return Ok(false);
        }
        cursor += count;
    }
    if cursor != frames.len() {
        return Ok(false);
    }
    let index_path = new_recording_root.join(&new_manifest.tick_index.path);
    let (sha256, bytes) = digest_file(&index_path)?;
    if sha256 != new_manifest.tick_index.sha256 || bytes != new_manifest.tick_index.bytes {
        return Ok(false);
    }
    let mut reviewed = frames.iter();
    let mut tick_count = 0_u64;
    for line in BufReader::new(File::open(index_path)?).lines() {
        let tick: V5BindingTick = serde_json::from_str(&line?)?;
        tick_count += 1;
        if tick.disposition.kind == "retained"
            && reviewed.next().map(|frame| frame.sequence) != Some(tick.sequence)
        {
            return Ok(false);
        }
    }
    Ok(tick_count == new_manifest.tick_count && reviewed.next().is_none())
}

impl From<std::io::Error> for MigrationError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for MigrationError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Deserialize)]
struct V4Manifest {
    schema: String,
    completeness: String,
    tick_count: usize,
    segments: Vec<V4Segment>,
    completeness_reasons: Vec<String>,
    game_version: Option<Value>,
    tick_index_sha256: Option<String>,
}

#[derive(Deserialize)]
struct V4Segment {
    path: String,
    first_sequence: u64,
    last_sequence: u64,
    frames: usize,
    bytes: u64,
    encoded_sha256: Option<String>,
}

#[derive(Deserialize)]
struct V4Session {
    schema: String,
    source_session_id: String,
    completeness: String,
    game_version: Value,
    canonical_frames: Vec<V4ReviewFrame>,
    artifacts: Vec<V4Artifact>,
}

#[derive(Deserialize)]
struct V4ReviewFrame {
    sequence: u64,
    artifact_sha256: String,
}

#[derive(Deserialize)]
struct V4Artifact {
    kind: String,
    source_path: String,
    sha256: String,
    bytes: u64,
}

// Local v5 binding view: legacy migration does not import compatibility into core.
#[derive(Deserialize)]
struct V5BindingManifest {
    schema: String,
    frame_contract: String,
    session_id: String,
    shape: Value,
    tick_index: V5BindingIndex,
    tick_count: u64,
    segments: Vec<V5BindingSegment>,
    completeness: String,
    completeness_reasons: Vec<Value>,
}

impl V5BindingManifest {
    fn valid(&self) -> bool {
        self.schema == V5_RECORDING
            && self.frame_contract == FRAME_CONTRACT
            && self.shape == json!({"width":1920,"height":1080,"pixel_format":"rgb8"})
            && self.completeness == "complete"
            && self.completeness_reasons.is_empty()
            && self.tick_count > 0
            && self.tick_count <= MAX_TICKS as u64
            && self.tick_count == self.tick_index.count
            && self.tick_index.path == "canonical-ticks.ndjson"
            && is_sha256(&self.tick_index.sha256)
            && self.tick_index.bytes > 0
            && !self.segments.is_empty()
            && self.segments.len() <= MAX_SEGMENTS
            && self.segments.iter().enumerate().all(|(index, segment)| {
                segment.path == format!("segment-{index:04}.mkv")
                    && segment.frames > 0
                    && segment.frames <= 600
                    && segment.first_sequence <= segment.last_sequence
                    && is_sha256(&segment.sha256)
            })
            && self
                .segments
                .windows(2)
                .all(|pair| pair[0].last_sequence < pair[1].first_sequence)
    }
}

#[derive(Deserialize)]
struct V5BindingIndex {
    path: String,
    sha256: String,
    bytes: u64,
    count: u64,
}

#[derive(Deserialize)]
struct V5BindingSegment {
    path: String,
    first_sequence: u64,
    last_sequence: u64,
    frames: u64,
    sha256: String,
}

#[derive(Deserialize)]
struct V5BindingTick {
    sequence: u64,
    disposition: V5BindingDisposition,
}

#[derive(Deserialize)]
struct V5BindingDisposition {
    kind: String,
}

#[derive(Deserialize, Serialize)]
struct V4Tick {
    sequence: u64,
    source_sequence: u64,
    monotonic_ms: u64,
    screen: Value,
    semantic_episode_id: Option<u64>,
    disposition: String,
}

fn read_document(path: &Path) -> Result<Vec<u8>, MigrationError> {
    let metadata = path.symlink_metadata()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_DOCUMENT {
        return Err(MigrationError::Invalid(
            "v4 document is not a bounded regular file",
        ));
    }
    Ok(fs::read(path)?)
}

fn digest_file(path: &Path) -> Result<(String, u64), MigrationError> {
    let metadata = path.symlink_metadata()?;
    if !metadata.file_type().is_file() {
        return Err(MigrationError::Invalid("v4 segment is not a regular file"));
    }
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        bytes = bytes.saturating_add(count as u64);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        write!(&mut hex, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok((hex, bytes))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn materialize_object(
    old_store: &Path,
    object_mirror: Option<&Path>,
    artifact: &V4Artifact,
    destination: &Path,
) -> Result<(), MigrationError> {
    if !is_sha256(&artifact.sha256) {
        return Err(MigrationError::Invalid("v4 object digest is invalid"));
    }
    let local = old_store.join("objects").join(&artifact.sha256);
    let source = if local.exists() {
        local
    } else if let Some(mirror) = object_mirror
        && mirror.join(&artifact.sha256).exists()
    {
        mirror.join(&artifact.sha256)
    } else {
        return Err(MigrationError::MissingObject(artifact.sha256.clone()));
    };
    let (digest, bytes) = digest_file(&source)?;
    if digest != artifact.sha256 || bytes != artifact.bytes {
        return Err(MigrationError::Invalid("v4 object integrity differs"));
    }
    fs::copy(&source, destination)?;
    let (copied_digest, copied_bytes) = digest_file(destination)?;
    if copied_digest != digest || copied_bytes != bytes {
        return Err(MigrationError::Invalid("v4 object changed while copying"));
    }
    Ok(())
}

fn one_artifact<'a>(
    session: &'a V4Session,
    kind: &str,
    source_path: &str,
) -> Result<&'a V4Artifact, MigrationError> {
    let matches = session
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == kind && artifact.source_path == source_path)
        .collect::<Vec<_>>();
    let [artifact] = matches.as_slice() else {
        return Err(MigrationError::Invalid("v4 object binding missing"));
    };
    Ok(artifact)
}

/// Reconstructs an already-imported v4 recording from the old corpus object store and
/// converts it to v5. Both the old store and an optional read-only mirror of remote segment
/// objects are immutable inputs. A missing remote object is reported by digest.
///
/// # Errors
/// Rejects a session digest mismatch, missing or changed objects, or an invalid v4 recording.
pub fn migrate_imported_session(
    old_store: &Path,
    session_sha256: &str,
    object_mirror: Option<&Path>,
    destination: &Path,
) -> Result<(), MigrationError> {
    migrate_imported_with_kind(
        old_store,
        session_sha256,
        object_mirror,
        destination,
        LegacyKind::V4,
    )
}

/// Reconstructs the v2 canonical recording embedded in an imported v4 capture session.
/// All v2 decoding and schema adaptation stays in this removable corpus migration module.
///
/// # Errors
/// Rejects a missing or changed old object, incomplete v2 recording, or invalid conversion.
pub fn migrate_imported_v2_session(
    old_store: &Path,
    session_sha256: &str,
    object_mirror: Option<&Path>,
    destination: &Path,
) -> Result<(), MigrationError> {
    migrate_imported_with_kind(
        old_store,
        session_sha256,
        object_mirror,
        destination,
        LegacyKind::V2,
    )
}

#[derive(Clone, Copy)]
enum LegacyKind {
    V2,
    V4,
}

impl LegacyKind {
    const fn manifest_schema(self) -> &'static str {
        match self {
            Self::V2 => V2_RECORDING,
            Self::V4 => V4_RECORDING,
        }
    }

    const fn manifest_artifact_kind(self) -> &'static str {
        match self {
            Self::V2 => "recognition",
            Self::V4 => "canonical_manifest",
        }
    }

    const fn tick_artifact_kind(self) -> &'static str {
        match self {
            Self::V2 => "recognition",
            Self::V4 => "canonical_tick_index",
        }
    }

    const fn segment_artifact_kind(self) -> &'static str {
        match self {
            Self::V2 => "recognition",
            Self::V4 => "canonical_video_segment",
        }
    }
}

fn migrate_imported_with_kind(
    old_store: &Path,
    session_sha256: &str,
    object_mirror: Option<&Path>,
    destination: &Path,
    kind: LegacyKind,
) -> Result<(), MigrationError> {
    if !is_sha256(session_sha256) || destination.exists() {
        return Err(MigrationError::Invalid(
            "legacy import migration request is invalid",
        ));
    }
    let session_path = old_store
        .join("sessions")
        .join(format!("{session_sha256}.json"));
    let session_bytes = read_document(&session_path)?;
    if digest_file(&session_path)?.0 != session_sha256 {
        return Err(MigrationError::Invalid("v4 session digest differs"));
    }
    let session: V4Session = serde_json::from_slice(&session_bytes)?;
    if session.schema != V4_SESSION || session.artifacts.len() > MAX_SEGMENTS + 16 {
        return Err(MigrationError::Invalid("v4 imported session is invalid"));
    }
    let staging = tempfile::tempdir()?;
    let recording = staging.path().join("recording");
    fs::create_dir(&recording)?;
    for (kind, source_path, filename) in [
        (
            kind.manifest_artifact_kind(),
            "recognition/canonical-manifest.json",
            "canonical-manifest.json",
        ),
        (
            kind.tick_artifact_kind(),
            "recognition/canonical-ticks.ndjson",
            "canonical-ticks.ndjson",
        ),
    ] {
        materialize_object(
            old_store,
            object_mirror,
            one_artifact(&session, kind, source_path)?,
            &recording.join(filename),
        )?;
    }
    let manifest: V4Manifest =
        serde_json::from_slice(&read_document(&recording.join("canonical-manifest.json"))?)?;
    if manifest.schema != kind.manifest_schema() || manifest.segments.len() > MAX_SEGMENTS {
        return Err(MigrationError::Invalid("v4 imported manifest is invalid"));
    }
    for (index, segment) in manifest.segments.iter().enumerate() {
        if segment.path != format!("segment-{index:04}.mkv") {
            return Err(MigrationError::Invalid("v4 segment path is invalid"));
        }
        materialize_object(
            old_store,
            object_mirror,
            one_artifact(
                &session,
                kind.segment_artifact_kind(),
                &format!("recognition/{}", segment.path),
            )?,
            &recording.join(&segment.path),
        )?;
    }
    migrate_recording(&recording, &session_path, destination)
}

fn verify_v4_artifact(
    session: &V4Session,
    kind: &str,
    source_path: &str,
    source: &Path,
) -> Result<String, MigrationError> {
    let matches = session
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == kind && artifact.source_path == source_path)
        .collect::<Vec<_>>();
    let [artifact] = matches.as_slice() else {
        return Err(MigrationError::Invalid(
            "v4 canonical artifact binding missing",
        ));
    };
    let (actual_sha256, actual_bytes) = digest_file(source)?;
    if artifact.sha256 != actual_sha256 || artifact.bytes != actual_bytes {
        return Err(MigrationError::Invalid(
            "v4 canonical artifact digest differs",
        ));
    }
    Ok(actual_sha256)
}

fn valid_version(value: &Value) -> bool {
    let Some(status) = value.get("status").and_then(Value::as_str) else {
        return false;
    };
    match status {
        "not_observed" | "ambiguous" | "observer_failed" => value.get("version").is_none(),
        "identified" => value
            .get("version")
            .and_then(Value::as_str)
            .is_some_and(|version| {
                let bytes = version.as_bytes();
                bytes.len() == 20
                    && bytes.iter().enumerate().all(|(index, byte)| {
                        if [3, 5, 7, 9].contains(&index) {
                            *byte == b':'
                        } else {
                            byte.is_ascii_alphanumeric()
                        }
                    })
            }),
        _ => false,
    }
}

fn converted_disposition(value: &str) -> Result<Value, MigrationError> {
    match value {
        "retained" => Ok(json!({"kind":"retained"})),
        "title"
        | "play_interior"
        | "mode_select_interior"
        | "unknown_interior"
        | "recording_failure" => Ok(json!({"kind":"elided","reason":value})),
        _ => Err(MigrationError::Invalid("unknown v4 tick disposition")),
    }
}

struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Converts a complete v4 recording and its capture-session document into a standalone v5
/// recording. The caller chooses a new destination; no file in either source is modified.
///
/// # Errors
/// Returns a typed I/O, JSON, or v4 contract error. The destination is never published on error.
#[allow(
    clippy::too_many_lines,
    reason = "the removable v4 conversion is kept in one file"
)]
pub fn migrate_recording(
    source_recording: &Path,
    source_session: &Path,
    destination: &Path,
) -> Result<(), MigrationError> {
    if destination.exists() {
        return Err(MigrationError::Invalid(
            "migration destination already exists",
        ));
    }
    let manifest: V4Manifest = serde_json::from_slice(&read_document(
        &source_recording.join("canonical-manifest.json"),
    )?)?;
    let session: V4Session = serde_json::from_slice(&read_document(source_session)?)?;
    let v2 = manifest.schema == V2_RECORDING;
    if (!v2 && manifest.schema != V4_RECORDING)
        || session.schema != V4_SESSION
        || session.completeness != "complete"
        || (v2 && manifest.game_version.is_some())
        || (!v2 && manifest.game_version.as_ref() != Some(&session.game_version))
        || manifest.completeness != "complete"
        || !manifest.completeness_reasons.is_empty()
        || manifest.tick_count == 0
        || manifest.tick_count > MAX_TICKS
        || manifest.segments.is_empty()
        || manifest.segments.len() > MAX_SEGMENTS
        || !valid_version(&session.game_version)
        || session.source_session_id.is_empty()
        || session.source_session_id.len() > 128
        || !session
            .source_session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(MigrationError::Invalid(
            "v4 recording or session contract is invalid",
        ));
    }
    let manifest_kind = if v2 {
        "recognition"
    } else {
        "canonical_manifest"
    };
    let tick_kind = if v2 {
        "recognition"
    } else {
        "canonical_tick_index"
    };
    let segment_kind = if v2 {
        "recognition"
    } else {
        "canonical_video_segment"
    };
    verify_v4_artifact(
        &session,
        manifest_kind,
        "recognition/canonical-manifest.json",
        &source_recording.join("canonical-manifest.json"),
    )?;
    let verified_tick_sha256 = verify_v4_artifact(
        &session,
        tick_kind,
        "recognition/canonical-ticks.ndjson",
        &source_recording.join("canonical-ticks.ndjson"),
    )?;
    if v2 && manifest.tick_index_sha256.as_deref() != Some(&verified_tick_sha256) {
        return Err(MigrationError::Invalid("v2 tick index digest differs"));
    }
    let parent = destination
        .parent()
        .ok_or(MigrationError::Invalid("destination has no parent"))?;
    let staging = tempfile::Builder::new()
        .prefix(".scorepeek-legacy-migration-")
        .tempdir_in(parent)?;
    let staging_path = staging.keep();
    let staging = Staging(staging_path);
    let source_ticks = source_recording.join("canonical-ticks.ndjson");
    let tick_file = File::open(&source_ticks)?;
    let mut tick_writer = File::create(staging.0.join("canonical-ticks.ndjson"))?;
    let mut retained = Vec::new();
    let mut last_sequence = None;
    let mut count = 0_usize;
    for line in BufReader::new(tick_file).lines() {
        if count >= MAX_TICKS {
            return Err(MigrationError::Invalid("v4 tick count exceeds bound"));
        }
        let line = line?;
        let tick: V4Tick = serde_json::from_str(&line)?;
        if last_sequence.is_some_and(|last| tick.sequence <= last) {
            return Err(MigrationError::Invalid(
                "v4 tick sequence is not increasing",
            ));
        }
        let disposition = converted_disposition(&tick.disposition)?;
        if tick.disposition == "retained" {
            retained.push(tick.sequence);
        }
        let converted = json!({
            "sequence":tick.sequence,
            "source_sequence":tick.source_sequence,
            "source_timestamp_ms":tick.monotonic_ms,
            "screen":tick.screen,
            "semantic_episode_id":tick.semantic_episode_id,
            "disposition":disposition,
        });
        serde_json::to_writer(&mut tick_writer, &converted)?;
        tick_writer.write_all(b"\n")?;
        last_sequence = Some(tick.sequence);
        count += 1;
    }
    tick_writer.sync_all()?;
    if digest_file(&source_ticks)?.0 != verified_tick_sha256 {
        return Err(MigrationError::Invalid(
            "v4 tick index changed while converting",
        ));
    }
    if count != manifest.tick_count {
        return Err(MigrationError::Invalid("v4 tick count differs"));
    }
    let mut cursor = 0_usize;
    let mut segments = Vec::new();
    for (index, segment) in manifest.segments.iter().enumerate() {
        if segment.path != format!("segment-{index:04}.mkv")
            || segment.frames == 0
            || segment.frames > 600
        {
            return Err(MigrationError::Invalid(
                "v4 segment path or frame count is invalid",
            ));
        }
        let end = cursor
            .checked_add(segment.frames)
            .ok_or(MigrationError::Invalid("v4 frame count overflow"))?;
        let range = retained.get(cursor..end).ok_or(MigrationError::Invalid(
            "v4 retained frame coverage differs",
        ))?;
        if range.first() != Some(&segment.first_sequence)
            || range.last() != Some(&segment.last_sequence)
        {
            return Err(MigrationError::Invalid("v4 segment sequence differs"));
        }
        let source = source_recording.join(&segment.path);
        let (sha256, bytes) = digest_file(&source)?;
        if bytes != segment.bytes || (v2 && segment.encoded_sha256.as_deref() != Some(&sha256)) {
            return Err(MigrationError::Invalid("v4 segment byte count differs"));
        }
        let declared = session
            .artifacts
            .iter()
            .filter(|artifact| {
                artifact.kind == segment_kind
                    && artifact.source_path == format!("recognition/{}", segment.path)
            })
            .collect::<Vec<_>>();
        if !matches!(declared.as_slice(), [artifact] if artifact.sha256 == sha256 && artifact.bytes == bytes)
        {
            return Err(MigrationError::Invalid(
                "v4 segment artifact binding differs",
            ));
        }
        let Some(review_frames) = session.canonical_frames.get(cursor..end) else {
            return Err(MigrationError::Invalid("v4 review frame coverage differs"));
        };
        if review_frames
            .iter()
            .zip(range)
            .any(|(frame, sequence)| frame.sequence != *sequence || frame.artifact_sha256 != sha256)
        {
            return Err(MigrationError::Invalid("v4 review frame binding differs"));
        }
        fs::copy(&source, staging.0.join(&segment.path))?;
        let (copied_digest, copied_bytes) = digest_file(&staging.0.join(&segment.path))?;
        if copied_digest != sha256 || copied_bytes != bytes {
            return Err(MigrationError::Invalid("v4 segment changed while copying"));
        }
        segments.push(json!({"path":segment.path,"first_sequence":segment.first_sequence,
            "last_sequence":segment.last_sequence,"frames":segment.frames,"bytes":bytes,"sha256":sha256}));
        cursor = end;
    }
    if cursor != retained.len() {
        return Err(MigrationError::Invalid(
            "v4 retained frame coverage differs",
        ));
    }
    if session.canonical_frames.len() != retained.len() {
        return Err(MigrationError::Invalid("v4 review frame count differs"));
    }
    let (tick_sha256, tick_bytes) = digest_file(&staging.0.join("canonical-ticks.ndjson"))?;
    let converted = json!({
        "schema": V5_RECORDING,
        "frame_contract": FRAME_CONTRACT,
        "session_id": session.source_session_id,
        "shape":{"width":1920,"height":1080,"pixel_format":"rgb8"},
        "tick_index":{"path":"canonical-ticks.ndjson","sha256":tick_sha256,"bytes":tick_bytes,"count":count},
        "tick_count":count,
        "segments":segments,
        "completeness":"complete",
        "completeness_reasons":[],
        "game_version":session.game_version,
    });
    let mut output = File::create(staging.0.join("canonical-manifest.json"))?;
    serde_json::to_writer_pretty(&mut output, &converted)?;
    output.write_all(b"\n")?;
    output.sync_all()?;
    crate::canonical::read_complete(&staging.0).map_err(MigrationError::Recording)?;
    fs::rename(&staging.0, destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reviewed_label_migration_requires_active_content_binding_and_keeps_source_read_only() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path();
        let new_recording = old.join("new-recording");
        fs::create_dir(&new_recording).unwrap();
        let segment_sha = "b".repeat(64);
        let tick_index = serde_json::to_vec(&json!({
            "sequence":1,"source_sequence":1,"source_timestamp_ms":100,
            "screen":"result","semantic_episode_id":1,
            "disposition":{"kind":"retained"}
        }))
        .unwrap();
        let tick_index = [tick_index, b"\n".to_vec()].concat();
        fs::write(new_recording.join("canonical-ticks.ndjson"), &tick_index).unwrap();
        fs::write(
            new_recording.join("canonical-manifest.json"),
            serde_json::to_vec(&json!({
                "schema":V5_RECORDING, "frame_contract":FRAME_CONTRACT,
                "session_id":"synthetic-session",
                "shape":{"width":1920,"height":1080,"pixel_format":"rgb8"},
                "tick_index":{"path":"canonical-ticks.ndjson",
                    "sha256":crate::resources::sha256(&tick_index),"bytes":tick_index.len(),"count":1},
                "tick_count":1,
                "segments":[{"path":"segment-0000.mkv","first_sequence":1,"last_sequence":1,
                    "frames":1,"bytes":1,"sha256":segment_sha}],
                "completeness":"complete","completeness_reasons":[],
                "game_version":{"status":"not_observed"}
            })).unwrap(),
        ).unwrap();
        for directory in ["sessions", "labels", "suites"] {
            fs::create_dir(old.join(directory)).unwrap();
        }
        let session = serde_json::to_vec(&json!({
            "schema":V4_SESSION, "source_session_id":"synthetic-session",
            "completeness":"complete", "game_version":{"status":"not_observed"},
            "canonical_frames":[{"sequence":1,"artifact_sha256":segment_sha}], "artifacts":[]
        }))
        .unwrap();
        let session_sha = crate::resources::sha256(&session);
        fs::write(
            old.join("sessions").join(format!("{session_sha}.json")),
            &session,
        )
        .unwrap();
        let label = serde_json::to_vec(&json!({
            "schema":V6_LABEL, "session_sha256":session_sha,
            "disposition":"include", "episodes":[], "negative_frames":[]
        }))
        .unwrap();
        let label_sha = crate::resources::sha256(&label);
        fs::write(old.join("labels").join(format!("{label_sha}.json")), &label).unwrap();
        let suite = serde_json::to_vec(&json!({
            "schema":"scorepeek-private-regression-suite-v1",
            "previous_generation_sha256":null,
            "entries":[{"session_sha256":session_sha,"label_sha256":label_sha}]
        }))
        .unwrap();
        let suite_sha = crate::resources::sha256(&suite);
        fs::write(old.join("suites").join(format!("{suite_sha}.json")), &suite).unwrap();
        fs::write(
            old.join("active-suite.json"),
            serde_json::to_vec(&json!({
                "schema":"scorepeek-private-regression-suite-active-v1",
                "generation_sha256":suite_sha
            }))
            .unwrap(),
        )
        .unwrap();
        let new_sha = "a".repeat(64);
        let migrated = migrate_reviewed_label(old, &session_sha, &new_sha, &new_recording).unwrap();
        assert_eq!(migrated.session_sha256, new_sha);
        assert_eq!(migrated.schema, V2_LABEL);
        assert!(migrated.episodes.is_empty());
        let manifest_path = new_recording.join("canonical-manifest.json");
        let mut wrong: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        let original_manifest = wrong.clone();
        wrong["segments"][0]["sha256"] = json!("d".repeat(64));
        fs::write(&manifest_path, serde_json::to_vec(&wrong).unwrap()).unwrap();
        assert!(migrate_reviewed_label(old, &session_sha, &new_sha, &new_recording).is_err());
        let changed_index =
            String::from_utf8(tick_index)
                .unwrap()
                .replacen("\"sequence\":1", "\"sequence\":2", 1);
        fs::write(new_recording.join("canonical-ticks.ndjson"), &changed_index).unwrap();
        let mut wrong = original_manifest;
        wrong["tick_index"]["sha256"] = json!(crate::resources::sha256(changed_index.as_bytes()));
        wrong["tick_index"]["bytes"] = json!(changed_index.len());
        fs::write(&manifest_path, serde_json::to_vec(&wrong).unwrap()).unwrap();
        assert!(migrate_reviewed_label(old, &session_sha, &new_sha, &new_recording).is_err());
        assert_eq!(
            fs::read(old.join("labels").join(format!("{label_sha}.json"))).unwrap(),
            label
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "exercises source and imported-store v4 migration with one fixture"
    )]
    fn converts_v4_without_writing_into_either_source() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("old-recording");
        fs::create_dir(&source).unwrap();
        let session = root.path().join("old-session.json");
        let destination = root.path().join("new-recording");
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=1920x1080:r=1",
                "-frames:v",
                "1",
                "-c:v",
                "ffv1",
                "-pix_fmt",
                "bgr0",
            ])
            .arg(source.join("segment-0000.mkv"))
            .status()
            .unwrap();
        assert!(status.success());
        let segment_bytes = fs::metadata(source.join("segment-0000.mkv")).unwrap().len();
        fs::write(source.join("canonical-ticks.ndjson"), b"{\"sequence\":1,\"source_sequence\":7,\"monotonic_ms\":90,\"screen\":\"result\",\"semantic_episode_id\":null,\"disposition\":\"retained\"}\n").unwrap();
        let old_manifest = json!({
            "schema":V4_RECORDING,"completeness":"complete","tick_count":1,
            "segments":[{"path":"segment-0000.mkv","first_sequence":1,"last_sequence":1,
                "frames":1,"bytes":segment_bytes}],"completeness_reasons":[],
            "game_version":{"status":"identified","version":"P2D:J:B:A:2026080500"}
        });
        fs::write(
            source.join("canonical-manifest.json"),
            serde_json::to_vec(&old_manifest).unwrap(),
        )
        .unwrap();
        let artifact = |kind: &str, source_path: &str, path: &Path| {
            let (sha256, bytes) = digest_file(path).unwrap();
            json!({"kind":kind,"source_path":source_path,"sha256":sha256,"bytes":bytes})
        };
        let segment_artifact = artifact(
            "canonical_video_segment",
            "recognition/segment-0000.mkv",
            &source.join("segment-0000.mkv"),
        );
        let session_document = json!({
            "schema":V4_SESSION,"source_session_id":"session-1","completeness":"complete",
            "game_version":{"status":"identified","version":"P2D:J:B:A:2026080500"},
            "canonical_frames":[{"sequence":1,"artifact_sha256":segment_artifact["sha256"]}],
            "artifacts":[
                artifact("canonical_manifest","recognition/canonical-manifest.json",&source.join("canonical-manifest.json")),
                artifact("canonical_tick_index","recognition/canonical-ticks.ndjson",&source.join("canonical-ticks.ndjson")),
                segment_artifact,
            ],
        });
        fs::write(&session, serde_json::to_vec(&session_document).unwrap()).unwrap();
        let old_manifest_bytes = fs::read(source.join("canonical-manifest.json")).unwrap();
        let old_session_bytes = fs::read(&session).unwrap();
        migrate_recording(&source, &session, &destination).unwrap();
        let converted: Value =
            serde_json::from_slice(&fs::read(destination.join("canonical-manifest.json")).unwrap())
                .unwrap();
        assert_eq!(converted["schema"], V5_RECORDING);
        assert_eq!(converted["game_version"]["status"], "identified");
        assert_eq!(converted["game_version"]["version"], "P2D:J:B:A:2026080500");
        assert_eq!(
            converted["segments"][0]["sha256"].as_str().unwrap().len(),
            64
        );
        assert_eq!(
            fs::read(source.join("canonical-manifest.json")).unwrap(),
            old_manifest_bytes
        );
        assert_eq!(fs::read(&session).unwrap(), old_session_bytes);
        let old_store = root.path().join("old-store");
        fs::create_dir_all(old_store.join("sessions")).unwrap();
        fs::create_dir(old_store.join("objects")).unwrap();
        let session_sha256 = digest_file(&session).unwrap().0;
        fs::copy(
            &session,
            old_store
                .join("sessions")
                .join(format!("{session_sha256}.json")),
        )
        .unwrap();
        for artifact in session_document["artifacts"].as_array().unwrap() {
            let path = artifact["source_path"].as_str().unwrap();
            let source_file = source.join(path.strip_prefix("recognition/").unwrap());
            fs::copy(
                source_file,
                old_store
                    .join("objects")
                    .join(artifact["sha256"].as_str().unwrap()),
            )
            .unwrap();
        }
        let restored = root.path().join("restored-recording");
        migrate_imported_session(&old_store, &session_sha256, None, &restored).unwrap();
        crate::canonical::read_complete(&restored).unwrap();
        assert_eq!(fs::read(&session).unwrap(), old_session_bytes);
        let segment_digest = session_document["artifacts"][2]["sha256"].as_str().unwrap();
        let mirror = root.path().join("read-only-object-mirror");
        fs::create_dir(&mirror).unwrap();
        fs::rename(
            old_store.join("objects").join(segment_digest),
            mirror.join(segment_digest),
        )
        .unwrap();
        let missing = root.path().join("missing-recording");
        assert!(matches!(
            migrate_imported_session(&old_store, &session_sha256, None, &missing),
            Err(MigrationError::MissingObject(digest)) if digest == segment_digest
        ));
        assert!(!missing.exists());
        let mirrored = root.path().join("mirrored-recording");
        migrate_imported_session(&old_store, &session_sha256, Some(&mirror), &mirrored).unwrap();
        crate::canonical::read_complete(&mirrored).unwrap();
        let mismatched = root.path().join("mismatched-session.json");
        let mut wrong_session = session_document;
        wrong_session["artifacts"][0]["sha256"] = Value::String("0".repeat(64));
        fs::write(&mismatched, serde_json::to_vec(&wrong_session).unwrap()).unwrap();
        let rejected = root.path().join("rejected-recording");
        assert!(migrate_recording(&source, &mismatched, &rejected).is_err());
        assert!(!rejected.exists());
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one synthetic v2 store exercises the removable conversion and immutable source"
    )]
    fn converts_v2_recording_bound_to_a_v4_session() {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("old-store");
        fs::create_dir_all(store.join("sessions")).unwrap();
        fs::create_dir(store.join("objects")).unwrap();
        let video = root.path().join("segment-0000.mkv");
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=1920x1080:r=1",
                "-frames:v",
                "1",
                "-c:v",
                "libx264rgb",
                "-crf",
                "0",
                "-pix_fmt",
                "bgr0",
            ])
            .arg(&video)
            .status()
            .unwrap();
        assert!(status.success());
        let ticks = root.path().join("canonical-ticks.ndjson");
        fs::write(&ticks, b"{\"sequence\":1,\"source_sequence\":7,\"monotonic_ms\":90,\"screen\":\"unknown\",\"semantic_episode_id\":null,\"disposition\":\"retained\"}\n").unwrap();
        let (video_sha, video_bytes) = digest_file(&video).unwrap();
        let (tick_sha, tick_bytes) = digest_file(&ticks).unwrap();
        let manifest = root.path().join("canonical-manifest.json");
        fs::write(
            &manifest,
            serde_json::to_vec(&json!({
                "schema":V2_RECORDING,"completeness":"complete","tick_count":1,
                "tick_index_sha256":tick_sha,"completeness_reasons":[],
                "segments":[{"path":"segment-0000.mkv","first_sequence":1,
                    "last_sequence":1,"frames":1,"bytes":video_bytes,
                    "encoded_sha256":video_sha,"raw_rgb24_sha256":"0".repeat(64)}],
            }))
            .unwrap(),
        )
        .unwrap();
        let artifact = |source_path: &str, path: &Path| {
            let (sha256, bytes) = digest_file(path).unwrap();
            json!({"kind":"recognition","source_path":source_path,"sha256":sha256,"bytes":bytes})
        };
        let session = root.path().join("session.json");
        fs::write(
            &session,
            serde_json::to_vec(&json!({
                "schema":V4_SESSION,"source_session_id":"session-1","completeness":"complete",
                "game_version":{"status":"not_observed"},
                "canonical_frames":[{"sequence":1,"artifact_sha256":video_sha}],
                "artifacts":[
                    artifact("recognition/canonical-manifest.json",&manifest),
                    artifact("recognition/canonical-ticks.ndjson",&ticks),
                    artifact("recognition/segment-0000.mkv",&video),
                ],
            }))
            .unwrap(),
        )
        .unwrap();
        let session_sha = digest_file(&session).unwrap().0;
        let original_session = fs::read(&session).unwrap();
        fs::copy(
            &session,
            store.join("sessions").join(format!("{session_sha}.json")),
        )
        .unwrap();
        for path in [&manifest, &ticks, &video] {
            let digest = digest_file(path).unwrap().0;
            fs::copy(path, store.join("objects").join(digest)).unwrap();
        }
        let destination = root.path().join("converted");
        migrate_imported_v2_session(&store, &session_sha, None, &destination).unwrap();
        let converted = crate::canonical::read_complete(&destination).unwrap();
        assert_eq!(
            serde_json::to_value(&converted.manifest.game_version).unwrap(),
            json!({"status":"not_observed"})
        );
        assert_eq!(converted.manifest.tick_count, 1);
        assert_eq!(fs::read(&session).unwrap(), original_session);
        assert_eq!(tick_bytes, fs::metadata(&ticks).unwrap().len());
    }

    #[test]
    fn rejects_incomplete_v4_before_publication() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("old");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("canonical-manifest.json"), b"{\"schema\":\"scorepeek-canonical-session-recording-v4\",\"completeness\":\"partial\",\"tick_count\":1,\"segments\":[],\"completeness_reasons\":[],\"game_version\":{\"status\":\"not_observed\"}}").unwrap();
        let session = root.path().join("session.json");
        fs::write(&session, b"{\"schema\":\"scorepeek-private-capture-session-v4\",\"source_session_id\":\"session-1\",\"completeness\":\"complete\",\"game_version\":{\"status\":\"not_observed\"},\"canonical_frames\":[],\"artifacts\":[]}").unwrap();
        let target = root.path().join("target");
        assert!(migrate_recording(&source, &session, &target).is_err());
        assert!(!target.exists());
    }
}
