use super::*;
use object_store::ObjectStore;
use object_store::memory::InMemory;
use std::io::Seek as _;

#[test]
fn replay_observer_rejects_an_oversized_run_event_marker() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = temporary.path().join("runtime");
    let mut diagnostics =
        scorepeek_runtime::diagnostics::inspect::RunDiagnostics::start_ephemeral_at(
            &runtime, "replay-0",
        );
    assert!(diagnostics.run_root().is_none());
    let stream_observer =
        scorepeek_runtime::diagnostics::inspect::DiagnosticObserver::connect_at(&runtime, None)
            .unwrap();
    let observer = start_replay_observer(stream_observer, 0, None, false).unwrap();
    diagnostics.sink().record(
        "run_event",
        &serde_json::json!({
            "payload": "x".repeat(scorepeek_runtime::diagnostics::inspect::MAX_RECORD_BYTES)
        }),
        false,
    );
    diagnostics.finish("success");
    let Err(error) = observer.join().unwrap() else {
        panic!("oversized run event unexpectedly reached replay");
    };
    assert!(error.contains("exceeded the stream record bound"));
}

#[test]
fn replay_observer_validates_field_observations_before_dropping_them() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = temporary.path().join("runtime");
    let mut diagnostics =
        scorepeek_runtime::diagnostics::inspect::RunDiagnostics::start_ephemeral_at(
            &runtime, "replay-0",
        );
    let stream_observer =
        scorepeek_runtime::diagnostics::inspect::DiagnosticObserver::connect_at(&runtime, None)
            .unwrap();
    let observer = start_replay_observer(stream_observer, 0, None, false).unwrap();
    diagnostics.sink().record(
        "run_event",
        &serde_json::json!({"event":"field_observation"}),
        false,
    );
    diagnostics.finish("success");
    let Err(error) = observer.join().unwrap() else {
        panic!("malformed field observation unexpectedly reached replay");
    };
    assert!(error.contains("run event"));
}

#[test]
fn observer_spawn_failure_reaps_the_registered_trace_session() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = temporary.path().join("runtime");
    let mut diagnostics =
        scorepeek_runtime::diagnostics::inspect::RunDiagnostics::start_ephemeral_at(
            &runtime, "replay-0",
        );
    let stream_observer =
        scorepeek_runtime::diagnostics::inspect::DiagnosticObserver::connect_at(&runtime, None)
            .unwrap();
    let trace = Arc::new(Mutex::new(ReplayTrace::new(
        temporary.path().join("trace"),
        "generation",
    )));
    trace.lock().unwrap().start_session(0, "session").unwrap();

    let error = start_replay_observer(stream_observer, 0, Some(&trace), true).unwrap_err();

    assert!(error.contains("injected failure"));
    assert_eq!(trace.lock().unwrap().active_session_count(), 0);
    diagnostics.finish("error");
}

#[test]
fn dropping_a_failed_replay_stream_reaps_its_observer_and_trace_writer() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = temporary.path().join("runtime");
    let mut replay_trace = ReplayTrace::new(temporary.path().join("trace"), "generation");
    let writer_gate = replay_trace.block_writer(0);
    replay_trace.start_session(0, "session").unwrap();
    let trace = Arc::new(Mutex::new(replay_trace));
    let diagnostics = scorepeek_runtime::diagnostics::inspect::RunDiagnostics::start_ephemeral_at(
        &runtime, "replay-0",
    );
    let stream_observer =
        scorepeek_runtime::diagnostics::inspect::DiagnosticObserver::connect_at(&runtime, None)
            .unwrap();
    let observer = start_replay_observer(stream_observer, 0, Some(&trace), false).unwrap();
    let event_stream = ReplayEventStream {
        output: ReplayEventOutput::new(diagnostics),
        observer: Some(observer),
    };

    let (dropped_sender, dropped_receiver) = mpsc::channel();
    let dropper = thread::spawn(move || {
        drop(event_stream);
        dropped_sender.send(()).unwrap();
    });

    assert!(matches!(
        dropped_receiver.recv_timeout(Duration::from_millis(50)),
        Err(RecvTimeoutError::Timeout)
    ));
    writer_gate.release();
    dropped_receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    dropper.join().unwrap();

    assert_eq!(trace.lock().unwrap().active_session_count(), 0);
    assert!(temporary.path().join("trace/session-0.ndjson").is_file());
}

#[test]
fn run_import_requires_the_selected_sessions_saved_completion_record() {
    let root = tempfile::tempdir().unwrap();
    let run_id = "run-1-0-1";
    let session_id = "run-1-0-1-session-1";
    let event = serde_json::json!({
        "schema":"scorepeek-diagnostic-event-v1", "run_id":run_id, "sequence":1,
        "observed_unix_us":1, "operation":"run_event", "data":{
            "schema":"scorepeek-run-event-v15", "event":"session_started",
            "session_id":session_id, "capture_generation":1,
            "capture_profile_sha256":"1".repeat(64),
            "normalizer_artifact_sha256":"2".repeat(64)
        }
    });
    std::fs::write(
        root.path().join("diagnostics.ndjson"),
        format!("{}\n", serde_json::to_string(&event).unwrap()),
    )
    .unwrap();
    let error = verify_run_diagnostic(root.path(), session_id).unwrap_err();
    assert!(error.to_string().contains("recording_completed"));
}

