//! Semantic replay from verified canonical input, without runtime diagnostic artifacts.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use scorepeek_core::canonical_recording::{CanonicalTick, TickDisposition};
use scorepeek_core::event::coordinator::{CoordinatorError, CoordinatorPolicy, DomainCoordinator};
use scorepeek_core::event::{
    RUN_EVENT_SCHEMA, RunEvent, RunEventKind, RunReducerEffect, run_event_from_field_observation,
};
use scorepeek_core::recognition::screen::{
    ScreenClass, ScreenPredicateObservation, TitleConfirmationState, confirm_title_screen,
    inspect_canonical_rgb8, route_screen_rgb8_crops,
};
use scorepeek_core::session::episode::RawScreenState;
use scorepeek_core::session::timeline::{TimelineAction, TimelineDriver};

use crate::canonical::{self, RecordingError};
use crate::resources;
use crate::store::{self, ExpectedTransition, LabelDisposition, RegressionLabel, StoreError};
use scorepeek_core::recognition::registered_field::{PendingFieldRecognition, RegisteredFieldPool};

const FRAME_BYTES: usize = 1920 * 1080 * 3;

#[derive(Debug)]
pub enum ReplayError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Recording(RecordingError),
    Store(StoreError),
    Coordinator(CoordinatorError),
    Resource(String),
    Field(String),
    Oracle(String),
    Invalid(&'static str),
}
impl From<std::io::Error> for ReplayError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<serde_json::Error> for ReplayError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}
impl From<RecordingError> for ReplayError {
    fn from(value: RecordingError) -> Self {
        Self::Recording(value)
    }
}
impl From<StoreError> for ReplayError {
    fn from(value: StoreError) -> Self {
        Self::Store(value)
    }
}
impl From<CoordinatorError> for ReplayError {
    fn from(value: CoordinatorError) -> Self {
        Self::Coordinator(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct ReplayReport {
    pub inputs: u64,
    pub retained_frames: u64,
    pub elided_inputs: u64,
    pub transitions: Vec<ExpectedTransition>,
    pub domain_outputs: u64,
    pub domain_event_count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplayPhase {
    Validating,
    Processing,
    Complete,
}

#[derive(Clone, Debug)]
pub struct ReplayProgress {
    pub recording: std::path::PathBuf,
    pub phase: ReplayPhase,
    pub processed_inputs: u64,
    pub total_inputs: u64,
    pub retained_frames: u64,
    pub total_retained_frames: u64,
    pub segments_seen: usize,
    pub total_segments: usize,
    pub elapsed: Duration,
}

pub(crate) trait ReplayObserver {
    fn validate(&mut self, _ticks: &[CanonicalTick]) -> Result<(), ReplayError> {
        Ok(())
    }
    fn screen(&mut self, _sequence: u64, _screen: ScreenClass) -> Result<(), ReplayError> {
        Ok(())
    }
    fn event(&mut self, _event: &RunEvent) -> Result<(), ReplayError> {
        Ok(())
    }
}

impl ReplayObserver for () {}

fn consume_outputs(
    report: &mut ReplayReport,
    outputs: &[RunReducerEffect],
    observer: &mut dyn ReplayObserver,
) -> Result<(), ReplayError> {
    report.domain_outputs += outputs.len() as u64;
    for output in outputs {
        if let RunReducerEffect::Event(event) = output {
            observer.event(event)?;
            report.domain_event_count += 1;
        }
    }
    Ok(())
}

struct SegmentDecoder {
    child: Child,
    stdout: ChildStdout,
    remaining: u64,
}
impl SegmentDecoder {
    fn start(path: &Path, frames: u64) -> Result<Self, ReplayError> {
        let mut child = Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(path)
            .args(["-map", "0:v:0", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or(ReplayError::Invalid("decoder stdout is unavailable"))?;
        Ok(Self {
            child,
            stdout,
            remaining: frames,
        })
    }
    fn frame(&mut self) -> Result<Vec<u8>, ReplayError> {
        if self.remaining == 0 {
            return Err(ReplayError::Invalid("segment frame count exhausted"));
        }
        let mut pixels = vec![0_u8; FRAME_BYTES];
        self.stdout.read_exact(&mut pixels)?;
        self.remaining -= 1;
        Ok(pixels)
    }
    fn finish(mut self) -> Result<(), ReplayError> {
        if self.remaining != 0 {
            return Err(ReplayError::Invalid("segment frame count incomplete"));
        }
        let mut extra = [0_u8; 1];
        if self.stdout.read(&mut extra)? != 0 {
            return Err(ReplayError::Invalid(
                "segment contains extra decoded frames",
            ));
        }
        if !self.child.wait()?.success() {
            return Err(ReplayError::Invalid("segment decoder failed"));
        }
        Ok(())
    }
}
impl Drop for SegmentDecoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

const PREPROCESS_BATCH: usize = 8;
type InspectedFrame = (Vec<u8>, ScreenPredicateObservation);

struct PreparedInput {
    observed: Option<ScreenPredicateObservation>,
    field_response: Option<PendingFieldRecognition>,
}

fn inspect_batch(frames: Vec<Option<Vec<u8>>>) -> Result<Vec<Option<InspectedFrame>>, ReplayError> {
    thread::scope(|scope| {
        let pending = frames
            .into_iter()
            .map(|frame| {
                frame.map(|pixels| {
                    scope.spawn(move || {
                        let observation = inspect_canonical_rgb8(&pixels).map_err(|_| {
                            ReplayError::Invalid("canonical frame inspection failed")
                        })?;
                        Ok((pixels, observation))
                    })
                })
            })
            .collect::<Vec<_>>();
        pending
            .into_iter()
            .map(|job| {
                job.map(|job| {
                    job.join()
                        .map_err(|_| ReplayError::Invalid("canonical frame inspector panicked"))?
                })
                .transpose()
            })
            .collect()
    })
}

fn screen_name(screen: ScreenClass) -> &'static str {
    match screen {
        ScreenClass::Title => "title",
        ScreenClass::Result => "result",
        ScreenClass::MusicSelect => "music_select",
        ScreenClass::ModeSelect => "mode_select",
        ScreenClass::DecideTransition => "decide_transition",
        ScreenClass::Play => "play",
        ScreenClass::Unknown => "unknown",
    }
}

fn raw_input(
    tick: &CanonicalTick,
    screen: ScreenClass,
    semantic_episode_id: Option<u64>,
    observed: Option<&ScreenPredicateObservation>,
    session_id: &str,
) -> RunEvent {
    let kind = RunEventKind::RawScreenObserved {
        session_id: Some(session_id.to_owned()),
        capture_generation: Some(0),
        semantic_episode_id,
        sequence: tick.sequence,
        monotonic_start_ms: tick.source_timestamp_ms,
        monotonic_end_ms: tick.source_timestamp_ms,
        screen: screen_name(screen).into(),
        result_presence: observed.map(|value| value.result_presence),
        play_presence: observed.map(|value| value.play_presence),
        unknown_reason: (screen == ScreenClass::Unknown).then(|| "predicate_not_matched".into()),
    };
    RunEvent {
        schema: RUN_EVENT_SCHEMA.into(),
        kind,
    }
}

fn apply_event(
    coordinator: &mut DomainCoordinator,
    next_sequence: &mut u64,
    report: &mut ReplayReport,
    kind: RunEventKind,
    observer: &mut dyn ReplayObserver,
) -> Result<(), ReplayError> {
    let event = RunEvent {
        schema: RUN_EVENT_SCHEMA.into(),
        kind,
    };
    let output = coordinator.step(*next_sequence, &event)?;
    consume_outputs(report, output.effects(), observer)?;
    *next_sequence += 1;
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "the ordered timeline action retains explicit coordinator and sequence state"
)]
fn apply_timeline_actions(
    coordinator: &mut DomainCoordinator,
    next_sequence: &mut u64,
    report: &mut ReplayReport,
    actions: Vec<TimelineAction>,
    sequence: u64,
    timestamp_ms: u64,
    session_id: &str,
    observer: &mut dyn ReplayObserver,
) -> Result<(), ReplayError> {
    for action in actions {
        if let TimelineAction::Semantic { episode, phase } = action {
            apply_event(
                coordinator,
                next_sequence,
                report,
                RunEventKind::SemanticScreenEpisodeChanged {
                    session_id: Some(session_id.to_owned()),
                    capture_generation: Some(0),
                    screen_episode_id: episode.id,
                    sequence,
                    monotonic_end_ms: timestamp_ms,
                    screen: screen_name(episode.screen).into(),
                    phase,
                },
                observer,
            )?;
        }
    }
    Ok(())
}

/// Replays one complete recording through the same core domain coordinator used by live events.
/// A bounded frame batch is inspected in parallel and consumed in input order.
///
/// # Errors
/// Rejects changed recording bytes, undecodable frames, or core transition failures.
#[allow(
    clippy::too_many_lines,
    reason = "one sequential pass consumes and verifies each canonical input"
)]
pub fn replay_recording(root: &Path) -> Result<ReplayReport, ReplayError> {
    replay_recording_with_progress(root, &|_| {})
}

/// Replays one recording and reports bounded progress without retaining diagnostic events.
///
/// # Errors
/// Rejects changed recording bytes, undecodable frames, or core transition failures.
pub fn replay_recording_with_progress(
    root: &Path,
    on_progress: &(dyn Fn(&ReplayProgress) + Sync),
) -> Result<ReplayReport, ReplayError> {
    replay_recording_observed(root, on_progress, &mut ())
}

#[allow(
    clippy::too_many_lines,
    clippy::similar_names,
    reason = "one ordered pass consumes bounded frame batches and reviewed observations"
)]
fn replay_recording_observed(
    root: &Path,
    on_progress: &(dyn Fn(&ReplayProgress) + Sync),
    observer: &mut dyn ReplayObserver,
) -> Result<ReplayReport, ReplayError> {
    let started = Instant::now();
    on_progress(&ReplayProgress {
        recording: root.to_path_buf(),
        phase: ReplayPhase::Validating,
        processed_inputs: 0,
        total_inputs: 0,
        retained_frames: 0,
        total_retained_frames: 0,
        segments_seen: 0,
        total_segments: 0,
        elapsed: started.elapsed(),
    });
    let recording = canonical::read_for_replay(root, &|verified, total| {
        on_progress(&ReplayProgress {
            recording: root.to_path_buf(),
            phase: ReplayPhase::Validating,
            processed_inputs: 0,
            total_inputs: 0,
            retained_frames: 0,
            total_retained_frames: 0,
            segments_seen: verified,
            total_segments: total,
            elapsed: started.elapsed(),
        });
    })?;
    observer.validate(&recording.ticks)?;
    let total_retained_frames = recording
        .manifest
        .segments
        .iter()
        .map(|segment| segment.frames)
        .sum();
    let total_segments = recording.manifest.segments.len();
    let mut last_progress = Instant::now();
    let publish_progress = |phase, report: &ReplayReport, segments_seen| {
        on_progress(&ReplayProgress {
            recording: root.to_path_buf(),
            phase,
            processed_inputs: report.inputs,
            total_inputs: recording.manifest.tick_count,
            retained_frames: report.retained_frames,
            total_retained_frames,
            segments_seen,
            total_segments,
            elapsed: started.elapsed(),
        });
    };
    let mut coordinator = DomainCoordinator::new(CoordinatorPolicy::default())?;
    let mut report = ReplayReport {
        inputs: 0,
        retained_frames: 0,
        elided_inputs: 0,
        transitions: Vec::new(),
        domain_outputs: 0,
        domain_event_count: 0,
    };
    let mut core_sequence = 1_u64;
    apply_event(
        &mut coordinator,
        &mut core_sequence,
        &mut report,
        RunEventKind::CanonicalSessionStarted {
            session_id: recording.manifest.session_id.clone(),
        },
        observer,
    )?;
    coordinator.step_canonical_game_version(core_sequence, &recording.manifest.game_version)?;
    core_sequence += 1;
    let mut segment_index = 0_usize;
    publish_progress(ReplayPhase::Processing, &report, segment_index);
    let mut decoder: Option<SegmentDecoder> = None;
    let mut previous = None;
    let mut timeline = TimelineDriver::default();
    let mut title_confirmation = TitleConfirmationState::default();
    let mut resource_root = None;
    let mut field_pool: Option<RegisteredFieldPool> = None;
    for ticks in recording.ticks.chunks(PREPROCESS_BATCH) {
        let mut frames = Vec::with_capacity(ticks.len());
        for tick in ticks {
            if matches!(tick.screen, ScreenClass::Result | ScreenClass::MusicSelect)
                && tick.disposition != TickDisposition::Retained
            {
                return Err(ReplayError::Invalid("field screen lacks retained pixels"));
            }
            if tick.disposition == TickDisposition::Retained {
                if decoder
                    .as_ref()
                    .is_none_or(|current| current.remaining == 0)
                {
                    if let Some(previous_decoder) = decoder.take() {
                        previous_decoder.finish()?;
                    }
                    let segment = recording
                        .manifest
                        .segments
                        .get(segment_index)
                        .ok_or(ReplayError::Invalid("retained frame has no segment"))?;
                    decoder = Some(SegmentDecoder::start(
                        &root.join(&segment.path),
                        segment.frames,
                    )?);
                    segment_index += 1;
                }
                frames.push(Some(
                    decoder
                        .as_mut()
                        .ok_or(ReplayError::Invalid("decoder unavailable"))?
                        .frame()?,
                ));
            } else {
                frames.push(None);
            }
        }
        let mut prepared_inputs = Vec::with_capacity(ticks.len());
        for prepared in inspect_batch(frames)? {
            if let Some((pixels, inspected)) = prepared {
                let (next_title_confirmation, inspected) =
                    confirm_title_screen(title_confirmation, inspected);
                title_confirmation = next_title_confirmation;
                let field_response = if matches!(
                    inspected.screen,
                    ScreenClass::Result | ScreenClass::MusicSelect
                ) {
                    let route = inspected.crop_route().ok_or(ReplayError::Invalid(
                        "field screen has no registered crop route",
                    ))?;
                    let crops = route_screen_rgb8_crops(&pixels, route).map_err(|error| {
                        ReplayError::Field(format!("crop routing failed: {error:?}"))
                    })?;
                    if field_pool.is_none() {
                        let root = tempfile::tempdir()?;
                        let resources = resources::prepare_registered(root.path())
                            .map_err(ReplayError::Resource)?;
                        let registered = resources
                            .load_observer_resources()
                            .map_err(ReplayError::Resource)?;
                        let (catalog, title_runtime) = registered.into_catalog_and_title_runtime();
                        field_pool = Some(
                            RegisteredFieldPool::start(&catalog, title_runtime)
                                .map_err(ReplayError::Field)?,
                        );
                        resource_root = Some(root);
                    }
                    Some(
                        field_pool
                            .as_mut()
                            .ok_or(ReplayError::Invalid(
                                "field observer workers are unavailable",
                            ))?
                            .submit(crops)
                            .map_err(ReplayError::Field)?,
                    )
                } else {
                    None
                };
                prepared_inputs.push(PreparedInput {
                    observed: Some(inspected),
                    field_response,
                });
            } else {
                prepared_inputs.push(PreparedInput {
                    observed: None,
                    field_response: None,
                });
            }
        }
        for (tick, prepared) in ticks.iter().zip(prepared_inputs) {
            let observed = prepared.observed;
            if observed.is_some() {
                report.retained_frames += 1;
            } else {
                report.elided_inputs += 1;
            }
            let screen = observed.as_ref().map_or(tick.screen, |value| value.screen);
            observer.screen(tick.sequence, screen)?;
            let changed = previous != Some(screen);
            if changed {
                report.transitions.push(ExpectedTransition {
                    sequence: tick.sequence,
                    screen,
                });
            }
            let step = timeline.observe(
                RawScreenState::from(screen),
                tick.sequence,
                tick.source_timestamp_ms,
            );
            let input = raw_input(
                tick,
                screen,
                step.active_episode_id,
                observed.as_ref(),
                &recording.manifest.session_id,
            );
            let output = coordinator.step(core_sequence, &input)?;
            consume_outputs(&mut report, output.effects(), observer)?;
            core_sequence += 1;
            apply_timeline_actions(
                &mut coordinator,
                &mut core_sequence,
                &mut report,
                step.actions,
                tick.sequence,
                tick.source_timestamp_ms,
                &recording.manifest.session_id,
                observer,
            )?;
            if let Some(response) = prepared.field_response {
                let field_output = response.join().map_err(ReplayError::Field)?;
                let episode = step.active_episode_id.ok_or(ReplayError::Invalid(
                    "field observation has no semantic episode",
                ))?;
                let field_event = run_event_from_field_observation(
                    &recording.manifest.session_id,
                    0,
                    episode,
                    tick.sequence,
                    tick.source_timestamp_ms,
                    tick.source_timestamp_ms,
                    &field_output,
                )
                .map_err(ReplayError::Field)?;
                let output = coordinator.step(core_sequence, &field_event)?;
                consume_outputs(&mut report, output.effects(), observer)?;
                core_sequence += 1;
            }
            report.inputs += 1;
            previous = Some(screen);
        }
        if last_progress.elapsed() >= Duration::from_secs(15) {
            publish_progress(ReplayPhase::Processing, &report, segment_index);
            last_progress = Instant::now();
        }
    }
    drop(field_pool);
    drop(resource_root);
    if let Some(last) = recording.ticks.last() {
        apply_timeline_actions(
            &mut coordinator,
            &mut core_sequence,
            &mut report,
            timeline.finish(),
            last.sequence,
            last.source_timestamp_ms,
            &recording.manifest.session_id,
            observer,
        )?;
    }
    apply_event(
        &mut coordinator,
        &mut core_sequence,
        &mut report,
        RunEventKind::CanonicalSessionFinished {
            session_id: recording.manifest.session_id.clone(),
        },
        observer,
    )?;
    if let Some(decoder) = decoder.take() {
        decoder.finish()?;
    }
    if segment_index != recording.manifest.segments.len() {
        return Err(ReplayError::Invalid("unused canonical segment"));
    }
    publish_progress(ReplayPhase::Complete, &report, segment_index);
    Ok(report)
}

