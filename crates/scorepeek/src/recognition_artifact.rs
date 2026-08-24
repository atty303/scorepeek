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
    CatalogCandidateEvidenceTable, ResultSongResolution, ScreenCatalogCandidateObservations,
    ScreenFieldObservations,
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

const CATALOG_SCHEMA: &str = "scorepeek-recognition-catalog-evidence-v1";
const OBSERVATION_SCHEMA: &str = "scorepeek-recognition-observation-v2";
const MANIFEST_SCHEMA: &str = "scorepeek-recognition-evidence-manifest-v1";
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
    candidates: StoredCandidates<'a>,
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
        status: &'static str,
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

#[derive(Serialize)]
#[serde(tag = "screen", rename_all = "snake_case")]
enum StoredCandidates<'a> {
    Result {
        comparison_key_id: &'static str,
        candidates: &'a [scorepeek::recognition::ResultSongCandidateObservation],
    },
    MusicSelect {
        comparison_key_id: &'static str,
        candidates: &'a [scorepeek::recognition::MusicSelectSongCandidateObservation],
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
    observation_count: usize,
    observation_bytes: u64,
}

impl RecognitionArtifactWriter {
    pub fn create(root: &Path, run_id: String, profile_sha256: String) -> Result<Self, String> {
        let parent = root
            .parent()
            .ok_or_else(|| "recognition artifact root has no parent".to_owned())?;
        let parent_metadata = fs::symlink_metadata(parent)
            .map_err(|error| format!("recognition artifact parent is unavailable: {error}"))?;
        if !parent_metadata.file_type().is_dir() || parent_metadata.file_type().is_symlink() {
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
        })
    }

    pub fn record(
        &mut self,
        sequence: u64,
        timing: RecognitionArtifactTiming,
        fields: &ScreenFieldObservations,
        candidates: &ScreenCatalogCandidateObservations,
        result_resolution: Option<&ResultSongResolution>,
        expected: Option<RecognitionArtifactExpected<'_>>,
    ) -> Result<(), String> {
        if self.observation_count >= MAX_OBSERVATIONS {
            return Err("recognition artifact observation capacity exceeded".to_owned());
        }
        self.ensure_catalog(candidates.catalog_evidence())?;
        let decision = match (fields, result_resolution) {
            (ScreenFieldObservations::Result(_), Some(resolution)) => {
                StoredDecision::Result { resolution }
            }
            (ScreenFieldObservations::MusicSelect(_), None) => StoredDecision::MusicSelect {
                status: "resolver_not_implemented",
            },
            _ => return Err("recognition artifact decision does not match screen".to_owned()),
        };
        let stored = StoredObservation {
            schema: OBSERVATION_SCHEMA,
            sequence,
            timing,
            fields: StoredFields::from(fields),
            candidates: StoredCandidates::from(candidates),
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

    pub fn finish(mut self, succeeded: bool) -> Result<String, String> {
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
            observation_count: self.observation_count,
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

    fn start_inner(
        root: PathBuf,
        run_id: String,
        profile_sha256: String,
        capacity: usize,
        supervisor: Option<&Mutex<Weak<()>>>,
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
            },
            |outcome| outcome,
        )
    }
}

fn unavailable_finish() -> RecognitionArtifactFinishOutcome {
    RecognitionArtifactFinishOutcome {
        status: RecognitionArtifactFinishStatus::WorkerUnavailable,
        manifest_sha256: None,
    }
}

fn run_live_writer(
    receiver: &Receiver<LiveWriterMessage>,
    root: &Path,
    run_id: String,
    profile_sha256: String,
    mut supervisor_token: Option<Arc<()>>,
    startup: &SyncSender<bool>,
) {
    let mut writer = RecognitionArtifactWriter::create(root, run_id, profile_sha256);
    let mut write_failed = writer.is_err();
    let _ = startup.send(!write_failed);
    while let Ok(message) = receiver.recv() {
        match message {
            LiveWriterMessage::Record(record) => {
                if write_failed {
                    continue;
                }
                let output = &record.observation;
                if writer
                    .as_mut()
                    .expect("successful writer remains available")
                    .record(
                        record.sequence,
                        RecognitionArtifactTiming::Live {
                            monotonic_start_ms: record.monotonic_start_ms,
                            monotonic_end_ms: record.monotonic_end_ms,
                        },
                        output.fields(),
                        output.candidates(),
                        output.result_resolution(),
                        None,
                    )
                    .is_err()
                {
                    write_failed = true;
                }
            }
            LiveWriterMessage::Finish {
                succeeded,
                response,
            } => {
                let outcome = if write_failed {
                    drop(writer);
                    RecognitionArtifactFinishOutcome {
                        status: RecognitionArtifactFinishStatus::WriteFailed,
                        manifest_sha256: None,
                    }
                } else {
                    match writer
                        .expect("successful writer remains available")
                        .finish(succeeded)
                    {
                        Ok(digest) => RecognitionArtifactFinishOutcome {
                            status: RecognitionArtifactFinishStatus::Complete,
                            manifest_sha256: Some(digest),
                        },
                        Err(_) => RecognitionArtifactFinishOutcome {
                            status: RecognitionArtifactFinishStatus::WriteFailed,
                            manifest_sha256: None,
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

impl<'a> From<&'a ScreenCatalogCandidateObservations> for StoredCandidates<'a> {
    fn from(candidates: &'a ScreenCatalogCandidateObservations) -> Self {
        match candidates {
            ScreenCatalogCandidateObservations::Result {
                comparison_key_id,
                candidates,
                ..
            } => Self::Result {
                comparison_key_id,
                candidates,
            },
            ScreenCatalogCandidateObservations::MusicSelect {
                comparison_key_id,
                candidates,
                ..
            } => Self::MusicSelect {
                comparison_key_id,
                candidates,
            },
        }
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
        CatalogCandidateDomain, CatalogCandidateEvidenceTable, DynamicTextObservation,
        FieldNotObserved, FieldNotObservedReason, RESULT_SONG_RESOLVER_ID,
        ResultScreenFieldObservations, ResultSongResolution, ResultSongUnknownReason,
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
            candidates: Vec::new(),
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
                Some(&ResultSongResolution::Unknown {
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
        assert!(manifest.contains("\"observation_count\":1"));
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
                Some(&ResultSongResolution::Unknown {
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
        assert!(manifest.contains("\"observation_count\":1"));
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
        assert!(stored.contains("scorepeek-recognition-observation-v2"));
        assert!(stored.contains("\"source\":\"live\""));
        assert!(stored.contains("\"monotonic_start_ms\":1000"));
        assert!(stored.contains("\"monotonic_end_ms\":1017"));
        assert!(stored.contains("ABSOLUTE EVIL"));
        assert!(stored.contains("Yuta Imai"));
        assert!(stored.contains("FAILED"));
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