#[test]
fn run_import_rejects_a_complete_manifest_without_video() {
    let root = tempfile::tempdir().unwrap();
    let run = root.path().join("run-1-0-1");
    let session_id = "run-1-0-1-session-1";
    let canonical = run.join("sessions").join(session_id).join("canonical");
    fs::create_dir_all(&canonical).unwrap();
    fs::write(
            canonical.join("canonical-ticks.ndjson"),
            b"{\"sequence\":1,\"source_sequence\":1,\"monotonic_ms\":100,\"screen\":\"unknown\",\"semantic_episode_id\":null,\"disposition\":\"elided\"}\n",
        )
        .unwrap();
    fs::write(
        canonical.join("canonical-manifest.json"),
        canonical_json(&serde_json::json!({
            "schema":"scorepeek-canonical-session-recording-v4",
            "completeness":"complete",
            "ffmpeg_sha256":"4".repeat(64),
            "ffmpeg_version":"test",
            "tick_count":1,
            "segments":[],
            "dropped_frames":0,
            "completeness_reasons":[],
            "memory_limit_bytes":1_073_741_824_u64,
            "memory_high_water_bytes":0,
            "game_version":{"status":"not_observed"}
        }))
        .unwrap(),
    )
    .unwrap();
    let records = [
        serde_json::json!({
            "schema":"scorepeek-diagnostic-event-v1", "run_id":"run-1-0-1",
            "sequence":1, "observed_unix_us":1, "operation":"run_event", "data":{
                "schema":"scorepeek-run-event-v15", "event":"session_started",
                "session_id":session_id, "capture_generation":1,
                "capture_profile_sha256":"1".repeat(64),
                "normalizer_artifact_sha256":"2".repeat(64)
            }
        }),
        serde_json::json!({
            "schema":"scorepeek-diagnostic-event-v1", "run_id":"run-1-0-1",
            "sequence":2, "observed_unix_us":2, "operation":"public_event", "data":{
                "public_event":{"capture":{"session_id":session_id,"binding":{
                    "capture_profile_sha256":"1".repeat(64),
                    "normalizer_sha256":"2".repeat(64),
                    "canonical_layout_sha256":"5".repeat(64),
                    "catalog_sha256":"3".repeat(64),
                    "model_sha256":"6".repeat(64),
                    "runtime_sha256":"7".repeat(64)
                }}}
            }
        }),
        serde_json::json!({
            "schema":"scorepeek-diagnostic-event-v1", "run_id":"run-1-0-1",
            "sequence":3, "observed_unix_us":3, "operation":"run_event", "data":{
                "schema":"scorepeek-run-event-v15", "event":"recording_completed",
                "session_id":session_id, "directory":canonical.parent().unwrap()
            }
        }),
    ];
    let mut stream = Vec::new();
    for record in records {
        serde_json::to_writer(&mut stream, &record).unwrap();
        stream.push(b'\n');
    }
    fs::write(run.join("diagnostics.ndjson"), stream).unwrap();

    let error = verify_run_diagnostic(&run, session_id).unwrap_err();
    assert!(error.to_string().contains("no video"));
}

