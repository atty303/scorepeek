use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{BufWriter, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread;
use std::time::Duration;
use std::time::Instant;

use scorepeek::catalog::ScorepeekSongId;
use scorepeek::recognition::{
    CatalogCandidateEvidenceTable, MusicSelectSongResolution, ResultSongResolution,
    ScreenCatalogCandidateObservations, ScreenFieldObservations, ScreenSongResolution,
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

const CATALOG_SCHEMA: &str = "scorepeek-recognition-catalog-evidence-v1";
const OBSERVATION_SCHEMA: &str = "scorepeek-recognition-observation-v4";
const MANIFEST_SCHEMA: &str = "scorepeek-recognition-evidence-manifest-v2";
const MAX_CATALOG_BYTES: u64 = 16 * 1024 * 1024;
const MAX_OBSERVATION_BYTES: u64 = 256 * 1024 * 1024;
const MAX_OBSERVATIONS: usize = 3_600;

pub struct RecognitionArtifactWriter {
    root: PathBuf,
    observations: BufWriter<File>,
    observation_hasher: Sha256,
    observation_bytes: u64,
    observation_count: usize,
    catalog_sha256: Option<String>,
    catalog_entries: usize,
    profile_sha256: String,
    run_id: String,
    retention: RecognitionArtifactRetention,
}

#[derive(Serialize)]
struct StoredCatalog<'a> {
    schema: &'static str,
    profile_sha256: &'a str,
    catalog: &'a CatalogCandidateEvidenceTable,
}

#[derive(Serialize)]
struct StoredObservation<'a> {
    schema: &'static str,
    sequence: u64,
    timing: RecognitionArtifactTiming,
    fields: StoredFields<'a>,
    candidates: StoredCandidates,
    decision: StoredDecision<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected: Option<RecognitionArtifactExpected<'a>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum RecognitionArtifactTiming {
    Recording {
        source_pts_ms: u64,
    },
    Live {
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
    },
}

#[derive(Clone, Copy, Serialize)]
pub struct RecognitionArtifactExpected<'a> {
    pub episode_id: &'a str,
    pub song_id: Option<ScorepeekSongId>,
    pub clear_type: &'a str,
}

#[derive(Serialize)]
#[serde(tag = "screen", rename_all = "snake_case")]
enum StoredDecision<'a> {
    Result {
        resolution: &'a ResultSongResolution,
    },
    MusicSelect {
        resolution: &'a MusicSelectSongResolution,
    },
}

#[derive(Serialize)]
#[serde(tag = "screen", rename_all = "snake_case")]
enum StoredFields<'a> {
    Result {
        title: StoredText<'a>,
        artist: StoredText<'a>,
        clear_type: StoredText<'a>,
        difficulty: &'static str,
        level: &'static str,
        notes: &'static str,
        current_score: &'static str,
    },
    MusicSelect {
        central_title: StoredText<'a>,
        artist: StoredText<'a>,
        selected_chart: &'static str,
        active_list_title: StoredText<'a>,
    },
}

#[derive(Serialize)]
struct StoredText<'a> {
    input_width: usize,
    output_timesteps: usize,
    open_text: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(tag = "screen", rename_all = "snake_case")]
enum StoredCandidates {
    Result {
        comparison_key_id: &'static str,
        candidate_order: &'static str,
        candidates: Vec<[usize; 6]>,
    },
    MusicSelect {
        comparison_key_id: &'static str,
        candidate_order: &'static str,
        candidates: Vec<[usize; 12]>,
    },
}

#[derive(Serialize)]
struct StoredManifest<'a> {
    schema: &'static str,
    run_id: &'a str,
    profile_sha256: &'a str,
    status: &'static str,
    catalog_sha256: &'a str,
    catalog_entries: usize,
    observations_sha256: String,
    retention: RecognitionArtifactRetention,
    input_observation_count: usize,
    retained_observation_count: usize,
    observation_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecognitionArtifactRetention {
    Complete,
    ForegroundCompactedV1,
}

impl RecognitionArtifactWriter {
    pub fn create(root: &Path, run_id: String, profile_sha256: String) -> Result<Self, String> {
        Self::create_with_retention(
            root,
            run_id,
            profile_sha256,
            RecognitionArtifactRetention::Complete,
        )
    }

    fn create_with_retention(
        root: &Path,
        run_id: String,
        profile_sha256: String,
        retention: RecognitionArtifactRetention,
    ) -> Result<Self, String> {
        let parent = root
            .parent()
            .ok_or_else(|| "recognition artifact root has no parent".to_owned())?;
        let parent_metadata = fs::metadata(parent)
            .map_err(|error| format!("recognition artifact parent is unavailable: {error}"))?;
        if !parent_metadata.file_type().is_dir() {
            return Err("recognition artifact parent must be a directory".to_owned());
        }
        DirBuilder::new()
            .mode(0o700)
            .create(root)
            .map_err(|error| format!("recognition artifact root creation failed: {error}"))?;
        sync_directory(parent)?;
        let observations = open_create_only(&root.join("observations.ndjson"))?;
        sync_directory(root)?;
        Ok(Self {
            root: root.to_owned(),
            observations: BufWriter::new(observations),
            observation_hasher: Sha256::new(),
            observation_bytes: 0,
            observation_count: 0,
            catalog_sha256: None,
            catalog_entries: 0,
            profile_sha256,
            run_id,
            retention,
        })
    }

