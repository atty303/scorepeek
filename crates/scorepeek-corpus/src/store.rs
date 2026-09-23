//! Local immutable corpus publication and two-stage human review.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::canonical::{self, RecordingError};

const SESSION_SCHEMA: &str = "scorepeek-private-canonical-session-v1";
const DRAFT_SCHEMA: &str = "scorepeek-private-canonical-review-draft-v1";
const LABEL_SCHEMA: &str = "scorepeek-private-canonical-regression-label-v2";
const GENERATION_SCHEMA: &str = "scorepeek-private-canonical-generation-v1";
const MAX_DOCUMENT: u64 = 16 * 1024 * 1024;

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Recording(RecordingError),
    Invalid(&'static str),
}

impl From<std::io::Error> for StoreError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<serde_json::Error> for StoreError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}
impl From<RecordingError> for StoreError {
    fn from(value: RecordingError) -> Self {
        Self::Recording(value)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusSession {
    pub schema: String,
    pub recording_sha256: String,
    pub session_id: String,
    pub tick_count: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewDraft {
    pub schema: String,
    pub session_sha256: String,
    pub session_id: String,
    pub input_sequences: Vec<u64>,
    pub retained_sequences: Vec<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelDisposition {
    Include,
    Exclude,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedTransition {
    pub sequence: u64,
    pub screen: scorepeek_core::recognition::screen::ScreenClass,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegressionLabel {
    pub schema: String,
    pub session_sha256: String,
    pub disposition: LabelDisposition,
    pub episodes: Vec<crate::oracle::RegressionEpisode>,
    pub negative_frames: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transitions: Option<Vec<ExpectedTransition>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Generation {
    pub schema: String,
    pub sessions: Vec<String>,
}

pub struct ImportSummary {
    pub session_sha256: String,
    pub draft: PathBuf,
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut hex, "{byte:02x}").expect("String write");
    }
    hex
}

fn read_document(path: &Path) -> Result<Vec<u8>, StoreError> {
    let metadata = path.symlink_metadata()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_DOCUMENT {
        return Err(StoreError::Invalid(
            "corpus document is not a bounded regular file",
        ));
    }
    Ok(fs::read(path)?)
}

pub(crate) fn verify_session_descriptor(
    session_dir: &Path,
    session_sha256: &str,
) -> Result<CorpusSession, StoreError> {
    let document = read_document(&session_dir.join(format!("{session_sha256}.json")))?;
    if sha256(&document) != session_sha256 {
        return Err(StoreError::Invalid(
            "corpus session descriptor digest differs",
        ));
    }
    let session: CorpusSession = serde_json::from_slice(&document)?;
    if session.schema != SESSION_SCHEMA
        || session.recording_sha256
            != sha256(&read_document(
                &session_dir.join("canonical-manifest.json"),
            )?)
    {
        return Err(StoreError::Invalid("corpus recording binding differs"));
    }
    Ok(session)
}

fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, StoreError> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn sync_dir(path: &Path) -> Result<(), StoreError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .ok_or(StoreError::Invalid("corpus path has no parent"))?;
    let staged = tempfile::Builder::new()
        .prefix(".scorepeek-document-")
        .tempfile_in(parent)?;
    let (mut file, temporary) = staged.keep().map_err(|error| StoreError::Io(error.error))?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        sync_dir(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

struct WriterLock(File);
impl WriterLock {
    fn acquire(root: &Path) -> Result<Self, StoreError> {
        fs::create_dir_all(root)?;
        let lock = root.join(".writer.lock");
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock)?;
        file.try_lock()
            .map_err(|_| StoreError::Invalid("corpus writer lock is held"))?;
        sync_dir(root)?;
        Ok(Self(file))
    }
}
impl Drop for WriterLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn load_generation(path: &Path) -> Result<Generation, StoreError> {
    if !path.exists() {
        return Ok(Generation {
            schema: GENERATION_SCHEMA.into(),
            sessions: Vec::new(),
        });
    }
    let value: Generation = serde_json::from_slice(&read_document(path)?)?;
    if value.schema != GENERATION_SCHEMA
        || value.sessions.windows(2).any(|pair| pair[0] >= pair[1])
        || value.sessions.iter().any(|digest| {
            digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return Err(StoreError::Invalid("corpus generation is invalid"));
    }
    Ok(value)
}

fn publish_generation(path: &Path, session: &str) -> Result<(), StoreError> {
    let mut generation = load_generation(path)?;
    match generation
        .sessions
        .binary_search_by(|candidate| candidate.as_str().cmp(session))
    {
        Ok(_) => return Ok(()),
        Err(index) => generation.sessions.insert(index, session.to_owned()),
    }
    atomic_write(path, &canonical_json(&generation)?)
}

fn copy_regular(source: &Path, destination: &Path) -> Result<(), StoreError> {
    if !source.symlink_metadata()?.file_type().is_file() {
        return Err(StoreError::Invalid(
            "canonical source changed into a non-regular file",
        ));
    }
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
    }
    output.sync_all()?;
    Ok(())
}

/// Imports one complete recording directory without writing to the source. Import publishes an
/// immutable session and a review draft; it cannot add the session to active replay.
///
/// # Errors
/// Returns before imported-generation publication if the source changes or validation fails.
pub fn import_recording(store: &Path, source: &Path) -> Result<ImportSummary, StoreError> {
    let verified = canonical::read_complete(source)?;
    let _lock = WriterLock::acquire(store)?;
    let sessions = store.join("sessions");
    fs::create_dir_all(&sessions)?;
    sync_dir(store)?;
    let staging = tempfile::Builder::new()
        .prefix(".scorepeek-import-")
        .tempdir_in(&sessions)?;
    let staging_root = staging.path();
    copy_regular(
        &source.join("canonical-manifest.json"),
        &staging_root.join("canonical-manifest.json"),
    )?;
    copy_regular(
        &source.join(&verified.manifest.tick_index.path),
        &staging_root.join(&verified.manifest.tick_index.path),
    )?;
    for segment in &verified.manifest.segments {
        copy_regular(
            &source.join(&segment.path),
            &staging_root.join(&segment.path),
        )?;
    }
    let copied = canonical::read_complete(staging_root)?;
    if copied.manifest != verified.manifest || copied.ticks != verified.ticks {
        return Err(StoreError::Invalid(
            "canonical source changed during import",
        ));
    }
    let recording_sha256 = sha256(&read_document(
        &staging_root.join("canonical-manifest.json"),
    )?);
    let session = CorpusSession {
        schema: SESSION_SCHEMA.into(),
        recording_sha256,
        session_id: copied.manifest.session_id.clone(),
        tick_count: copied.manifest.tick_count,
    };
    let session_bytes = canonical_json(&session)?;
    let session_sha256 = sha256(&session_bytes);
    let draft = ReviewDraft {
        schema: DRAFT_SCHEMA.into(),
        session_sha256: session_sha256.clone(),
        session_id: session.session_id.clone(),
        input_sequences: copied.ticks.iter().map(|tick| tick.sequence).collect(),
        retained_sequences: copied
            .ticks
            .iter()
            .filter_map(|tick| {
                (tick.disposition == scorepeek_core::canonical_recording::TickDisposition::Retained)
                    .then_some(tick.sequence)
            })
            .collect(),
    };
    let draft_bytes = canonical_json(&draft)?;
    let draft_name = format!("{session_sha256}.review.json");
    let session_name = format!("{session_sha256}.json");
    fs::write(staging_root.join(&session_name), &session_bytes)?;
    File::open(staging_root.join(&session_name))?.sync_all()?;
    fs::write(staging_root.join(&draft_name), &draft_bytes)?;
    File::open(staging_root.join(&draft_name))?.sync_all()?;
    sync_dir(staging_root)?;
    let destination = sessions.join(&session_sha256);
    if destination.exists() {
        let existing = canonical::read_complete(&destination)?;
        if existing.manifest != copied.manifest || existing.ticks != copied.ticks {
            return Err(StoreError::Invalid(
                "session digest collides with different recording",
            ));
        }
    } else {
        fs::rename(staging.path(), &destination)?;
        sync_dir(&sessions)?;
    }
    publish_generation(&store.join("imported.json"), &session_sha256)?;
    Ok(ImportSummary {
        session_sha256,
        draft: destination.join(draft_name),
    })
}

/// Applies human labels for one imported session and activates it for regression replay.
///
/// # Errors
/// Rejects a draft or label mismatch without changing the active suite.
pub fn review_apply(store: &Path, draft_path: &Path, labels_path: &Path) -> Result<(), StoreError> {
    let label: RegressionLabel = serde_json::from_slice(&read_document(labels_path)?)?;
    review_apply_label(store, draft_path, &label)
}

/// Publishes an already reviewed label, including one converted by the removable legacy reader.
///
/// # Errors
/// Rejects an invalid reviewed oracle or changed session without publishing an active generation.
pub fn review_apply_label(
    store: &Path,
    draft_path: &Path,
    label: &RegressionLabel,
) -> Result<(), StoreError> {
    let _lock = WriterLock::acquire(store)?;
    let draft: ReviewDraft = serde_json::from_slice(&read_document(draft_path)?)?;
    if draft.schema != DRAFT_SCHEMA
        || label.schema != LABEL_SCHEMA
        || label.session_sha256 != draft.session_sha256
        || !load_generation(&store.join("imported.json"))?
            .sessions
            .contains(&draft.session_sha256)
        || label.transitions.as_ref().is_some_and(|transitions| {
            transitions
                .windows(2)
                .any(|pair| pair[0].sequence >= pair[1].sequence)
                || transitions
                    .iter()
                    .any(|item| !draft.input_sequences.contains(&item.sequence))
        })
    {
        return Err(StoreError::Invalid(
            "review label does not match imported draft",
        ));
    }
    let session_dir = store.join("sessions").join(&draft.session_sha256);
    let stored_draft =
        read_document(&session_dir.join(format!("{}.review.json", draft.session_sha256)))?;
    if stored_draft != read_document(draft_path)? {
        return Err(StoreError::Invalid(
            "review draft differs from imported session",
        ));
    }
    let session = verify_session_descriptor(&session_dir, &draft.session_sha256)?;
    if session.schema != SESSION_SCHEMA || session.session_id != draft.session_id {
        return Err(StoreError::Invalid("review session binding differs"));
    }
    let recording = canonical::read_complete(&session_dir)?;
    if recording.manifest.session_id != draft.session_id
        || recording.manifest.tick_count != session.tick_count
    {
        return Err(StoreError::Invalid("review recording binding differs"));
    }
    crate::oracle::validate_label(&label.episodes, &label.negative_frames, &recording.ticks)
        .map_err(StoreError::Invalid)?;
    if label.disposition == LabelDisposition::Include
        && label.episodes.is_empty()
        && recording
            .ticks
            .iter()
            .any(|tick| tick.screen == scorepeek_core::recognition::screen::ScreenClass::Result)
    {
        return Err(StoreError::Invalid(
            "included RESULT recording has no reviewed episodes",
        ));
    }
    let label_bytes = canonical_json(&label)?;
    let label_path = session_dir.join("label.json");
    if label_path.exists() {
        if read_document(&label_path)? != label_bytes {
            return Err(StoreError::Invalid("review label is already fixed"));
        }
    } else {
        atomic_write(&label_path, &label_bytes)?;
    }
    if label.disposition == LabelDisposition::Include {
        publish_generation(&store.join("active.json"), &draft.session_sha256)?;
    }
    Ok(())
}

/// Reads the reviewed active session IDs for explicit private replay.
///
/// # Errors
/// Rejects an invalid or absent active generation.
pub fn active_sessions(store: &Path) -> Result<Vec<String>, StoreError> {
    let path = store.join("active.json");
    if !path.exists() {
        return Err(StoreError::Invalid(
            "active corpus generation is unavailable",
        ));
    }
    Ok(load_generation(&path)?.sessions)
}