#[test]
fn run_import_preserves_video_and_replay_metadata_before_releasing_local_video() {
    let root = tempfile::tempdir().unwrap();
    let run = root.path().join("run-1-0-1");
    let session_id = "run-1-0-1-session-1";
    let canonical = run.join("sessions").join(session_id).join("canonical");
    fs::create_dir_all(&canonical).unwrap();
    let segment = canonical.join("segment-0000.mkv");
    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-video_size",
            "1920x1080",
            "-framerate",
            "10",
            "-i",
            "pipe:0",
            "-an",
            "-c:v",
            "libx264rgb",
            "-crf",
            "0",
            "-preset",
            "ultrafast",
            "-frames:v",
            "1",
            "-f",
            "matroska",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::from(File::create(&segment).unwrap()))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&vec![0; 1_920 * 1_080 * 3])
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let segment_bytes = segment.metadata().unwrap().len();
    fs::write(
            canonical.join("canonical-ticks.ndjson"),
            b"{\"sequence\":1,\"source_sequence\":1,\"monotonic_ms\":100,\"screen\":\"result\",\"semantic_episode_id\":1,\"disposition\":\"retained\"}\n",
        )
        .unwrap();
    fs::write(
        canonical.join("canonical-manifest.json"),
        canonical_json(&serde_json::json!({
            "schema":"scorepeek-canonical-session-recording-v4",
            "completeness":"complete",
            "ffmpeg_sha256":"4".repeat(64),
            "ffmpeg_version":"test",
            "tick_count":1,
            "segments":[{
                "path":"segment-0000.mkv", "first_sequence":1,
                "last_sequence":1, "frames":1, "bytes":segment_bytes
            }],
            "dropped_frames":0,
            "completeness_reasons":[],
            "memory_limit_bytes":1_073_741_824_u64,
            "memory_high_water_bytes":6_220_800_u64,
            "game_version":{
                "status":"identified", "version":"P2D:J:B:A:2026080500"
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let records = [
        serde_json::json!({
            "schema":"scorepeek-diagnostic-event-v1", "run_id":"run-1-0-1",
            "sequence":1, "observed_unix_us":1, "operation":"run_event", "data":{
                "schema":"scorepeek-run-event-v15", "event":"session_started",
                "session_id":session_id, "capture_generation":1,
                "capture_profile_sha256":"1".repeat(64),
                "normalizer_artifact_sha256":"2".repeat(64)
            }
        }),
        serde_json::json!({
            "schema":"scorepeek-diagnostic-event-v1", "run_id":"run-1-0-1",
            "sequence":2, "observed_unix_us":2, "operation":"public_event", "data":{
                "public_event":{"capture":{"session_id":session_id,"binding":{
                    "capture_profile_sha256":"1".repeat(64),
                    "normalizer_sha256":"2".repeat(64),
                    "canonical_layout_sha256":"5".repeat(64),
                    "catalog_sha256":"3".repeat(64),
                    "model_sha256":"6".repeat(64),
                    "runtime_sha256":"7".repeat(64)
                }}}
            }
        }),
        serde_json::json!({
            "schema":"scorepeek-diagnostic-event-v1", "run_id":"run-1-0-1",
            "sequence":3, "observed_unix_us":3, "operation":"run_event", "data":{
                "schema":"scorepeek-run-event-v15", "event":"field_observation",
                "session_id":session_id, "capture_generation":1, "sequence":1,
                "monotonic_start_ms":90, "monotonic_end_ms":100, "screen":"result",
                "fields":{"screen":"result"},
                "result_song_resolution":{"status":"accepted"},
                "music_select_song_resolution":{"status":"unknown"},
                "song_resolution_presentation":{"status":"unknown","selected":null}
            }
        }),
        serde_json::json!({
            "schema":"scorepeek-diagnostic-event-v1", "run_id":"run-1-0-1",
            "sequence":4, "observed_unix_us":4, "operation":"run_event", "data":{
                "schema":"scorepeek-run-event-v15", "event":"recording_completed",
                "session_id":session_id, "directory":canonical.parent().unwrap()
            }
        }),
    ];
    let mut stream = Vec::new();
    for record in records {
        serde_json::to_writer(&mut stream, &record).unwrap();
        stream.push(b'\n');
    }
    fs::write(run.join("diagnostics.ndjson"), stream).unwrap();

    let store = root.path().join("store");
    let draft = root.path().join("review.json");
    let verification = verify_run_diagnostic(&run, session_id).unwrap();
    assert!(
        serde_json::to_value(verification)
            .unwrap()
            .get("diagnostic_sha256")
            .is_none()
    );
    let summary =
        import_run_diagnostic_with_remote(&store, &run, session_id, &draft, None).unwrap();
    assert!(
        serde_json::to_value(&summary)
            .unwrap()
            .get("diagnostic_sha256")
            .is_none()
    );
    let (session, _) = read_json::<CaptureSession>(
        &store
            .join("sessions")
            .join(format!("{}.json", summary.session_sha256)),
    )
    .unwrap();
    assert!(
        session
            .artifacts
            .iter()
            .any(|artifact| { artifact.source_path == "recognition/canonical-manifest.json" })
    );
    assert!(
        session
            .artifacts
            .iter()
            .any(|artifact| { artifact.source_path == "recognition/canonical-ticks.ndjson" })
    );
    assert!(
        session
            .artifacts
            .iter()
            .any(|artifact| { artifact.source_path == "recognition/segment-0000.mkv" })
    );
    assert!(
        session
            .artifacts
            .iter()
            .any(|artifact| { artifact.source_path == "capture/run.json" })
    );
    let binding = session_binding(&store, &session).unwrap();
    assert_eq!(binding.capture_profile_sha256, "1".repeat(64));
    assert_eq!(binding.normalizer_sha256, "2".repeat(64));
    let (manifest, _) = read_json::<CanonicalRecordingManifest>(
        &session_object_for_source(&store, &session, "recognition/canonical-manifest.json")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest.schema, "scorepeek-canonical-session-recording-v4");
    assert_eq!(
        session.game_version,
        GameVersionState::Identified("P2D:J:B:A:2026080500".to_owned())
    );
    assert!(manifest.segments[0].raw_rgb24_sha256.is_none());
    assert!(
        session_object_for_source(&store, &session, "recognition/segment-0000.mkv")
            .unwrap()
            .is_file()
    );
    let analysis = session
        .artifacts
        .iter()
        .find(|artifact| artifact.source_path == "analysis/observations.ndjson")
        .unwrap();
    let analysis: Value =
        serde_json::from_slice(&fs::read(store.join("objects").join(&analysis.sha256)).unwrap())
            .unwrap();
    assert_eq!(analysis["schema"], CORPUS_OBSERVATION_SCHEMA);
    assert_eq!(analysis["tick_sequence"], 1);
    assert!(analysis.get("event").is_none());
    assert!(!segment.exists());
    assert!(canonical.join("import-receipt.json").is_file());
    fs::write(&segment, b"leftover already transferred segment").unwrap();
    let repeated =
        import_run_diagnostic_with_remote(&store, &run, session_id, &draft, None).unwrap();
    assert_eq!(repeated.session_sha256, summary.session_sha256);
    assert!(!segment.exists());
    assert!(run.join("diagnostics.ndjson").exists());
}

fn diagnostic_manifest() -> DiagnosticManifest {
    DiagnosticManifest {
        schema: DIAGNOSTIC_SCHEMA.to_owned(),
        source_kind: SourceKind::LiveRun,
        session_id: "run-1-session-1".to_owned(),
        capture_generation: 1,
        profile_sha256: "1".repeat(64),
        catalog_sha256: "2".repeat(64),
        recognition_interval_ms: 100,
        processed_ticks: 1,
        busy_skips: 0,
        maximum_consecutive_busy_skips: 0,
        field_observation_busy_skips: Some(0),
        maximum_consecutive_field_observation_busy_skips: Some(0),
        completeness: "complete".to_owned(),
        capture_manifest_sha256: "3".repeat(64),
        recognition_manifest_sha256: "4".repeat(64),
        event_manifest_sha256: "5".repeat(64),
        canonical_manifest_sha256: Some("6".repeat(64)),
        canonical_completeness: Some("complete".to_owned()),
        artifacts: Vec::new(),
    }
}

#[test]
fn diagnostic_manifest_requires_v5_canonical_and_field_busy_bindings() {
    let mut manifest = diagnostic_manifest();
    manifest.artifacts.push(DiagnosticArtifact {
        kind: "capture_manifest".to_owned(),
        path: "capture/manifest.json".to_owned(),
        sha256: "6".repeat(64),
        bytes: 1,
    });
    manifest.field_observation_busy_skips = Some(17);
    manifest.maximum_consecutive_field_observation_busy_skips = Some(3);
    assert!(validate_diagnostic_manifest(&manifest).is_ok());

    manifest.maximum_consecutive_field_observation_busy_skips = Some(18);
    assert!(validate_diagnostic_manifest(&manifest).is_err());

    manifest.schema = "scorepeek-private-diagnostic-session-v4".to_owned();
    manifest.field_observation_busy_skips = None;
    manifest.maximum_consecutive_field_observation_busy_skips = None;
    assert!(validate_diagnostic_manifest(&manifest).is_err());
}

#[test]
fn decimal_video_timestamps_are_converted_without_float_rounding() {
    assert_eq!(parse_timestamp_ms("0.000000").unwrap(), 0);
    assert_eq!(parse_timestamp_ms("12.345678").unwrap(), 12_345);
    assert_eq!(parse_timestamp_ms("1.5").unwrap(), 1_500);
    assert!(parse_timestamp_ms("-0.1").is_err());
}

#[test]
fn canonical_tick_chronology_rejects_cross_segment_sequence_or_time_reset() {
    let tick = CanonicalTick {
        sequence: 11,
        source_sequence: 11,
        monotonic_ms: 1_000,
        screen: ScreenClass::Result,
        semantic_episode_id: Some(1),
        disposition: "retained".to_owned(),
    };
    assert!(canonical_tick_follows(Some((10, 1_000)), &tick));
    assert!(!canonical_tick_follows(Some((11, 900)), &tick));
    assert!(!canonical_tick_follows(Some((12, 1_000)), &tick));
    assert!(!canonical_tick_follows(Some((10, 1_001)), &tick));
}

#[test]
fn replay_decoder_activity_tracks_four_independent_sessions() {
    let activity = Arc::new(ReplayDecodeActivity::default());
    let entered = Arc::new(std::sync::Barrier::new(5));
    let release = Arc::new(std::sync::Barrier::new(5));
    thread::scope(|scope| {
        for _ in 0..4 {
            let activity = Arc::clone(&activity);
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            scope.spawn(move || {
                let memory = activity.reserve_decoder();
                let _decoder = activity.enter(std::process::id(), memory);
                entered.wait();
                release.wait();
            });
        }
        entered.wait();
        assert_eq!(activity.active.load(Ordering::Acquire), 4);
        assert_eq!(activity.maximum_active.load(Ordering::Acquire), 4);
        assert_eq!(activity.children.load(Ordering::Acquire), 4);
        assert_eq!(
            activity.tracked_peak_bytes.load(Ordering::Acquire),
            u64::try_from(4 * DECODER_RESERVATION_BYTES).unwrap()
        );
        release.wait();
    });
    assert_eq!(activity.active.load(Ordering::Acquire), 0);
    assert_eq!(activity.tracked_bytes.load(Ordering::Acquire), 0);
}

#[test]
fn four_ffmpeg_children_decode_the_same_immutable_segment_concurrently() {
    let root = tempfile::tempdir().unwrap();
    let segment = root.path().join("fixture.mkv");
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=1920x1080:r=10:d=1",
            "-frames:v",
            "10",
            "-c:v",
            "ffv1",
        ])
        .arg(&segment)
        .status()
        .unwrap();
    assert!(status.success());

    let activity = Arc::new(ReplayDecodeActivity::default());
    let decoded = Arc::new(std::sync::Barrier::new(5));
    let release = Arc::new(std::sync::Barrier::new(5));
    thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..4 {
            let activity = Arc::clone(&activity);
            let decoded = Arc::clone(&decoded);
            let release = Arc::clone(&release);
            let segment = segment.clone();
            handles.push(scope.spawn(move || {
                decode_canonical_frames_with_activity(
                    &segment,
                    10,
                    DecodeContext::Replay,
                    Some(&activity),
                    |index, _| {
                        if index == 0 {
                            decoded.wait();
                            release.wait();
                        }
                        Ok(())
                    },
                )
            }));
        }
        decoded.wait();
        let pids = activity
            .live_pids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(pids.len(), 4);
        assert!(pids.into_iter().all(|pid| process_rss_bytes(pid).is_some()));
        release.wait();
        for handle in handles {
            handle.join().unwrap().unwrap();
        }
    });
    assert_eq!(activity.children.load(Ordering::Acquire), 4);
    assert_eq!(activity.maximum_active.load(Ordering::Acquire), 4);
    let details = activity
        .decoder_details
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(details.len(), 4);
    assert!(details.iter().all(|detail| detail.rss_peak_bytes > 0));
    assert!(activity.ffmpeg_rss_peak_total_bytes.load(Ordering::Acquire) > 0);
    assert!(activity.process_rss_peak_bytes.load(Ordering::Acquire) > 0);
}