/// Replays the operator-reviewed active generation and compares its regression oracle.
///
/// # Errors
/// Rejects an absent suite, invalid labels, or any semantic mismatch.
pub fn replay_active(store_root: &Path) -> Result<Vec<ReplayReport>, ReplayError> {
    replay_active_with_progress(store_root, &|_| {})
}

/// Replays the active generation while reporting per-session progress from bounded workers.
///
/// # Errors
/// Rejects an absent suite, invalid labels, or any semantic mismatch.
pub fn replay_active_with_progress(
    store_root: &Path,
    on_progress: &(dyn Fn(&ReplayProgress) + Sync),
) -> Result<Vec<ReplayReport>, ReplayError> {
    let sessions = store::active_sessions(store_root)?;
    let workers = thread::available_parallelism()
        .map_or(1, usize::from)
        .div_ceil(4)
        .clamp(1, 4);
    let mut reports = Vec::with_capacity(sessions.len());
    for batch in sessions.chunks(workers) {
        let completed = thread::scope(|scope| {
            let pending = batch
                .iter()
                .map(|digest| {
                    scope.spawn(move || replay_active_session(store_root, digest, on_progress))
                })
                .collect::<Vec<_>>();
            pending
                .into_iter()
                .map(|job| {
                    job.join()
                        .map_err(|_| ReplayError::Invalid("corpus replay worker panicked"))?
                })
                .collect::<Result<Vec<_>, _>>()
        })?;
        reports.extend(completed);
    }
    Ok(reports)
}