    pub fn record(
        &mut self,
        sequence: u64,
        timing: RecognitionArtifactTiming,
        fields: &ScreenFieldObservations,
        candidates: &ScreenCatalogCandidateObservations,
        song_resolution: &ScreenSongResolution,
        expected: Option<RecognitionArtifactExpected<'_>>,
    ) -> Result<(), String> {
        if self.observation_count >= MAX_OBSERVATIONS {
            return Err("recognition artifact observation capacity exceeded".to_owned());
        }
        self.ensure_catalog(candidates.catalog_evidence())?;
        let decision = match (fields, song_resolution) {
            (ScreenFieldObservations::Result(_), ScreenSongResolution::Result(resolution)) => {
                StoredDecision::Result { resolution }
            }
            (
                ScreenFieldObservations::MusicSelect(_),
                ScreenSongResolution::MusicSelect(resolution),
            ) => StoredDecision::MusicSelect { resolution },
            _ => return Err("recognition artifact decision does not match screen".to_owned()),
        };
        let stored = StoredObservation {
            schema: OBSERVATION_SCHEMA,
            sequence,
            timing,
            fields: StoredFields::from(fields),
            candidates: StoredCandidates::try_from(candidates)?,
            decision,
            expected,
        };
        let mut bytes = serde_json::to_vec(&stored)
            .map_err(|_| "recognition observation serialization failed".to_owned())?;
        bytes.push(b'\n');
        let next_bytes = self
            .observation_bytes
            .checked_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
            .ok_or_else(|| "recognition artifact byte count overflow".to_owned())?;
        if next_bytes > MAX_OBSERVATION_BYTES {
            return Err("recognition artifact byte capacity exceeded".to_owned());
        }
        self.observations
            .write_all(&bytes)
            .map_err(|error| format!("recognition observation write failed: {error}"))?;
        self.observation_hasher.update(&bytes);
        self.observation_bytes = next_bytes;
        self.observation_count += 1;
        Ok(())
    }

    pub fn finish(self, succeeded: bool) -> Result<String, String> {
        let input_observation_count = self.observation_count;
        self.finish_with_input_count(succeeded, input_observation_count)
    }

    fn finish_with_input_count(
        mut self,
        succeeded: bool,
        input_observation_count: usize,
    ) -> Result<String, String> {
        if input_observation_count < self.observation_count {
            return Err("recognition artifact input count is invalid".to_owned());
        }
        let catalog_sha256 = self
            .catalog_sha256
            .as_deref()
            .ok_or_else(|| "recognition artifact catalog evidence is missing".to_owned())?;
        self.observations
            .flush()
            .map_err(|error| format!("recognition observation flush failed: {error}"))?;
        self.observations
            .get_ref()
            .sync_all()
            .map_err(|error| format!("recognition observation sync failed: {error}"))?;
        let manifest = StoredManifest {
            schema: MANIFEST_SCHEMA,
            run_id: &self.run_id,
            profile_sha256: &self.profile_sha256,
            status: if succeeded { "success" } else { "error" },
            catalog_sha256,
            catalog_entries: self.catalog_entries,
            observations_sha256: hex_digest(self.observation_hasher.finalize()),
            retention: self.retention,
            input_observation_count,
            retained_observation_count: self.observation_count,
            observation_bytes: self.observation_bytes,
        };
        let bytes = serde_json::to_vec(&manifest)
            .map_err(|_| "recognition artifact manifest serialization failed".to_owned())?;
        let digest = sha256_bytes(&bytes);
        write_create_only(&self.root.join("manifest.json"), &bytes)?;
        sync_directory(&self.root)?;
        Ok(digest)
    }

    fn ensure_catalog(&mut self, catalog: &CatalogCandidateEvidenceTable) -> Result<(), String> {
        if self.catalog_sha256.is_some() {
            return Ok(());
        }
        let stored = StoredCatalog {
            schema: CATALOG_SCHEMA,
            profile_sha256: &self.profile_sha256,
            catalog,
        };
        let bytes = serde_json::to_vec(&stored)
            .map_err(|_| "recognition catalog serialization failed".to_owned())?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_CATALOG_BYTES {
            return Err("recognition catalog artifact byte capacity exceeded".to_owned());
        }
        let digest = sha256_bytes(&bytes);
        write_create_only(&self.root.join("catalog.json"), &bytes)?;
        sync_directory(&self.root)?;
        self.catalog_entries = catalog.songs.len();
        self.catalog_sha256 = Some(digest);
        Ok(())
    }
}