#[test]
fn verified_temporary_segment_decodes_through_ffmpeg_stdin() {
    let root = tempfile::tempdir().unwrap();
    let segment = root.path().join("fixture.mkv");
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=1920x1080:r=10:d=0.1",
            "-frames:v",
            "1",
            "-c:v",
            "ffv1",
        ])
        .arg(&segment)
        .status()
        .unwrap();
    assert!(status.success());
    let digest = decode_canonical_source_with_program_and_timing(
        DecodeSource::File(File::open(segment).unwrap()),
        1,
        DecodeContext::Replay,
        None,
        OsStr::new("ffmpeg"),
        |_, pixels, _| {
            assert_eq!(pixels.len(), 1920 * 1080 * 3);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(digest, crate::digest_bytes(&vec![0_u8; 1920 * 1080 * 3]));
}

#[test]
fn segment_resolver_gets_a_missing_local_object_from_remote() {
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("store");
    ensure_store(&store).unwrap();
    let bytes = b"remote segment";
    let sha256 = digest(bytes);
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let remote = SegmentRemote::new(object_store, "test".to_owned()).unwrap();
    let mut source = tempfile::tempfile().unwrap();
    source.write_all(bytes).unwrap();
    source.rewind().unwrap();
    remote
        .upload_verified(source, &sha256, bytes.len() as u64)
        .unwrap();
    let resolver = SegmentResolver {
        remote: Some(remote),
        local_segment_decodes: Arc::new(AtomicU64::new(0)),
    };
    let session = CaptureSession {
        schema: SESSION_SCHEMA.to_owned(),
        diagnostic_sha256: Some("1".repeat(64)),
        source_kind: SourceKind::LiveRun,
        source_session_id: "session".to_owned(),
        capture_generation: 1,
        profile_sha256: "2".repeat(64),
        catalog_sha256: "3".repeat(64),
        recognition_interval_ms: 100,
        processed_ticks: 1,
        busy_skips: 0,
        maximum_consecutive_busy_skips: 0,
        completeness: "complete".to_owned(),
        game_version: GameVersionState::NotObserved,
        canonical_frames: Vec::new(),
        normalization_pairs: Vec::new(),
        artifacts: vec![CorpusArtifact {
            kind: "canonical_segment".to_owned(),
            source_path: "recognition/segment-0000.mkv".to_owned(),
            sha256,
            bytes: bytes.len() as u64,
        }],
    };
    let ResolvedSegment::Remote(segment) = resolver
        .resolve(&store, &session, "recognition/segment-0000.mkv")
        .unwrap()
    else {
        panic!("missing local segment must resolve remotely");
    };
    let mut actual = Vec::new();
    segment.input().unwrap().read_to_end(&mut actual).unwrap();
    assert_eq!(actual, bytes);
    assert!(
        !store
            .join("objects")
            .join(&session.artifacts[0].sha256)
            .exists()
    );
}

#[test]
fn replay_segment_prefetch_resolves_exactly_once() {
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("store");
    ensure_store(&store).unwrap();
    let bytes = b"prefetched remote segment";
    let sha256 = digest(bytes);
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let remote = SegmentRemote::new(object_store, "test".to_owned()).unwrap();
    let mut source = tempfile::tempfile().unwrap();
    source.write_all(bytes).unwrap();
    source.rewind().unwrap();
    remote
        .upload_verified(source, &sha256, bytes.len() as u64)
        .unwrap();
    let resolver = SegmentResolver {
        remote: Some(remote.clone()),
        local_segment_decodes: Arc::new(AtomicU64::new(0)),
    };
    let session = CaptureSession {
        schema: SESSION_SCHEMA.to_owned(),
        diagnostic_sha256: Some("1".repeat(64)),
        source_kind: SourceKind::LiveRun,
        source_session_id: "session".to_owned(),
        capture_generation: 1,
        profile_sha256: "2".repeat(64),
        catalog_sha256: "3".repeat(64),
        recognition_interval_ms: 100,
        processed_ticks: 1,
        busy_skips: 0,
        maximum_consecutive_busy_skips: 0,
        completeness: "complete".to_owned(),
        game_version: GameVersionState::NotObserved,
        canonical_frames: Vec::new(),
        normalization_pairs: Vec::new(),
        artifacts: vec![CorpusArtifact {
            kind: "canonical_segment".to_owned(),
            source_path: "recognition/segment-0001.mkv".to_owned(),
            sha256,
            bytes: bytes.len() as u64,
        }],
    };
    let prefetched = PrefetchedReplaySegment::start(
        1,
        store,
        session,
        "recognition/segment-0001.mkv".to_owned(),
        resolver,
    );
    assert_eq!(prefetched.segment_index, 1);
    assert!(matches!(
        prefetched.finish().unwrap(),
        ResolvedSegment::Remote(_)
    ));
    assert_eq!(remote.metrics().downloaded_segments, 1);
    assert_eq!(remote.metrics().downloaded_bytes, bytes.len() as u64);
}

#[test]
fn local_segment_resolution_does_not_touch_the_configured_remote() {
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("store");
    ensure_store(&store).unwrap();
    let bytes = b"local segment";
    let sha256 = digest(bytes);
    fs::write(store.join("objects").join(&sha256), bytes).unwrap();
    let remote = SegmentRemote::new(Arc::new(InMemory::new()), "test".to_owned()).unwrap();
    let resolver = SegmentResolver {
        remote: Some(remote.clone()),
        local_segment_decodes: Arc::new(AtomicU64::new(0)),
    };
    let session = CaptureSession {
        schema: SESSION_SCHEMA.to_owned(),
        diagnostic_sha256: Some("1".repeat(64)),
        source_kind: SourceKind::LiveRun,
        source_session_id: "session".to_owned(),
        capture_generation: 1,
        profile_sha256: "2".repeat(64),
        catalog_sha256: "3".repeat(64),
        recognition_interval_ms: 100,
        processed_ticks: 1,
        busy_skips: 0,
        maximum_consecutive_busy_skips: 0,
        completeness: "complete".to_owned(),
        game_version: GameVersionState::NotObserved,
        canonical_frames: Vec::new(),
        normalization_pairs: Vec::new(),
        artifacts: vec![CorpusArtifact {
            kind: "canonical_segment".to_owned(),
            source_path: "recognition/segment-0000.mkv".to_owned(),
            sha256: sha256.clone(),
            bytes: bytes.len() as u64,
        }],
    };
    assert!(matches!(
        resolver
            .resolve(&store, &session, "recognition/segment-0000.mkv")
            .unwrap(),
        ResolvedSegment::Local(_)
    ));
    assert_eq!(
        remote.metrics(),
        crate::ingest::remote::RemoteMetrics::default()
    );
}

#[test]
fn failed_decoder_spawn_releases_memory_without_counting_a_child() {
    let activity = ReplayDecodeActivity::default();
    let result = decode_canonical_frames_with_program(
        Path::new("/does/not/matter.mkv"),
        1,
        DecodeContext::Replay,
        Some(&activity),
        OsStr::new("/scorepeek/missing-ffmpeg"),
        |_, _| Ok(()),
    );
    assert!(result.is_err());
    assert_eq!(activity.children.load(Ordering::Acquire), 0);
    assert_eq!(activity.active.load(Ordering::Acquire), 0);
    assert_eq!(activity.tracked_bytes.load(Ordering::Acquire), 0);
    assert!(
        activity
            .decoder_details
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
}

#[test]
fn panicking_decoder_consumer_kills_reaps_and_releases_the_child() {
    let root = tempfile::tempdir().unwrap();
    let segment = root.path().join("fixture.mkv");
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=1920x1080:r=10:d=1",
            "-frames:v",
            "10",
            "-c:v",
            "ffv1",
        ])
        .arg(&segment)
        .status()
        .unwrap();
    assert!(status.success());

    let activity = ReplayDecodeActivity::default();
    let result = decode_canonical_frames_with_activity(
        &segment,
        10,
        DecodeContext::Replay,
        Some(&activity),
        |_, _| panic!("consumer failure"),
    );
    assert!(matches!(result, Err(CorpusError::InvalidReplay(_))));
    assert_eq!(activity.active.load(Ordering::Acquire), 0);
    assert_eq!(activity.tracked_bytes.load(Ordering::Acquire), 0);
    assert!(
        activity
            .live_pids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    assert_eq!(
        activity
            .decoder_details
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        1
    );
}

fn expected_result() -> ExpectedResult {
    ExpectedResult {
        play_side: "one_player".to_owned(),
        play_mode: "single_play".to_owned(),
        play_type: PlayType::Single,
        difficulty: Difficulty::Hyper,
        level: 8,
        notes: 100,
        current_score: 150,
        judgments: Some(ResultJudgments {
            pgreat: 70,
            great: 10,
            good: 5,
            bad: 3,
            poor: 2,
        }),
        miss_count: Some(SupplementalResultValue::Known { value: 2 }),
        timing: Some(ResultTiming {
            fast: SupplementalResultValue::Known { value: 4 },
            slow: SupplementalResultValue::Known { value: 5 },
        }),
        combo_break: Some(SupplementalResultValue::Known { value: 1 }),
        previous_best: Some(PreviousBest {
            clear_type: PreviousBestValue::Known {
                value: "CLEAR".to_owned(),
            },
            score: PreviousBestValue::Known { value: 140 },
            miss_count: PreviousBestValue::Known { value: 3 },
        }),
        play_options: Some(vec![PlayOption::Random, PlayOption::Legacy]),
    }
}

fn episode_with_outcome(outcome: AttemptOutcome) -> RegressionEpisode {
    RegressionEpisode {
        episode_id: "result-1".to_owned(),
        expected_song_id: "00000000-0000-0000-0000-000000000001".to_owned(),
        expected_clear_type: "FAILED".to_owned(),
        expected_result: expected_result(),
        stable_sequences: vec![4],
        attempt: Some(AttemptTruth {
            attempt_key: "attempt-1".to_owned(),
            parent_attempt_key: None,
            select_span: Some(SequenceSpan {
                first_sequence: 1,
                last_sequence: 1,
            }),
            decide_span: Some(SequenceSpan {
                first_sequence: 2,
                last_sequence: 2,
            }),
            play_span: Some(SequenceSpan {
                first_sequence: 3,
                last_sequence: 3,
            }),
            result_span: SequenceSpan {
                first_sequence: 4,
                last_sequence: 4,
            },
            outcome,
        }),
    }
}

#[test]
fn no_result_retains_field_truth_without_requiring_clear_type_or_an_event() {
    let accepted = episode_with_outcome(AttemptOutcome::Accepted);
    assert!(episode_requires_clear_type(&accepted));
    assert!(episode_expects_result_event(&accepted));

    let no_result = episode_with_outcome(AttemptOutcome::NoResult);
    assert!(!episode_requires_clear_type(&no_result));
    assert!(!episode_expects_result_event(&no_result));
}

#[test]
fn numeric_dataset_selects_only_the_stable_result_episode() {
    let screen_sequences = [
        (8, true),
        (18, true),
        (30, true),
        (41, false),
        (50, true),
        (61, true),
    ];
    assert_eq!(
        numeric_episode_sequences(&screen_sequences, &[18], 32).unwrap(),
        vec![18, 8, 30]
    );
    assert!(numeric_episode_sequences(&screen_sequences, &[41], 32).is_err());
}

#[test]
fn numeric_dataset_caps_frames_nearest_to_stable_evidence() {
    let screen_sequences = (1..=10)
        .map(|sequence| (sequence, true))
        .collect::<Vec<_>>();
    assert_eq!(
        numeric_episode_sequences(&screen_sequences, &[6], 4).unwrap(),
        vec![6, 5, 7, 4]
    );
}

#[test]
fn numeric_dataset_collects_visible_level_and_notes_truth() {
    let labels = numeric_field_labels(&expected_result()).unwrap();
    assert_eq!(
        labels.get(&NumericField::Level).map(String::as_str),
        Some("8")
    );
    assert_eq!(
        labels.get(&NumericField::Notes).map(String::as_str),
        Some("0100")
    );
    assert_eq!(labels.len(), 14);
    assert!(numeric_field_uses_sequence(NumericField::Level, 20, &[20]));
    assert!(!numeric_field_uses_sequence(NumericField::Notes, 19, &[20]));
    assert!(numeric_field_uses_sequence(NumericField::Good, 19, &[20]));
}

#[test]
fn result_play_side_labels_follow_panel_side_for_sp_and_dp() {
    assert!(valid_expected_play_side("one_player"));
    assert!(valid_expected_play_side("two_player"));
    assert!(!valid_expected_play_side("not_applicable"));
    assert!(expected_play_side_matches(
        ResultPanelSide::Left,
        "one_player"
    ));
    assert!(expected_play_side_matches(
        ResultPanelSide::Right,
        "two_player"
    ));
    assert!(!expected_play_side_matches(
        ResultPanelSide::Right,
        "one_player"
    ));
}

#[test]
fn numeric_dataset_authors_from_segment_backed_canonical_frames() {
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("store");
    ensure_store(&store).unwrap();

    let expected = expected_result();
    let labels = numeric_field_labels(&expected).unwrap();
    let mut pixels = [200_u8, 100, 20].repeat(1_920 * 1_080);
    for row in pixels.chunks_exact_mut(1_920 * 3) {
        row[1_320 * 3..].fill(0);
    }
    for y in [451, 655] {
        for x in 0..518 {
            pixels[(y * 1_920 + x) * 3..][..3].copy_from_slice(&[0, 0, 0]);
        }
    }
    let ScreenRgb8Crops::Result(crops) = route_screen_rgb8_crops(
        &pixels,
        scorepeek_core::replay::ScreenCropRoute::Result(
            scorepeek_core::replay::ResultPanelSide::Left,
        ),
    )
    .unwrap() else {
        unreachable!();
    };
    let rois = numeric_crops(&crops, &labels)
        .into_iter()
        .map(|(_, _, crop)| crop.roi)
        .collect::<Vec<_>>();
    for (index, roi) in rois.iter().enumerate() {
        let offset = (roi.y as usize * 1_920 + roi.x as usize) * 3;
        pixels[offset..offset + 3].copy_from_slice(&[
            u8::try_from(index + 1).unwrap(),
            u8::try_from(index + 2).unwrap(),
            u8::try_from(index + 3).unwrap(),
        ]);
    }
    assert_eq!(
        inspect_canonical_rgb8(&pixels).unwrap().screen,
        ScreenClass::Result
    );

    let segment_path = root.path().join("segment.mkv");
    let output = File::create(&segment_path).unwrap();
    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-video_size",
            "1920x1080",
            "-framerate",
            "10",
            "-i",
            "pipe:0",
            "-an",
            "-c:v",
            "libx264rgb",
            "-crf",
            "0",
            "-preset",
            "ultrafast",
            "-frames:v",
            "1",
            "-f",
            "matroska",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::from(output))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&pixels).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let segment_bytes = fs::read(&segment_path).unwrap();
    let segment_sha256 = digest(&segment_bytes);
    fs::write(store.join("objects").join(&segment_sha256), &segment_bytes).unwrap();
    let raw_sha256 = digest(&pixels);
    let tick_bytes = b"{\"sequence\":1,\"source_sequence\":1,\"monotonic_ms\":100,\"screen\":\"result\",\"semantic_episode_id\":1,\"disposition\":\"retained\"}\n";
    let tick_sha256 = digest(tick_bytes);
    fs::write(store.join("objects").join(&tick_sha256), tick_bytes).unwrap();
    let canonical_bytes = canonical_json(&serde_json::json!({
        "schema": "scorepeek-canonical-session-recording-v2",
        "completeness": "complete",
        "ffmpeg_sha256": "1".repeat(64),
        "ffmpeg_version": "test",
        "tick_index_sha256": tick_sha256,
        "tick_count": 1,
        "segments": [{
            "path": "segment-0000.mkv",
            "first_sequence": 1,
            "last_sequence": 1,
            "frames": 1,
            "raw_rgb24_sha256": raw_sha256,
            "encoded_sha256": segment_sha256,
            "bytes": segment_bytes.len(),
        }],
        "dropped_frames": 0,
        "completeness_reasons": [],
        "memory_limit_bytes": 1_073_741_824_u64,
        "memory_high_water_bytes": 6_220_800_u64,
        "integrity_verification": "deferred_to_import",
    }))
    .unwrap();
    let canonical_sha256 = digest(&canonical_bytes);
    let canonical_bytes_len = u64::try_from(canonical_bytes.len()).unwrap();
    fs::write(
        store.join("objects").join(&canonical_sha256),
        canonical_bytes,
    )
    .unwrap();

    let session = CaptureSession {
        schema: SESSION_SCHEMA.to_owned(),
        diagnostic_sha256: Some("2".repeat(64)),
        source_kind: SourceKind::LiveRun,
        source_session_id: "segment-backed".to_owned(),
        capture_generation: 1,
        profile_sha256: "3".repeat(64),
        catalog_sha256: "4".repeat(64),
        recognition_interval_ms: 100,
        processed_ticks: 1,
        busy_skips: 0,
        maximum_consecutive_busy_skips: 0,
        completeness: "complete".to_owned(),
        game_version: GameVersionState::NotObserved,
        canonical_frames: vec![ReviewFrame {
            sequence: 1,
            artifact_sha256: segment_sha256.clone(),
        }],
        normalization_pairs: Vec::new(),
        artifacts: vec![
            CorpusArtifact {
                kind: "canonical_manifest".to_owned(),
                source_path: "recognition/canonical-manifest.json".to_owned(),
                sha256: canonical_sha256,
                bytes: canonical_bytes_len,
            },
            CorpusArtifact {
                kind: "canonical_ticks".to_owned(),
                source_path: "recognition/canonical-ticks.ndjson".to_owned(),
                sha256: tick_sha256,
                bytes: u64::try_from(tick_bytes.len()).unwrap(),
            },
            CorpusArtifact {
                kind: "canonical_segment".to_owned(),
                source_path: "recognition/segment-0000.mkv".to_owned(),
                sha256: segment_sha256,
                bytes: u64::try_from(segment_bytes.len()).unwrap(),
            },
        ],
    };
    let session_bytes = canonical_json(&session).unwrap();
    let session_sha256 = digest(&session_bytes);
    publish_document(
        &store
            .join("sessions")
            .join(format!("{session_sha256}.json")),
        &session_bytes,
    )
    .unwrap();
    let label = RegressionLabel {
        schema: LABEL_SCHEMA.to_owned(),
        session_sha256: session_sha256.clone(),
        disposition: LabelDisposition::Include,
        episodes: vec![RegressionEpisode {
            episode_id: "result-1".to_owned(),
            expected_song_id: "00000000-0000-0000-0000-000000000001".to_owned(),
            expected_clear_type: "CLEAR".to_owned(),
            expected_result: expected,
            stable_sequences: vec![1],
            attempt: None,
        }],
        negative_frames: Vec::new(),
    };
    let label_bytes = canonical_json(&label).unwrap();
    let label_sha256 = digest(&label_bytes);
    publish_document(
        &store.join("labels").join(format!("{label_sha256}.json")),
        &label_bytes,
    )
    .unwrap();
    let suite = RegressionSuite {
        schema: SUITE_SCHEMA.to_owned(),
        previous_generation_sha256: None,
        entries: vec![SuiteEntry {
            session_sha256,
            label_sha256,
        }],
    };
    let suite_bytes = canonical_json(&suite).unwrap();
    let suite_sha256 = digest(&suite_bytes);
    publish_document(
        &store.join("suites").join(format!("{suite_sha256}.json")),
        &suite_bytes,
    )
    .unwrap();
    publish_active(&store, &suite_sha256).unwrap();

    let summary = author_numeric_dataset(&store, &root.path().join("dataset")).unwrap();
    assert_eq!(summary.sessions, 1);
    assert_eq!(summary.episodes, 1);
    assert!(summary.samples > 0);
}

