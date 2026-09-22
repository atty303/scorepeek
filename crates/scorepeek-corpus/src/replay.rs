//! Semantic replay from verified canonical input, without runtime diagnostic artifacts.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};

use scorepeek_core::canonical_recording::{CanonicalTick, TickDisposition};
use scorepeek_core::event::coordinator::{CoordinatorError, CoordinatorPolicy, DomainCoordinator};
use scorepeek_core::event::{
    RUN_EVENT_SCHEMA, RunEvent, RunEventKind, RunReducerEffect, run_event_from_field_observation,
};
use scorepeek_core::recognition::screen::{
    ScreenClass, ScreenPredicateObservation, inspect_canonical_rgb8, route_screen_rgb8_crops,
};
use scorepeek_core::session::episode::RawScreenState;
use scorepeek_core::session::timeline::{TimelineAction, TimelineDriver};

use crate::canonical::{self, RecordingError};
use crate::field::RegisteredFieldObserver;
use crate::resources;
use crate::store::{self, ExpectedTransition, LabelDisposition, RegressionLabel, StoreError};

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
    pub domain_event_sha256: String,
    pub domain_event_count: u64,
}

fn consume_outputs(
    report: &mut ReplayReport,
    digest: &mut Sha256,
    input_sequence: u64,
    outputs: &[RunReducerEffect],
) -> Result<(), ReplayError> {
    report.domain_outputs += outputs.len() as u64;
    for output in outputs {
        if let RunReducerEffect::Event(event) = output {
            let encoded = serde_json::to_vec(event)?;
            digest.update(input_sequence.to_le_bytes());
            digest.update((encoded.len() as u64).to_le_bytes());
            digest.update(encoded);
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
    observed: Option<&ScreenPredicateObservation>,
    session_id: &str,
) -> RunEvent {
    let kind = RunEventKind::RawScreenObserved {
        session_id: Some(session_id.to_owned()),
        capture_generation: Some(0),
        semantic_episode_id: tick.semantic_episode_id,
        sequence: tick.sequence,
        monotonic_start_ms: tick.source_timestamp_ms,
        monotonic_end_ms: tick.source_timestamp_ms,
        screen: screen_name(tick.screen).into(),
        result_presence: observed.map(|value| value.result_presence),
        play_presence: observed.map(|value| value.play_presence),
        unknown_reason: (tick.screen == ScreenClass::Unknown)
            .then(|| "predicate_not_matched".into()),
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
    digest: &mut Sha256,
    kind: RunEventKind,
) -> Result<(), ReplayError> {
    let event = RunEvent {
        schema: RUN_EVENT_SCHEMA.into(),
        kind,
    };
    let output = coordinator.step(*next_sequence, &event)?;
    consume_outputs(report, digest, *next_sequence, output.effects())?;
    *next_sequence += 1;
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "the ordered timeline action retains explicit coordinator and digest state"
)]
fn apply_timeline_actions(
    coordinator: &mut DomainCoordinator,
    next_sequence: &mut u64,
    report: &mut ReplayReport,
    digest: &mut Sha256,
    actions: Vec<TimelineAction>,
    sequence: u64,
    timestamp_ms: u64,
    session_id: &str,
) -> Result<(), ReplayError> {
    for action in actions {
        if let TimelineAction::Semantic { episode, phase } = action {
            apply_event(
                coordinator,
                next_sequence,
                report,
                digest,
                RunEventKind::SemanticScreenEpisodeChanged {
                    session_id: Some(session_id.to_owned()),
                    capture_generation: Some(0),
                    screen_episode_id: episode.id,
                    sequence,
                    monotonic_end_ms: timestamp_ms,
                    screen: screen_name(episode.screen).into(),
                    phase,
                },
            )?;
        }
    }
    Ok(())
}

/// Replays one complete recording through the same core domain coordinator used by live events.
/// Each frame is decoded, inspected, and released before the next input.
///
/// # Errors
/// Rejects changed recording bytes, undecodable frames, or core transition failures.
#[allow(
    clippy::too_many_lines,
    reason = "one sequential pass consumes and verifies each canonical input"
)]
pub fn replay_recording(root: &Path) -> Result<ReplayReport, ReplayError> {
    let recording = canonical::read_complete(root)?;
    let mut coordinator = DomainCoordinator::new(CoordinatorPolicy::default())?;
    let mut report = ReplayReport {
        inputs: 0,
        retained_frames: 0,
        elided_inputs: 0,
        transitions: Vec::new(),
        domain_outputs: 0,
        domain_event_sha256: String::new(),
        domain_event_count: 0,
    };
    let mut domain_digest = Sha256::new();
    let mut core_sequence = 1_u64;
    apply_event(
        &mut coordinator,
        &mut core_sequence,
        &mut report,
        &mut domain_digest,
        RunEventKind::CanonicalSessionStarted {
            session_id: recording.manifest.session_id.clone(),
        },
    )?;
    coordinator.step_canonical_game_version(core_sequence, &recording.manifest.game_version)?;
    core_sequence += 1;
    let mut segment_index = 0_usize;
    let mut decoder: Option<SegmentDecoder> = None;
    let mut previous = None;
    let mut timeline = TimelineDriver::default();
    let mut resource_root = None;
    let mut field_observer: Option<RegisteredFieldObserver> = None;
    for tick in &recording.ticks {
        if matches!(tick.screen, ScreenClass::Result | ScreenClass::MusicSelect)
            && tick.disposition != TickDisposition::Retained
        {
            return Err(ReplayError::Invalid("field screen lacks retained pixels"));
        }
        let (observed, field_output) = if tick.disposition == TickDisposition::Retained {
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
            let pixels = decoder
                .as_mut()
                .ok_or(ReplayError::Invalid("decoder unavailable"))?
                .frame()?;
            let inspected = inspect_canonical_rgb8(&pixels)
                .map_err(|_| ReplayError::Invalid("canonical frame inspection failed"))?;
            report.retained_frames += 1;
            if inspected.screen != tick.screen {
                return Err(ReplayError::Invalid(
                    "recorded screen and current predicate differ",
                ));
            }
            let field_output =
                if matches!(tick.screen, ScreenClass::Result | ScreenClass::MusicSelect) {
                    let route = inspected.crop_route().ok_or(ReplayError::Invalid(
                        "field screen has no registered crop route",
                    ))?;
                    let crops = route_screen_rgb8_crops(&pixels, route).map_err(|error| {
                        ReplayError::Field(format!("crop routing failed: {error:?}"))
                    })?;
                    if field_observer.is_none() {
                        let root = tempfile::tempdir()?;
                        let resources = resources::load_registered(root.path())
                            .map_err(ReplayError::Resource)?;
                        field_observer = Some(
                            RegisteredFieldObserver::new(resources)
                                .map_err(ReplayError::Resource)?,
                        );
                        resource_root = Some(root);
                    }
                    Some(
                        field_observer
                            .as_mut()
                            .ok_or(ReplayError::Invalid("field observer is unavailable"))?
                            .observe(&crops)
                            .map_err(ReplayError::Field)?,
                    )
                } else {
                    None
                };
            (Some(inspected), field_output)
        } else {
            report.elided_inputs += 1;
            (None, None)
        };
        let screen = tick.screen;
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
        if step.active_episode_id != tick.semantic_episode_id {
            return Err(ReplayError::Invalid(
                "recorded semantic episode differs from current timeline",
            ));
        }
        let input = raw_input(tick, observed.as_ref(), &recording.manifest.session_id);
        let output = coordinator.step(core_sequence, &input)?;
        consume_outputs(
            &mut report,
            &mut domain_digest,
            core_sequence,
            output.effects(),
        )?;
        core_sequence += 1;
        apply_timeline_actions(
            &mut coordinator,
            &mut core_sequence,
            &mut report,
            &mut domain_digest,
            step.actions,
            tick.sequence,
            tick.source_timestamp_ms,
            &recording.manifest.session_id,
        )?;
        if let Some(field_output) = field_output {
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
            consume_outputs(
                &mut report,
                &mut domain_digest,
                core_sequence,
                output.effects(),
            )?;
            core_sequence += 1;
        }
        report.inputs += 1;
        previous = Some(screen);
    }
    drop(field_observer);
    drop(resource_root);
    if let Some(last) = recording.ticks.last() {
        apply_timeline_actions(
            &mut coordinator,
            &mut core_sequence,
            &mut report,
            &mut domain_digest,
            timeline.finish(),
            last.sequence,
            last.source_timestamp_ms,
            &recording.manifest.session_id,
        )?;
    }
    apply_event(
        &mut coordinator,
        &mut core_sequence,
        &mut report,
        &mut domain_digest,
        RunEventKind::CanonicalSessionFinished {
            session_id: recording.manifest.session_id.clone(),
        },
    )?;
    if let Some(decoder) = decoder.take() {
        decoder.finish()?;
    }
    if segment_index != recording.manifest.segments.len() {
        return Err(ReplayError::Invalid("unused canonical segment"));
    }
    for byte in domain_digest.finalize() {
        use std::fmt::Write as _;
        write!(&mut report.domain_event_sha256, "{byte:02x}")
            .expect("writing to String cannot fail");
    }
    Ok(report)
}