const LIVE_QUEUE_CAPACITY: usize = 2;
pub const LIVE_FINISH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecognitionArtifactEnqueueOutcome {
    Enqueued,
    QueueFull,
    WorkerUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecognitionArtifactFinishStatus {
    Complete,
    WriteFailed,
    Timeout,
    WorkerUnavailable,
}

#[derive(Debug)]
pub struct RecognitionArtifactFinishOutcome {
    pub status: RecognitionArtifactFinishStatus,
    pub manifest_sha256: Option<String>,
    pub input_observations: usize,
    pub retained_observations: usize,
}

struct LiveRecord {
    sequence: u64,
    monotonic_start_ms: u64,
    monotonic_end_ms: u64,
    observation: crate::recognition_live::screen_field_observer::RegisteredScreenFieldObservation,
}

enum LiveWriterMessage {
    Record(Box<LiveRecord>),
    Finish {
        succeeded: bool,
        response: SyncSender<RecognitionArtifactFinishOutcome>,
    },
}

pub struct RecognitionArtifactWorker {
    sender: Option<SyncSender<LiveWriterMessage>>,
}

impl RecognitionArtifactWorker {
    #[must_use]
    pub fn start(root: PathBuf, run_id: String, profile_sha256: String) -> Self {
        Self::start_inner(
            root,
            run_id,
            profile_sha256,
            LIVE_QUEUE_CAPACITY,
            Some(production_supervisor()),
        )
    }

    #[must_use]
    pub fn start_foreground(root: PathBuf, run_id: String, profile_sha256: String) -> Self {
        Self::start_inner_with_retention(
            root,
            run_id,
            profile_sha256,
            LIVE_QUEUE_CAPACITY,
            Some(production_supervisor()),
            RecognitionArtifactRetention::ForegroundCompactedV1,
        )
    }

    fn start_inner(
        root: PathBuf,
        run_id: String,
        profile_sha256: String,
        capacity: usize,
        supervisor: Option<&Mutex<Weak<()>>>,
    ) -> Self {
        Self::start_inner_with_retention(
            root,
            run_id,
            profile_sha256,
            capacity,
            supervisor,
            RecognitionArtifactRetention::Complete,
        )
    }

    fn start_inner_with_retention(
        root: PathBuf,
        run_id: String,
        profile_sha256: String,
        capacity: usize,
        supervisor: Option<&Mutex<Weak<()>>>,
        retention: RecognitionArtifactRetention,
    ) -> Self {
        if capacity == 0 {
            return Self { sender: None };
        }
        let supervisor_token = if let Some(supervisor) = supervisor {
            let Some(token) = acquire_worker_token(supervisor) else {
                return Self { sender: None };
            };
            Some(token)
        } else {
            None
        };
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("scorepeek-recognition-artifact-writer".to_owned())
            .spawn(move || {
                run_live_writer(
                    &receiver,
                    &root,
                    run_id,
                    profile_sha256,
                    supervisor_token,
                    &startup_sender,
                    retention,
                );
            });
        let sender = worker.ok().map(|_| sender);
        let _ = startup_receiver.recv_timeout(LIVE_FINISH_TIMEOUT);
        Self { sender }
    }

    pub fn try_record(
        &mut self,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        observation: crate::recognition_live::screen_field_observer::RegisteredScreenFieldObservation,
    ) -> RecognitionArtifactEnqueueOutcome {
        let Some(sender) = &self.sender else {
            return RecognitionArtifactEnqueueOutcome::WorkerUnavailable;
        };
        match sender.try_send(LiveWriterMessage::Record(Box::new(LiveRecord {
            sequence,
            monotonic_start_ms,
            monotonic_end_ms,
            observation,
        }))) {
            Ok(()) => RecognitionArtifactEnqueueOutcome::Enqueued,
            Err(TrySendError::Full(_)) => RecognitionArtifactEnqueueOutcome::QueueFull,
            Err(TrySendError::Disconnected(_)) => {
                self.sender = None;
                RecognitionArtifactEnqueueOutcome::WorkerUnavailable
            }
        }
    }

    #[must_use]
    pub fn finish(mut self, succeeded: bool) -> RecognitionArtifactFinishOutcome {
        let Some(sender) = self.sender.take() else {
            return unavailable_finish();
        };
        let (response_sender, response_receiver) = mpsc::sync_channel(1);
        let deadline = Instant::now() + LIVE_FINISH_TIMEOUT;
        let mut message = LiveWriterMessage::Finish {
            succeeded,
            response: response_sender,
        };
        loop {
            match sender.try_send(message) {
                Ok(()) => break,
                Err(TrySendError::Full(returned)) => {
                    if Instant::now() >= deadline {
                        return RecognitionArtifactFinishOutcome {
                            status: RecognitionArtifactFinishStatus::Timeout,
                            manifest_sha256: None,
                            input_observations: 0,
                            retained_observations: 0,
                        };
                    }
                    message = returned;
                    thread::sleep(Duration::from_millis(1));
                }
                Err(TrySendError::Disconnected(_)) => return unavailable_finish(),
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        response_receiver.recv_timeout(remaining).map_or(
            RecognitionArtifactFinishOutcome {
                status: RecognitionArtifactFinishStatus::Timeout,
                manifest_sha256: None,
                input_observations: 0,
                retained_observations: 0,
            },
            |outcome| outcome,
        )
    }
}

fn unavailable_finish() -> RecognitionArtifactFinishOutcome {
    RecognitionArtifactFinishOutcome {
        status: RecognitionArtifactFinishStatus::WorkerUnavailable,
        manifest_sha256: None,
        input_observations: 0,
        retained_observations: 0,
    }
}

#[allow(clippy::too_many_lines)]
fn run_live_writer(
    receiver: &Receiver<LiveWriterMessage>,
    root: &Path,
    run_id: String,
    profile_sha256: String,
    mut supervisor_token: Option<Arc<()>>,
    startup: &SyncSender<bool>,
    retention: RecognitionArtifactRetention,
) {
    const FOREGROUND_MUSIC_SELECT_INTERVAL_MS: u64 = 5 * 60 * 1_000;
    const FOREGROUND_RESULT_INTERVAL_MAX_MS: u64 = 30_000;

    let mut writer =
        RecognitionArtifactWriter::create_with_retention(root, run_id, profile_sha256, retention);
    let mut write_failed = writer.is_err();
    let mut input_observations = 0_usize;
    let mut retained_observations = 0_usize;
    let mut pending_result: Option<Box<LiveRecord>> = None;
    let mut last_music_select_ms: Option<u64> = None;
    let _ = startup.send(!write_failed);
    while let Ok(message) = receiver.recv() {
        match message {
            LiveWriterMessage::Record(record) => {
                input_observations = input_observations.saturating_add(1);
                if write_failed {
                    continue;
                }
                match retention {
                    RecognitionArtifactRetention::Complete => {
                        if record_live(writer.as_mut().expect("writer is available"), &record)
                            .is_err()
                        {
                            write_failed = true;
                        } else {
                            retained_observations = retained_observations.saturating_add(1);
                        }
                    }
                    RecognitionArtifactRetention::ForegroundCompactedV1 => {
                        if matches!(
                            record.observation.fields(),
                            ScreenFieldObservations::Result(_)
                        ) {
                            let new_interval = pending_result.as_ref().is_some_and(|pending| {
                                record
                                    .monotonic_start_ms
                                    .saturating_sub(pending.monotonic_start_ms)
                                    > FOREGROUND_RESULT_INTERVAL_MAX_MS
                            });
                            if new_interval {
                                let previous = pending_result.take().expect("checked as present");
                                if record_live(
                                    writer.as_mut().expect("writer is available"),
                                    &previous,
                                )
                                .is_err()
                                {
                                    write_failed = true;
                                    continue;
                                }
                                retained_observations = retained_observations.saturating_add(1);
                            }
                            let replace = pending_result.as_ref().is_none_or(|pending| {
                                result_record_priority(&record) >= result_record_priority(pending)
                            });
                            if replace {
                                pending_result = Some(record);
                            }
                        } else {
                            let after_result = if let Some(result) = pending_result.take() {
                                if record_live(
                                    writer.as_mut().expect("writer is available"),
                                    &result,
                                )
                                .is_err()
                                {
                                    write_failed = true;
                                    false
                                } else {
                                    retained_observations = retained_observations.saturating_add(1);
                                    true
                                }
                            } else {
                                false
                            };
                            let due = last_music_select_ms.is_none_or(|previous| {
                                record.monotonic_start_ms.saturating_sub(previous)
                                    >= FOREGROUND_MUSIC_SELECT_INTERVAL_MS
                            });
                            if !write_failed && (after_result || due) {
                                if record_live(
                                    writer.as_mut().expect("writer is available"),
                                    &record,
                                )
                                .is_err()
                                {
                                    write_failed = true;
                                } else {
                                    retained_observations = retained_observations.saturating_add(1);
                                    last_music_select_ms = Some(record.monotonic_start_ms);
                                }
                            }
                        }
                    }
                }
            }
            LiveWriterMessage::Finish {
                succeeded,
                response,
            } => {
                if !write_failed && let Some(result) = pending_result.take() {
                    if record_live(writer.as_mut().expect("writer is available"), &result).is_err()
                    {
                        write_failed = true;
                    } else {
                        retained_observations = retained_observations.saturating_add(1);
                    }
                }
                let outcome = if write_failed {
                    drop(writer);
                    RecognitionArtifactFinishOutcome {
                        status: RecognitionArtifactFinishStatus::WriteFailed,
                        manifest_sha256: None,
                        input_observations,
                        retained_observations,
                    }
                } else {
                    match writer
                        .expect("successful writer remains available")
                        .finish_with_input_count(succeeded, input_observations)
                    {
                        Ok(digest) => RecognitionArtifactFinishOutcome {
                            status: RecognitionArtifactFinishStatus::Complete,
                            manifest_sha256: Some(digest),
                            input_observations,
                            retained_observations,
                        },
                        Err(_) => RecognitionArtifactFinishOutcome {
                            status: RecognitionArtifactFinishStatus::WriteFailed,
                            manifest_sha256: None,
                            input_observations,
                            retained_observations,
                        },
                    }
                };
                drop(supervisor_token.take());
                let _ = response.send(outcome);
                return;
            }
        }
    }
}

fn record_live(writer: &mut RecognitionArtifactWriter, record: &LiveRecord) -> Result<(), String> {
    let output = &record.observation;
    writer.record(
        record.sequence,
        RecognitionArtifactTiming::Live {
            monotonic_start_ms: record.monotonic_start_ms,
            monotonic_end_ms: record.monotonic_end_ms,
        },
        output.fields(),
        output.candidates(),
        output.song_resolution(),
        None,
    )
}

fn result_record_priority(record: &LiveRecord) -> u8 {
    match record.observation.result_resolution() {
        Some(ResultSongResolution::Accepted { .. }) => 1,
        Some(ResultSongResolution::Unknown { .. }) | None => 0,
    }
}

fn production_supervisor() -> &'static Mutex<Weak<()>> {
    static SUPERVISOR: OnceLock<Mutex<Weak<()>>> = OnceLock::new();
    SUPERVISOR.get_or_init(|| Mutex::new(Weak::new()))
}

fn acquire_worker_token(supervisor: &Mutex<Weak<()>>) -> Option<Arc<()>> {
    let mut current = supervisor.lock().ok()?;
    if current.upgrade().is_some() {
        return None;
    }
    let token = Arc::new(());
    *current = Arc::downgrade(&token);
    Some(token)
}

impl<'a> From<&'a ScreenFieldObservations> for StoredFields<'a> {
    fn from(fields: &'a ScreenFieldObservations) -> Self {
        match fields {
            ScreenFieldObservations::Result(fields) => Self::Result {
                title: StoredText::from(&fields.title),
                artist: StoredText::from(&fields.artist),
                clear_type: StoredText::from(&fields.clear_type),
                difficulty: "observer_not_implemented",
                level: "observer_not_implemented",
                notes: "observer_not_implemented",
                current_score: "observer_not_implemented",
            },
            ScreenFieldObservations::MusicSelect(fields) => Self::MusicSelect {
                central_title: StoredText::from(&fields.central_title),
                artist: StoredText::from(&fields.artist),
                selected_chart: "observer_not_implemented",
                active_list_title: StoredText::from(&fields.active_list_title),
            },
        }
    }
}

impl<'a> From<&'a scorepeek::recognition::DynamicTextObservation> for StoredText<'a> {
    fn from(observation: &'a scorepeek::recognition::DynamicTextObservation) -> Self {
        Self {
            input_width: observation.input_width,
            output_timesteps: observation.output_timesteps,
            open_text: &observation.open_text,
        }
    }
}

