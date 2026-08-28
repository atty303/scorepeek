use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::Path;
use std::time::Duration;

use scorepeek::catalog::ScorepeekSongId;
use scorepeek::recognition::{
    CanonicalFrame, CanonicalLayout, ResultSongResolution, ScreenClass, ScreenFieldObservations,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::canonical_source::{
    CanonicalFrameSource, ExtractionFrameSelection, RecordingCanonicalFrameSource,
};
use crate::diagnostic_recording::{
    DiagnosticBinding, DiagnosticCompleteness, DiagnosticPolicy, DiagnosticReplayBinding,
    DiagnosticResource, DiagnosticRunDescriptor, DiagnosticRunStatus,
};
use crate::recognition_artifact::{
    RecognitionArtifactExpected, RecognitionArtifactTiming, RecognitionArtifactWriter,
};
use crate::recognition_live::field_observer::{
    DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT, FieldObserverFinishStatus,
};
use crate::recognition_live::field_session::{
    FieldObservationSession, FieldObservationSessionPoll, FieldObservationSubmission,
};
use crate::recognition_live::screen_field_observer::{
    RegisteredScreenFieldObservation, RegisteredScreenFieldObserver,
};

type RegisteredFieldObservationSession = FieldObservationSession<RegisteredScreenFieldObserver>;

const PROFILE_SCHEMA_V1: &str = "scorepeek-recording-field-simulation-profile-v1";
const PROFILE_SCHEMA_V2: &str = "scorepeek-recording-recognition-simulation-profile-v2";
const REPORT_SCHEMA_V1: &str = "scorepeek-recording-field-simulation-report-v1";
const REPORT_SCHEMA_V2: &str = "scorepeek-recording-recognition-simulation-report-v2";
const RECORDING_SCHEMA: &str = "scorepeek-recording-v1";
const EXTRACTION_SCHEMA: &str = "scorepeek-private-canonical-frame-extraction-v1";
const COVERAGE_LABEL_SCHEMA: &str = "scorepeek-private-song-context-replay-label-v1";
const MAX_PROFILE_BYTES: u64 = 64 * 1024;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_EPISODES: usize = 64;
const MAX_EXTRACTION_FRAMES: usize = 3_600;
const REQUIRED_EXACT_CLEAR_TYPE_FRAMES: usize = 2;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingSimulationProfile {
    schema: String,
    recording: RecordingBinding,
    canonical: CanonicalBinding,
    recognition: RecognitionBinding,
    diagnostic: DiagnosticProfile,
    episodes: Vec<ResultEpisode>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RecordingBinding {
    recording_sha256: String,
    recording_bytes: u64,
    recording_manifest_sha256: String,
    capture_profile_sha256: String,
    media_probe_sha256: String,
    coverage_label_sha256: String,
    source_manifest_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CanonicalBinding {
    extraction_sha256: String,
    normalizer_sha256: String,
    canonical_layout_sha256: String,
    first_source_pts_ms: u64,
    last_source_pts_ms: u64,
    frame_count: usize,
    delivery_interval_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_field_names)]
struct RecognitionBinding {
    catalog_sha256: String,
    model_sha256: String,
    runtime_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DiagnosticProfile {
    sample_interval_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ResultEpisode {
    episode_id: String,
    label_source_pts_ms: u64,
    window_start_source_pts_ms: u64,
    window_end_source_pts_ms: u64,
    expected_clear_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_song_id: Option<ScorepeekSongId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingSimulationStatus {
    Success,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingSimulationErrorType {
    ProfileInvalid,
    SourceInvalid,
    ResourceLoadFailed,
    UnexpectedResultFrame,
    FieldSubmissionFailed,
    FieldObservationFailed,
    CandidateSetMissing,
    ClearTypeMismatch,
    SongMismatch,
    SongMissing,
    EpisodeMissing,
    FieldWorkerFinishFailed,
    DiagnosticFinishFailed,
    RecognitionArtifactFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SimulationFieldWorkerStatus {
    Complete,
    Timeout,
    WorkerUnavailable,
}

impl From<FieldObserverFinishStatus> for SimulationFieldWorkerStatus {
    fn from(status: FieldObserverFinishStatus) -> Self {
        match status {
            FieldObserverFinishStatus::Complete => Self::Complete,
            FieldObserverFinishStatus::Timeout => Self::Timeout,
            FieldObserverFinishStatus::WorkerUnavailable => Self::WorkerUnavailable,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RecordingSimulationReport {
    schema: &'static str,
    status: RecordingSimulationStatus,
    error_type: Option<RecordingSimulationErrorType>,
    profile_sha256: String,
    canonical_frames: usize,
    expected_episodes: usize,
    completed_episodes: usize,
    failed_result_episodes: usize,
    success_result_episodes: usize,
    result_frames: usize,
    submitted_field_frames: usize,
    candidate_sets: usize,
    scored_candidates: u64,
    exact_clear_type_matches: usize,
    #[serde(skip_serializing_if = "is_zero")]
    exact_song_matches: usize,
    #[serde(skip_serializing_if = "is_zero")]
    accepted_song_decisions: usize,
    #[serde(skip_serializing_if = "is_zero")]
    unknown_song_decisions: usize,
    field_worker_status: Option<SimulationFieldWorkerStatus>,
    diagnostic_completeness: Option<DiagnosticCompleteness>,
    diagnostic_manifest_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recognition_artifact_manifest_sha256: Option<String>,
    #[serde(skip)]
    include_song_decisions: bool,
}

pub fn author_recording_simulation_profile(
    candidate_path: &Path,
    expected_candidate_sha256: &str,
    recording_manifest_path: &Path,
    extraction_directory: &Path,
    coverage_label_path: &Path,
    output_path: &Path,
) -> Result<String, String> {
    let (profile, bytes) = read_profile(candidate_path, expected_candidate_sha256)?;
    validate_recording_manifest(&profile, recording_manifest_path)?;
    validate_coverage_label(&profile, coverage_label_path)?;
    let selections = load_extraction_selections(&profile, extraction_directory)?;
    validate_profile_frames(&profile, extraction_directory, &selections)?;
    publish_create_only(output_path, &bytes)?;
    Ok(expected_candidate_sha256.to_owned())
}

pub struct RecordingSimulationRunConfig<'a> {
    pub profile_path: &'a Path,
    pub expected_profile_sha256: &'a str,
    pub extraction_directory: &'a Path,
    pub diagnostic_root: &'a Path,
    pub catalog_root: &'a Path,
    pub bundle_root: &'a Path,
    pub run_id: String,
    pub build_sha256: String,
    pub policy: DiagnosticPolicy,
    pub recognition_artifact_root: Option<&'a Path>,
    pub require_song_resolution: bool,
}

#[derive(Clone, Copy, Default)]
struct EpisodeState {
    result_seen: bool,
    candidate_set_seen: bool,
    exact_clear_type_frames: usize,
    exact_song_frames: usize,
}

pub fn run_recording_simulation(
    config: RecordingSimulationRunConfig<'_>,
) -> RecordingSimulationReport {
    let profile_digest = config.expected_profile_sha256.to_owned();
    let Ok((profile, _)) = read_profile(config.profile_path, config.expected_profile_sha256) else {
        return error_report(
            profile_digest,
            RecordingSimulationErrorType::ProfileInvalid,
            config.require_song_resolution,
        );
    };
    if config.require_song_resolution && !profile.is_recognition_profile() {
        return error_report(
            profile_digest,
            RecordingSimulationErrorType::ProfileInvalid,
            true,
        );
    }
    let Ok(selections) = load_extraction_selections(&profile, config.extraction_directory) else {
        return error_report(
            profile_digest,
            RecordingSimulationErrorType::SourceInvalid,
            config.require_song_resolution,
        );
    };
    let mut recognition_artifact = match config.recognition_artifact_root {
        Some(root) => match RecognitionArtifactWriter::create(
            root,
            config.run_id.clone(),
            profile_digest.clone(),
        ) {
            Ok(writer) => Some(writer),
            Err(_) => {
                return error_report(
                    profile_digest,
                    RecordingSimulationErrorType::RecognitionArtifactFailed,
                    config.require_song_resolution,
                );
            }
        },
        None => None,
    };
    let descriptor = profile.descriptor(config.run_id, config.build_sha256, &profile_digest);
    let mut policy = config.policy;
    policy.sample_interval_ms = profile.diagnostic.sample_interval_ms;
    let recording_enabled = policy.enabled;
    let Ok(mut session) = FieldObservationSession::start_registered(
        config.diagnostic_root,
        descriptor,
        policy,
        config.catalog_root,
        config.bundle_root,
    ) else {
        return error_report(
            profile_digest,
            RecordingSimulationErrorType::ResourceLoadFailed,
            config.require_song_resolution,
        );
    };
    let mut source = RecordingCanonicalFrameSource::new(
        config.extraction_directory,
        profile.canonical.extraction_sha256.clone(),
        1,
        selections,
    );
    let mut states = vec![EpisodeState::default(); profile.episodes.len()];
    let mut report = RecordingSimulationReport::started(&profile, profile_digest);
    process_frames(
        &profile,
        &mut source,
        &mut session,
        &mut states,
        &mut report,
        &mut recognition_artifact,
    );
    finish_simulation(
        &profile,
        session,
        &states,
        report,
        recording_enabled,
        recognition_artifact,
    )
}

fn process_frames(
    profile: &RecordingSimulationProfile,
    source: &mut RecordingCanonicalFrameSource,
    session: &mut RegisteredFieldObservationSession,
    states: &mut [EpisodeState],
    report: &mut RecordingSimulationReport,
    recognition_artifact: &mut Option<RecognitionArtifactWriter>,
) {
    loop {
        let frame = match source.next_frame(Duration::from_millis(
            profile.canonical.delivery_interval_ms,
        )) {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(_) => {
                report.error_type = Some(RecordingSimulationErrorType::SourceInvalid);
                break;
            }
        };
        report.canonical_frames += 1;
        let Ok(inspected) = session.inspect(&frame) else {
            report.error_type = Some(RecordingSimulationErrorType::SourceInvalid);
            break;
        };
        let episode_index = profile.episode_at(frame.monotonic_end_ms());
        if inspected.observation.screen() == ScreenClass::Result {
            report.result_frames += 1;
            let Some(index) = episode_index else {
                report.error_type = Some(RecordingSimulationErrorType::UnexpectedResultFrame);
                break;
            };
            states[index].result_seen = true;
        }
        match inspected.field_submission {
            FieldObservationSubmission::NotApplicable => {}
            FieldObservationSubmission::Rejected(_) => {
                report.error_type = Some(RecordingSimulationErrorType::FieldSubmissionFailed);
                break;
            }
            FieldObservationSubmission::Submitted(pending) => {
                report.submitted_field_frames += 1;
                let FieldObservationSessionPoll::Ready { observation, .. } =
                    session.wait_field_observation(&pending, DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT)
                else {
                    report.error_type = Some(RecordingSimulationErrorType::FieldObservationFailed);
                    break;
                };
                let Ok(output) = observation.output() else {
                    report.error_type = Some(RecordingSimulationErrorType::FieldObservationFailed);
                    break;
                };
                if let Err(error_type) = process_completed_observation(
                    profile,
                    frame.sequence(),
                    frame.monotonic_end_ms(),
                    episode_index,
                    output,
                    states,
                    report,
                    recognition_artifact,
                ) {
                    retain_first_error(report, error_type);
                    break;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn process_completed_observation(
    profile: &RecordingSimulationProfile,
    sequence: u64,
    source_pts_ms: u64,
    episode_index: Option<usize>,
    output: &RegisteredScreenFieldObservation,
    states: &mut [EpisodeState],
    report: &mut RecordingSimulationReport,
    recognition_artifact: &mut Option<RecognitionArtifactWriter>,
) -> Result<(), RecordingSimulationErrorType> {
    let candidate_count = output.candidates().candidate_count();
    let result_resolution = output.result_resolution();
    if report.include_song_decisions
        && let Some(resolution) = result_resolution
    {
        match resolution {
            ResultSongResolution::Accepted { .. } => report.accepted_song_decisions += 1,
            ResultSongResolution::Unknown { .. } => report.unknown_song_decisions += 1,
        }
    }
    let expected = episode_index.map(|index| RecognitionArtifactExpected {
        episode_id: &profile.episodes[index].episode_id,
        song_id: profile.episodes[index].expected_song_id,
        clear_type: &profile.episodes[index].expected_clear_type,
    });
    if recognition_artifact.as_mut().is_some_and(|artifact| {
        artifact
            .record(
                sequence,
                RecognitionArtifactTiming::Recording { source_pts_ms },
                output.fields(),
                output.candidates(),
                output.song_resolution(),
                expected,
            )
            .is_err()
    }) {
        retain_first_error(
            report,
            RecordingSimulationErrorType::RecognitionArtifactFailed,
        );
        *recognition_artifact = None;
    }
    if candidate_count == 0 {
        return Err(RecordingSimulationErrorType::CandidateSetMissing);
    }
    report.candidate_sets += 1;
    report.scored_candidates = report
        .scored_candidates
        .saturating_add(u64::try_from(candidate_count).unwrap_or(u64::MAX));
    if let (Some(index), ScreenFieldObservations::Result(fields)) = (episode_index, output.fields())
    {
        update_episode(
            &profile.episodes[index],
            &mut states[index],
            fields.clear_type.open_text.as_str(),
            result_resolution,
            report,
        );
    }
    Ok(())
}

fn update_episode(
    episode: &ResultEpisode,
    state: &mut EpisodeState,
    observed_clear_type: &str,
    resolution: Option<&ResultSongResolution>,
    report: &mut RecordingSimulationReport,
) {
    state.candidate_set_seen = true;
    if observed_clear_type == episode.expected_clear_type {
        state.exact_clear_type_frames = state.exact_clear_type_frames.saturating_add(1);
    }
    if let Some(expected_song_id) = episode.expected_song_id
        && let Some(observed_song_id) = resolution.and_then(ResultSongResolution::accepted_song_id)
    {
        if observed_song_id == expected_song_id {
            state.exact_song_frames = state.exact_song_frames.saturating_add(1);
        } else {
            report
                .error_type
                .get_or_insert(RecordingSimulationErrorType::SongMismatch);
        }
    }
}

fn finish_simulation(
    profile: &RecordingSimulationProfile,
    session: RegisteredFieldObservationSession,
    states: &[EpisodeState],
    mut report: RecordingSimulationReport,
    recording_enabled: bool,
    recognition_artifact: Option<RecognitionArtifactWriter>,
) -> RecordingSimulationReport {
    report.exact_clear_type_matches = states
        .iter()
        .map(|state| state.exact_clear_type_frames)
        .sum();
    report.exact_song_matches = states.iter().map(|state| state.exact_song_frames).sum();
    report.completed_episodes = states
        .iter()
        .enumerate()
        .filter(|(index, state)| {
            state.result_seen
                && state.candidate_set_seen
                && state.exact_clear_type_frames >= REQUIRED_EXACT_CLEAR_TYPE_FRAMES
                && (profile.episodes[*index].expected_song_id.is_none()
                    || state.exact_song_frames >= REQUIRED_EXACT_CLEAR_TYPE_FRAMES)
        })
        .count();
    if report.error_type.is_none() {
        if states.iter().any(|state| !state.result_seen) {
            report.error_type = Some(RecordingSimulationErrorType::EpisodeMissing);
        } else if states
            .iter()
            .any(|state| state.exact_clear_type_frames < REQUIRED_EXACT_CLEAR_TYPE_FRAMES)
        {
            report.error_type = Some(RecordingSimulationErrorType::ClearTypeMismatch);
        } else if states.iter().enumerate().any(|(index, state)| {
            profile.episodes[index].expected_song_id.is_some()
                && state.exact_song_frames < REQUIRED_EXACT_CLEAR_TYPE_FRAMES
        }) {
            report.error_type = Some(RecordingSimulationErrorType::SongMissing);
        } else if report.completed_episodes != report.expected_episodes {
            report.error_type = Some(RecordingSimulationErrorType::CandidateSetMissing);
        }
    }
    let run_status = if report.error_type.is_none() {
        DiagnosticRunStatus::Success
    } else {
        DiagnosticRunStatus::Error
    };
    let finish = session.finish(
        run_status,
        profile.canonical.last_source_pts_ms,
        DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT,
    );
    report.field_worker_status = Some(finish.field_observer.status.into());
    report.diagnostic_completeness = finish.diagnostic.completeness;
    report.diagnostic_manifest_sha256 = finish.diagnostic.manifest_sha256;
    if finish.field_observer.status != FieldObserverFinishStatus::Complete {
        report
            .error_type
            .get_or_insert(RecordingSimulationErrorType::FieldWorkerFinishFailed);
    }
    if recording_enabled
        && (report.diagnostic_completeness != Some(DiagnosticCompleteness::Complete)
            || finish.diagnostic.error_type.is_some()
            || report.diagnostic_manifest_sha256.is_none())
    {
        report
            .error_type
            .get_or_insert(RecordingSimulationErrorType::DiagnosticFinishFailed);
    }
    if let Some(artifact) = recognition_artifact {
        match artifact.finish(report.error_type.is_none()) {
            Ok(digest) => report.recognition_artifact_manifest_sha256 = Some(digest),
            Err(_) => {
                report
                    .error_type
                    .get_or_insert(RecordingSimulationErrorType::RecognitionArtifactFailed);
            }
        }
    }
    if report.error_type.is_some() {
        report.status = RecordingSimulationStatus::Error;
    }
    report
}

impl RecordingSimulationProfile {
    fn descriptor(
        &self,
        run_id: String,
        build_sha256: String,
        profile_sha256: &str,
    ) -> DiagnosticRunDescriptor {
        DiagnosticRunDescriptor {
            run_id,
            monotonic_start_ms: self.canonical.first_source_pts_ms,
            resource: DiagnosticResource {
                program: "scorepeek",
                version: env!("CARGO_PKG_VERSION"),
                build_sha256,
            },
            binding: DiagnosticBinding {
                capture_generation: 1,
                capture_profile_sha256: self.recording.capture_profile_sha256.clone(),
                normalizer_sha256: self.canonical.normalizer_sha256.clone(),
                canonical_layout_sha256: self.canonical.canonical_layout_sha256.clone(),
                catalog_sha256: self.recognition.catalog_sha256.clone(),
                model_sha256: self.recognition.model_sha256.clone(),
                runtime_sha256: self.recognition.runtime_sha256.clone(),
                replay: Some(DiagnosticReplayBinding {
                    request_sha256: profile_sha256.to_owned(),
                    extraction_sha256: self.canonical.extraction_sha256.clone(),
                }),
            },
        }
    }

    fn episode_at(&self, source_pts_ms: u64) -> Option<usize> {
        self.episodes.iter().position(|episode| {
            (episode.window_start_source_pts_ms..=episode.window_end_source_pts_ms)
                .contains(&source_pts_ms)
        })
    }

    fn validate(&self) -> bool {
        let mut episode_ids = BTreeSet::new();
        ((self.schema == PROFILE_SCHEMA_V1
            && self
                .episodes
                .iter()
                .all(|episode| episode.expected_song_id.is_none()))
            || self.is_recognition_profile())
            && self.recording.recording_bytes > 0
            && self.canonical.frame_count > 0
            && self.canonical.frame_count <= MAX_EXTRACTION_FRAMES
            && (1..=1_000).contains(&self.canonical.delivery_interval_ms)
            && self.canonical.first_source_pts_ms <= self.canonical.last_source_pts_ms
            && (1_000..=60_000).contains(&self.diagnostic.sample_interval_ms)
            && self.diagnostic.sample_interval_ms.is_multiple_of(1_000)
            && self.episodes.len() >= 3
            && self.episodes.len() <= MAX_EPISODES
            && self.canonical.canonical_layout_sha256 == CanonicalLayout::sha256()
            && [
                &self.recording.recording_sha256,
                &self.recording.recording_manifest_sha256,
                &self.recording.source_manifest_sha256,
                &self.recording.capture_profile_sha256,
                &self.recording.media_probe_sha256,
                &self.recording.coverage_label_sha256,
                &self.canonical.extraction_sha256,
                &self.canonical.normalizer_sha256,
                &self.canonical.canonical_layout_sha256,
                &self.recognition.catalog_sha256,
                &self.recognition.model_sha256,
                &self.recognition.runtime_sha256,
            ]
            .into_iter()
            .all(|digest| valid_sha256(digest))
            && self.episodes.iter().enumerate().all(|(index, episode)| {
                valid_token(&episode.episode_id)
                    && episode_ids.insert(episode.episode_id.as_str())
                    && episode.window_start_source_pts_ms <= episode.label_source_pts_ms
                    && episode.label_source_pts_ms <= episode.window_end_source_pts_ms
                    && episode.window_start_source_pts_ms >= self.canonical.first_source_pts_ms
                    && episode.window_end_source_pts_ms <= self.canonical.last_source_pts_ms
                    && valid_clear_type(&episode.expected_clear_type)
                    && index.checked_sub(1).is_none_or(|previous| {
                        self.episodes[previous].window_end_source_pts_ms
                            < episode.window_start_source_pts_ms
                    })
            })
            && self
                .episodes
                .iter()
                .any(|episode| episode.expected_clear_type == "FAILED")
            && self
                .episodes
                .iter()
                .any(|episode| episode.expected_clear_type != "FAILED")
    }

    fn is_recognition_profile(&self) -> bool {
        self.schema == PROFILE_SCHEMA_V2
            && self
                .episodes
                .iter()
                .all(|episode| episode.expected_song_id.is_some())
    }
}

#[derive(Deserialize)]
struct RecordingManifest {
    schema: String,
    recording_sha256: String,
    recording_bytes: u64,
    source_manifest_sha256: String,
    capture_profile_sha256: String,
    media_probe_sha256: String,
}

#[derive(Deserialize)]
struct ExtractionManifest {
    schema: String,
    source_manifest_sha256: String,
    media_probe_sha256: String,
    capture_profile_id: String,
    normalizer_artifact_sha256: String,
    canonical_frame_contract_id: String,
    source_time_base: SourceTimeBase,
    frames: Vec<ExtractionManifestFrame>,
}

#[derive(Deserialize)]
struct SourceTimeBase {
    numerator: u64,
    denominator: u64,
}

#[derive(Deserialize)]
struct ExtractionManifestFrame {
    frame_id: String,
    source_pts: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CoverageRecording {
    source_sha256: String,
    source_bytes: u64,
    capture_profile_sha256: String,
    media_probe_sha256: String,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CoverageObservation {
    Neutral {
        source_pts_ms: u64,
        detail: String,
    },
    Clear {
        source_pts_ms: u64,
        reason: String,
    },
    StableSelection {
        source_pts_ms: u64,
        song_token: String,
    },
    Preserve {
        source_pts_ms: u64,
        detail: String,
    },
    Result {
        source_pts_ms: u64,
        song_token: String,
    },
}

impl CoverageObservation {
    fn is_valid(&self, maximum_source_pts_ms: u64) -> bool {
        match self {
            Self::Neutral {
                source_pts_ms,
                detail,
            }
            | Self::Preserve {
                source_pts_ms,
                detail,
            } => {
                *source_pts_ms <= maximum_source_pts_ms
                    && !detail.is_empty()
                    && detail.len() <= 1_024
            }
            Self::Clear {
                source_pts_ms,
                reason,
            } => {
                *source_pts_ms <= maximum_source_pts_ms
                    && !reason.is_empty()
                    && reason.len() <= 1_024
            }
            Self::StableSelection {
                source_pts_ms,
                song_token,
            }
            | Self::Result {
                source_pts_ms,
                song_token,
            } => *source_pts_ms <= maximum_source_pts_ms && valid_token(song_token),
        }
    }

    const fn result_source_pts_ms(&self) -> Option<u64> {
        if let Self::Result { source_pts_ms, .. } = self {
            Some(*source_pts_ms)
        } else {
            None
        }
    }
}

fn read_profile(
    path: &Path,
    expected_sha256: &str,
) -> Result<(RecordingSimulationProfile, Vec<u8>), String> {
    let bytes = read_bounded_regular(path, MAX_PROFILE_BYTES, "recording simulation profile")?;
    if !valid_sha256(expected_sha256) || digest_bytes(&bytes) != expected_sha256 {
        return Err("recording simulation profile digest mismatch".to_owned());
    }
    let profile: RecordingSimulationProfile = serde_json::from_slice(&bytes)
        .map_err(|_| "recording simulation profile schema is invalid".to_owned())?;
    if !profile.validate() || canonical_json(&profile)? != bytes {
        return Err("recording simulation profile contract is invalid".to_owned());
    }
    Ok((profile, bytes))
}

fn validate_recording_manifest(
    profile: &RecordingSimulationProfile,
    path: &Path,
) -> Result<(), String> {
    let bytes = read_bounded_regular(path, MAX_PROFILE_BYTES, "recording manifest")?;
    if digest_bytes(&bytes) != profile.recording.recording_manifest_sha256 {
        return Err("recording manifest digest mismatch".to_owned());
    }
    let manifest: RecordingManifest = serde_json::from_slice(&bytes)
        .map_err(|_| "recording manifest schema is invalid".to_owned())?;
    if manifest.schema != RECORDING_SCHEMA
        || manifest.recording_sha256 != profile.recording.recording_sha256
        || manifest.recording_bytes != profile.recording.recording_bytes
        || manifest.source_manifest_sha256 != profile.recording.source_manifest_sha256
        || manifest.capture_profile_sha256 != profile.recording.capture_profile_sha256
        || manifest.media_probe_sha256 != profile.recording.media_probe_sha256
    {
        return Err("recording manifest binding mismatch".to_owned());
    }
    Ok(())
}

fn load_extraction_selections(
    profile: &RecordingSimulationProfile,
    directory: &Path,
) -> Result<Vec<ExtractionFrameSelection>, String> {
    let bytes = read_bounded_regular(
        &directory.join("manifest.json"),
        MAX_MANIFEST_BYTES,
        "canonical extraction manifest",
    )?;
    if digest_bytes(&bytes) != profile.canonical.extraction_sha256 {
        return Err("canonical extraction manifest digest mismatch".to_owned());
    }
    let manifest: ExtractionManifest = serde_json::from_slice(&bytes)
        .map_err(|_| "canonical extraction manifest schema is invalid".to_owned())?;
    if manifest.schema != EXTRACTION_SCHEMA
        || manifest.source_manifest_sha256 != profile.recording.source_manifest_sha256
        || manifest.media_probe_sha256 != profile.recording.media_probe_sha256
        || manifest.capture_profile_id != profile.recording.capture_profile_sha256
        || manifest.normalizer_artifact_sha256 != profile.canonical.normalizer_sha256
        || manifest.canonical_frame_contract_id != "scorepeek-canonical-rgb8-1920x1080-v1"
        || manifest.source_time_base.numerator != 1
        || manifest.source_time_base.denominator != 1_000
        || manifest.frames.len() != profile.canonical.frame_count
        || manifest.frames.first().map(|frame| frame.source_pts)
            != Some(profile.canonical.first_source_pts_ms)
        || manifest.frames.last().map(|frame| frame.source_pts)
            != Some(profile.canonical.last_source_pts_ms)
    {
        return Err("canonical extraction manifest binding mismatch".to_owned());
    }
    let mut frame_ids = BTreeSet::new();
    let mut previous = None;
    manifest
        .frames
        .into_iter()
        .enumerate()
        .map(|(index, frame)| {
            if !valid_token(&frame.frame_id)
                || !frame_ids.insert(frame.frame_id.clone())
                || previous.is_some_and(|value| value >= frame.source_pts)
            {
                return Err("canonical extraction frame sequence is invalid".to_owned());
            }
            previous = Some(frame.source_pts);
            Ok(ExtractionFrameSelection {
                sequence: u64::try_from(index + 1)
                    .map_err(|_| "canonical extraction is too large".to_owned())?,
                frame_id: frame.frame_id,
                source_pts_ms: frame.source_pts,
            })
        })
        .collect()
}

fn validate_profile_frames(
    profile: &RecordingSimulationProfile,
    extraction_directory: &Path,
    selections: &[ExtractionFrameSelection],
) -> Result<(), String> {
    let mut result_seen = vec![false; profile.episodes.len()];
    for selection in selections {
        let frame = CanonicalFrame::read_extraction(
            extraction_directory,
            &selection.frame_id,
            &profile.canonical.extraction_sha256,
        )
        .map_err(|_| "canonical extraction frame is invalid")?;
        if frame.capture_profile_id() != profile.recording.capture_profile_sha256
            || frame.normalizer_artifact_sha256() != profile.canonical.normalizer_sha256
            || u64::try_from(frame.source_pts_ms()).ok() != Some(selection.source_pts_ms)
        {
            return Err("canonical extraction frame binding mismatch".to_owned());
        }
        let screen = scorepeek::recognition::inspect_canonical_rgb8(frame.pixels())
            .map_err(|_| "canonical extraction frame is invalid")?
            .screen;
        if screen == ScreenClass::Result {
            let Some(index) = profile.episode_at(selection.source_pts_ms) else {
                return Err("result frame is outside every expected episode".to_owned());
            };
            result_seen[index] = true;
        }
    }
    if result_seen.into_iter().any(|seen| !seen) {
        return Err("expected result episode has no classified frame".to_owned());
    }
    Ok(())
}

fn validate_coverage_label(
    profile: &RecordingSimulationProfile,
    label_path: &Path,
) -> Result<(), String> {
    let bytes = read_bounded_regular(label_path, MAX_MANIFEST_BYTES, "coverage label")?;
    if digest_bytes(&bytes) != profile.recording.coverage_label_sha256 {
        return Err("coverage label digest mismatch".to_owned());
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| "coverage label schema is invalid".to_owned())?;
    validate_coverage_label_value(profile, &value)
}

fn validate_coverage_label_value(
    profile: &RecordingSimulationProfile,
    value: &serde_json::Value,
) -> Result<(), String> {
    let Some(object) = value.as_object() else {
        return Err("coverage label schema is invalid".to_owned());
    };
    let expected_fields = [
        "schema",
        "recording",
        "coverage",
        "observations",
        "not_observed",
        "operator_facts_not_established_by_this_recording",
        "operator_review",
        "review_status",
        "supersedes_private_label_sha256",
    ];
    if object.len() != expected_fields.len()
        || expected_fields
            .iter()
            .any(|field| !object.contains_key(*field))
        || object.get("schema").and_then(serde_json::Value::as_str) != Some(COVERAGE_LABEL_SCHEMA)
    {
        return Err("coverage label schema is invalid".to_owned());
    }
    let recording: CoverageRecording = serde_json::from_value(
        object
            .get("recording")
            .cloned()
            .ok_or_else(|| "coverage label schema is invalid".to_owned())?,
    )
    .map_err(|_| "coverage label schema is invalid".to_owned())?;
    if recording.source_sha256 != profile.recording.recording_sha256
        || recording.source_bytes != profile.recording.recording_bytes
        || recording.capture_profile_sha256 != profile.recording.capture_profile_sha256
        || recording.media_probe_sha256 != profile.recording.media_probe_sha256
    {
        return Err("coverage label binding mismatch".to_owned());
    }
    let observations = object
        .get("observations")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "coverage label schema is invalid".to_owned())?;
    let mut labeled_results = BTreeSet::new();
    for observation in observations {
        let observation: CoverageObservation = serde_json::from_value(observation.clone())
            .map_err(|_| "coverage label observation is invalid".to_owned())?;
        if !observation.is_valid(profile.canonical.last_source_pts_ms) {
            return Err("coverage label observation is invalid".to_owned());
        }
        if let Some(source_pts_ms) = observation.result_source_pts_ms()
            && !labeled_results.insert(source_pts_ms)
        {
            return Err("coverage label contains duplicate result timestamps".to_owned());
        }
    }
    let expected_results: BTreeSet<_> = profile
        .episodes
        .iter()
        .map(|episode| episode.label_source_pts_ms)
        .collect();
    if labeled_results != expected_results {
        return Err("coverage label result set does not match the profile".to_owned());
    }
    Ok(())
}

impl RecordingSimulationReport {
    fn started(profile: &RecordingSimulationProfile, profile_sha256: String) -> Self {
        let include_song_decisions = profile.is_recognition_profile();
        Self {
            schema: if include_song_decisions {
                REPORT_SCHEMA_V2
            } else {
                REPORT_SCHEMA_V1
            },
            status: RecordingSimulationStatus::Success,
            error_type: None,
            profile_sha256,
            canonical_frames: 0,
            expected_episodes: profile.episodes.len(),
            completed_episodes: 0,
            failed_result_episodes: profile
                .episodes
                .iter()
                .filter(|episode| episode.expected_clear_type == "FAILED")
                .count(),
            success_result_episodes: profile
                .episodes
                .iter()
                .filter(|episode| episode.expected_clear_type != "FAILED")
                .count(),
            result_frames: 0,
            submitted_field_frames: 0,
            candidate_sets: 0,
            scored_candidates: 0,
            exact_clear_type_matches: 0,
            exact_song_matches: 0,
            accepted_song_decisions: 0,
            unknown_song_decisions: 0,
            field_worker_status: None,
            diagnostic_completeness: None,
            diagnostic_manifest_sha256: None,
            recognition_artifact_manifest_sha256: None,
            include_song_decisions,
        }
    }

    #[must_use]
    pub const fn succeeded(&self) -> bool {
        matches!(self.status, RecordingSimulationStatus::Success)
    }
}

fn retain_first_error(
    report: &mut RecordingSimulationReport,
    error_type: RecordingSimulationErrorType,
) {
    report.error_type.get_or_insert(error_type);
}

fn error_report(
    profile_sha256: String,
    error_type: RecordingSimulationErrorType,
    include_song_decisions: bool,
) -> RecordingSimulationReport {
    RecordingSimulationReport {
        schema: if include_song_decisions {
            REPORT_SCHEMA_V2
        } else {
            REPORT_SCHEMA_V1
        },
        status: RecordingSimulationStatus::Error,
        error_type: Some(error_type),
        profile_sha256,
        canonical_frames: 0,
        expected_episodes: 0,
        completed_episodes: 0,
        failed_result_episodes: 0,
        success_result_episodes: 0,
        result_frames: 0,
        submitted_field_frames: 0,
        candidate_sets: 0,
        scored_candidates: 0,
        exact_clear_type_matches: 0,
        exact_song_matches: 0,
        accepted_song_decisions: 0,
        unknown_song_decisions: 0,
        field_worker_status: None,
        diagnostic_completeness: None,
        diagnostic_manifest_sha256: None,
        recognition_artifact_manifest_sha256: None,
        include_song_decisions,
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_zero(value: &usize) -> bool {
    *value == 0
}

fn publish_create_only(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "profile output parent is invalid".to_owned())?;
    let parent_metadata = parent
        .metadata()
        .map_err(|_| "profile output parent is unavailable".to_owned())?;
    if !path.is_absolute() || !parent_metadata.is_dir() {
        return Err("profile output path is invalid".to_owned());
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| "profile output already exists or is unavailable".to_owned())?;
    let publication = output
        .write_all(bytes)
        .and_then(|()| output.sync_all())
        .and_then(|()| File::open(parent)?.sync_all());
    if publication.is_err() {
        drop(output);
        let cleanup = fs::remove_file(path).and_then(|()| File::open(parent)?.sync_all());
        return if cleanup.is_ok() {
            Err("profile publication failed".to_owned())
        } else {
            Err("profile publication and cleanup failed".to_owned())
        };
    }
    Ok(())
}

fn read_bounded_regular(path: &Path, maximum: u64, name: &str) -> Result<Vec<u8>, String> {
    let before = path
        .metadata()
        .map_err(|_| format!("{name} is unavailable"))?;
    if !path.is_absolute() || !before.is_file() || before.len() == 0 || before.len() > maximum {
        return Err(format!("{name} is invalid"));
    }
    let mut file = File::open(path).map_err(|_| format!("{name} read failed"))?;
    let opened = file.metadata().map_err(|_| format!("{name} read failed"))?;
    if opened.dev() != before.dev() || opened.ino() != before.ino() {
        return Err(format!("{name} changed while opening"));
    }
    let capacity = usize::try_from(before.len()).map_err(|_| format!("{name} is invalid"))?;
    let mut bytes = Vec::with_capacity(capacity);
    (&mut file)
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| format!("{name} read failed"))?;
    let after = file.metadata().map_err(|_| format!("{name} read failed"))?;
    if u64::try_from(bytes.len()).ok() != Some(before.len())
        || after.dev() != before.dev()
        || after.ino() != before.ino()
        || after.len() != before.len()
        || after.mtime() != before.mtime()
        || after.mtime_nsec() != before.mtime_nsec()
    {
        return Err(format!("{name} changed while reading"));
    }
    Ok(bytes)
}

fn canonical_json(value: &impl Serialize) -> Result<Vec<u8>, String> {
    let mut bytes =
        serde_json::to_vec(value).map_err(|_| "profile serialization failed".to_owned())?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn digest_bytes(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        use std::fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

fn valid_clear_type(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && !value.starts_with(' ')
        && !value.ends_with(' ')
        && !value.contains("  ")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte == b' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> RecordingSimulationProfile {
        RecordingSimulationProfile {
            schema: PROFILE_SCHEMA_V1.to_owned(),
            recording: RecordingBinding {
                recording_sha256: "1".repeat(64),
                recording_bytes: 1,
                recording_manifest_sha256: "2".repeat(64),
                capture_profile_sha256: "3".repeat(64),
                media_probe_sha256: "4".repeat(64),
                coverage_label_sha256: "5".repeat(64),
                source_manifest_sha256: "b".repeat(64),
            },
            canonical: CanonicalBinding {
                extraction_sha256: "6".repeat(64),
                normalizer_sha256: "7".repeat(64),
                canonical_layout_sha256: CanonicalLayout::sha256(),
                first_source_pts_ms: 0,
                last_source_pts_ms: 30_000,
                frame_count: 31,
                delivery_interval_ms: 250,
            },
            recognition: RecognitionBinding {
                catalog_sha256: "8".repeat(64),
                model_sha256: "9".repeat(64),
                runtime_sha256: "a".repeat(64),
            },
            diagnostic: DiagnosticProfile {
                sample_interval_ms: 5_000,
            },
            episodes: vec![
                ResultEpisode {
                    episode_id: "failed-1".to_owned(),
                    label_source_pts_ms: 5_000,
                    window_start_source_pts_ms: 4_000,
                    window_end_source_pts_ms: 6_000,
                    expected_clear_type: "FAILED".to_owned(),
                    expected_song_id: None,
                },
                ResultEpisode {
                    episode_id: "failed-2".to_owned(),
                    label_source_pts_ms: 15_000,
                    window_start_source_pts_ms: 14_000,
                    window_end_source_pts_ms: 16_000,
                    expected_clear_type: "FAILED".to_owned(),
                    expected_song_id: None,
                },
                ResultEpisode {
                    episode_id: "success-1".to_owned(),
                    label_source_pts_ms: 25_000,
                    window_start_source_pts_ms: 24_000,
                    window_end_source_pts_ms: 26_000,
                    expected_clear_type: "CLEAR".to_owned(),
                    expected_song_id: None,
                },
            ],
        }
    }

    fn coverage_label(profile: &RecordingSimulationProfile) -> serde_json::Value {
        serde_json::json!({
            "schema": COVERAGE_LABEL_SCHEMA,
            "recording": {
                "source_sha256": profile.recording.recording_sha256,
                "source_bytes": profile.recording.recording_bytes,
                "capture_profile_sha256": profile.recording.capture_profile_sha256,
                "media_probe_sha256": profile.recording.media_probe_sha256,
            },
            "coverage": {},
            "observations": [
                {"kind": "result", "source_pts_ms": 5_000, "song_token": "song_a"},
                {"kind": "result", "source_pts_ms": 15_000, "song_token": "song_b"},
                {"kind": "result", "source_pts_ms": 25_000, "song_token": "song_a"},
            ],
            "not_observed": [],
            "operator_facts_not_established_by_this_recording": [],
            "operator_review": {},
            "review_status": "complete",
            "supersedes_private_label_sha256": "0".repeat(64),
        })
    }

    #[test]
    fn dedicated_profile_binds_pacing_sampling_and_exact_clear_types() {
        let mut profile = profile();
        assert!(profile.validate());
        let bytes = canonical_json(&profile).unwrap();
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert!(
            String::from_utf8(bytes)
                .unwrap()
                .contains("\"expected_clear_type\":\"FAILED\"")
        );

        profile.episodes[2].expected_clear_type = "EASY CLEAR".to_owned();
        assert!(profile.validate());
    }

    #[test]
    fn recognition_profile_requires_an_expected_song_for_every_episode() {
        let mut profile = profile();
        profile.schema = PROFILE_SCHEMA_V2.to_owned();
        for (index, episode) in profile.episodes.iter_mut().enumerate() {
            episode.expected_song_id =
                Some(serde_json::from_str(&format!("\"{:032x}\"", index + 1)).unwrap());
        }
        assert!(profile.validate());
        assert!(profile.is_recognition_profile());

        profile.episodes[1].expected_song_id = None;
        assert!(!profile.validate());
        assert!(!profile.is_recognition_profile());
    }

    #[test]
    fn recognition_report_uses_v2_without_changing_the_v1_field_report() {
        let field_profile = profile();
        let field_report = serde_json::to_value(RecordingSimulationReport::started(
            &field_profile,
            "a".repeat(64),
        ))
        .unwrap();
        assert_eq!(field_report["schema"], REPORT_SCHEMA_V1);
        assert!(field_report.get("exact_song_matches").is_none());

        let mut recognition_profile = field_profile;
        recognition_profile.schema = PROFILE_SCHEMA_V2.to_owned();
        for (index, episode) in recognition_profile.episodes.iter_mut().enumerate() {
            episode.expected_song_id =
                Some(serde_json::from_str(&format!("\"{:032x}\"", index + 1)).unwrap());
        }
        let mut recognition_report =
            RecordingSimulationReport::started(&recognition_profile, "b".repeat(64));
        recognition_report.exact_song_matches = 2;
        let recognition_report = serde_json::to_value(recognition_report).unwrap();
        assert_eq!(recognition_report["schema"], REPORT_SCHEMA_V2);
        assert_eq!(recognition_report["exact_song_matches"], 2);
    }

    #[test]
    fn overlapping_episodes_and_unbounded_execution_settings_fail_closed() {
        let mut overlapping = profile();
        overlapping.episodes[1].window_start_source_pts_ms = 6_000;
        assert!(!overlapping.validate());

        let mut pacing = profile();
        pacing.canonical.delivery_interval_ms = 0;
        assert!(!pacing.validate());

        let mut sampling = profile();
        sampling.diagnostic.sample_interval_ms = 1_500;
        assert!(!sampling.validate());

        let mut clear_type = profile();
        clear_type.episodes[2].expected_clear_type = "clear".to_owned();
        assert!(!clear_type.validate());

        let mut source_manifest = profile();
        source_manifest.recording.source_manifest_sha256 = "not-a-digest".to_owned();
        assert!(!source_manifest.validate());
    }

    #[test]
    fn coverage_label_result_set_is_strict_and_complete() {
        let profile = profile();
        let value = coverage_label(&profile);
        assert!(validate_coverage_label_value(&profile, &value).is_ok());

        let mut extra = value.clone();
        extra["observations"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "kind": "result",
                "source_pts_ms": 20_000,
                "song_token": "song_c",
            }));
        assert!(validate_coverage_label_value(&profile, &extra).is_err());

        let mut malformed = value.clone();
        malformed["observations"][0]["unexpected"] = serde_json::json!(true);
        assert!(validate_coverage_label_value(&profile, &malformed).is_err());

        let mut duplicate = value;
        duplicate["observations"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "kind": "result",
                "source_pts_ms": 5_000,
                "song_token": "song_a",
            }));
        assert!(validate_coverage_label_value(&profile, &duplicate).is_err());
    }

    #[test]
    fn bounded_reader_rejects_an_oversized_regular_file() {
        let directory = std::env::temp_dir().join(format!(
            "scorepeek-recording-simulation-reader-test-{}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("oversized.json");
        fs::write(
            &path,
            vec![b'x'; usize::try_from(MAX_PROFILE_BYTES + 1).unwrap()],
        )
        .unwrap();
        assert!(read_bounded_regular(&path, MAX_PROFILE_BYTES, "test file").is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn later_candidate_failure_does_not_hide_artifact_failure() {
        let mut report = RecordingSimulationReport::started(&profile(), "c".repeat(64));
        retain_first_error(
            &mut report,
            RecordingSimulationErrorType::RecognitionArtifactFailed,
        );
        retain_first_error(
            &mut report,
            RecordingSimulationErrorType::CandidateSetMissing,
        );
        assert_eq!(
            report.error_type,
            Some(RecordingSimulationErrorType::RecognitionArtifactFailed)
        );
    }
}