#[test]
fn optional_numeric_unknown_is_safe_but_wrong_known_is_not() {
    let expected = SupplementalResultValue::Known { value: 7_u32 };
    assert!(optional_supplemental_matches(
        &SupplementalResultValue::Unknown {
            reason: scorepeek_core::replay::ResultFieldUnknownReason::Empty,
        },
        &expected,
    ));
    assert!(!optional_supplemental_matches(
        &SupplementalResultValue::Known { value: 8 },
        &expected,
    ));
    assert!(optional_previous_matches(
        &PreviousBestValue::Unknown {
            reason: scorepeek_core::replay::ResultFieldUnknownReason::Empty,
        },
        &PreviousBestValue::Known { value: 7_u32 },
    ));
}

#[test]
fn v5_result_validation_rejects_unreplayable_typed_values() {
    assert!(valid_expected_result(&expected_result()));

    let mut unbounded = expected_result();
    unbounded.miss_count = Some(SupplementalResultValue::Known { value: 101 });
    unbounded.timing = Some(ResultTiming {
        fast: SupplementalResultValue::Known { value: 102 },
        slow: SupplementalResultValue::Known { value: 103 },
    });
    unbounded.combo_break = Some(SupplementalResultValue::Known { value: 104 });
    unbounded.judgments.as_mut().unwrap().poor = 105;
    unbounded.previous_best.as_mut().unwrap().miss_count = PreviousBestValue::Known { value: 106 };
    assert!(valid_expected_result(&unbounded));

    let mut note_judgment_overflow = expected_result();
    note_judgment_overflow.judgments.as_mut().unwrap().bad = 101;
    assert!(!valid_expected_result(&note_judgment_overflow));

    let mut inconsistent_no_play = expected_result();
    inconsistent_no_play
        .previous_best
        .as_mut()
        .unwrap()
        .clear_type = PreviousBestValue::NotPlayed;
    assert!(!valid_expected_result(&inconsistent_no_play));

    let mut invalid_clear = expected_result();
    invalid_clear.previous_best.as_mut().unwrap().clear_type = PreviousBestValue::Known {
        value: "CLEER".to_owned(),
    };
    assert!(!valid_expected_result(&invalid_clear));

    let mut score_overflow = expected_result();
    score_overflow.notes = u32::MAX;
    score_overflow.current_score = u32::MAX;
    score_overflow.judgments = Some(ResultJudgments {
        pgreat: u32::MAX,
        great: 1,
        good: 0,
        bad: 0,
        poor: 0,
    });
    assert!(!valid_expected_result(&score_overflow));
}