fn replay_active_session(
    store_root: &Path,
    digest: &str,
    on_progress: &(dyn Fn(&ReplayProgress) + Sync),
) -> Result<ReplayReport, ReplayError> {
    let session_root = store_root.join("sessions").join(digest);
    let descriptor = store::verify_session_descriptor(&session_root, digest)?;
    let bytes = fs::read(session_root.join("label.json"))?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(ReplayError::Invalid("label exceeds bound"));
    }
    let label: RegressionLabel = serde_json::from_slice(&bytes)?;
    if label.session_sha256 != digest || label.disposition != LabelDisposition::Include {
        return Err(ReplayError::Invalid("active label binding differs"));
    }
    if label.schema != "scorepeek-private-canonical-regression-label-v2" {
        return Err(ReplayError::Invalid("active reviewed label schema differs"));
    }
    let mut oracle = crate::oracle::OracleObserver::new(&label.episodes, &label.negative_frames);
    let report = replay_recording_observed(&session_root, on_progress, &mut oracle)?;
    oracle.finish()?;
    if report.inputs != descriptor.tick_count {
        return Err(ReplayError::Invalid(
            "replayed input count differs from session",
        ));
    }
    if label
        .transitions
        .as_ref()
        .is_some_and(|expected| report.transitions != *expected)
    {
        return Err(ReplayError::Invalid("semantic regression oracle differs"));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    fn digest(path: &Path) -> String {
        let bytes = fs::read(path).unwrap();
        let mut hex = String::with_capacity(64);
        for byte in Sha256::digest(&bytes) {
            use std::fmt::Write as _;
            write!(&mut hex, "{byte:02x}").unwrap();
        }
        hex
    }

    fn synthetic_recording(root: &Path) {
        fs::create_dir(root).unwrap();
        let segment = root.join("segment-0000.mkv");
        let status = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=1920x1080:r=1",
                "-frames:v",
                "2",
                "-c:v",
                "ffv1",
                "-pix_fmt",
                "bgr0",
            ])
            .arg(&segment)
            .status()
            .unwrap();
        assert!(status.success());
        let ticks = [
            json!({"sequence":1,"source_sequence":1,"source_timestamp_ms":100,
                "screen":"unknown","semantic_episode_id":null,"disposition":{"kind":"retained"}}),
            json!({"sequence":2,"source_sequence":2,"source_timestamp_ms":200,
                "screen":"play","semantic_episode_id":1,
                "disposition":{"kind":"elided","reason":"play_interior"}}),
            json!({"sequence":3,"source_sequence":3,"source_timestamp_ms":300,
                "screen":"unknown","semantic_episode_id":1,"disposition":{"kind":"retained"}}),
        ];
        let tick_path = root.join("canonical-ticks.ndjson");
        let mut tick_bytes = String::new();
        for tick in &ticks {
            use std::fmt::Write as _;
            writeln!(&mut tick_bytes, "{tick}").unwrap();
        }
        fs::write(&tick_path, tick_bytes).unwrap();
        let manifest = json!({
            "schema":"scorepeek-canonical-session-recording-v5",
            "frame_contract":"scorepeek-canonical-rgb8-1920x1080-v1",
            "session_id":"synthetic-1",
            "shape":{"width":1920,"height":1080,"pixel_format":"rgb8"},
            "tick_index":{"path":"canonical-ticks.ndjson","sha256":digest(&tick_path),
                "bytes":fs::metadata(&tick_path).unwrap().len(),"count":3},
            "tick_count":3,
            "segments":[{"path":"segment-0000.mkv","first_sequence":1,"last_sequence":3,
                "frames":2,"bytes":fs::metadata(&segment).unwrap().len(),"sha256":digest(&segment)}],
            "completeness":"complete","completeness_reasons":[],
            "game_version":{"status":"not_observed"}
        });
        fs::write(
            root.join("canonical-manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one fixture exercises replay, review, and failure"
    )]
    fn synthetic_input_state_outputs_review_oracle_and_failure() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let store_root = root.path().join("store");
        synthetic_recording(&source);
        let source_manifest = fs::read(source.join("canonical-manifest.json")).unwrap();
        let progress = std::sync::Mutex::new(Vec::new());
        let report = replay_recording_with_progress(&source, &|update| {
            progress
                .lock()
                .unwrap()
                .push((update.phase, update.processed_inputs));
        })
        .unwrap();
        let progress = progress.into_inner().unwrap();
        assert_eq!(progress.first(), Some(&(ReplayPhase::Validating, 0)));
        assert_eq!(progress.last(), Some(&(ReplayPhase::Complete, 3)));
        assert_eq!(report.inputs, 3);
        assert_eq!(report.retained_frames, 2);
        assert_eq!(report.elided_inputs, 1);
        assert_eq!(
            report
                .transitions
                .iter()
                .map(|item| item.screen)
                .collect::<Vec<_>>(),
            [
                ScreenClass::Unknown,
                ScreenClass::Play,
                ScreenClass::Unknown
            ]
        );
        assert!(report.domain_outputs > 0);
        let imported = store::import_recording(&store_root, &source).unwrap();
        assert!(store::active_sessions(&store_root).is_err());
        let label_path = root.path().join("labels.json");
        let label = RegressionLabel {
            schema: "scorepeek-private-canonical-regression-label-v2".into(),
            session_sha256: imported.session_sha256.clone(),
            disposition: LabelDisposition::Include,
            episodes: Vec::new(),
            negative_frames: Vec::new(),
            transitions: Some(report.transitions.clone()),
        };
        fs::write(&label_path, serde_json::to_vec(&label).unwrap()).unwrap();
        store::review_apply(&store_root, &imported.draft, &label_path).unwrap();
        assert_eq!(replay_active(&store_root).unwrap().first(), Some(&report));
        let active_before = fs::read(store_root.join("active.json")).unwrap();
        let failed_import = root.path().join("failed-import");
        synthetic_recording(&failed_import);
        fs::write(failed_import.join("segment-0000.mkv"), b"changed segment").unwrap();
        assert!(store::import_recording(&store_root, &failed_import).is_err());
        assert_eq!(
            fs::read(store_root.join("active.json")).unwrap(),
            active_before
        );
        assert_eq!(
            fs::read(source.join("canonical-manifest.json")).unwrap(),
            source_manifest
        );
        let mut wrong = label.clone();
        wrong.transitions.as_mut().unwrap()[0].screen = ScreenClass::Result;
        fs::write(
            store_root
                .join("sessions")
                .join(&imported.session_sha256)
                .join("label.json"),
            serde_json::to_vec(&wrong).unwrap(),
        )
        .unwrap();
        assert!(replay_active(&store_root).is_err());
    }

    #[test]
    fn retained_pixels_determine_current_screen_instead_of_recorded_metadata() {
        let root = tempfile::tempdir().unwrap();
        let field_source = root.path().join("field-recognition");
        synthetic_recording(&field_source);
        let tick_path = field_source.join("canonical-ticks.ndjson");
        let ticks = fs::read_to_string(&tick_path).unwrap().replacen(
            "\"screen\":\"unknown\"",
            "\"screen\":\"result\"",
            1,
        );
        fs::write(&tick_path, ticks).unwrap();
        let mut manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(field_source.join("canonical-manifest.json")).unwrap(),
        )
        .unwrap();
        manifest["tick_index"]["sha256"] = digest(&tick_path).into();
        manifest["tick_index"]["bytes"] = fs::metadata(&tick_path).unwrap().len().into();
        fs::write(
            field_source.join("canonical-manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let report = replay_recording(&field_source).unwrap();
        assert_eq!(report.transitions[0].screen, ScreenClass::Unknown);
    }

    #[test]
    fn active_sessions_replay_in_suite_order_with_bounded_parallelism() {
        let root = tempfile::tempdir().unwrap();
        let store_root = root.path().join("store");
        let mut expected = std::collections::BTreeMap::new();
        for index in 0..2 {
            let source = root.path().join(format!("source-{index}"));
            synthetic_recording(&source);
            let manifest_path = source.join("canonical-manifest.json");
            let mut manifest: serde_json::Value =
                serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
            manifest["session_id"] = format!("synthetic-{index}").into();
            fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
            let report = replay_recording(&source).unwrap();
            let imported = store::import_recording(&store_root, &source).unwrap();
            let label = RegressionLabel {
                schema: "scorepeek-private-canonical-regression-label-v2".into(),
                session_sha256: imported.session_sha256.clone(),
                disposition: LabelDisposition::Include,
                episodes: Vec::new(),
                negative_frames: Vec::new(),
                transitions: Some(report.transitions.clone()),
            };
            let label_path = root.path().join(format!("labels-{index}.json"));
            fs::write(&label_path, serde_json::to_vec(&label).unwrap()).unwrap();
            store::review_apply(&store_root, &imported.draft, &label_path).unwrap();
            expected.insert(imported.session_sha256, report);
        }
        let observed = replay_active(&store_root).unwrap();
        assert_eq!(observed, expected.into_values().collect::<Vec<_>>());
    }
}