/// Replays the operator-reviewed active generation and compares its regression oracle.
///
/// # Errors
/// Rejects an absent suite, invalid labels, or any semantic mismatch.
pub fn replay_active(store_root: &Path) -> Result<Vec<ReplayReport>, ReplayError> {
    let sessions = store::active_sessions(store_root)?;
    let mut reports = Vec::new();
    for digest in sessions {
        let session_root = store_root.join("sessions").join(&digest);
        let descriptor = store::verify_session_descriptor(&session_root, &digest)?;
        let bytes = fs::read(session_root.join("label.json"))?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(ReplayError::Invalid("label exceeds bound"));
        }
        let label: RegressionLabel = serde_json::from_slice(&bytes)?;
        if label.session_sha256 != digest || label.disposition != LabelDisposition::Include {
            return Err(ReplayError::Invalid("active label binding differs"));
        }
        let report = replay_recording(&session_root)?;
        if report.inputs != descriptor.tick_count {
            return Err(ReplayError::Invalid(
                "replayed input count differs from session",
            ));
        }
        if report.transitions != label.transitions {
            return Err(ReplayError::Invalid("semantic regression oracle differs"));
        }
        if report.domain_event_count != label.domain_event_count
            || report.domain_event_sha256 != label.domain_event_sha256
        {
            return Err(ReplayError::Invalid(
                "domain event regression oracle differs",
            ));
        }
        reports.push(report);
    }
    Ok(reports)
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
    fn synthetic_input_state_outputs_review_oracle_and_failure() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let store_root = root.path().join("store");
        synthetic_recording(&source);
        let source_manifest = fs::read(source.join("canonical-manifest.json")).unwrap();
        let report = replay_recording(&source).unwrap();
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
            schema: "scorepeek-private-canonical-regression-label-v1".into(),
            session_sha256: imported.session_sha256.clone(),
            disposition: LabelDisposition::Include,
            transitions: report.transitions.clone(),
            domain_event_sha256: report.domain_event_sha256.clone(),
            domain_event_count: report.domain_event_count,
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
        wrong.transitions[0].screen = ScreenClass::Result;
        fs::write(
            store_root
                .join("sessions")
                .join(&imported.session_sha256)
                .join("label.json"),
            serde_json::to_vec(&wrong).unwrap(),
        )
        .unwrap();
        assert!(replay_active(&store_root).is_err());

        let mut wrong_digest = label.clone();
        wrong_digest.domain_event_sha256 = "0".repeat(64);
        fs::write(
            store_root
                .join("sessions")
                .join(&imported.session_sha256)
                .join("label.json"),
            serde_json::to_vec(&wrong_digest).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            replay_active(&store_root),
            Err(ReplayError::Invalid(
                "domain event regression oracle differs"
            ))
        ));

        let invalid_source = root.path().join("invalid-episode");
        synthetic_recording(&invalid_source);
        let tick_path = invalid_source.join("canonical-ticks.ndjson");
        let mut ticks = fs::read_to_string(&tick_path).unwrap();
        ticks = ticks.replacen("\"semantic_episode_id\":1", "\"semantic_episode_id\":2", 1);
        fs::write(&tick_path, ticks).unwrap();
        let mut manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(invalid_source.join("canonical-manifest.json")).unwrap(),
        )
        .unwrap();
        manifest["tick_index"]["sha256"] = digest(&tick_path).into();
        manifest["tick_index"]["bytes"] = fs::metadata(&tick_path).unwrap().len().into();
        fs::write(
            invalid_source.join("canonical-manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            replay_recording(&invalid_source),
            Err(ReplayError::Invalid(
                "recorded semantic episode differs from current timeline"
            ))
        ));
    }

    #[test]
    fn recorded_field_screen_must_match_current_pixels_before_resource_load() {
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
        assert!(matches!(
            replay_recording(&field_source),
            Err(ReplayError::Invalid(
                "recorded screen and current predicate differ"
            ))
        ));
    }
}