#[test]
fn v5_play_options_require_an_ordered_distinct_list() {
    assert!(!valid_play_options(None));
    assert!(valid_play_options(Some(&[])));
    assert!(valid_play_options(Some(&[
        PlayOption::Random,
        PlayOption::Legacy,
    ])));
    assert!(!valid_play_options(Some(&[
        PlayOption::Random,
        PlayOption::Random,
    ])));
}

#[test]
fn v5_replay_requires_exact_known_play_options_in_display_order() {
    let expected = [PlayOption::Random, PlayOption::Legacy];
    assert!(expected_play_options_match(
        &PlayOptions::Known {
            values: expected.to_vec(),
        },
        &expected,
    ));
    assert!(!expected_play_options_match(
        &PlayOptions::Known {
            values: vec![PlayOption::Legacy, PlayOption::Random],
        },
        &expected,
    ));
    assert!(!expected_play_options_match(
        &PlayOptions::Unknown {
            reason: scorepeek_core::replay::PlayOptionsUnknownReason::Unrecognized,
        },
        &expected,
    ));
}

#[test]
fn invalid_v5_result_does_not_publish_an_active_suite() {
    let temporary = tempfile::tempdir().unwrap();
    let store = temporary.path().join("store");
    let draft_path = temporary.path().join("draft.json");
    let label_path = temporary.path().join("label.json");
    let session_sha256 = "1".repeat(64);
    let draft = ReviewDraft {
        schema: DRAFT_SCHEMA.to_owned(),
        session_sha256: session_sha256.clone(),
        diagnostic_sha256: Some("2".repeat(64)),
        source_session_id: "session".to_owned(),
        canonical_frames: vec![ReviewFrame {
            sequence: 1,
            artifact_sha256: "3".repeat(64),
        }],
        observation_count: 1,
        completeness: "complete".to_owned(),
    };
    let mut invalid_result = expected_result();
    invalid_result.judgments.as_mut().unwrap().bad = 101;
    let label = RegressionLabel {
        schema: LABEL_SCHEMA.to_owned(),
        session_sha256,
        disposition: LabelDisposition::Include,
        episodes: vec![RegressionEpisode {
            episode_id: "episode-1".to_owned(),
            expected_song_id: "song-1".to_owned(),
            expected_clear_type: "CLEAR".to_owned(),
            expected_result: invalid_result,
            stable_sequences: vec![1],
            attempt: None,
        }],
        negative_frames: Vec::new(),
    };
    fs::write(&draft_path, canonical_json(&draft).unwrap()).unwrap();
    fs::write(&label_path, canonical_json(&label).unwrap()).unwrap();

    assert!(apply_review(&store, &draft_path, &label_path).is_err());
    assert!(!store.join("active-suite.json").exists());
}