impl TryFrom<&ScreenCatalogCandidateObservations> for StoredCandidates {
    type Error = String;

    fn try_from(candidates: &ScreenCatalogCandidateObservations) -> Result<Self, Self::Error> {
        match candidates {
            ScreenCatalogCandidateObservations::Result {
                comparison_key_id,
                catalog,
                candidates,
                ..
            } => {
                require_catalog_order(
                    catalog,
                    candidates.iter().map(|candidate| candidate.song_id),
                )?;
                Ok(Self::Result {
                    comparison_key_id,
                    candidate_order: "catalog_songs",
                    candidates: candidates
                        .iter()
                        .map(|candidate| {
                            [
                                candidate.title.minimum_edit_distance,
                                candidate.title.maximum_normalized_similarity.matching_units,
                                candidate.title.maximum_normalized_similarity.compared_units,
                                candidate.artist.minimum_edit_distance,
                                candidate
                                    .artist
                                    .maximum_normalized_similarity
                                    .matching_units,
                                candidate
                                    .artist
                                    .maximum_normalized_similarity
                                    .compared_units,
                            ]
                        })
                        .collect(),
                })
            }
            ScreenCatalogCandidateObservations::MusicSelect {
                comparison_key_id,
                catalog,
                candidates,
                ..
            } => {
                require_catalog_order(
                    catalog,
                    candidates.iter().map(|candidate| candidate.song_id),
                )?;
                Ok(Self::MusicSelect {
                    comparison_key_id,
                    candidate_order: "catalog_songs",
                    candidates: candidates
                        .iter()
                        .map(|candidate| {
                            [
                                candidate.central_title.minimum_edit_distance,
                                candidate
                                    .central_title
                                    .maximum_normalized_similarity
                                    .matching_units,
                                candidate
                                    .central_title
                                    .maximum_normalized_similarity
                                    .compared_units,
                                candidate.artist.minimum_edit_distance,
                                candidate
                                    .artist
                                    .maximum_normalized_similarity
                                    .matching_units,
                                candidate
                                    .artist
                                    .maximum_normalized_similarity
                                    .compared_units,
                                candidate.active_list_title.minimum_edit_distance,
                                candidate
                                    .active_list_title
                                    .maximum_normalized_similarity
                                    .matching_units,
                                candidate
                                    .active_list_title
                                    .maximum_normalized_similarity
                                    .compared_units,
                                candidate.active_list_title_prefix.minimum_edit_distance,
                                candidate
                                    .active_list_title_prefix
                                    .maximum_normalized_similarity
                                    .matching_units,
                                candidate
                                    .active_list_title_prefix
                                    .maximum_normalized_similarity
                                    .compared_units,
                            ]
                        })
                        .collect(),
                })
            }
        }
    }
}