#[test]
fn partial_review_accepts_only_explicitly_retained_negative_frames() {
    let digest = "1".repeat(64);
    let draft = ReviewDraft {
        schema: DRAFT_SCHEMA.to_owned(),
        session_sha256: digest.clone(),
        diagnostic_sha256: Some("2".repeat(64)),
        source_session_id: "session".to_owned(),
        canonical_frames: vec![ReviewFrame {
            sequence: 1,
            artifact_sha256: "3".repeat(64),
        }],
        observation_count: 0,
        completeness: "partial".to_owned(),
    };
    let label = RegressionLabel {
        schema: LABEL_SCHEMA.to_owned(),
        session_sha256: digest,
        disposition: LabelDisposition::Include,
        episodes: Vec::new(),
        negative_frames: vec![1],
    };
    assert!(validate_label(&draft, &label).is_ok());
    let missing = RegressionLabel {
        negative_frames: vec![2],
        ..label
    };
    assert!(validate_label(&draft, &missing).is_err());
}

#[test]
fn play_mode_truth_requires_matching_sp_or_dp() {
    assert!(play_mode_matches_type("single_play", PlayType::Single));
    assert!(play_mode_matches_type("double_play", PlayType::Double));
    assert!(!play_mode_matches_type("single_play", PlayType::Double));
    assert!(!play_mode_matches_type("double_play", PlayType::Single));
    assert!(!play_mode_matches_type("unknown", PlayType::Single));
}

#[test]
fn only_retained_unknown_play_endpoints_allow_layout_calibration() {
    let tick = CanonicalTick {
        sequence: 1,
        source_sequence: 1,
        monotonic_ms: 100,
        screen: ScreenClass::Unknown,
        semantic_episode_id: None,
        disposition: "retained".to_owned(),
    };
    let ticks = BTreeMap::from([(1, &tick)]);
    let span = SequenceSpan {
        first_sequence: 1,
        last_sequence: 1,
    };
    assert!(validate_screen_span(&ticks, span, ScreenClass::Play, false).is_ok());
    for screen in [
        ScreenClass::MusicSelect,
        ScreenClass::DecideTransition,
        ScreenClass::Result,
    ] {
        assert!(validate_screen_span(&ticks, span, screen, false).is_err());
    }
    let elided = CanonicalTick {
        disposition: "elided".to_owned(),
        ..tick
    };
    assert!(
        validate_screen_span(
            &BTreeMap::from([(1, &elided)]),
            span,
            ScreenClass::Play,
            false
        )
        .is_err()
    );
}