fn require_catalog_order(
    catalog: &CatalogCandidateEvidenceTable,
    candidate_ids: impl Iterator<Item = ScorepeekSongId>,
) -> Result<(), String> {
    if catalog
        .songs
        .iter()
        .map(|song| song.song_id)
        .eq(candidate_ids)
    {
        Ok(())
    } else {
        Err("recognition candidate order does not match catalog evidence".to_owned())
    }
}

fn open_create_only(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("recognition artifact file creation failed: {error}"))
}

fn write_create_only(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = open_create_only(path)?;
    file.write_all(bytes)
        .map_err(|error| format!("recognition artifact write failed: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("recognition artifact sync failed: {error}"))
}

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("recognition artifact directory sync failed: {error}"))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex_digest(Sha256::digest(bytes))
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    let mut output = String::with_capacity(64);
    for byte in digest.as_ref() {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::Arc;

    use scorepeek::recognition::{
        CatalogCandidateDomain, CatalogCandidateEvidenceTable, CatalogNormalizedSimilarity,
        CatalogTextCandidateScore, DynamicTextObservation, FieldNotObserved,
        FieldNotObservedReason, MusicSelectScreenFieldObservations, RESULT_SONG_RESOLVER_ID,
        ResultScreenFieldObservations, ResultSongCandidateObservation, ResultSongResolution,
        ResultSongUnknownReason,
    };

    use super::*;

    fn result_fields() -> ScreenFieldObservations {
        let text = |value: &str| DynamicTextObservation {
            input_width: 64,
            output_timesteps: 12,
            open_text: value.to_owned(),
        };
        let missing = FieldNotObserved {
            reason: FieldNotObservedReason::ObserverNotImplemented,
        };
        ScreenFieldObservations::Result(ResultScreenFieldObservations {
            title: text("ABSOLUTE EVIL"),
            artist: text("Yuta Imai"),
            clear_type: text("FAILED"),
            difficulty: missing,
            level: missing,
            notes: missing,
            current_score: missing,
        })
    }

    fn music_select_fields() -> ScreenFieldObservations {
        let text = |value: &str| DynamicTextObservation {
            input_width: 64,
            output_timesteps: 12,
            open_text: value.to_owned(),
        };
        ScreenFieldObservations::MusicSelect(MusicSelectScreenFieldObservations {
            central_title: text("texture"),
            artist: text("artist"),
            selected_chart: FieldNotObserved {
                reason: FieldNotObservedReason::ObserverNotImplemented,
            },
            active_list_title: text("VISIBLE TITLE"),
        })
    }

    fn empty_candidates() -> ScreenCatalogCandidateObservations {
        let song_id = serde_json::from_str("\"6ef33da9-090a-500c-844a-8bffd14de63f\"")
            .expect("fixture song ID is valid");
        ScreenCatalogCandidateObservations::Result {
            comparison_key_id: "test-comparison-v1",
            catalog: Arc::new(CatalogCandidateEvidenceTable {
                comparison_key_id: "test-comparison-v1",
                songs: vec![scorepeek::recognition::CatalogCandidateSongEvidence {
                    song_id,
                    title: scorepeek::recognition::CatalogCandidateTextEvidence {
                        display: vec!["ABSOLUTE EVIL".to_owned()],
                        exact: vec!["ABSOLUTEEVIL".to_owned()],
                        folded: vec!["ABSOLUTEEVIL".to_owned()],
                    },
                    artist: scorepeek::recognition::CatalogCandidateTextEvidence {
                        display: vec!["Yuta Imai".to_owned()],
                        exact: vec!["YutaImai".to_owned()],
                        folded: Vec::new(),
                    },
                }],
            }),
            candidates: vec![ResultSongCandidateObservation {
                song_id,
                title: CatalogTextCandidateScore {
                    minimum_edit_distance: 0,
                    maximum_normalized_similarity: CatalogNormalizedSimilarity {
                        matching_units: 12,
                        compared_units: 12,
                    },
                },
                artist: CatalogTextCandidateScore {
                    minimum_edit_distance: 1,
                    maximum_normalized_similarity: CatalogNormalizedSimilarity {
                        matching_units: 8,
                        compared_units: 9,
                    },
                },
            }],
        }
    }

    fn empty_catalog_candidates() -> ScreenCatalogCandidateObservations {
        ScreenCatalogCandidateObservations::Result {
            comparison_key_id: "test-comparison-v1",
            catalog: Arc::new(CatalogCandidateEvidenceTable {
                comparison_key_id: "test-comparison-v1",
                songs: Vec::new(),
            }),
            candidates: Vec::new(),
        }
    }

    #[test]
    fn artifact_retains_exact_values_and_finalizes_last() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("run");
        let mut writer =
            RecognitionArtifactWriter::create(&root, "simulation-001".to_owned(), "a".repeat(64))
                .unwrap();
        writer
            .record(
                7,
                RecognitionArtifactTiming::Recording {
                    source_pts_ms: 140_000,
                },
                &result_fields(),
                &empty_candidates(),
                &ScreenSongResolution::Result(ResultSongResolution::Unknown {
                    resolver_id: RESULT_SONG_RESOLVER_ID,
                    reason: ResultSongUnknownReason::EmptyTitle,
                    selected: None,
                    runner_up: None,
                    title_edit_margin: None,
                }),
                None,
            )
            .unwrap();
        let digest = writer.finish(true).unwrap();

        assert_eq!(digest.len(), 64);
        let observation = fs::read_to_string(root.join("observations.ndjson")).unwrap();
        assert!(observation.contains("ABSOLUTE EVIL"));
        assert!(observation.contains("Yuta Imai"));
        assert!(observation.contains("FAILED"));
        let catalog = fs::read_to_string(root.join("catalog.json")).unwrap();
        assert!(catalog.contains("test-comparison-v1"));
        assert!(catalog.contains("ABSOLUTE EVIL"));
        assert!(catalog.contains("ABSOLUTEEVIL"));
        assert!(catalog.contains("Yuta Imai"));
        let manifest = fs::read_to_string(root.join("manifest.json")).unwrap();
        assert!(manifest.contains("\"input_observation_count\":1"));
        assert!(manifest.contains("\"retained_observation_count\":1"));
        assert_eq!(
            fs::metadata(root.join("catalog.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn artifact_root_is_create_only() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("run");
        fs::create_dir(&root).unwrap();
        assert!(
            RecognitionArtifactWriter::create(&root, "simulation-001".to_owned(), "a".repeat(64),)
                .is_err()
        );
    }

    #[test]
    fn artifact_retains_typed_music_select_resolution() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("music-select");
        let domain =
            CatalogCandidateDomain::from_catalog(&scorepeek::catalog::Catalog::default()).unwrap();
        let output = crate::recognition_live::screen_field_observer::RegisteredScreenFieldObservation::from_fields(
            &domain,
            music_select_fields(),
        );
        let mut writer =
            RecognitionArtifactWriter::create(&root, "music-001".to_owned(), "d".repeat(64))
                .unwrap();
        writer
            .record(
                10,
                RecognitionArtifactTiming::Live {
                    monotonic_start_ms: 100,
                    monotonic_end_ms: 101,
                },
                output.fields(),
                output.candidates(),
                output.song_resolution(),
                None,
            )
            .unwrap();
        writer.finish(true).unwrap();

        let stored = fs::read_to_string(root.join("observations.ndjson")).unwrap();
        assert!(stored.contains("\"screen\":\"music_select\""));
        assert!(stored.contains("no_catalog_candidates"));
        assert!(!stored.contains("resolver_not_implemented"));
    }

    #[test]
    fn failed_empty_catalog_run_retains_observation_and_expected_values() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("run");
        let expected_song_id = serde_json::from_str("\"6ef33da9-090a-500c-844a-8bffd14de63f\"")
            .expect("fixture song ID is valid");
        let mut writer =
            RecognitionArtifactWriter::create(&root, "simulation-002".to_owned(), "b".repeat(64))
                .unwrap();
        writer
            .record(
                8,
                RecognitionArtifactTiming::Recording {
                    source_pts_ms: 141_000,
                },
                &result_fields(),
                &empty_catalog_candidates(),
                &ScreenSongResolution::Result(ResultSongResolution::Unknown {
                    resolver_id: RESULT_SONG_RESOLVER_ID,
                    reason: ResultSongUnknownReason::NoCatalogCandidates,
                    selected: None,
                    runner_up: None,
                    title_edit_margin: None,
                }),
                Some(RecognitionArtifactExpected {
                    episode_id: "failed-result-1",
                    song_id: Some(expected_song_id),
                    clear_type: "FAILED",
                }),
            )
            .unwrap();
        writer.finish(false).unwrap();

        let catalog: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("catalog.json")).unwrap()).unwrap();
        assert_eq!(catalog["catalog"]["songs"], serde_json::json!([]));
        let observation = fs::read_to_string(root.join("observations.ndjson")).unwrap();
        assert!(observation.contains("ABSOLUTE EVIL"));
        assert!(observation.contains("no_catalog_candidates"));
        assert!(observation.contains("failed-result-1"));
        assert!(observation.contains("6ef33da9-090a-500c-844a-8bffd14de63f"));
        let manifest = fs::read_to_string(root.join("manifest.json")).unwrap();
        assert!(manifest.contains("\"status\":\"error\""));
        assert!(manifest.contains("\"catalog_entries\":0"));
        assert!(manifest.contains("\"retained_observation_count\":1"));
    }

    #[test]
    fn live_worker_retains_exact_values_with_live_timing() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("live");
        let domain =
            CatalogCandidateDomain::from_catalog(&scorepeek::catalog::Catalog::default()).unwrap();
        let observation = crate::recognition_live::screen_field_observer::RegisteredScreenFieldObservation::from_fields(
            &domain,
            result_fields(),
        );
        let mut worker = RecognitionArtifactWorker::start_inner(
            root.clone(),
            "live-001".to_owned(),
            "c".repeat(64),
            LIVE_QUEUE_CAPACITY,
            None,
        );

        assert_eq!(
            worker.try_record(9, 1_000, 1_017, observation),
            RecognitionArtifactEnqueueOutcome::Enqueued
        );
        let outcome = worker.finish(true);

        assert_eq!(outcome.status, RecognitionArtifactFinishStatus::Complete);
        assert_eq!(outcome.manifest_sha256.unwrap().len(), 64);
        let stored = fs::read_to_string(root.join("observations.ndjson")).unwrap();
        assert!(stored.contains("scorepeek-recognition-observation-v4"));
        assert!(stored.contains("\"source\":\"live\""));
        assert!(stored.contains("\"monotonic_start_ms\":1000"));
        assert!(stored.contains("\"monotonic_end_ms\":1017"));
        assert!(stored.contains("ABSOLUTE EVIL"));
        assert!(stored.contains("Yuta Imai"));
        assert!(stored.contains("FAILED"));
    }

    #[test]
    fn foreground_worker_compacts_one_result_interval_and_reports_omissions() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("foreground");
        let domain =
            CatalogCandidateDomain::from_catalog(&scorepeek::catalog::Catalog::default()).unwrap();
        let mut worker = RecognitionArtifactWorker::start_inner_with_retention(
            root.clone(),
            "foreground-001".to_owned(),
            "d".repeat(64),
            32,
            None,
            RecognitionArtifactRetention::ForegroundCompactedV1,
        );

        for sequence in 1..=20 {
            let observation = crate::recognition_live::screen_field_observer::RegisteredScreenFieldObservation::from_fields(
                &domain,
                result_fields(),
            );
            assert_eq!(
                worker.try_record(sequence, sequence * 200, sequence * 200 + 17, observation),
                RecognitionArtifactEnqueueOutcome::Enqueued
            );
        }
        let outcome = worker.finish(true);

        assert_eq!(outcome.status, RecognitionArtifactFinishStatus::Complete);
        assert_eq!(outcome.input_observations, 20);
        assert_eq!(outcome.retained_observations, 1);
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["retention"], "foreground_compacted_v1");
        assert_eq!(manifest["input_observation_count"], 20);
        assert_eq!(manifest["retained_observation_count"], 1);
    }

    #[test]
    fn foreground_worker_does_not_merge_result_intervals_across_a_long_gap() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("foreground-separated-results");
        let mut worker = RecognitionArtifactWorker::start_inner_with_retention(
            root.clone(),
            "foreground-separated-results".to_owned(),
            "f".repeat(64),
            8,
            None,
            RecognitionArtifactRetention::ForegroundCompactedV1,
        );
        let domain =
            CatalogCandidateDomain::from_catalog(&scorepeek::catalog::Catalog::default()).unwrap();
        for (sequence, time) in [(1, 0), (2, 1_000), (3, 60_000), (4, 61_000)] {
            let observation = crate::recognition_live::screen_field_observer::RegisteredScreenFieldObservation::from_fields(
                &domain,
                result_fields(),
            );
            assert_eq!(
                worker.try_record(sequence, time, time + 17, observation),
                RecognitionArtifactEnqueueOutcome::Enqueued
            );
        }
        let outcome = worker.finish(true);

        assert_eq!(outcome.status, RecognitionArtifactFinishStatus::Complete);
        assert_eq!(outcome.input_observations, 4);
        assert_eq!(outcome.retained_observations, 2);
    }

    #[test]
    fn unavailable_live_worker_is_typed() {
        let parent = tempfile::tempdir().unwrap();
        let mut worker = RecognitionArtifactWorker::start_inner(
            parent.path().join("live"),
            "live-002".to_owned(),
            "d".repeat(64),
            0,
            None,
        );
        let domain =
            CatalogCandidateDomain::from_catalog(&scorepeek::catalog::Catalog::default()).unwrap();
        let observation = crate::recognition_live::screen_field_observer::RegisteredScreenFieldObservation::from_fields(
            &domain,
            result_fields(),
        );

        assert_eq!(
            worker.try_record(1, 1, 2, observation),
            RecognitionArtifactEnqueueOutcome::WorkerUnavailable
        );
        assert_eq!(
            worker.finish(false).status,
            RecognitionArtifactFinishStatus::WorkerUnavailable
        );
    }

    #[test]
    fn live_worker_queue_full_is_typed_without_blocking() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let mut worker = RecognitionArtifactWorker {
            sender: Some(sender),
        };
        let domain =
            CatalogCandidateDomain::from_catalog(&scorepeek::catalog::Catalog::default()).unwrap();
        let observation = || {
            crate::recognition_live::screen_field_observer::RegisteredScreenFieldObservation::from_fields(
                &domain,
                result_fields(),
            )
        };

        assert_eq!(
            worker.try_record(1, 1, 2, observation()),
            RecognitionArtifactEnqueueOutcome::Enqueued
        );
        assert_eq!(
            worker.try_record(2, 2, 3, observation()),
            RecognitionArtifactEnqueueOutcome::QueueFull
        );
    }

    #[test]
    fn live_worker_supervisor_rejects_overlap_until_the_writer_exits() {
        let parent = tempfile::tempdir().unwrap();
        let supervisor = Mutex::new(Weak::new());
        let first = RecognitionArtifactWorker::start_inner(
            parent.path().join("first"),
            "first".to_owned(),
            "a".repeat(64),
            1,
            Some(&supervisor),
        );
        let overlapping = RecognitionArtifactWorker::start_inner(
            parent.path().join("overlapping"),
            "overlapping".to_owned(),
            "b".repeat(64),
            1,
            Some(&supervisor),
        );

        assert_eq!(
            overlapping.finish(false).status,
            RecognitionArtifactFinishStatus::WorkerUnavailable
        );
        assert_eq!(
            first.finish(false).status,
            RecognitionArtifactFinishStatus::WriteFailed
        );

        let after_exit = RecognitionArtifactWorker::start_inner(
            parent.path().join("after-exit"),
            "after-exit".to_owned(),
            "c".repeat(64),
            1,
            Some(&supervisor),
        );
        assert_eq!(
            after_exit.finish(false).status,
            RecognitionArtifactFinishStatus::WriteFailed
        );
    }
}
