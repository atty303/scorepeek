use std::io::{BufRead as _, BufReader, Write};
use std::os::unix::fs::symlink;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use scorepeek_core::recognition::result::ResultFieldValue;

use super::*;

#[test]
fn scores_survive_socket_path_collision_at_startup() {
    let temporary = tempfile::tempdir().unwrap();
    let socket_directory = temporary.path().join("scorepeek");
    fs::create_dir(&socket_directory).unwrap();
    let socket_path = socket_directory.join(SOCKET_NAME);
    fs::write(&socket_path, b"operator file").unwrap();
    let state = state();
    let channel = EventChannel::start_at(temporary.path(), Arc::clone(&state));
    assert!(channel.is_err());
    let mut output = RoutineOutput::from_channel(state, channel, false, None)
        .unwrap_or_else(|(error, _)| panic!("{error}"));
    let path = temporary.path().join("scores.sqlite3");
    output.enable_scores(&path).unwrap();
    prepare_accepted_attempt(&mut output);
    output.publish(&accepted_result_event(1)).unwrap();
    output.publish(&accepted_result_event(2)).unwrap();
    output
        .publish(&semantic_episode_event(
            3,
            "result",
            SemanticEpisodePhase::Closing,
        ))
        .unwrap();
    output
        .publish(&semantic_episode_event(
            3,
            "result",
            SemanticEpisodePhase::Finalized,
        ))
        .unwrap();
    let health = output.scores.as_mut().unwrap().finish();
    assert_eq!(health.committed, 2);
    assert!(health.failure.is_none(), "{health:?}");
    assert!(output.state.lock().unwrap().channel_start_failure.is_some());
    output.refresh().unwrap();
    drop(output);
    assert_eq!(fs::read(socket_path).unwrap(), b"operator file");
    let database = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        database
            .query_row("SELECT count(*) FROM play_results", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn scores_consume_public_results_independently_of_socket_and_database_failures() {
    let temporary = tempfile::tempdir().unwrap();
    for failing_database in [false, true] {
        let mut output = test_output(state(), disconnected_test_channel());
        output.publish_frontend_snapshots = false;
        let path = if failing_database {
            temporary.path().to_owned()
        } else {
            temporary.path().join("scores.sqlite3")
        };
        output.enable_scores(&path).unwrap();
        prepare_accepted_attempt(&mut output);
        output.publish(&accepted_result_event(1)).unwrap();
        output.publish(&accepted_result_event(2)).unwrap();
        output
            .publish(&semantic_episode_event(
                3,
                "result",
                SemanticEpisodePhase::Closing,
            ))
            .unwrap();
        output
            .publish(&semantic_episode_event(
                3,
                "result",
                SemanticEpisodePhase::Finalized,
            ))
            .unwrap();
        let snapshot = serde_json::to_value(&output.state.lock().unwrap().public).unwrap();
        let result = &snapshot["result"];
        assert_eq!(result["state"]["result"]["current_score"], 1286);
        assert!(result["emitted_unix_ms"].as_i64().unwrap() > 0);
        assert!(
            output
                .channel
                .as_ref()
                .unwrap()
                .health
                .server_failed
                .load(Ordering::Acquire)
        );
        let health = output.scores.as_mut().unwrap().finish();
        if failing_database {
            assert_eq!(health.failure.as_deref(), Some("database_open"));
        } else {
            assert!(health.failure.is_none(), "{health:?}");
            assert_eq!(health.committed, 2);
            let database = rusqlite::Connection::open(&path).unwrap();
            let json: String = database
                .query_row("SELECT event_json FROM play_results", [], |row| row.get(0))
                .unwrap();
            let stored = serde_json::from_str::<Value>(&json).unwrap();
            assert_eq!(stored["schema"], "scorepeek-stored-result-v2");
            assert_eq!(stored["result"], result["state"]["result"]);
            assert_eq!(stored["event_id"], result["event_id"]);
            let values: (i64, i64, i64) = database
                .query_row("SELECT score,miss,clear FROM chart_bests", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .unwrap();
            assert_eq!(values, (1286, 3, 4));
        }
    }
}

#[test]
fn scores_select_only_uses_production_projection_without_creating_a_play() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("scores.sqlite3");
    let mut output = RoutineOutput::start_headless("invocation-1".into(), "a".repeat(64));
    assert!(output.scores.is_none());
    output.enable_scores(&path).unwrap();
    let session = "invocation-1-session-1".to_owned();
    output
        .publish(&select_best_test_episode(
            &session,
            SemanticEpisodePhase::Started,
            1,
        ))
        .unwrap();
    let (fields, evidence, presentation) = select_best_test_observation();
    for sequence in 2..=5 {
        output
            .reduce_music_select_observation(
                Some(&session),
                Some(1),
                sequence,
                sequence * 100,
                &fields,
                &evidence,
                &presentation,
            )
            .unwrap();
    }
    let before_store_change = output.state.lock().unwrap().public.next_sequence;
    let health = output.scores.as_mut().unwrap().finish();
    assert!(health.failure.is_none(), "{health:?}");
    assert_eq!(health.committed, 1);
    output.refresh_scores().unwrap();
    assert_eq!(
        output.state.lock().unwrap().public.next_sequence,
        before_store_change + 1
    );
    let database = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        database
            .query_row("SELECT count(*) FROM play_results", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let (score, result): (i64, Option<String>) = database
        .query_row("SELECT score,result_score FROM chart_bests", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(score, 1200);
    assert!(result.is_none());
}

#[test]
fn watcher_stop_publishes_store_changes_drained_during_shutdown() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("scores.sqlite3");
    let mut output = RoutineOutput::start_headless("invocation-1".into(), "a".repeat(64));
    output.enable_scores(&path).unwrap();
    let session = "invocation-1-session-1".to_owned();
    output
        .publish(&select_best_test_episode(
            &session,
            SemanticEpisodePhase::Started,
            1,
        ))
        .unwrap();
    let (fields, evidence, presentation) = select_best_test_observation();
    for sequence in 2..=5 {
        output
            .reduce_music_select_observation(
                Some(&session),
                Some(1),
                sequence,
                sequence * 100,
                &fields,
                &evidence,
                &presentation,
            )
            .unwrap();
    }
    let before_stop = output.state.lock().unwrap().public.next_sequence;
    output
        .publish(&RunEvent {
            schema: RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::WatcherStopped {
                invocation_id: "invocation-1".into(),
                reason: "test".into(),
            },
        })
        .unwrap();

    let snapshot = serde_json::to_value(&output.state.lock().unwrap().public).unwrap();
    assert_eq!(snapshot["status"]["watcher"], "stopped");
    assert_eq!(snapshot["next_sequence"], before_stop + 2);
    assert_eq!(
        rusqlite::Connection::open(path)
            .unwrap()
            .query_row("SELECT count(*) FROM chart_bests", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

fn state() -> Arc<Mutex<RunViewState>> {
    Arc::new(Mutex::new(RunViewState::new(
        "invocation-1".to_owned(),
        "a".repeat(64),
        true,
    )))
}

fn test_output(state: Arc<Mutex<RunViewState>>, channel: EventChannel) -> RoutineOutput {
    RoutineOutput {
        state,
        channel: Some(channel),
        scores: None,
        publish_frontend_snapshots: false,
        next_sequence: 1,
        timing_active: false,
        output_us: 0,
        headless_events: Vec::new(),
        core_reducer: RunEventReducer::new(),
        diagnostics: None,
    }
}

fn disconnected_test_channel() -> EventChannel {
    let (sender, receiver) = std::sync::mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
    drop(receiver);
    EventChannel {
        sender,
        stop: Arc::new(AtomicBool::new(false)),
        health: Arc::new(ChannelHealth::default()),
        thread: None,
        socket_path: PathBuf::new(),
        socket_identity: (0, 0),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the fixture intentionally spells out one complete accepted result contract"
)]
fn accepted_result_event(sequence: u64) -> RunEvent {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
    RunEvent {
        schema: RUN_EVENT_SCHEMA.to_owned(),
        kind: RunEventKind::FieldObservation {
            session_id: Some("invocation-1-session-1".to_owned()),
            capture_generation: Some(1),
            screen_episode_id: 0,
            sequence,
            monotonic_start_ms: sequence.saturating_mul(100),
            monotonic_end_ms: sequence.saturating_mul(100).saturating_add(25),
            screen: "result".to_owned(),
            fields: json!({
                "panel_side": ResultPanelSide::Left,
                "title": "OCR TITLE",
                "artist": "OCR ARTIST",
                "clear_type": "CLEAR",
                "clear_type_ocr": "CLEAR",
                "play_options": PlayOptionsObservation {
                    raw_text: Some("USE OPTION RANDOM,LEGACY".to_owned()),
                    parsed: PlayOptions::Known {
                        values: vec![PlayOption::Random, PlayOption::Legacy],
                    },
                    ..PlayOptionsObservation::default()
                }
            }),
            result_song_resolution: json!({ "status": "accepted" }),
            music_select_song_resolution: Value::Null,
            parsed_result_fields: Some(ParsedResultFields {
                resolver_id: "test".to_owned(),
                play_type: ResultFieldValue::Known {
                    value: PlayType::Single,
                },
                difficulty: ResultFieldValue::Known {
                    value: Difficulty::Hyper,
                },
                level: ResultFieldValue::Known { value: 8 },
                notes: ResultFieldValue::Known { value: 764 },
                current_score: ResultFieldValue::Known { value: 1_286 },
                previous_clear_type: PreviousBestValue::Known {
                    value: "CLEAR".to_owned(),
                },
                previous_score: PreviousBestValue::Known { value: 1_200 },
                previous_miss_count: PreviousBestValue::Known { value: 4 },
                miss_count: SupplementalResultValue::Known { value: 3 },
                pgreat: ResultFieldValue::Known { value: 600 },
                great: ResultFieldValue::Known { value: 86 },
                good: ResultFieldValue::Known { value: 10 },
                bad: ResultFieldValue::Known { value: 5 },
                poor: ResultFieldValue::Known { value: 3 },
                fast: SupplementalResultValue::Known { value: 20 },
                slow: SupplementalResultValue::Known { value: 21 },
                combo_break: SupplementalResultValue::Known { value: 2 },
            }),
            result_chart_resolution: Some(ResultChartResolution::Accepted {
                resolver_id: "scorepeek-result-fields-catalog-constrained-v6".to_owned(),
                chart: scorepeek_core::catalog::Chart {
                    key: scorepeek_core::catalog::ChartKey {
                        play_type: PlayType::Single,
                        difficulty: Difficulty::Hyper,
                    },
                    level: 8,
                    notes: 764,
                },
                current_score: 1_286,
            }),
            result_performance_resolution: Some(ResultPerformanceResolution::Accepted {
                resolver_id: "scorepeek-result-performance-v1".to_owned(),
                judgments: ResultJudgments {
                    pgreat: 600,
                    great: 86,
                    good: 10,
                    bad: 5,
                    poor: 3,
                },
                miss_count: SupplementalResultValue::Known { value: 3 },
                timing: ResultTiming {
                    fast: SupplementalResultValue::Known { value: 20 },
                    slow: SupplementalResultValue::Known { value: 21 },
                },
                combo_break: SupplementalResultValue::Known { value: 2 },
                previous_best: PreviousBest {
                    clear_type: scorepeek_core::recognition::result::PreviousBestValue::Known {
                        value: "CLEAR".to_owned(),
                    },
                    score: scorepeek_core::recognition::result::PreviousBestValue::Known {
                        value: 1_200,
                    },
                    miss_count: scorepeek_core::recognition::result::PreviousBestValue::Known {
                        value: 4,
                    },
                },
            }),
            current_score_ocr_resolution: None,
            numeric_batch: None,
            joint_evidence: JointEvidenceObservation {
                catalog_song_count: 0,
                candidates: vec![JointEvidenceCandidate {
                    song_id,
                    chart: scorepeek_core::catalog::Chart {
                        key: scorepeek_core::catalog::ChartKey {
                            play_type: PlayType::Single,
                            difficulty: Difficulty::Hyper,
                        },
                        level: 8,
                        notes: 764,
                    },
                    display_titles: vec!["CATALOG TITLE".to_owned()],
                    artist: "CATALOG ARTIST".to_owned(),
                    family_support: BTreeMap::from([
                        (EvidenceFamily::ResultTitle, 120),
                        (EvidenceFamily::ResultArtist, 120),
                        (EvidenceFamily::ResultChart, 160),
                    ]),
                    support: 400,
                }],
            },
            processing_timing: Value::Null,
            song_resolution_presentation: Box::new(SongResolutionPresentation::Accepted {
                reason: None,
                selected: SongPresentation {
                    scorepeek_song_id: song_id,
                    display_titles: vec!["CATALOG TITLE".to_owned()],
                    artist: "CATALOG ARTIST".to_owned(),
                },
                runner_up: SongPresentation {
                    scorepeek_song_id: serde_json::from_str(
                        "\"00000000-0000-0000-0000-000000000002\"",
                    )
                    .unwrap(),
                    display_titles: vec!["RUNNER UP".to_owned()],
                    artist: "RUNNER ARTIST".to_owned(),
                },
                evidence_summary: "title edit=0; runner-up margin=4".to_owned(),
            }),
        },
    }
}

fn accepted_double_result_event(sequence: u64) -> RunEvent {
    let mut event = accepted_result_event(sequence);
    let RunEventKind::FieldObservation {
        parsed_result_fields: Some(parsed),
        result_chart_resolution: Some(ResultChartResolution::Accepted { chart, .. }),
        joint_evidence,
        ..
    } = &mut event.kind
    else {
        unreachable!();
    };
    parsed.play_type = ResultFieldValue::Known {
        value: PlayType::Double,
    };
    chart.key.play_type = PlayType::Double;
    joint_evidence.candidates[0].chart.key.play_type = PlayType::Double;
    event
}

fn detected_result_event(
    session_id: &str,
    capture_generation: u64,
    source_sequence: u64,
    result: ResultDomainEvent,
) -> RunEvent {
    RunEvent {
        schema: RUN_EVENT_SCHEMA.to_owned(),
        kind: RunEventKind::ResultChanged {
            session_id: session_id.to_owned(),
            capture_generation,
            source_sequence,
            state: ResultState::Confirmed {
                song: None,
                result: Box::new(result),
            },
        },
    }
}

fn prepare_accepted_attempt(output: &mut RoutineOutput) {
    output.publish(&screen_event(0, "music_select")).unwrap();
    output.publish(&screen_event(0, "play")).unwrap();
    output.publish(&screen_event(0, "result")).unwrap();
    prime_left_result_panel(output);
}

fn prime_left_result_panel(output: &mut RoutineOutput) {
    prime_result_panel(output, ResultPanelSide::Left, 0);
}

fn prime_result_panel(output: &mut RoutineOutput, side: ResultPanelSide, first_sequence: u64) {
    for sequence in [first_sequence, first_sequence.saturating_add(1)] {
        output
            .publish(&RunEvent {
                schema: RUN_EVENT_SCHEMA.to_owned(),
                kind: RunEventKind::RawScreenObserved {
                    session_id: Some("invocation-1-session-1".to_owned()),
                    capture_generation: Some(1),
                    semantic_episode_id: Some(0),
                    sequence,
                    monotonic_start_ms: sequence,
                    monotonic_end_ms: sequence,
                    screen: "result".to_owned(),
                    result_presence: scorepeek_core::recognition::screen::ResultPresenceEvidence {
                        warm_pixels: 0,
                        warm_pixels_min: 0,
                        panel_side:
                            scorepeek_core::recognition::screen::ResultPanelSideState::Known(side),
                        panels: [
                            scorepeek_core::recognition::screen::ResultPanelPresenceEvidence {
                                panel_side: ResultPanelSide::Left,
                                upper_panel_edge_pixels: 0,
                                lower_panel_edge_pixels: 0,
                                qualifies: side == ResultPanelSide::Left,
                            },
                            scorepeek_core::recognition::screen::ResultPanelPresenceEvidence {
                                panel_side: ResultPanelSide::Right,
                                upper_panel_edge_pixels: 0,
                                lower_panel_edge_pixels: 0,
                                qualifies: side == ResultPanelSide::Right,
                            },
                        ],
                        horizontal_edge_pixels_min: 0,
                    },
                    play_presence: scorepeek_core::recognition::screen::PlayPresenceEvidence {
                        qualifying_candidates: 0,
                        top_edge_runs: 0,
                        bottom_edge_runs: 0,
                        candidates: [None, None],
                        top_edge_pixels_min: 0,
                        top_edge_pixels_max: 0,
                        bottom_edge_pixels_min: 0,
                        bottom_edge_pixels_max: 0,
                        vertical_distance_min: 0,
                        vertical_distance_max: 0,
                        edge_center_delta_x2_max: 0,
                        candidate_cluster_delta_x2_max: 0,
                        candidate_cluster_delta_y_max: 0,
                    },
                    unknown_reason: None,
                },
            })
            .unwrap();
    }
}

fn play_options_observation(values: Vec<PlayOption>) -> PlayOptionsObservation {
    PlayOptionsObservation {
        parsed: PlayOptions::Known { values },
        ..PlayOptionsObservation::default()
    }
}

#[test]
fn play_options_require_two_matching_episode_observations() {
    let expected = vec![PlayOption::Random, PlayOption::Legacy];
    let mut accumulator = PlayOptionsEpisodeAccumulator::default();
    accumulator.observe(1, play_options_observation(expected.clone()));
    accumulator.observe(1, play_options_observation(expected.clone()));
    assert_eq!(
        accumulator.resolved(),
        PlayOptions::Unknown {
            reason: PlayOptionsUnknownReason::InsufficientObservations
        }
    );
    accumulator.observe(2, play_options_observation(expected.clone()));
    assert_eq!(
        accumulator.resolved(),
        PlayOptions::Known { values: expected }
    );
}

#[test]
fn conflicting_play_options_remain_optional_unknown() {
    let mut accumulator = PlayOptionsEpisodeAccumulator::default();
    accumulator.observe(1, play_options_observation(vec![PlayOption::Random]));
    accumulator.observe(2, play_options_observation(vec![PlayOption::Mirror]));
    assert_eq!(
        accumulator.resolved(),
        PlayOptions::Unknown {
            reason: PlayOptionsUnknownReason::ConflictingObservations
        }
    );
}

fn accepted_result_without_joint_identity(sequence: u64) -> RunEvent {
    let mut event = accepted_result_event(sequence);
    let RunEventKind::FieldObservation { joint_evidence, .. } = &mut event.kind else {
        unreachable!();
    };
    joint_evidence.candidates.clear();
    event
}

fn screen_event(sequence: u64, screen: &str) -> RunEvent {
    RunEvent {
        schema: RUN_EVENT_SCHEMA.to_owned(),
        kind: RunEventKind::ScreenChanged {
            session_id: Some("invocation-1-session-1".to_owned()),
            capture_generation: Some(1),
            screen_episode_id: sequence,
            sequence,
            monotonic_start_ms: sequence.saturating_mul(100),
            monotonic_end_ms: sequence.saturating_mul(100).saturating_add(25),
            screen: screen.to_owned(),
        },
    }
}

fn semantic_episode_event(sequence: u64, screen: &str, phase: SemanticEpisodePhase) -> RunEvent {
    RunEvent {
        schema: RUN_EVENT_SCHEMA.to_owned(),
        kind: RunEventKind::SemanticScreenEpisodeChanged {
            session_id: Some("invocation-1-session-1".to_owned()),
            capture_generation: Some(1),
            screen_episode_id: sequence,
            sequence,
            monotonic_end_ms: sequence.saturating_mul(100).saturating_add(25),
            screen: screen.to_owned(),
            phase,
        },
    }
}

fn failed_session_finished_event() -> RunEvent {
    RunEvent::from_value(json!({
        "schema": RUN_EVENT_SCHEMA,
        "event": "session_finished",
        "session_id": "invocation-1-session-1",
        "capture_generation": 1,
        "outcome": "error",
        "report": { "error_type": "field_observer_finish_failed" }
    }))
    .unwrap()
}

fn assert_public_fold(events: Vec<RunEvent>) {
    let mut public = event_api::PublicState::new("invocation-1".into());
    let mut consumer = serde_json::to_value(&public).unwrap();
    for internal in events {
        for record in public.project(&internal) {
            let event = serde_json::to_value(record).unwrap();
            event_api::tests::fold(&mut consumer, &event);
            assert_eq!(consumer, serde_json::to_value(&public).unwrap());
        }
    }
}

fn wire_event(sequence: u64) -> QueuedEvent {
    let mut public = event_api::PublicState::new("invocation-1".into());
    public.next_sequence = sequence;
    let event = public
        .project(&RunEvent {
            schema: RUN_EVENT_SCHEMA.into(),
            kind: RunEventKind::WatcherStarted {
                invocation_id: "invocation-1".into(),
            },
        })
        .pop()
        .unwrap();
    QueuedEvent {
        sequence,
        bytes: event_api::encode(&event).unwrap(),
    }
}

fn read_events(reader: &mut BufReader<UnixStream>, count: usize) -> Vec<Value> {
    let mut events = Vec::with_capacity(count);
    while events.len() < count {
        let event = read_raw_event(reader);
        if event["event"] != "resolver_state_changed" {
            events.push(event);
        }
    }
    events
}

fn read_raw_event(reader: &mut BufReader<UnixStream>) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str::<Value>(&line).unwrap()
}

fn read_events_through(
    reader: &mut BufReader<UnixStream>,
    terminal_event: &str,
    maximum: usize,
) -> Vec<Value> {
    let mut events = Vec::new();
    while events.len() < maximum {
        let event = read_events(reader, 1).pop().unwrap();
        let complete = event["event"] == terminal_event;
        events.push(event);
        if complete {
            return events;
        }
    }
    panic!("event {terminal_event} was not observed within {maximum} events");
}

#[test]
fn socket_sends_snapshot_before_live_events_and_removes_its_own_path() {
    let temporary = tempfile::tempdir().unwrap();
    let channel = EventChannel::start_at(temporary.path(), state()).unwrap();
    let socket_path = channel.socket_path.clone();
    let stream = UnixStream::connect(&socket_path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let snapshot: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(snapshot["schema"], "scorepeek-event-snapshot-v4");
    assert_eq!(snapshot["invocation_id"], "invocation-1");
    assert_eq!(snapshot["next_sequence"], 1);
    assert_eq!(snapshot["status"]["watcher"], "starting");
    assert!(snapshot["music_select_best"].is_null());
    channel.publish(wire_event(1));
    line.clear();
    reader.read_line(&mut line).unwrap();
    let event: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(event["event"], "status_changed");
    drop(channel);
    assert!(!socket_path.exists());
}

#[test]
fn numeric_before_joint_identity_emits_once_after_attempt_confirmation() {
    let temporary = tempfile::tempdir().unwrap();
    let state = state();
    let channel = EventChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
    let socket_path = channel.socket_path.clone();
    let mut output = test_output(state, channel);
    prepare_accepted_attempt(&mut output);
    let stream = UnixStream::connect(&socket_path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let mut reader = BufReader::new(stream);
    let mut snapshot = String::new();
    reader.read_line(&mut snapshot).unwrap();

    output
        .publish(&accepted_result_without_joint_identity(1))
        .unwrap();
    output
        .publish(&accepted_result_without_joint_identity(2))
        .unwrap();
    output.publish(&accepted_result_event(3)).unwrap();
    output
        .publish(&semantic_episode_event(
            4,
            "result",
            SemanticEpisodePhase::Closing,
        ))
        .unwrap();
    output
        .publish(&semantic_episode_event(
            4,
            "result",
            SemanticEpisodePhase::Finalized,
        ))
        .unwrap();
    let mut completed = read_events_through(&mut reader, "result_changed", 20);
    completed.extend(read_events_through(&mut reader, "result_changed", 20));
    let provisional = completed
        .iter()
        .position(|event| {
            event["event"] == "result_changed" && event["state"]["status"] == "provisional"
        })
        .unwrap();
    let result = completed
        .iter()
        .position(|event| {
            event["event"] == "result_changed" && event["state"]["status"] == "confirmed"
        })
        .unwrap();
    assert!(provisional < result);
    assert_eq!(completed[provisional]["schema"], event_api::EVENT_SCHEMA);
    assert_eq!(
        completed[provisional]["state"]["result"],
        completed[result]["state"]["result"]
    );
    assert_eq!(completed[result]["source_sequence"], 4);
    assert!(
        completed
            .iter()
            .all(|event| event["event"] != "field_observation"
                && event["event"] != "play_attempt_changed")
    );
    let internal = output.take_headless_events();
    assert_public_fold(internal.clone());
    assert!(
        internal
            .iter()
            .any(|event| matches!(event.kind, RunEventKind::PlayAttemptChanged { .. }))
    );
    output.publish(&accepted_result_event(4)).unwrap();
    assert_eq!(output.state.lock().unwrap().result_count, 1);
}

#[test]
fn provisional_result_requires_two_numeric_observations_and_an_attempt_id() {
    let mut unlinked = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    unlinked.publish(&accepted_result_event(1)).unwrap();
    unlinked.publish(&accepted_result_event(2)).unwrap();
    assert!(!unlinked.take_headless_events().iter().any(|event| {
        matches!(
            event.kind,
            RunEventKind::ResultChanged {
                state: ResultState::Provisional { .. },
                ..
            }
        )
    }));

    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    prepare_accepted_attempt(&mut output);
    let mut identity_only = accepted_result_event(1);
    let RunEventKind::FieldObservation { fields, .. } = &mut identity_only.kind else {
        unreachable!();
    };
    fields["clear_type"] = Value::Null;
    output.publish(&identity_only).unwrap();
    assert!(!output.headless_events.iter().any(|event| {
        matches!(
            event.kind,
            RunEventKind::ResultChanged {
                state: ResultState::Provisional { .. },
                ..
            }
        )
    }));
    output.publish(&accepted_result_event(2)).unwrap();
    assert!(!output.headless_events.iter().any(|event| {
        matches!(
            event.kind,
            RunEventKind::ResultChanged {
                state: ResultState::Provisional { .. },
                ..
            }
        )
    }));
    output.publish(&accepted_result_event(3)).unwrap();
    let provisional = output
        .headless_events
        .iter()
        .find_map(|event| match &event.kind {
            RunEventKind::ResultChanged {
                state: ResultState::Provisional { result, .. },
                ..
            } => Some(result),
            _ => None,
        })
        .unwrap();
    assert_eq!(provisional.contract, "scorepeek-result-detected-v4");
    let encoded = output
        .headless_events
        .iter()
        .find(|event| {
            matches!(
                event.kind,
                RunEventKind::ResultChanged {
                    state: ResultState::Provisional { .. },
                    ..
                }
            )
        })
        .unwrap()
        .to_value()
        .unwrap();
    assert_eq!(encoded["schema"], RUN_EVENT_SCHEMA);
    assert!(matches!(
        RunEvent::from_value(encoded).unwrap().kind,
        RunEventKind::ResultChanged {
            state: ResultState::Provisional { .. },
            ..
        }
    ));
    assert_eq!(output.state.lock().unwrap().result_count, 0);
}

#[test]
fn supplemental_dropout_at_result_exit_does_not_withdraw_or_block_confirmation() {
    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    prepare_accepted_attempt(&mut output);
    for sequence in 1..=3 {
        let mut event = accepted_result_event(sequence);
        let RunEventKind::FieldObservation {
            parsed_result_fields,
            ..
        } = &mut event.kind
        else {
            unreachable!();
        };
        parsed_result_fields.as_mut().unwrap().miss_count = if sequence == 3 {
            SupplementalResultValue::Unknown {
                reason: scorepeek_core::recognition::result::ResultFieldUnknownReason::Empty,
            }
        } else {
            SupplementalResultValue::NotDisplayed
        };
        output.publish(&event).unwrap();
    }
    output
        .publish(&semantic_episode_event(
            4,
            "result",
            SemanticEpisodePhase::Finalized,
        ))
        .unwrap();
    let results: Vec<_> = output
        .headless_events
        .iter()
        .filter_map(|event| match &event.kind {
            RunEventKind::ResultChanged {
                state: ResultState::Confirmed { result, .. },
                ..
            } => Some(result),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].miss_count, SupplementalResultValue::NotDisplayed);
    assert!(!output.headless_events.iter().any(|event| matches!(
        event.kind,
        RunEventKind::ResultChanged {
            state: ResultState::Retracted { .. },
            ..
        }
    )));
}

#[test]
fn supplemental_changes_never_gate_mandatory_acceptance_and_can_update_afterward() {
    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    prepare_accepted_attempt(&mut output);
    for (sequence, miss) in [(1, 3), (2, 4), (3, 5), (4, 5)] {
        let mut event = accepted_result_event(sequence);
        let RunEventKind::FieldObservation {
            parsed_result_fields,
            ..
        } = &mut event.kind
        else {
            unreachable!();
        };
        parsed_result_fields.as_mut().unwrap().miss_count =
            SupplementalResultValue::Known { value: miss };
        output.publish(&event).unwrap();
        if sequence >= 2 {
            assert!(output.core_reducer.test_accepted_numeric_result().is_some());
        }
    }
    output
        .publish(&semantic_episode_event(
            5,
            "result",
            SemanticEpisodePhase::Finalized,
        ))
        .unwrap();
    let results: Vec<_> = output
        .headless_events
        .iter()
        .filter_map(|event| match &event.kind {
            RunEventKind::ResultChanged {
                state: ResultState::Provisional { result, .. },
                ..
            } => Some(result.miss_count.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        vec![
            SupplementalResultValue::Known { value: 4 },
            SupplementalResultValue::Known { value: 5 }
        ]
    );
    assert_eq!(output.state.lock().unwrap().result_count, 1);
    assert!(!output.headless_events.iter().any(|event| matches!(
        event.kind,
        RunEventKind::ResultChanged {
            state: ResultState::Retracted { .. },
            ..
        }
    )));
}

#[test]
fn one_changed_mandatory_observation_preserves_and_confirms_stable_result() {
    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    prepare_accepted_attempt(&mut output);
    output.publish(&accepted_result_event(1)).unwrap();
    output.publish(&accepted_result_event(2)).unwrap();
    let mut changed = accepted_result_event(3);
    let RunEventKind::FieldObservation {
        parsed_result_fields,
        ..
    } = &mut changed.kind
    else {
        unreachable!();
    };
    parsed_result_fields.as_mut().unwrap().good = ResultFieldValue::Known { value: 11 };
    output.publish(&changed).unwrap();
    output
        .publish(&semantic_episode_event(
            4,
            "result",
            SemanticEpisodePhase::Finalized,
        ))
        .unwrap();
    assert_eq!(output.state.lock().unwrap().result_count, 1);
    assert!(output.headless_events.iter().any(|event| matches!(
        &event.kind,
        RunEventKind::ResultChanged {
            state: ResultState::Confirmed { result, .. },
            ..
        } if result.judgments.good == 10
    )));
    assert!(!output.headless_events.iter().any(|event| matches!(
        event.kind,
        RunEventKind::ResultChanged {
            state: ResultState::Retracted { .. },
            ..
        }
    )));
}

#[test]
fn mandatory_challenger_stabilizes_independently_of_supplemental_changes() {
    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    prepare_accepted_attempt(&mut output);
    output.publish(&accepted_result_event(1)).unwrap();
    output.publish(&accepted_result_event(2)).unwrap();
    let changed = |sequence, miss_count| {
        let mut event = accepted_result_event(sequence);
        let RunEventKind::FieldObservation {
            fields,
            parsed_result_fields,
            ..
        } = &mut event.kind
        else {
            unreachable!();
        };
        fields["clear_type"] = Value::String("HARD CLEAR".to_owned());
        parsed_result_fields.as_mut().unwrap().miss_count =
            SupplementalResultValue::Known { value: miss_count };
        event
    };

    output.publish(&changed(3, 4)).unwrap();
    output.publish(&changed(4, 5)).unwrap();

    let provisional = output
        .core_reducer
        .test_active_provisional_result()
        .unwrap();
    assert_eq!(provisional.result.clear_type, "HARD CLEAR");
    assert_eq!(
        provisional.result.miss_count,
        SupplementalResultValue::Known { value: 3 }
    );
    assert_eq!(
        output
            .headless_events
            .iter()
            .filter(|event| matches!(
                event.kind,
                RunEventKind::ResultChanged {
                    state: ResultState::Retracted { .. },
                    ..
                }
            ))
            .count(),
        1
    );

    output.publish(&changed(5, 5)).unwrap();
    let provisional = output
        .core_reducer
        .test_active_provisional_result()
        .unwrap();
    assert_eq!(
        provisional.result.miss_count,
        SupplementalResultValue::Known { value: 5 }
    );
}

#[test]
fn one_different_song_challenger_cannot_confirm_the_stable_result() {
    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    prepare_accepted_attempt(&mut output);
    output.publish(&accepted_result_event(1)).unwrap();
    output.publish(&accepted_result_event(2)).unwrap();

    let other_song = serde_json::from_str("\"00000000-0000-0000-0000-000000000002\"").unwrap();
    output
        .core_reducer
        .test_engine_mut()
        .provisional_joint
        .as_mut()
        .unwrap()
        .song_id = other_song;
    let mut challenger = output
        .core_reducer
        .test_accepted_numeric_result()
        .cloned()
        .unwrap();
    challenger.song_id = other_song;
    challenger.source_sequence = 3;
    assert!(
        output
            .core_reducer
            .test_stabilize_numeric_result(challenger, false)
            .is_none()
    );

    output
        .publish(&semantic_episode_event(
            4,
            "result",
            SemanticEpisodePhase::Finalized,
        ))
        .unwrap();

    assert_eq!(output.state.lock().unwrap().result_count, 0);
    assert!(!output.headless_events.iter().any(|event| matches!(
        event.kind,
        RunEventKind::ResultChanged {
            state: ResultState::Confirmed { .. },
            ..
        }
    )));
}

#[test]
fn one_different_chart_challenger_cannot_confirm_the_stable_result() {
    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    prepare_accepted_attempt(&mut output);
    output.publish(&accepted_result_event(1)).unwrap();
    output.publish(&accepted_result_event(2)).unwrap();

    output
        .core_reducer
        .test_engine_mut()
        .provisional_joint
        .as_mut()
        .unwrap()
        .chart
        .key
        .difficulty = Difficulty::Another;
    let mut challenger = output
        .core_reducer
        .test_accepted_numeric_result()
        .cloned()
        .unwrap();
    challenger.chart.key.difficulty = Difficulty::Another;
    challenger.source_sequence = 3;
    assert!(
        output
            .core_reducer
            .test_stabilize_numeric_result(challenger, false)
            .is_none()
    );

    output
        .publish(&semantic_episode_event(
            4,
            "result",
            SemanticEpisodePhase::Finalized,
        ))
        .unwrap();

    assert_eq!(output.state.lock().unwrap().result_count, 0);
    assert!(!output.headless_events.iter().any(|event| matches!(
        event.kind,
        RunEventKind::ResultChanged {
            state: ResultState::Confirmed { .. },
            ..
        }
    )));
    assert!(matches!(
        output.core_reducer.test_engine().play_attempt.state(),
        PlayAttemptState::Attempt { attempt }
            if attempt.result_relation
                == scorepeek_core::session::attempt::PlayAttemptResultRelation::Conflict
                && attempt.reasons.contains(&PlayAttemptReason::LinkageConflict)
    ));
}

#[test]
fn provisional_result_retracts_and_re_resolves_as_one_state_stream() {
    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    prepare_accepted_attempt(&mut output);
    output.publish(&accepted_result_event(1)).unwrap();
    output.publish(&accepted_result_event(2)).unwrap();
    let changed_clear = |sequence| {
        let mut event = accepted_result_event(sequence);
        let RunEventKind::FieldObservation { fields, .. } = &mut event.kind else {
            unreachable!();
        };
        fields["clear_type"] = Value::String("HARD CLEAR".to_owned());
        event
    };
    output.publish(&changed_clear(3)).unwrap();
    assert!(!output.headless_events.iter().any(|event| matches!(
        event.kind,
        RunEventKind::ResultChanged {
            state: ResultState::Retracted { .. },
            ..
        }
    )));
    assert!(
        output
            .core_reducer
            .test_active_provisional_result()
            .is_some()
    );
    output.publish(&changed_clear(4)).unwrap();

    let lifecycle = output
        .headless_events
        .iter()
        .filter_map(|event| match &event.kind {
            RunEventKind::ResultChanged { state, .. }
                if !matches!(state, ResultState::Inactive) =>
            {
                Some(state)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(lifecycle.len(), 3);
    assert!(matches!(lifecycle[0], ResultState::Provisional { .. }));
    assert!(matches!(
        lifecycle[1],
        ResultState::Retracted {
            reason: ResultRetractionReason::EvidenceUnresolved,
            ..
        }
    ));
    assert!(matches!(
        lifecycle[2],
        ResultState::Provisional { result, .. } if result.clear_type == "HARD CLEAR"
    ));
}

#[test]
fn identity_conflict_withdraws_provisional_and_cannot_confirm() {
    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    prepare_accepted_attempt(&mut output);
    output.publish(&accepted_result_event(1)).unwrap();
    output.publish(&accepted_result_event(2)).unwrap();
    let mut conflict = accepted_result_event(3);
    let RunEventKind::FieldObservation { joint_evidence, .. } = &mut conflict.kind else {
        unreachable!();
    };
    joint_evidence.candidates[0].song_id =
        serde_json::from_str("\"00000000-0000-0000-0000-000000000002\"").unwrap();
    joint_evidence.candidates[0].display_titles = vec!["CONFLICT".to_owned()];
    output.publish(&conflict).unwrap();
    let mut repeated_conflict = conflict.clone();
    let RunEventKind::FieldObservation { sequence, .. } = &mut repeated_conflict.kind else {
        unreachable!();
    };
    *sequence = 4;
    output.publish(&repeated_conflict).unwrap();
    output
        .publish(&semantic_episode_event(
            5,
            "result",
            SemanticEpisodePhase::Finalized,
        ))
        .unwrap();

    assert!(output.headless_events.iter().any(|event| matches!(
        event.kind,
        RunEventKind::ResultChanged {
            state: ResultState::Retracted {
                reason: ResultRetractionReason::EvidenceUnresolved,
                ..
            },
            ..
        }
    )));
    assert!(!output.headless_events.iter().any(|event| matches!(
        event.kind,
        RunEventKind::ResultChanged {
            state: ResultState::Confirmed { .. },
            ..
        }
    )));
}

#[test]
fn final_result_reuses_the_last_provisional_payload_without_withdrawal() {
    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    prepare_accepted_attempt(&mut output);
    output.publish(&accepted_result_event(1)).unwrap();
    output.publish(&accepted_result_event(2)).unwrap();
    output
        .publish(&semantic_episode_event(
            3,
            "result",
            SemanticEpisodePhase::Finalized,
        ))
        .unwrap();

    let events = &output.headless_events;
    let provisional = events
        .iter()
        .find_map(|event| match &event.kind {
            RunEventKind::ResultChanged {
                state: ResultState::Provisional { result, .. },
                ..
            } => Some(result),
            _ => None,
        })
        .unwrap();
    let confirmation = events.iter().position(|event| matches!(
            &event.kind,
            RunEventKind::PlayAttemptChanged {
                state: PlayAttemptState::Attempt { attempt }, ..
            } if attempt.result_relation == scorepeek_core::session::attempt::PlayAttemptResultRelation::Confirmed
        )).unwrap();
    let detected = events
        .iter()
        .position(|event| {
            matches!(
                event.kind,
                RunEventKind::ResultChanged {
                    state: ResultState::Confirmed { .. },
                    ..
                }
            )
        })
        .unwrap();
    let RunEventKind::ResultChanged {
        state: ResultState::Confirmed {
            result: confirmed, ..
        },
        ..
    } = &events[detected].kind
    else {
        unreachable!();
    };
    assert!(confirmation < detected);
    assert_eq!(provisional.as_ref(), confirmed.as_ref());
    assert!(!events.iter().any(|event| matches!(
        event.kind,
        RunEventKind::ResultChanged {
            state: ResultState::Retracted { .. },
            ..
        }
    )));
}

#[test]
fn tui_shows_each_result_state_without_falling_back_after_retraction() {
    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    prepare_accepted_attempt(&mut output);
    output.publish(&accepted_result_event(1)).unwrap();
    output.publish(&accepted_result_event(2)).unwrap();
    let provisional = output
        .core_reducer
        .test_active_provisional_result()
        .cloned()
        .unwrap();
    output
        .publish(&semantic_episode_event(
            3,
            "result",
            SemanticEpisodePhase::Finalized,
        ))
        .unwrap();
    assert!(
        fixed_domain_lines(&output.state.lock().unwrap(), 160)[0]
            .to_string()
            .contains("CONFIRMED")
    );

    let resolved = RunEvent {
        schema: RUN_EVENT_SCHEMA.to_owned(),
        kind: RunEventKind::ResultChanged {
            session_id: "invocation-1-session-1".to_owned(),
            capture_generation: 1,
            source_sequence: 4,
            state: ResultState::Provisional {
                song: provisional.song.clone(),
                result: Box::new(provisional.result.clone()),
            },
        },
    };
    output.publish(&resolved).unwrap();
    assert!(
        fixed_domain_lines(&output.state.lock().unwrap(), 160)[0]
            .to_string()
            .contains("PROVISIONAL")
    );

    let withdrawn = RunEvent {
        schema: RUN_EVENT_SCHEMA.to_owned(),
        kind: RunEventKind::ResultChanged {
            session_id: "invocation-1-session-1".to_owned(),
            capture_generation: 1,
            source_sequence: 5,
            state: ResultState::Retracted {
                song: provisional.song,
                result: Box::new(provisional.result),
                reason: ResultRetractionReason::EvidenceUnresolved,
            },
        },
    };
    output.publish(&withdrawn).unwrap();
    assert!(
        fixed_domain_lines(&output.state.lock().unwrap(), 160)[0]
            .to_string()
            .contains("RETRACTED")
    );
    output
        .publish(&RunEvent {
            schema: RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::ResultChanged {
                session_id: "invocation-1-session-1".to_owned(),
                capture_generation: 1,
                source_sequence: 6,
                state: ResultState::Inactive,
            },
        })
        .unwrap();
    assert_eq!(
        fixed_domain_lines(&output.state.lock().unwrap(), 160)[0].to_string(),
        "No active result"
    );
    assert_eq!(output.state.lock().unwrap().result_count, 1);
}

#[test]
fn linkage_deficient_attempt_is_provisional_then_withdrawn_on_rejection() {
    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    prime_left_result_panel(&mut output);
    output
        .core_reducer
        .test_engine_mut()
        .play_attempt
        .observe_selection_screen();
    output
        .core_reducer
        .test_engine_mut()
        .play_attempt
        .observe_screen(PlayAttemptScreen::DecideTransition, 0);
    output
        .core_reducer
        .test_engine_mut()
        .play_attempt
        .observe_screen(PlayAttemptScreen::Result, 0);
    output.publish(&accepted_result_event(1)).unwrap();
    output.publish(&accepted_result_event(2)).unwrap();
    output
        .publish(&semantic_episode_event(
            3,
            "result",
            SemanticEpisodePhase::Finalized,
        ))
        .unwrap();
    let lifecycle = output
        .headless_events
        .iter()
        .filter_map(|event| match &event.kind {
            RunEventKind::ResultChanged { state, .. }
                if !matches!(state, ResultState::Inactive) =>
            {
                Some(state)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        lifecycle.as_slice(),
        [
            ResultState::Provisional { .. },
            ResultState::Retracted {
                reason: ResultRetractionReason::AttemptRejected,
                ..
            }
        ]
    ));
    assert!(!output.headless_events.iter().any(|event| matches!(
        event.kind,
        RunEventKind::ResultChanged {
            state: ResultState::Confirmed { .. },
            ..
        }
    )));
}

#[test]
fn incomplete_numeric_finalizes_the_attempt_as_rejected() {
    let temporary = tempfile::tempdir().unwrap();
    let state = state();
    let channel = EventChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
    let mut output = test_output(state, channel);
    prepare_accepted_attempt(&mut output);

    output.publish(&accepted_result_event(1)).unwrap();
    let mut second = accepted_result_event(2);
    let RunEventKind::FieldObservation { fields, .. } = &mut second.kind else {
        unreachable!("accepted result fixture is a field observation");
    };
    fields["clear_type"] = Value::Null;
    output.publish(&second).unwrap();
    output
        .publish(&semantic_episode_event(
            3,
            "result",
            SemanticEpisodePhase::Finalized,
        ))
        .unwrap();

    assert!(output.core_reducer.test_emitted_attempt_ids().is_empty());
    assert!(matches!(
        output.core_reducer.test_engine().play_attempt.state(),
        PlayAttemptState::Attempt { attempt }
            if attempt.result_relation
                == scorepeek_core::session::attempt::PlayAttemptResultRelation::Conflict
                && attempt.reasons.contains(&PlayAttemptReason::ResultEvidenceUnresolved)
    ));
}

#[test]
fn stale_numeric_from_another_chart_cannot_confirm_the_attempt() {
    let temporary = tempfile::tempdir().unwrap();
    let state = state();
    let channel = EventChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
    let mut output = test_output(state, channel);
    prepare_accepted_attempt(&mut output);

    output.publish(&accepted_result_event(1)).unwrap();
    output.publish(&accepted_result_event(2)).unwrap();
    output
        .core_reducer
        .test_engine_mut()
        .provisional_joint
        .as_mut()
        .unwrap()
        .chart
        .key
        .difficulty = Difficulty::Another;
    output
        .publish(&semantic_episode_event(
            3,
            "result",
            SemanticEpisodePhase::Finalized,
        ))
        .unwrap();

    assert!(output.core_reducer.test_emitted_attempt_ids().is_empty());
    assert!(matches!(
        output.core_reducer.test_engine().play_attempt.state(),
        PlayAttemptState::Attempt { attempt }
            if attempt.result_relation
                == scorepeek_core::session::attempt::PlayAttemptResultRelation::Conflict
                && attempt.reasons.contains(&PlayAttemptReason::LinkageConflict)
    ));
}

#[test]
fn failed_session_boundary_cannot_replace_semantic_result_finalization() {
    let temporary = tempfile::tempdir().unwrap();
    let state = state();
    let channel = EventChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
    let mut output = test_output(state, channel);
    prepare_accepted_attempt(&mut output);

    output.publish(&accepted_result_event(1)).unwrap();
    output.publish(&accepted_result_event(2)).unwrap();
    output.publish(&failed_session_finished_event()).unwrap();

    assert!(output.core_reducer.test_emitted_attempt_ids().is_empty());
    assert!(matches!(
        output.core_reducer.test_engine().play_attempt.state(),
        PlayAttemptState::Attempt { attempt }
            if attempt.phase == scorepeek_core::session::attempt::PlayAttemptPhase::Abandoned
                && attempt.reasons.contains(&PlayAttemptReason::SessionEnded)
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn normalized_result_evidence_completes_an_attempt_despite_wrong_select_title() {
    let temporary = tempfile::tempdir().unwrap();
    let state = state();
    let channel = EventChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
    let mut output = test_output(Arc::clone(&state), channel);
    let correct_song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
    let wrong_song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000002\"").unwrap();
    let collision_song_id =
        serde_json::from_str("\"00000000-0000-0000-0000-000000000003\"").unwrap();
    let chart = scorepeek_core::catalog::Chart {
        key: scorepeek_core::catalog::ChartKey {
            play_type: PlayType::Single,
            difficulty: Difficulty::Hyper,
        },
        level: 8,
        notes: 764,
    };

    output.publish(&screen_event(1, "music_select")).unwrap();
    output
        .core_reducer
        .test_engine_mut()
        .retained_select
        .observe(
            100,
            &JointEvidenceObservation {
                catalog_song_count: 0,
                candidates: vec![
                    JointEvidenceCandidate {
                        song_id: wrong_song_id,
                        chart: chart.clone(),
                        display_titles: vec!["X".to_owned()],
                        artist: "D.J.Amuro".to_owned(),
                        family_support: BTreeMap::from([
                            (EvidenceFamily::SelectTitleLexical, 300),
                            (EvidenceFamily::SelectTitleStructural, 60),
                            (EvidenceFamily::SelectChart, 50),
                        ]),
                        support: 410,
                    },
                    JointEvidenceCandidate {
                        song_id: correct_song_id,
                        chart: chart.clone(),
                        display_titles: vec!["〆".to_owned()],
                        artist: "lapix".to_owned(),
                        family_support: BTreeMap::from([
                            (EvidenceFamily::SelectTitleStructural, 60),
                            (EvidenceFamily::SelectArtist, 300),
                            (EvidenceFamily::SelectChart, 50),
                        ]),
                        support: 410,
                    },
                ],
            },
            None,
            None,
        );
    output
        .publish(&screen_event(2, "decide_transition"))
        .unwrap();
    output.publish(&screen_event(3, "play")).unwrap();
    output.publish(&screen_event(4, "result")).unwrap();

    let result = |sequence| {
        let mut event = accepted_result_event(sequence);
        let RunEventKind::FieldObservation { joint_evidence, .. } = &mut event.kind else {
            unreachable!();
        };
        joint_evidence.candidates = vec![
            JointEvidenceCandidate {
                song_id: correct_song_id,
                chart: chart.clone(),
                display_titles: vec!["〆".to_owned()],
                artist: "lapix".to_owned(),
                family_support: BTreeMap::from([
                    (EvidenceFamily::ResultArtist, 300),
                    (EvidenceFamily::ResultChart, 170),
                ]),
                support: 470,
            },
            JointEvidenceCandidate {
                song_id: wrong_song_id,
                chart: chart.clone(),
                display_titles: vec!["WRONG SELECT".to_owned()],
                artist: "WRONG ARTIST".to_owned(),
                family_support: BTreeMap::from([(EvidenceFamily::ResultChart, 70)]),
                support: 70,
            },
            JointEvidenceCandidate {
                song_id: collision_song_id,
                chart: chart.clone(),
                display_titles: vec!["Flying Castle".to_owned()],
                artist: "lapix".to_owned(),
                family_support: BTreeMap::from([
                    (EvidenceFamily::ResultArtist, 300),
                    (EvidenceFamily::ResultChart, 170),
                ]),
                support: 470,
            },
        ];
        event
    };
    output.publish(&result(5)).unwrap();
    output.publish(&result(6)).unwrap();
    output.publish(&result(7)).unwrap();

    assert_eq!(output.core_reducer.test_emitted_attempt_ids().len(), 0);
    output
        .publish(&semantic_episode_event(
            8,
            "result",
            SemanticEpisodePhase::Closing,
        ))
        .unwrap();
    output
        .publish(&semantic_episode_event(
            8,
            "result",
            SemanticEpisodePhase::Finalized,
        ))
        .unwrap();
    assert_eq!(output.core_reducer.test_emitted_attempt_ids().len(), 1);
    assert_eq!(state.lock().unwrap().result_count, 1);
    assert_eq!(
        state
            .lock()
            .unwrap()
            .result_history
            .back()
            .unwrap()
            .result
            .play_options,
        PlayOptions::Known {
            values: vec![PlayOption::Random, PlayOption::Legacy]
        }
    );
    assert_eq!(
        state
            .lock()
            .unwrap()
            .result_history
            .back()
            .unwrap()
            .song
            .as_ref()
            .unwrap()
            .display_titles[0],
        "〆"
    );
    assert!(matches!(
        output.core_reducer.test_engine().play_attempt.state(),
        PlayAttemptState::Attempt { attempt }
            if attempt.result_relation
                == scorepeek_core::session::attempt::PlayAttemptResultRelation::Confirmed
    ));
}

#[test]
fn result_reentry_does_not_reemit_the_same_attempt() {
    let temporary = tempfile::tempdir().unwrap();
    let state = state();
    let channel = EventChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
    let mut output = test_output(Arc::clone(&state), channel);
    prepare_accepted_attempt(&mut output);

    output.publish(&accepted_result_event(1)).unwrap();
    output.publish(&accepted_result_event(2)).unwrap();
    assert_eq!(state.lock().unwrap().result_count, 0);
    output
        .publish(&semantic_episode_event(
            3,
            "result",
            SemanticEpisodePhase::Closing,
        ))
        .unwrap();
    output
        .publish(&semantic_episode_event(
            3,
            "result",
            SemanticEpisodePhase::Finalized,
        ))
        .unwrap();
    assert_eq!(state.lock().unwrap().result_count, 1);

    output.publish(&screen_event(4, "result")).unwrap();
    output.publish(&accepted_result_event(5)).unwrap();
    output.publish(&accepted_result_event(6)).unwrap();

    assert_eq!(state.lock().unwrap().result_count, 1);
}

#[test]
fn socket_broadcasts_one_live_event_to_multiple_clients() {
    let temporary = tempfile::tempdir().unwrap();
    let channel = EventChannel::start_at(temporary.path(), state()).unwrap();
    let mut readers = (0..2)
        .map(|_| {
            let stream = UnixStream::connect(&channel.socket_path).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            BufReader::new(stream)
        })
        .collect::<Vec<_>>();
    for reader in &mut readers {
        let mut snapshot = String::new();
        reader.read_line(&mut snapshot).unwrap();
        assert!(snapshot.contains("scorepeek-event-snapshot-v4"));
    }
    channel.publish(wire_event(1));
    for reader in &mut readers {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["sequence"], 1);
    }
}

#[test]
fn publishing_without_clients_is_healthy() {
    let temporary = tempfile::tempdir().unwrap();
    let channel = EventChannel::start_at(temporary.path(), state()).unwrap();
    channel.publish(wire_event(1));
    std::thread::sleep(Duration::from_millis(40));
    assert!(!channel.health.server_failed.load(Ordering::Acquire));
    assert_eq!(channel.health.connected_clients.load(Ordering::Acquire), 0);
}

#[test]
fn a_slow_client_is_disconnected_without_degrading_the_server() {
    let temporary = tempfile::tempdir().unwrap();
    let channel = EventChannel::start_at(temporary.path(), state()).unwrap();
    let stream = UnixStream::connect(&channel.socket_path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let mut reader = BufReader::new(stream);
    let mut snapshot = String::new();
    reader.read_line(&mut snapshot).unwrap();
    let mut event = wire_event(1);
    event.bytes = vec![b' '; event_api::MAX_RECORD_BYTES];
    channel.publish(event);
    for _ in 0..50 {
        if channel.health.disconnected_clients.load(Ordering::Acquire) > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(channel.health.connected_clients.load(Ordering::Acquire), 0);
    assert_eq!(
        channel.health.disconnected_clients.load(Ordering::Acquire),
        1
    );
    assert!(!channel.health.server_failed.load(Ordering::Acquire));
}

#[test]
fn idle_clients_release_slots_without_public_events() {
    let temporary = tempfile::tempdir().unwrap();
    let channel = EventChannel::start_at(temporary.path(), state()).unwrap();
    for _ in 0..(MAX_CLIENTS * 2) {
        let stream = UnixStream::connect(&channel.socket_path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        assert_eq!(
            read_raw_event(&mut BufReader::new(stream))["next_sequence"],
            1
        );
    }
    let stream = UnixStream::connect(&channel.socket_path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut reader = BufReader::new(stream);
    assert_eq!(read_raw_event(&mut reader)["next_sequence"], 1);
    channel.publish(wire_event(1));
    assert_eq!(read_raw_event(&mut reader)["sequence"], 1);
    // Inject the full-queue notification while the worker's actual queue is empty.
    let (sender, _receiver) = std::sync::mpsc::sync_channel(1);
    try_send_event(&sender, &channel.health, wire_event(2));
    try_send_event(&sender, &channel.health, wire_event(3));
    assert_eq!(reader.read_line(&mut String::new()).unwrap(), 0);
}

#[test]
fn connecting_snapshot_skips_queued_history_and_overflow_disconnects() {
    let temporary = tempfile::tempdir().unwrap();
    let listener = UnixListener::bind(temporary.path().join("test.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let state = state();
    state.lock().unwrap().public.next_sequence = 3;
    let health = ChannelHealth::default();
    let mut clients = Vec::new();
    let stream = UnixStream::connect(temporary.path().join("test.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let mut reader = BufReader::new(stream);
    accept_clients(&listener, &state, &health, &mut clients);
    assert_eq!(read_raw_event(&mut reader)["next_sequence"], 3);
    broadcast(&wire_event(1), &health, &mut clients);
    broadcast(&wire_event(2), &health, &mut clients);
    broadcast(&wire_event(3), &health, &mut clients);
    assert_eq!(read_raw_event(&mut reader)["sequence"], 3);
    let (sender, _receiver) = std::sync::mpsc::sync_channel(1);
    try_send_event(&sender, &health, wire_event(4));
    try_send_event(&sender, &health, wire_event(5));
    state.lock().unwrap().public.next_sequence = 6;
    broadcast(&wire_event(4), &health, &mut clients);
    assert!(clients.is_empty());
    assert_eq!(reader.read_line(&mut String::new()).unwrap(), 0);
    let stream = UnixStream::connect(temporary.path().join("test.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let mut reader = BufReader::new(stream);
    accept_clients(&listener, &state, &health, &mut clients);
    assert_eq!(read_raw_event(&mut reader)["next_sequence"], 6);
    broadcast(&wire_event(4), &health, &mut clients);
    broadcast(&wire_event(6), &health, &mut clients);
    assert_eq!(read_raw_event(&mut reader)["sequence"], 6);
}

#[test]
fn partial_write_disconnects_only_that_client() {
    let health = ChannelHealth::default();
    let (slow, _slow_peer) = UnixStream::pair().unwrap();
    slow.set_nonblocking(true).unwrap();
    // Fill the send buffer deterministically without relying on the platform buffer size.
    let mut slow = slow;
    while slow.write(&[b'x'; 4096]).is_ok() {}
    let (fast, fast_peer) = UnixStream::pair().unwrap();
    fast.set_nonblocking(true).unwrap();
    fast_peer
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let mut clients = vec![
        EventClient {
            stream: slow,
            next_sequence: 1,
            epoch: 0,
        },
        EventClient {
            stream: fast,
            next_sequence: 1,
            epoch: 0,
        },
    ];
    broadcast(&wire_event(1), &health, &mut clients);
    assert_eq!(clients.len(), 1);
    assert_eq!(
        read_raw_event(&mut BufReader::new(fast_peer))["sequence"],
        1
    );
    assert_eq!(health.disconnected_clients.load(Ordering::Acquire), 1);
    assert!(!health.server_failed.load(Ordering::Acquire));
}

#[test]
fn public_worker_loss_and_oversize_do_not_fail_internal_publication() {
    let mut output = test_output(state(), disconnected_test_channel());
    output.publish_frontend_snapshots = false;
    output
        .publish(&RunEvent {
            schema: RUN_EVENT_SCHEMA.into(),
            kind: RunEventKind::WatcherStarted {
                invocation_id: "invocation-1".into(),
            },
        })
        .unwrap();
    assert!(
        output
            .channel
            .as_ref()
            .unwrap()
            .health
            .server_failed
            .load(Ordering::Acquire)
    );
    output.publish(&accepted_result_event(1)).unwrap();
    assert!(
        output
            .take_headless_events()
            .iter()
            .any(|event| matches!(event.kind, RunEventKind::FieldObservation { .. }))
    );

    let temporary = tempfile::tempdir().unwrap();
    let state = state();
    let channel = EventChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
    let mut output = test_output(state, channel);
    output.publish_frontend_snapshots = false;
    let mut projected = output.state.lock().unwrap().public.clone();
    let records = projected.project(&RunEvent {
        schema: RUN_EVENT_SCHEMA.into(),
        kind: RunEventKind::MusicSelectionChanged {
            session_id: Some("x".repeat(event_api::MAX_RECORD_BYTES)),
            capture_generation: Some(1),
            screen_episode_id: 1,
            source_sequence: 1,
            revision: 1,
            state: MusicSelectionState::Unresolved {
                reason: MusicSelectionUnresolvedReason::EvidenceUnresolved,
            },
        },
    });
    commit_public_projection(
        &mut output.state.lock().unwrap().public,
        projected,
        &records,
        output.channel.as_ref(),
        None,
    );
    assert_eq!(
        output
            .channel
            .as_ref()
            .unwrap()
            .health
            .oversized_records
            .load(Ordering::Acquire),
        1
    );
    assert_eq!(output.state.lock().unwrap().public.next_sequence, 2);
    output.publish(&accepted_result_event(1)).unwrap();
}

#[test]
fn stale_socket_is_replaced_but_other_entries_are_preserved() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("scorepeek");
    fs::create_dir(&directory).unwrap();
    let socket_path = directory.join(SOCKET_NAME);
    let stale = UnixListener::bind(&socket_path).unwrap();
    drop(stale);
    let channel = EventChannel::start_at(temporary.path(), state()).unwrap();
    drop(channel);

    fs::write(&socket_path, b"owned by operator").unwrap();
    let Err(error) = EventChannel::start_at(temporary.path(), state()) else {
        panic!("non-socket entry must not be replaced");
    };
    assert!(error.contains("non-socket"));
    assert_eq!(fs::read(&socket_path).unwrap(), b"owned by operator");

    fs::remove_file(&socket_path).unwrap();
    let target = directory.join("target");
    fs::write(&target, b"target").unwrap();
    symlink(&target, &socket_path).unwrap();
    let Err(error) = EventChannel::start_at(temporary.path(), state()) else {
        panic!("symlink must not be replaced");
    };
    assert!(error.contains("non-socket"));
    assert!(
        socket_path
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn active_socket_is_not_unlinked_or_rebound() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("scorepeek");
    fs::create_dir(&directory).unwrap();
    let socket_path = directory.join(SOCKET_NAME);
    let listener = UnixListener::bind(&socket_path).unwrap();
    let Err(error) = EventChannel::start_at(temporary.path(), state()) else {
        panic!("active socket must not be replaced");
    };
    assert!(error.contains("already active"));
    assert!(
        socket_path
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_socket()
    );
    drop(listener);
}

#[test]
fn initialization_guard_removes_only_the_socket_it_owns() {
    let temporary = tempfile::tempdir().unwrap();
    let socket_path = temporary.path().join("initializing.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let metadata = socket_path.symlink_metadata().unwrap();
    let identity = (metadata.dev(), metadata.ino());
    drop(SocketPathGuard::new(socket_path.clone(), identity));
    assert!(!socket_path.exists());
    drop(listener);
}

#[test]
fn cleanup_preserves_an_entry_that_replaced_the_owned_socket() {
    let temporary = tempfile::tempdir().unwrap();
    let channel = EventChannel::start_at(temporary.path(), state()).unwrap();
    let socket_path = channel.socket_path.clone();
    fs::remove_file(&socket_path).unwrap();
    fs::write(&socket_path, b"replacement").unwrap();
    drop(channel);
    assert_eq!(fs::read(&socket_path).unwrap(), b"replacement");
}

#[test]
fn cleanup_preserves_a_socket_with_a_different_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let channel = EventChannel::start_at(temporary.path(), state()).unwrap();
    let socket_path = channel.socket_path.clone();
    fs::remove_file(&socket_path).unwrap();
    let replacement = UnixListener::bind(&socket_path).unwrap();
    drop(channel);
    assert!(
        socket_path
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_socket()
    );
    drop(replacement);
}

#[test]
fn full_event_queue_is_counted_without_blocking_the_producer() {
    let (sender, _receiver) = std::sync::mpsc::sync_channel::<QueuedEvent>(1);
    let health = ChannelHealth::default();
    try_send_event(&sender, &health, wire_event(1));
    try_send_event(&sender, &health, wire_event(2));
    assert_eq!(health.dropped_events.load(Ordering::Acquire), 1);
    assert!(!health.server_failed.load(Ordering::Acquire));
}

#[test]
fn plain_status_does_not_change_for_a_field_observation() {
    let mut state = RunViewState::new("invocation-1".to_owned(), "e".repeat(64), true);
    let health = ChannelHealth::default();
    let before = plain_status_line(&state, &health);
    state.latest_observation = Some(json!({
        "event": "field_observation",
        "fields": { "title": "OCR VALUE" }
    }));
    assert_eq!(plain_status_line(&state, &health), before);
    assert!(!before.contains("OCR VALUE"));
}

#[test]
fn typed_reducer_tracks_session_report_and_stop_transitions() {
    let mut state = RunViewState::new("invocation-1".to_owned(), "d".repeat(64), true);
    let started = RunEvent::from_value(json!({
        "schema": RUN_EVENT_SCHEMA,
        "event": "session_started",
        "session_id": "invocation-1-session-1",
        "capture_generation": 1,
        "capture_profile_sha256": "profile",
        "normalizer_artifact_sha256": "normalizer"
    }))
    .unwrap();
    state.reduce(&started, &started.to_value().unwrap());
    assert_eq!(state.watcher_state, "session_active");
    assert_eq!(state.session_count, 1);
    assert_eq!(
        state.active_session_id.as_deref(),
        Some("invocation-1-session-1")
    );

    let finished = RunEvent::from_value(json!({
        "schema": RUN_EVENT_SCHEMA,
        "event": "session_finished",
        "session_id": "invocation-1-session-1",
        "capture_generation": 1,
        "outcome": "source_ended",
        "report": { "recognition_ticks": 3 }
    }))
    .unwrap();
    state.reduce(&finished, &finished.to_value().unwrap());
    assert_eq!(state.watcher_state, "session_finished");
    assert_eq!(
        state.latest_report.as_ref().unwrap()["recognition_ticks"],
        3
    );

    state.latest_observation = Some(json!({ "sequence": 9 }));
    state.latest_stabilized_result = Some(json!({ "state": { "song": "stable" } }));
    state.latest_temporal_music_select = Some(json!({ "state": { "status": "changing" } }));
    let next_started = RunEvent::from_value(json!({
        "schema": RUN_EVENT_SCHEMA,
        "event": "session_started",
        "session_id": "invocation-1-session-2",
        "capture_generation": 2,
        "capture_profile_sha256": "profile",
        "normalizer_artifact_sha256": "normalizer"
    }))
    .unwrap();
    state.reduce(&next_started, &next_started.to_value().unwrap());
    assert_eq!(
        state.active_session_id.as_deref(),
        Some("invocation-1-session-2")
    );
    assert!(state.latest_observation.is_none());
    assert!(state.latest_report.is_none());

    let stopped = RunEvent::from_value(json!({
        "schema": RUN_EVENT_SCHEMA,
        "event": "watcher_stopped",
        "invocation_id": "invocation-1",
        "reason": "signal"
    }))
    .unwrap();
    state.reduce(&stopped, &stopped.to_value().unwrap());
    assert_eq!(state.watcher_state, "stopped");
    assert!(state.latest_observation.is_none());
    assert!(state.latest_stabilized_result.is_none());
    assert!(state.latest_temporal_music_select.is_none());
}

#[test]
fn recording_health_and_ready_lifecycle_are_visible_in_the_typed_state() {
    let mut state = RunViewState::new("invocation-1".to_owned(), "a".repeat(64), true);
    assert_eq!(state.status_recording, "armed");
    let health = RunEvent::from_value(json!({
        "schema": RUN_EVENT_SCHEMA,
        "event": "recording_health_changed",
        "session_id": "session-1",
        "capture_generation": 1,
        "state": "pressured",
        "memory_limit_bytes": 1_073_741_824_u64,
        "memory_used_bytes": 900_000_000_u64,
        "memory_high_water_bytes": 950_000_000_u64,
        "dropped_frames": 0
    }))
    .unwrap();
    state.reduce(&health, &health.to_value().unwrap());
    assert_eq!(state.status_recording, "pressured");
    assert_eq!(state.recording_memory_used_bytes, 900_000_000);

    let finished = RunEvent::from_value(json!({
        "schema": RUN_EVENT_SCHEMA,
        "event": "session_finished",
        "session_id": "session-1",
        "capture_generation": 1,
        "outcome": "source_ended",
        "report": {}
    }))
    .unwrap();
    state.reduce(&finished, &finished.to_value().unwrap());
    assert_eq!(state.status_recording, "finalizing");

    let ready = RunEvent::from_value(json!({
        "schema": RUN_EVENT_SCHEMA,
        "event": "recording_completed",
        "session_id": "session-1",
        "directory": "/private/session-1"
    }))
    .unwrap();
    state.reduce(&ready, &ready.to_value().unwrap());
    assert_eq!(state.status_recording, "ready");
    assert!(state.message.contains("session-1"));
}

#[test]
fn result_history_remains_bounded_and_survives_session_changes() {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
    let mut state = RunViewState::new("invocation-1".to_owned(), "a".repeat(64), true);
    state.stable_result_song = Some(SongPresentation {
        scorepeek_song_id: song_id,
        display_titles: vec!["TITLE".to_owned()],
        artist: "ARTIST".to_owned(),
    });
    for source_sequence in 1..=(RESULT_HISTORY_CAPACITY as u64 + 1) {
        let result = detected_result_event(
            "session-1",
            1,
            source_sequence,
            ResultDomainEvent {
                contract: "scorepeek-result-detected-v4".to_owned(),
                attempt_id: source_sequence,
                parent_attempt_id: None,
                scorepeek_song_id: song_id,
                play_side: PlaySide::OnePlayer,
                play_mode: "single_play".to_owned(),
                play_type: PlayType::Single,
                difficulty: Difficulty::Normal,
                level: 5,
                notes: 100,
                current_score: 150,
                clear_type: "CLEAR".to_owned(),
                judgments: ResultJudgments {
                    pgreat: 70,
                    great: 10,
                    good: 5,
                    bad: 3,
                    poor: 2,
                },
                miss_count: SupplementalResultValue::Known { value: 2 },
                timing: ResultTiming {
                    fast: SupplementalResultValue::Known { value: 4 },
                    slow: SupplementalResultValue::Known { value: 5 },
                },
                combo_break: SupplementalResultValue::Known { value: 1 },
                previous_best: PreviousBest {
                    clear_type: scorepeek_core::recognition::result::PreviousBestValue::NotPlayed,
                    score: scorepeek_core::recognition::result::PreviousBestValue::NotPlayed,
                    miss_count: scorepeek_core::recognition::result::PreviousBestValue::NotPlayed,
                },
                play_options: PlayOptions::Known { values: Vec::new() },
            },
        );
        state.reduce(&result, &result.to_value().unwrap());
    }
    assert_eq!(state.result_count, RESULT_HISTORY_CAPACITY as u64 + 1);
    assert_eq!(state.result_history.len(), RESULT_HISTORY_CAPACITY);
    assert_eq!(state.result_history.front().unwrap().ordinal, 2);
    assert_eq!(
        state.result_history.back().unwrap().ordinal,
        RESULT_HISTORY_CAPACITY as u64 + 1
    );

    let next_session = RunEvent {
        schema: RUN_EVENT_SCHEMA.to_owned(),
        kind: RunEventKind::SessionStarted {
            session_id: Some("session-2".to_owned()),
            capture_generation: 2,
            capture_profile_sha256: "b".repeat(64),
            normalizer_artifact_sha256: "c".repeat(64),
        },
    };
    state.reduce(&next_session, &next_session.to_value().unwrap());
    assert_eq!(
        state.result_history.back().unwrap().ordinal,
        RESULT_HISTORY_CAPACITY as u64 + 1
    );
    assert_eq!(state.result_count, RESULT_HISTORY_CAPACITY as u64 + 1);
    assert!(state.stable_result_song.is_none());
}

#[test]
fn result_value_labels_preserve_domain_states_without_debug_reasons() {
    use scorepeek_core::recognition::result::ResultFieldUnknownReason;

    assert_eq!(
        supplemental_u32(&SupplementalResultValue::Known { value: 1_234 }),
        "1,234"
    );
    assert_eq!(
        supplemental_u32(&SupplementalResultValue::NotDisplayed),
        "--"
    );
    assert_eq!(
        supplemental_u32(&SupplementalResultValue::Unknown {
            reason: ResultFieldUnknownReason::InvalidFormat,
        }),
        "?"
    );
    assert_eq!(previous_text(&PreviousBestValue::NotPlayed), "NO PLAY");
    assert_eq!(previous_u32(&PreviousBestValue::NotDisplayed), "--");
    assert_eq!(
        previous_u32(&PreviousBestValue::Unknown {
            reason: ResultFieldUnknownReason::OutOfRange,
        }),
        "?"
    );
}

#[test]
fn fitted_song_text_uses_an_ellipsis_without_mutating_the_value() {
    let value = "非常に長い曲名を完全な状態で保持する";
    let rendered = fitted_value("Catalog title: ", value, 24);
    assert!(rendered.starts_with("Catalog title: "));
    assert!(rendered.ends_with('…'));
    assert!(Line::raw(rendered).width() <= 24);
    assert_eq!(value, "非常に長い曲名を完全な状態で保持する");
}

fn populated_select_test_state() -> MusicSelectResolverState {
    use scorepeek_core::recognition::music_select::{
        BestClearType, BestValue, MusicSelectBestValues,
    };
    let (_, evidence, _) = music_selection_test_observation();
    let candidate = &evidence.candidates[0];
    let selection = MusicSelectionState::Selected {
        scorepeek_song_id: candidate.song_id,
        play_side: PlaySide::OnePlayer,
        play_type: candidate.chart.key.play_type,
        difficulty: candidate.chart.key.difficulty,
        level: candidate.chart.level,
        notes: candidate.chart.notes,
        presentation: candidate_song_presentation(candidate),
    };
    let mut state = MusicSelectResolverState::default();
    state.active = true;
    state.screen_episode_id = 1;
    for _ in 0..2 {
        state.observe(
            BestChart::from_selection(selection.clone()).unwrap(),
            MusicSelectBestValues {
                score: BestValue::Known(1200),
                miss_count: BestValue::Unknown,
                clear_type: BestValue::Known(BestClearType::Clear),
            },
        );
    }
    state.publish_candidate("session", 1, 2, 200).unwrap();
    state
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the 80x25 fixture spells out every simultaneously visible resolver node and gate"
)]
fn four_pane_tui_keeps_all_gates_at_minimum_size() {
    let mut state = RunViewState::new("invocation-1".to_owned(), "a".repeat(64), true);
    state.watcher_state = "session_active".to_owned();
    state.capture_generation = Some(3);
    state.resolver = ResolverDebugSnapshot {
        now_ms: 14_900,
        raw_screen: Some("result".to_owned()),
        screen: Some("result".to_owned()),
        suspended: false,
        finalizing: false,
        screen_episode_id: 18,
        screen_episode_started_ms: Some(2_000),
        source_sequence: Some(1_240),
        latest_field_sequence: Some(1_238),
        latest_field_ms: Some(13_000),
        play_options: Some(PlayOptionsDebugSnapshot {
            latest: PlayOptionsObservation {
                raw_text: Some("USE OPTION RANDOM".to_owned()),
                parsed: PlayOptions::Known {
                    values: vec![PlayOption::Random],
                },
                ..PlayOptionsObservation::default()
            },
            observations: 2,
            conflicting: false,
            resolved: PlayOptions::Known {
                values: vec![PlayOption::Random],
            },
        }),
        selection_difficulty_target: Some(SelectionDifficultyTarget::Successor),
        selection_difficulty: Some(CurrentSelectionDifficulty::observed(
            Difficulty::Hyper,
            1_238,
            13_000,
        )),
        local: Some(ResolverNodeSnapshot {
            label: "RESULT resolver",
            started_ms: Some(3_000),
            last_observation_ms: Some(13_000),
            observations: 6,
            top: Some("TEST SONG / HYPER Lv8".to_owned()),
            runner_up: Some("OTHER SONG / HYPER Lv8".to_owned()),
            runner_song: Some("OTHER SONG / SP HYPER Lv8 notes=764".to_owned()),
            runner_chart: None,
            top_candidates: vec!["TEST SONG / SP HYPER Lv8 notes=764".to_owned()],
            support: 320,
            margin: 80,
            song_margin: 80,
            chart_margin: 320,
            select_play_type: None,
            result_play_type: Some(PlayType::Single),
            play_type_mismatch: false,
            family_contributions: vec!["result_title=300".to_owned()],
            current_difficulty: None,
            state: ResolverResolutionState::AcceptedJoint,
        }),
        successor: Some(ResolverNodeSnapshot {
            label: "successor",
            started_ms: Some(13_000),
            last_observation_ms: Some(14_000),
            observations: 2,
            top: Some("NEXT / SP HYPER Lv9 notes=900".to_owned()),
            runner_up: None,
            runner_song: None,
            runner_chart: None,
            top_candidates: vec!["NEXT / SP HYPER Lv9 notes=900".to_owned()],
            support: 140,
            margin: 140,
            song_margin: 140,
            chart_margin: 140,
            select_play_type: Some(PlayType::Single),
            result_play_type: None,
            play_type_mismatch: false,
            family_contributions: vec!["select_title=140".to_owned()],
            current_difficulty: Some(CurrentSelectionDifficulty::observed(
                Difficulty::Hyper,
                1_238,
                13_000,
            )),
            state: ResolverResolutionState::JointCandidate,
        }),
        attempt: Some(AttemptNodeSnapshot {
            attempt_id: Some(14),
            started_ms: Some(1_000),
            phase_started_ms: Some(2_000),
            phase: "result".to_owned(),
            path: "S-D-P-R".to_owned(),
            select_top: Some("TEST SONG / HYPER Lv8".to_owned()),
            result_top: Some("TEST SONG / HYPER Lv8".to_owned()),
            joint_top: Some("TEST SONG / HYPER Lv8".to_owned()),
            support: 400,
            margin: 160,
            song_margin: 160,
            chart_margin: 400,
            runner_song: Some("OTHER SONG / SP HYPER Lv8 notes=764".to_owned()),
            runner_chart: None,
            top_candidates: vec!["TEST SONG / SP HYPER Lv8 notes=764".to_owned()],
            family_contributions: vec!["result_title=300".to_owned()],
            state: ResolverResolutionState::AcceptedJoint,
        }),
        gate: "waiting: numeric performance".to_owned(),
        gates: vec![
            GateSnapshot {
                label: "link",
                state: GateState::Accepted,
            },
            GateSnapshot {
                label: "identity",
                state: GateState::Accepted,
            },
            GateSnapshot {
                label: "clear",
                state: GateState::Accepted,
            },
            GateSnapshot {
                label: "numeric",
                state: GateState::Pending,
            },
            GateSnapshot {
                label: "drain",
                state: GateState::Inactive,
            },
            GateSnapshot {
                label: "emit",
                state: GateState::Inactive,
            },
        ],
        raw_fields: vec![
            (
                "marker".to_owned(),
                "known:hyper score=500000 margin=250000".to_owned(),
            ),
            ("title".to_owned(), "OCR TITLE".to_owned()),
        ],
    };
    state.music_select = populated_select_test_state();
    let health = ChannelHealth::default();
    for (width, height) in [(120, 40), (80, 25), (79, 24)] {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, &state, Path::new("/run/scorepeek.sock"), &health))
            .unwrap();
        if width == 80 {
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>();
            assert!(rendered.contains("Music Select Resolver"));
            assert!(rendered.contains("SCORE 1200  MISS unknown"));
            assert!(rendered.contains("partial snapshot emitted revision=1"));
            assert!(rendered.contains("Watcher"));
            assert!(rendered.contains("Latest result"));
            assert!(rendered.contains("Resolver"));
            assert!(rendered.contains("episode=#18"));
            assert!(rendered.contains("ATTEMPT #14"));
            assert!(rendered.contains("numeric…"));
            assert!(rendered.contains("link✓"));
            assert!(rendered.contains("identity✓"));
            assert!(rendered.contains("clear✓"));
            assert!(rendered.contains("drain–"));
            assert!(rendered.contains("emit–"));
            assert!(
                terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .any(|cell| cell.fg == Color::Green)
            );
            assert!(
                terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .any(|cell| cell.fg == Color::Yellow)
            );
        }
    }
}

#[test]
fn selected_chart_remains_visible_at_minimum_width_with_a_long_title() {
    let mut state = RunViewState::new("invocation-1".to_owned(), "a".repeat(64), true);
    let scorepeek_song_id =
        serde_json::from_str("\"00000000-0000-0000-0000-000000000046\"").unwrap();
    for difficulty in [Difficulty::Hyper, Difficulty::Leggendaria] {
        state.latest_music_selection = Some(MusicSelectionState::Selected {
            scorepeek_song_id,
            play_side: PlaySide::OnePlayer,
            play_type: PlayType::Double,
            difficulty,
            level: 12,
            notes: 2000,
            presentation: SongPresentation {
                scorepeek_song_id,
                display_titles: vec!["長い曲名".repeat(30)],
                artist: "ARTIST".to_owned(),
            },
        });
        state.music_select.active = true;
        state.music_select.observe(
            BestChart::from_selection(state.latest_music_selection.clone().unwrap()).unwrap(),
            scorepeek_core::recognition::music_select::MusicSelectBestValues::default(),
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 25)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &state,
                    Path::new("/run/scorepeek.sock"),
                    &ChannelHealth::default(),
                );
            })
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains(&format!("DP {} / ", difficulty_label(difficulty))));
        assert!(rendered.contains('…'));
    }
}

#[test]
fn semantic_palette_keeps_typed_state_and_domain_colors_consistent() {
    assert_eq!(
        resolution_color(ResolverResolutionState::AcceptedJoint),
        Color::Green
    );
    assert_eq!(
        resolution_color(ResolverResolutionState::JointCandidate),
        Color::Cyan
    );
    assert_eq!(
        resolution_color(ResolverResolutionState::Unresolved),
        Color::Yellow
    );
    assert_eq!(
        resolution_color(ResolverResolutionState::Conflict),
        Color::Red
    );
    assert_eq!(gate_color(GateState::Inactive), Color::DarkGray);
    assert_eq!(gate_suffix(GateState::Accepted), "✓");
    assert_eq!(gate_suffix(GateState::Pending), "…");
    assert_eq!(gate_suffix(GateState::Failed), "✗");
    assert_eq!(gate_suffix(GateState::Inactive), "–");
    assert_eq!(difficulty_color(Difficulty::Beginner), Color::Green);
    assert_eq!(difficulty_color(Difficulty::Normal), Color::Blue);
    assert_eq!(difficulty_color(Difficulty::Hyper), Color::Yellow);
    assert_eq!(difficulty_color(Difficulty::Another), Color::Red);
    assert_eq!(difficulty_color(Difficulty::Leggendaria), Color::Magenta);
    assert_eq!(clear_type_color("FAILED"), Color::Red);
    assert_eq!(clear_type_color("ASSIST CLEAR"), Color::Yellow);
    assert_eq!(clear_type_color("F-COMBO"), Color::Green);
}

#[test]
fn episode_evidence_accumulates_by_family_caps_and_unknown_does_not_erase() {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
    let candidate = JointEvidenceCandidate {
        song_id,
        chart: scorepeek_core::catalog::Chart {
            key: scorepeek_core::catalog::ChartKey {
                play_type: PlayType::Single,
                difficulty: Difficulty::Hyper,
            },
            level: 8,
            notes: 764,
        },
        display_titles: vec!["TEST SONG".to_owned()],
        artist: "TEST ARTIST".to_owned(),
        family_support: BTreeMap::from([
            (EvidenceFamily::ResultTitle, 70),
            (EvidenceFamily::ResultArtist, 35),
            (EvidenceFamily::ResultChart, 50),
        ]),
        support: 155,
    };
    let observation = JointEvidenceObservation {
        catalog_song_count: 0,
        candidates: vec![candidate],
    };
    let mut accumulator = HypothesisAccumulator::default();
    let chart_factor = ResultChartFactor {
        play_type: Some(PlayType::Single),
        difficulty: Some(Difficulty::Hyper),
        notes: Some(764),
        level: Some(8),
    };
    accumulator.observe(100, &observation, None, Some(chart_factor));
    assert_eq!(
        accumulator.summary().state,
        ResolverResolutionState::JointCandidate
    );
    accumulator.observe(
        200,
        &JointEvidenceObservation {
            catalog_song_count: 0,
            candidates: Vec::new(),
        },
        None,
        None,
    );
    accumulator.observe(300, &observation, None, Some(chart_factor));
    assert_eq!(
        accumulator.summary().state,
        ResolverResolutionState::AcceptedJoint
    );
    for tick in 0..20 {
        accumulator.observe(400 + tick, &observation, None, None);
    }
    let accepted = accumulator.summary().accepted().unwrap();
    let stored = &accumulator.candidates[&JointKey {
        song_id,
        chart_key: accepted.chart.key,
    }];
    assert!(
        stored
            .family_support
            .values()
            .any(|support| { *support > u64::from(EVIDENCE_FAMILY_CAP) })
    );
    assert!(
        accumulator
            .summary()
            .selected_family_support
            .values()
            .all(|support| support.normalized() <= EVIDENCE_FAMILY_CAP)
    );
}

#[test]
fn family_normalization_preserves_candidate_ratios_above_the_cap() {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
    let make = |difficulty, support| JointEvidenceCandidate {
        song_id,
        chart: scorepeek_core::catalog::Chart {
            key: scorepeek_core::catalog::ChartKey {
                play_type: PlayType::Single,
                difficulty,
            },
            level: 8,
            notes: 764,
        },
        display_titles: vec!["TEST SONG".to_owned()],
        artist: "TEST ARTIST".to_owned(),
        family_support: BTreeMap::from([(EvidenceFamily::ResultArtist, support)]),
        support,
    };
    let observation = JointEvidenceObservation {
        catalog_song_count: 0,
        candidates: vec![make(Difficulty::Hyper, 170), make(Difficulty::Another, 70)],
    };
    let mut accumulator = HypothesisAccumulator::default();
    for tick in 0..3 {
        accumulator.observe(100 + tick, &observation, None, None);
    }
    let summary = accumulator.summary();
    assert_eq!(summary.support, 300);
    assert_eq!(summary.support.saturating_sub(summary.margin), 123);
    assert_eq!(summary.margin, 177);
    assert_eq!(
        summary.selected_family_support[&EvidenceFamily::ResultArtist],
        EvidenceContribution::new(510, 300)
    );
    assert_eq!(
        summary.runner_up_family_support[&EvidenceFamily::ResultArtist],
        EvidenceContribution::new(210, 123)
    );
}

#[test]
fn chart_factors_wait_for_song_evidence_and_apply_to_later_candidates() {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000011\"").unwrap();
    let mut accumulator = HypothesisAccumulator::default();
    accumulator.observe(
        100,
        &JointEvidenceObservation {
            catalog_song_count: 2,
            candidates: Vec::new(),
        },
        Some(Difficulty::Hyper),
        Some(ResultChartFactor {
            play_type: None,
            difficulty: Some(Difficulty::Hyper),
            notes: Some(1_136),
            level: Some(10),
        }),
    );
    assert_eq!(
        accumulator.summary().state,
        ResolverResolutionState::Unresolved
    );

    let candidate = |difficulty, notes| JointEvidenceCandidate {
        song_id,
        chart: scorepeek_core::catalog::Chart {
            key: scorepeek_core::catalog::ChartKey {
                play_type: PlayType::Single,
                difficulty,
            },
            level: 10,
            notes,
        },
        display_titles: vec!["∀".to_owned()],
        artist: "BEMANI Sound Team \"HuΣeR\" respect for D.J.Amuro".to_owned(),
        family_support: BTreeMap::from([(EvidenceFamily::ResultArtist, 220)]),
        support: 220,
    };
    accumulator.observe(
        200,
        &JointEvidenceObservation {
            catalog_song_count: 2,
            candidates: vec![
                candidate(Difficulty::Hyper, 1_136),
                candidate(Difficulty::Another, 1_500),
            ],
        },
        None,
        None,
    );
    let summary = accumulator.summary();
    assert_eq!(
        summary.selected.unwrap().chart.key.difficulty,
        Difficulty::Hyper
    );
    assert_eq!(summary.support, 370);
    assert_eq!(summary.chart_margin, 140);
}

#[test]
fn selection_epoch_retains_difficulty_until_song_evidence_arrives() {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000012\"").unwrap();
    let mut epochs = SelectionEpochTracker::default();
    let pending = epochs.observe(
        100,
        &JointEvidenceObservation {
            catalog_song_count: 2,
            candidates: Vec::new(),
        },
        Some(Difficulty::Hyper),
    );
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].target, SelectionDifficultyTarget::Pending);
    assert_eq!(
        pending[0].reason,
        SelectionDifficultyTransitionReason::Changed
    );
    let candidate = |difficulty| JointEvidenceCandidate {
        song_id,
        chart: scorepeek_core::catalog::Chart {
            key: scorepeek_core::catalog::ChartKey {
                play_type: PlayType::Single,
                difficulty,
            },
            level: 10,
            notes: if difficulty == Difficulty::Hyper {
                1_136
            } else {
                1_500
            },
        },
        display_titles: vec!["∀".to_owned()],
        artist: "ARTIST".to_owned(),
        family_support: BTreeMap::from([(EvidenceFamily::SelectTitle, 300)]),
        support: 300,
    };
    let applied = epochs.observe(
        200,
        &JointEvidenceObservation {
            catalog_song_count: 2,
            candidates: vec![candidate(Difficulty::Hyper), candidate(Difficulty::Another)],
        },
        None,
    );
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].target, SelectionDifficultyTarget::Incumbent);
    assert_eq!(
        applied[0].reason,
        SelectionDifficultyTransitionReason::PendingApplied
    );
    let summary = epochs.incumbent.summary();
    assert_eq!(
        summary.selected.unwrap().chart.key.difficulty,
        Difficulty::Hyper
    );
    assert_eq!(summary.chart_margin, 50);
    assert!(epochs.pending_difficulty.is_none());
}

#[test]
fn selection_difficulty_tracks_every_known_change_without_changing_song_evidence() {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000013\"").unwrap();
    let candidate = |difficulty| JointEvidenceCandidate {
        song_id,
        chart: scorepeek_core::catalog::Chart {
            key: scorepeek_core::catalog::ChartKey {
                play_type: PlayType::Single,
                difficulty,
            },
            level: 10,
            notes: 1_000,
        },
        display_titles: vec!["X".to_owned()],
        artist: "ARTIST".to_owned(),
        family_support: BTreeMap::from([(EvidenceFamily::SelectTitle, 300)]),
        support: 300,
    };
    let song_evidence = JointEvidenceObservation {
        catalog_song_count: 2,
        candidates: vec![
            candidate(Difficulty::Normal),
            candidate(Difficulty::Hyper),
            candidate(Difficulty::Another),
        ],
    };
    let no_song_evidence = JointEvidenceObservation {
        catalog_song_count: 2,
        candidates: Vec::new(),
    };
    let mut epochs = SelectionEpochTracker::default();
    let first = epochs.observe_at(3_240, 324_000, &song_evidence, Some(Difficulty::Hyper));
    assert_eq!(first.len(), 1);
    for sequence in 3_241..=3_252 {
        assert!(
            epochs
                .observe_at(
                    sequence,
                    sequence * 100,
                    &song_evidence,
                    Some(Difficulty::Hyper)
                )
                .is_empty()
        );
    }
    let song_support = epochs
        .incumbent
        .candidates
        .values()
        .next()
        .unwrap()
        .family_support[&EvidenceFamily::SelectTitle];
    let observation_count = epochs.incumbent.observation_count;

    for (sequence, difficulty) in [
        (3_255, Difficulty::Another),
        (3_296, Difficulty::Normal),
        (3_302, Difficulty::Hyper),
        (3_307, Difficulty::Another),
    ] {
        let transitions = epochs.observe_at(
            sequence,
            sequence * 100,
            &no_song_evidence,
            Some(difficulty),
        );
        assert_eq!(transitions.len(), 1);
        assert_eq!(
            transitions[0].reason,
            SelectionDifficultyTransitionReason::Changed
        );
        assert_eq!(transitions[0].target, SelectionDifficultyTarget::Incumbent);
        assert_eq!(transitions[0].current.unwrap().difficulty, difficulty);
        assert_eq!(
            epochs
                .incumbent
                .summary()
                .selected
                .unwrap()
                .chart
                .key
                .difficulty,
            difficulty
        );
        assert_eq!(epochs.incumbent.observation_count, observation_count);
        assert_eq!(
            epochs
                .incumbent
                .candidates
                .values()
                .next()
                .unwrap()
                .family_support[&EvidenceFamily::SelectTitle],
            song_support
        );
    }
}

#[test]
fn unknown_difficulty_gap_retains_current_and_emits_no_transition() {
    let mut accumulator = HypothesisAccumulator::default();
    assert!(accumulator.observe_select_difficulty(Difficulty::Another, 10, 1_000));
    let retained = accumulator.select_difficulty.unwrap();
    let mut epochs = SelectionEpochTracker {
        incumbent: accumulator,
        ..SelectionEpochTracker::default()
    };
    epochs.incumbent.observation_count = 1;
    let transitions = epochs.observe_at(
        11,
        1_100,
        &JointEvidenceObservation {
            catalog_song_count: 2,
            candidates: Vec::new(),
        },
        None,
    );
    assert!(transitions.is_empty());
    assert_eq!(epochs.incumbent.select_difficulty, Some(retained));
}

#[test]
fn snapshot_merge_adopts_newer_difficulty_once_instead_of_adding_votes() {
    let mut retained = HypothesisAccumulator::default();
    retained.observe_select_difficulty(Difficulty::Hyper, 100, 1_000);
    for sequence in 101..110 {
        retained.observe_select_difficulty(Difficulty::Hyper, sequence, sequence * 10);
    }
    let mut incoming = HypothesisAccumulator::default();
    incoming.observe_select_difficulty(Difficulty::Another, 200, 2_000);
    retained.add_from(&incoming);
    retained.add_from(&incoming);
    let current = retained.select_difficulty.unwrap();
    assert_eq!(current.difficulty, Difficulty::Another);
    assert_eq!(current.consecutive_known, 1);
    assert_eq!(current.last_sequence(), 200);
}

#[test]
fn late_difficulty_observation_cannot_replace_newer_current_state() {
    let mut accumulator = HypothesisAccumulator::default();
    assert!(accumulator.observe_select_difficulty(Difficulty::Another, 200, 2_000));
    assert!(!accumulator.observe_select_difficulty(Difficulty::Normal, 199, 1_990));
    let current = accumulator.select_difficulty.unwrap();
    assert_eq!(current.difficulty, Difficulty::Another);
    assert_eq!(current.consecutive_known, 1);
    assert_eq!(current.last_sequence(), 200);
}

#[test]
fn diagnostic_top_does_not_truncate_resolver_authority() {
    let first = serde_json::from_str("\"00000000-0000-0000-0000-000000000051\"").unwrap();
    let second = serde_json::from_str("\"00000000-0000-0000-0000-000000000052\"").unwrap();
    let keys = [
        (PlayType::Single, Difficulty::Beginner),
        (PlayType::Single, Difficulty::Normal),
        (PlayType::Single, Difficulty::Hyper),
        (PlayType::Single, Difficulty::Another),
        (PlayType::Single, Difficulty::Leggendaria),
        (PlayType::Double, Difficulty::Normal),
        (PlayType::Double, Difficulty::Hyper),
        (PlayType::Double, Difficulty::Another),
    ];
    let candidate = |song_id, play_type, difficulty, support| JointEvidenceCandidate {
        song_id,
        chart: scorepeek_core::catalog::Chart {
            key: scorepeek_core::catalog::ChartKey {
                play_type,
                difficulty,
            },
            level: 10,
            notes: 1_000,
        },
        display_titles: vec![format!("SONG-{song_id:?}")],
        artist: "ARTIST".to_owned(),
        family_support: BTreeMap::from([(EvidenceFamily::ResultArtist, support)]),
        support,
    };
    let mut candidates = keys
        .into_iter()
        .map(|(play_type, difficulty)| candidate(first, play_type, difficulty, 300))
        .collect::<Vec<_>>();
    candidates.push(candidate(second, PlayType::Single, Difficulty::Hyper, 270));
    let mut event = accepted_result_event(1);
    let RunEventKind::FieldObservation { joint_evidence, .. } = &mut event.kind else {
        unreachable!();
    };
    joint_evidence.candidates = candidates;
    joint_evidence.catalog_song_count = 2;

    let mut authority = HypothesisAccumulator::default();
    authority.observe(100, joint_evidence, None, None);
    assert_eq!(authority.summary().state, ResolverResolutionState::Conflict);
    assert_eq!(joint_evidence.candidates.len(), 9);
    let diagnostic = diagnostic_run_event_value(&event).unwrap();
    assert_eq!(
        diagnostic["joint_evidence"]["candidates"]
            .as_array()
            .unwrap()
            .len(),
        8
    );
    let RunEventKind::FieldObservation { joint_evidence, .. } = &event.kind else {
        unreachable!();
    };
    assert_eq!(joint_evidence.candidates.len(), 9);
}

#[test]
fn runner_song_and_runner_chart_are_independent_hierarchical_competitors() {
    let first = serde_json::from_str("\"00000000-0000-0000-0000-000000000021\"").unwrap();
    let second = serde_json::from_str("\"00000000-0000-0000-0000-000000000022\"").unwrap();
    let candidate = |song_id, play_type, support| JointEvidenceCandidate {
        song_id,
        chart: scorepeek_core::catalog::Chart {
            key: scorepeek_core::catalog::ChartKey {
                play_type,
                difficulty: Difficulty::Hyper,
            },
            level: 10,
            notes: 1_136,
        },
        display_titles: vec![format!("SONG-{song_id:?}")],
        artist: "ARTIST".to_owned(),
        family_support: BTreeMap::from([(EvidenceFamily::ResultArtist, support)]),
        support,
    };
    let mut accumulator = HypothesisAccumulator::default();
    accumulator.observe(
        100,
        &JointEvidenceObservation {
            catalog_song_count: 2,
            candidates: vec![
                candidate(first, PlayType::Single, 400),
                candidate(first, PlayType::Double, 380),
                candidate(second, PlayType::Single, 350),
            ],
        },
        None,
        None,
    );
    let summary = accumulator.summary();
    assert_eq!(summary.runner_up.as_ref().unwrap().song_id, first);
    assert_eq!(summary.runner_chart.as_ref().unwrap().song_id, first);
    assert_eq!(summary.runner_song.as_ref().unwrap().song_id, second);
    assert_eq!(summary.song_margin, 38);
    assert_eq!(summary.chart_margin, 15);
}

#[test]
fn latest_failure_oracle_resolves_forall_from_cross_screen_factors() {
    let wrong = serde_json::from_str("\"00000000-0000-0000-0000-000000000031\"").unwrap();
    let expected = serde_json::from_str("\"00000000-0000-0000-0000-000000000032\"").unwrap();
    let chart = |song_id, play_type, notes, family, support| JointEvidenceCandidate {
        song_id,
        chart: scorepeek_core::catalog::Chart {
            key: scorepeek_core::catalog::ChartKey {
                play_type,
                difficulty: Difficulty::Hyper,
            },
            level: 10,
            notes,
        },
        display_titles: vec![if song_id == expected { "∀" } else { "A" }.to_owned()],
        artist: if song_id == expected {
            "BEMANI Sound Team \"HuΣeR\" respect for D.J.Amuro"
        } else {
            "OTHER"
        }
        .to_owned(),
        family_support: BTreeMap::from([(family, support)]),
        support,
    };
    let mut select = HypothesisAccumulator::default();
    select.observe(
        100,
        &JointEvidenceObservation {
            catalog_song_count: 2,
            candidates: vec![chart(
                wrong,
                PlayType::Single,
                1_000,
                EvidenceFamily::SelectTitle,
                300,
            )],
        },
        Some(Difficulty::Hyper),
        None,
    );
    let mut result = HypothesisAccumulator::default();
    result.observe(
        200,
        &JointEvidenceObservation {
            catalog_song_count: 2,
            candidates: vec![
                chart(
                    expected,
                    PlayType::Single,
                    1_136,
                    EvidenceFamily::ResultArtist,
                    300,
                ),
                chart(
                    expected,
                    PlayType::Double,
                    1_500,
                    EvidenceFamily::ResultArtist,
                    300,
                ),
            ],
        },
        None,
        Some(ResultChartFactor {
            play_type: Some(PlayType::Single),
            difficulty: Some(Difficulty::Hyper),
            notes: Some(1_136),
            level: None,
        }),
    );
    result.observe(
        201,
        &JointEvidenceObservation {
            catalog_song_count: 2,
            candidates: Vec::new(),
        },
        None,
        Some(ResultChartFactor {
            play_type: Some(PlayType::Single),
            difficulty: Some(Difficulty::Hyper),
            notes: Some(1_136),
            level: None,
        }),
    );
    select.add_from(&result);
    let summary = select.summary();
    let accepted = summary.accepted().expect("∀ SP HYPER should resolve");
    assert_eq!(accepted.song_id, expected);
    assert_eq!(accepted.chart.key.play_type, PlayType::Single);
    assert_eq!(accepted.chart.notes, 1_136);
    assert_eq!(summary.song_margin, 100);
    assert_eq!(summary.chart_margin, 200);
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the regression covers unresolved, accepted, conflicting, and mismatched SP/DP evidence"
)]
fn result_play_type_breaks_an_otherwise_identical_sibling_tie() {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000043\"").unwrap();
    let chart = |play_type| JointEvidenceCandidate {
        song_id,
        chart: scorepeek_core::catalog::Chart {
            key: scorepeek_core::catalog::ChartKey {
                play_type,
                difficulty: Difficulty::Hyper,
            },
            level: 8,
            notes: 829,
        },
        display_titles: vec!["Wizards!".to_owned()],
        artist: "ARTIST".to_owned(),
        family_support: BTreeMap::from([(EvidenceFamily::ResultTitle, 300)]),
        support: 300,
    };
    let observation = JointEvidenceObservation {
        catalog_song_count: 2,
        candidates: vec![chart(PlayType::Single), chart(PlayType::Double)],
    };
    let mut without_play_type = HypothesisAccumulator::default();
    without_play_type.observe(
        100,
        &observation,
        None,
        Some(ResultChartFactor {
            play_type: None,
            difficulty: Some(Difficulty::Hyper),
            notes: Some(829),
            level: Some(8),
        }),
    );
    assert_eq!(
        without_play_type.summary().state,
        ResolverResolutionState::SongProjected
    );

    let mut with_play_type = HypothesisAccumulator::default();
    with_play_type.observe(
        100,
        &observation,
        None,
        Some(ResultChartFactor {
            play_type: Some(PlayType::Single),
            difficulty: Some(Difficulty::Hyper),
            notes: Some(829),
            level: Some(8),
        }),
    );
    assert_eq!(
        with_play_type.summary().state,
        ResolverResolutionState::SongProjected
    );
    with_play_type.observe(
        101,
        &observation,
        None,
        Some(ResultChartFactor {
            play_type: Some(PlayType::Single),
            difficulty: Some(Difficulty::Hyper),
            notes: Some(829),
            level: Some(8),
        }),
    );
    let summary = with_play_type.summary();
    assert_eq!(summary.state, ResolverResolutionState::AcceptedJoint);
    assert_eq!(
        summary
            .selected
            .as_ref()
            .map(|candidate| candidate.chart.key.play_type),
        Some(PlayType::Single)
    );
    assert_eq!(summary.chart_margin, 100);
    assert_eq!(
        summary.selected_family_support[&EvidenceFamily::ResultPlayType].normalized(),
        100
    );

    with_play_type.observe(
        102,
        &observation,
        None,
        Some(ResultChartFactor {
            play_type: Some(PlayType::Double),
            difficulty: Some(Difficulty::Hyper),
            notes: Some(829),
            level: Some(8),
        }),
    );
    assert_eq!(
        with_play_type.summary().state,
        ResolverResolutionState::SongProjected
    );

    let mut mismatched_only = HypothesisAccumulator::default();
    let double_only = JointEvidenceObservation {
        catalog_song_count: 1,
        candidates: vec![chart(PlayType::Double)],
    };
    for monotonic_ms in [200, 201] {
        mismatched_only.observe(
            monotonic_ms,
            &double_only,
            None,
            Some(ResultChartFactor {
                play_type: Some(PlayType::Single),
                difficulty: Some(Difficulty::Hyper),
                notes: Some(829),
                level: Some(8),
            }),
        );
    }
    assert_eq!(
        mismatched_only.summary().state,
        ResolverResolutionState::AcceptedJoint
    );
    assert_eq!(
        mismatched_only
            .summary()
            .selected
            .as_ref()
            .map(|candidate| candidate.chart.key.play_type),
        Some(PlayType::Double)
    );
}

#[test]
fn select_play_type_resolves_ui_chart_without_joint_acceptance() {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000044\"").unwrap();
    let candidate = |play_type| JointEvidenceCandidate {
        song_id,
        chart: scorepeek_core::catalog::Chart {
            key: scorepeek_core::catalog::ChartKey {
                play_type,
                difficulty: Difficulty::Hyper,
            },
            level: 8,
            notes: 829,
        },
        display_titles: vec!["Wizards!".to_owned()],
        artist: "ARTIST".to_owned(),
        family_support: BTreeMap::from([(EvidenceFamily::SelectTitle, 300)]),
        support: 300,
    };
    let evidence = JointEvidenceObservation {
        catalog_song_count: 1,
        candidates: vec![candidate(PlayType::Single), candidate(PlayType::Double)],
    };
    let mut select = HypothesisAccumulator::default();
    for sequence in [1, 2] {
        select.observe_at(
            sequence,
            sequence * 100,
            &evidence,
            Some(Difficulty::Hyper),
            Some(PlayType::Double),
            None,
        );
    }
    let summary = select.summary();
    assert_ne!(summary.state, ResolverResolutionState::AcceptedJoint);
    assert_eq!(summary.select_play_type, Some(PlayType::Double));
    select.observe_at(
        3,
        300,
        &JointEvidenceObservation {
            catalog_song_count: 1,
            candidates: Vec::new(),
        },
        None,
        None,
        Some(ResultChartFactor {
            play_type: None,
            difficulty: None,
            notes: None,
            level: None,
        }),
    );
    assert_eq!(
        select.summary().state,
        ResolverResolutionState::SongProjected
    );
    assert_eq!(
        summary
            .selected
            .as_ref()
            .map(|candidate| candidate.chart.key.play_type),
        Some(PlayType::Double)
    );
}

fn mode_conflict_fixture(
    select_mode: PlayType,
    result_mode: Option<PlayType>,
    notes: Option<u32>,
) -> HypothesisAccumulator {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000045\"").unwrap();
    let evidence = |family| JointEvidenceObservation {
        catalog_song_count: 1,
        candidates: [(PlayType::Single, 829), (PlayType::Double, 900)]
            .map(|(play_type, notes)| JointEvidenceCandidate {
                song_id,
                chart: scorepeek_core::catalog::Chart {
                    key: scorepeek_core::catalog::ChartKey {
                        play_type,
                        difficulty: Difficulty::Hyper,
                    },
                    level: 8,
                    notes,
                },
                display_titles: vec!["Wizards!".to_owned()],
                artist: "ARTIST".to_owned(),
                family_support: BTreeMap::from([(family, 300)]),
                support: 300,
            })
            .to_vec(),
    };
    let mut joint = HypothesisAccumulator::default();
    for sequence in [1, 2] {
        joint.observe_at(
            sequence,
            sequence * 100,
            &evidence(EvidenceFamily::SelectTitle),
            Some(Difficulty::Hyper),
            Some(select_mode),
            None,
        );
    }
    for sequence in [3, 4] {
        joint.observe_at(
            sequence,
            sequence * 100,
            &evidence(EvidenceFamily::ResultTitle),
            None,
            None,
            Some(ResultChartFactor {
                play_type: result_mode,
                difficulty: Some(Difficulty::Hyper),
                notes,
                level: Some(8),
            }),
        );
    }
    joint
}

#[test]
fn select_result_mode_conflicts_follow_chart_evidence_in_both_directions() {
    for (select_mode, result_mode) in [
        (PlayType::Single, PlayType::Double),
        (PlayType::Double, PlayType::Single),
    ] {
        for (notes, expected) in [(829, PlayType::Single), (900, PlayType::Double)] {
            let summary =
                mode_conflict_fixture(select_mode, Some(result_mode), Some(notes)).summary();
            assert_eq!(summary.state, ResolverResolutionState::AcceptedJoint);
            assert_eq!(summary.select_play_type, Some(select_mode));
            assert_eq!(summary.result_play_type, Some(result_mode));
            assert_eq!(summary.selected.unwrap().chart.key.play_type, expected);
        }
    }
}

#[test]
fn mode_conflicts_require_the_existing_chart_margin() {
    let mut joint = mode_conflict_fixture(PlayType::Single, Some(PlayType::Double), None);
    assert_eq!(
        joint.summary().state,
        ResolverResolutionState::SongProjected
    );
    assert_eq!(joint.summary().chart_margin, 0);
    for candidate in joint.candidates.values_mut() {
        if candidate.candidate.chart.key.play_type == PlayType::Double {
            candidate
                .family_support
                .insert(EvidenceFamily::ResultArtist, 20);
        }
    }
    assert_eq!(joint.summary().chart_margin, 20);
    assert_eq!(
        joint.summary().state,
        ResolverResolutionState::SongProjected
    );
}

#[test]
fn select_mode_supplements_unknown_result_mode() {
    let summary = mode_conflict_fixture(PlayType::Double, None, None).summary();
    assert_eq!(summary.state, ResolverResolutionState::AcceptedJoint);
    assert_eq!(summary.result_play_type, None);
    assert_eq!(
        summary.selected.unwrap().chart.key.play_type,
        PlayType::Double
    );
    assert_eq!(
        summary.selected_family_support[&EvidenceFamily::SelectPlayType].normalized(),
        100
    );
}

#[test]
fn music_select_fields_update_the_typed_tui_snapshot() {
    let shared = state();
    shared.lock().unwrap().current_screen = Some("music_select".to_owned());
    let mut output = test_output(Arc::clone(&shared), disconnected_test_channel());
    output.publish(&screen_event(0, "music_select")).unwrap();
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000041\"").unwrap();
    let fields = json!({
        "active_list_title": "A",
        "artist": "BEMANI Sound Team \"HuΣeR\" respect for D.J.Amuro",
        "selected_difficulty": {
            "state": { "status": "known", "value": "hyper" },
            "winner_score_ppm": 500_000,
            "margin_ppm": 250_000
        },
        "title_evidence": {
            "foreground": { "open_text": "A" },
            "normalized_scalar_count": 1,
            "geometry": {
                "occupancy_width_ppm": 42000,
                "touches_left_edge": false,
                "touches_right_edge": false
            }
        }
    });
    output
        .reduce_music_select_observation(
            Some(&"invocation-1-session-1".to_owned()),
            Some(1),
            42,
            4_200,
            &fields,
            &JointEvidenceObservation {
                catalog_song_count: 2,
                candidates: vec![JointEvidenceCandidate {
                    song_id,
                    chart: scorepeek_core::catalog::Chart {
                        key: scorepeek_core::catalog::ChartKey {
                            play_type: PlayType::Single,
                            difficulty: Difficulty::Hyper,
                        },
                        level: 10,
                        notes: 1_136,
                    },
                    display_titles: vec!["∀".to_owned()],
                    artist: "BEMANI Sound Team \"HuΣeR\" respect for D.J.Amuro".to_owned(),
                    family_support: BTreeMap::from([
                        (EvidenceFamily::SelectTitle, 180),
                        (EvidenceFamily::SelectArtist, 300),
                    ]),
                    support: 480,
                }],
            },
            &SongResolutionPresentation::Unknown {
                reason: json!("test"),
                selected: None,
                runner_up: None,
                evidence_summary: None,
            },
        )
        .unwrap();
    let snapshot = shared.lock().unwrap().resolver.clone();
    assert_eq!(snapshot.latest_field_sequence, Some(42));
    assert!(
        snapshot
            .raw_fields
            .iter()
            .any(|(key, value)| { key == "artist" && value.contains("HuΣeR") })
    );
    assert!(
        snapshot
            .raw_fields
            .iter()
            .any(|(key, value)| { key == "marker" && value.contains("known:hyper") })
    );
    assert_eq!(snapshot.local.unwrap().top_candidates.len(), 1);
    output
        .publish(&RunEvent {
            schema: RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::SemanticScreenEpisodeChanged {
                session_id: Some("invocation-1-session-1".to_owned()),
                capture_generation: Some(1),
                screen_episode_id: 43,
                sequence: 43,
                monotonic_end_ms: 4_300,
                screen: "play".to_owned(),
                phase: SemanticEpisodePhase::Started,
            },
        })
        .unwrap();
    let snapshot = shared.lock().unwrap().resolver.clone();
    assert!(snapshot.raw_fields.is_empty());
    assert_eq!(snapshot.latest_field_sequence, None);
    assert_eq!(snapshot.latest_field_ms, None);
}

fn music_selection_test_observation()
-> (Value, JointEvidenceObservation, SongResolutionPresentation) {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000046\"").unwrap();
    let fields = json!({
        "play_type": {
            "state": { "status": "known", "value": "double" },
            "single_score_ppm": 974_000,
            "double_score_ppm": 999_000
        },
        "selected_difficulty": {
            "state": { "status": "known", "value": "hyper" }
        },
        "play_side": {
            "state": { "status": "known", "value": "two_player" }
        }
    });
    let evidence = JointEvidenceObservation {
        catalog_song_count: 2,
        candidates: vec![JointEvidenceCandidate {
            song_id,
            chart: scorepeek_core::catalog::Chart {
                key: scorepeek_core::catalog::ChartKey {
                    play_type: PlayType::Double,
                    difficulty: Difficulty::Hyper,
                },
                level: 8,
                notes: 829,
            },
            display_titles: vec!["Wizards!".to_owned()],
            artist: "ARTIST".to_owned(),
            family_support: BTreeMap::from([(EvidenceFamily::SelectTitle, 300)]),
            support: 300,
        }],
    };
    let presentation = SongResolutionPresentation::Unknown {
        reason: json!("test"),
        selected: None,
        runner_up: None,
        evidence_summary: None,
    };
    (fields, evidence, presentation)
}

fn select_best_test_episode(session: &str, phase: SemanticEpisodePhase, sequence: u64) -> RunEvent {
    RunEvent {
        schema: RUN_EVENT_SCHEMA.to_owned(),
        kind: RunEventKind::SemanticScreenEpisodeChanged {
            session_id: Some(session.to_owned()),
            capture_generation: Some(1),
            screen_episode_id: 1,
            sequence,
            monotonic_end_ms: sequence * 100,
            screen: "music_select".to_owned(),
            phase,
        },
    }
}

fn select_best_test_observation() -> (Value, JointEvidenceObservation, SongResolutionPresentation) {
    use scorepeek_core::recognition::music_select::{BestNumericObservation, BestValue};
    let (mut fields, evidence, presentation) = music_selection_test_observation();
    fields["best"] = serde_json::to_value(
        scorepeek_core::recognition::music_select::resolve_music_select_best(
            "SCORE DATA".into(),
            "CLEAR".into(),
            BestNumericObservation {
                score: BestValue::Known(1200),
                miss_count: BestValue::Unknown,
                ..Default::default()
            },
        ),
    )
    .unwrap();
    (fields, evidence, presentation)
}

fn assert_connected_best(output: &RoutineOutput, observation_id: &str) {
    let connected: Value =
        serde_json::from_slice(&snapshot_bytes(&output.state, &ChannelHealth::default()).unwrap())
            .unwrap();
    assert_eq!(
        connected["music_select_best"]["snapshot"]["observation_id"],
        observation_id
    );
}

#[test]
fn select_best_is_frame_bound_suspended_and_separate_from_results() {
    use scorepeek_core::recognition::music_select::{BestClearType, BestValue};
    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    let session = "invocation-1-session-1".to_owned();
    let episode = |phase, sequence| select_best_test_episode(&session, phase, sequence);
    output
        .publish(&episode(SemanticEpisodePhase::Started, 1))
        .unwrap();
    let (mut fields, evidence, presentation) = select_best_test_observation();
    let observe = |output: &mut RoutineOutput, sequence, fields: &Value| {
        output
            .reduce_music_select_observation(
                Some(&session),
                Some(1),
                sequence,
                sequence * 100,
                fields,
                &evidence,
                &presentation,
            )
            .unwrap();
    };
    for sequence in 2..=5 {
        observe(&mut output, sequence, &fields);
    }
    let first = output
        .state
        .lock()
        .unwrap()
        .music_select
        .snapshot
        .clone()
        .unwrap();
    assert_eq!(first.values.score, BestValue::Known(1200));
    assert_eq!(
        first.values.clear_type,
        BestValue::Known(BestClearType::Clear)
    );
    assert_eq!(first.revision, 1);
    assert_connected_best(&output, &first.observation_id);
    observe(&mut output, 5, &fields);
    observe(&mut output, 3, &fields);
    assert_eq!(
        output.state.lock().unwrap().music_select.snapshot.as_ref(),
        Some(&first)
    );
    output
        .publish(&episode(SemanticEpisodePhase::Suspended, 6))
        .unwrap();
    fields["best"]["values"]["score"] = json!({"status":"known","value":1300});
    observe(&mut output, 7, &fields);
    let state = output.state.lock().unwrap().music_select.clone();
    assert!(state.active && state.suspended);
    assert_eq!(state.snapshot.as_ref(), Some(&first));
    output
        .publish(&episode(SemanticEpisodePhase::Resumed, 9))
        .unwrap();
    observe(&mut output, 8, &fields);
    assert_eq!(
        output.state.lock().unwrap().music_select.snapshot.as_ref(),
        Some(&first)
    );
    observe(&mut output, 10, &fields);
    assert_eq!(
        output.state.lock().unwrap().music_select.score.consecutive,
        1
    );
    observe(&mut output, 11, &fields);
    assert_eq!(
        output.state.lock().unwrap().music_select.score.accepted(),
        BestValue::Known(1300)
    );
    fields["selected_difficulty"]["state"]["value"] = json!("another");
    observe(&mut output, 12, &fields);
    assert!(output.state.lock().unwrap().music_select.snapshot.is_none());
    output
        .publish(&episode(SemanticEpisodePhase::Closing, 13))
        .unwrap();
    output
        .publish(&episode(SemanticEpisodePhase::Finalized, 13))
        .unwrap();
    observe(&mut output, 14, &fields);
    assert!(!output.state.lock().unwrap().music_select.active);
    assert!(output.state.lock().unwrap().result_history.is_empty());
    let events = output.take_headless_events();
    let snapshots: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.kind {
            RunEventKind::MusicSelectBestObserved { snapshot, .. } => Some(snapshot),
            _ => None,
        })
        .collect();
    assert_eq!(snapshots.len(), 2); // Initial and updated score after two fresh resume observations.
    assert!(!events.iter().any(|e| matches!(
        e.kind,
        RunEventKind::ResultChanged {
            state: ResultState::Confirmed { .. },
            ..
        }
    )));
    assert_public_fold(events);
}

#[test]
fn held_select_pane_keeps_wait_reason_and_previous_revision_visible() {
    use ratatui::backend::TestBackend;
    let mut state = RunViewState::new("invocation".into(), "a".repeat(64), false);
    state.music_select = populated_select_test_state();
    state
        .music_select
        .hold(SelectIdentityStatus::AwaitingDifficulty);
    for suspended in [false, true] {
        state.music_select.suspended = suspended;
        for (width, height) in [(120, 40), (80, 25)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    render(
                        frame,
                        &state,
                        Path::new("/run/scorepeek.sock"),
                        &ChannelHealth::default(),
                    );
                })
                .unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>();
            for expected in [
                "held",
                "last r1 S=1200",
                "SCORE waiting",
                "Latest result",
                "Music Select Resolver",
                "Watcher",
            ] {
                assert!(text.contains(expected), "{width}x{height}: {expected}");
            }
            assert!(text.contains(if suspended {
                "waiting: suspended"
            } else {
                "waiting: difficulty"
            }));
        }
    }
}

#[test]
fn select_notifications_skip_resolved_clock_updates_and_keep_connected_snapshot() {
    let mut output = RoutineOutput::start_headless("invocation-1".into(), "a".repeat(64));
    let session = "invocation-1-session-1".to_owned();
    output
        .publish(&select_best_test_episode(
            &session,
            SemanticEpisodePhase::Started,
            1,
        ))
        .unwrap();
    let (fields, evidence, presentation) = select_best_test_observation();
    for sequence in 2..=5 {
        output
            .reduce_music_select_observation(
                Some(&session),
                Some(1),
                sequence,
                sequence * 100,
                &fields,
                &evidence,
                &presentation,
            )
            .unwrap();
    }
    let published = output.state.lock().unwrap().music_select.clone();
    output.take_headless_events();
    for sequence in 6..=20 {
        output
            .reduce_music_select_observation(
                Some(&session),
                Some(1),
                sequence,
                sequence * 100,
                &fields,
                &evidence,
                &presentation,
            )
            .unwrap();
    }
    assert_eq!(
        output
            .core_reducer
            .test_music_select_resolver()
            .best
            .current_difficulty
            .unwrap()
            .last_sequence(),
        20
    );
    let events = output.take_headless_events();
    assert!(!events.iter().any(|event| matches!(
        event.kind,
        RunEventKind::MusicSelectResolverChanged { .. }
            | RunEventKind::MusicSelectBestObserved { .. }
    )));
    let connected: Value =
        serde_json::from_slice(&snapshot_bytes(&output.state, &ChannelHealth::default()).unwrap())
            .unwrap();
    assert_eq!(
        connected["music_select_best"]["snapshot"],
        serde_json::to_value(&published.snapshot).unwrap()
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the regression keeps each frame-identity case and its lifecycle assertions together"
)]
fn select_missing_frame_identity_holds_interval_without_adopting_values() {
    for missing in ["difficulty", "mode", "song"] {
        let mut output = RoutineOutput::start_headless("invocation-1".into(), "a".repeat(64));
        let session = "invocation-1-session-1".to_owned();
        output
            .publish(&select_best_test_episode(
                &session,
                SemanticEpisodePhase::Started,
                1,
            ))
            .unwrap();
        let (fields, evidence, presentation) = select_best_test_observation();
        for sequence in 2..=5 {
            output
                .reduce_music_select_observation(
                    Some(&session),
                    Some(1),
                    sequence,
                    sequence * 100,
                    &fields,
                    &evidence,
                    &presentation,
                )
                .unwrap();
        }
        let first = output
            .core_reducer
            .test_music_select_resolver()
            .best
            .snapshot
            .clone()
            .unwrap();
        output.take_headless_events();
        let mut missing_fields = fields.clone();
        let mut missing_evidence = evidence.clone();
        missing_fields["best"]["values"]["score"] = json!({"status":"known","value":1400});
        match missing {
            "difficulty" => {
                missing_fields["selected_difficulty"]["state"] = json!({"status":"unknown"});
            }
            "mode" => missing_fields["play_type"]["state"] = json!({"status":"unknown"}),
            _ => missing_evidence.candidates.clear(),
        }
        for sequence in 6..=8 {
            output
                .reduce_music_select_observation(
                    Some(&session),
                    Some(1),
                    sequence,
                    sequence * 100,
                    &missing_fields,
                    &missing_evidence,
                    &presentation,
                )
                .unwrap();
            assert_eq!(
                output
                    .core_reducer
                    .test_music_select_resolver()
                    .best
                    .snapshot
                    .as_ref(),
                Some(&first),
                "{missing}"
            );
            assert_eq!(
                output
                    .core_reducer
                    .test_music_select_resolver()
                    .best
                    .score
                    .consecutive,
                0,
                "{missing}"
            );
        }
        for sequence in 9..=10 {
            output
                .reduce_music_select_observation(
                    Some(&session),
                    Some(1),
                    sequence,
                    sequence * 100,
                    &fields,
                    &evidence,
                    &presentation,
                )
                .unwrap();
            assert_eq!(
                output
                    .core_reducer
                    .test_music_select_resolver()
                    .best
                    .score
                    .consecutive,
                u8::try_from(sequence - 8).unwrap()
            );
        }
        assert_eq!(
            output
                .core_reducer
                .test_music_select_resolver()
                .best
                .snapshot
                .as_ref(),
            Some(&first)
        );
        assert!(
            !output
                .take_headless_events()
                .iter()
                .any(|event| matches!(event.kind, RunEventKind::MusicSelectBestObserved { .. }))
        );
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the regression keeps each conflict case and its lifecycle assertions together"
)]
fn select_conflicting_frames_end_interval_even_without_successor_resolution() {
    for conflict in ["difficulty", "mode", "song", "ambiguous"] {
        let mut output = RoutineOutput::start_headless("invocation-1".into(), "a".repeat(64));
        let session = "invocation-1-session-1".to_owned();
        output
            .publish(&select_best_test_episode(
                &session,
                SemanticEpisodePhase::Started,
                1,
            ))
            .unwrap();
        let (fields, evidence, presentation) = select_best_test_observation();
        for sequence in 2..=5 {
            output
                .reduce_music_select_observation(
                    Some(&session),
                    Some(1),
                    sequence,
                    sequence * 100,
                    &fields,
                    &evidence,
                    &presentation,
                )
                .unwrap();
        }
        let first = output
            .core_reducer
            .test_music_select_resolver()
            .best
            .snapshot
            .clone()
            .unwrap();
        let mut changed_fields = fields.clone();
        let mut changed_evidence = evidence.clone();
        match conflict {
            "difficulty" => {
                changed_fields["selected_difficulty"]["state"]["value"] = json!("another");
            }
            "mode" => changed_fields["play_type"]["state"]["value"] = json!("single"),
            _ => {
                let mut other = evidence.candidates[0].clone();
                other.song_id =
                    serde_json::from_str("\"00000000-0000-0000-0000-000000000047\"").unwrap();
                if conflict == "song" {
                    other.family_support = BTreeMap::from([(EvidenceFamily::SelectTitle, 100)]);
                    other.support = 100;
                    changed_evidence.candidates.clear();
                }
                changed_evidence.candidates.push(other);
                changed_evidence.catalog_song_count = 3;
            }
        }
        output
            .reduce_music_select_observation(
                Some(&session),
                Some(1),
                6,
                600,
                &changed_fields,
                &changed_evidence,
                &presentation,
            )
            .unwrap();
        assert!(
            output
                .core_reducer
                .test_music_select_resolver()
                .best
                .chart
                .is_none(),
            "{conflict}"
        );
        assert!(
            output
                .core_reducer
                .test_music_select_resolver()
                .best
                .snapshot
                .is_none(),
            "{conflict}"
        );
        for sequence in 7..=15 {
            output
                .reduce_music_select_observation(
                    Some(&session),
                    Some(1),
                    sequence,
                    sequence * 100,
                    &fields,
                    &evidence,
                    &presentation,
                )
                .unwrap();
        }
        if conflict == "mode" {
            // Conflicting mode support remains unresolved in the existing identity resolver.
            assert!(
                output
                    .core_reducer
                    .test_music_select_resolver()
                    .selected()
                    .is_none(),
                "{conflict}"
            );
            assert!(
                output
                    .core_reducer
                    .test_music_select_resolver()
                    .best
                    .snapshot
                    .is_none(),
                "{conflict}"
            );
            continue;
        }
        let revisit = output
            .core_reducer
            .test_music_select_resolver()
            .best
            .snapshot
            .as_ref()
            .unwrap();
        assert_ne!(
            first.selection_interval, revisit.selection_interval,
            "{conflict}"
        );
        assert_eq!(first.values, revisit.values, "{conflict}");
    }
}

#[test]
fn best_suppression_does_not_discard_admitted_selection_identity() {
    for phase in [
        SemanticEpisodePhase::Suspended,
        SemanticEpisodePhase::Closing,
    ] {
        let mut output = RoutineOutput::start_headless("invocation-1".into(), "a".repeat(64));
        let session = "invocation-1-session-1".to_owned();
        output
            .publish(&select_best_test_episode(
                &session,
                SemanticEpisodePhase::Started,
                1,
            ))
            .unwrap();
        output
            .publish(&select_best_test_episode(&session, phase, 4))
            .unwrap();
        let (fields, evidence, presentation) = select_best_test_observation();
        // These frames were admitted before the boundary and finish during drain.
        for sequence in [2, 3] {
            output
                .reduce_music_select_observation(
                    Some(&session),
                    Some(1),
                    sequence,
                    sequence * 100,
                    &fields,
                    &evidence,
                    &presentation,
                )
                .unwrap();
        }
        assert!(
            output
                .core_reducer
                .test_music_select_resolver()
                .selected()
                .is_some()
        );
        assert!(output.state.lock().unwrap().music_select.snapshot.is_none());
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the regression verifies the complete deduplicated selection lifecycle"
)]
fn music_selection_lifecycle_is_deduplicated_and_does_not_accept_joint() {
    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    output.publish(&screen_event(0, "music_select")).unwrap();
    let session_id = "invocation-1-session-1".to_owned();
    let (fields, evidence, presentation) = music_selection_test_observation();
    for sequence in [1, 2, 3] {
        output
            .reduce_music_select_observation(
                Some(&session_id),
                Some(1),
                sequence,
                sequence * 100,
                &fields,
                &evidence,
                &presentation,
            )
            .unwrap();
    }
    assert_double_play_selection(&output);
    output.core_reducer.test_engine_mut().selection_epochs = SelectionEpochTracker::default();
    assert!(
        output
            .core_reducer
            .test_music_select_resolver()
            .selected()
            .is_some()
    );
    let mut changed_song = evidence.clone();
    changed_song.candidates[0].song_id =
        serde_json::from_str("\"00000000-0000-0000-0000-000000000047\"").unwrap();
    let mut conflicting_fields = fields.clone();
    conflicting_fields["play_type"]["state"]["value"] = json!("single");
    output
        .reduce_music_select_observation(
            Some(&session_id),
            Some(1),
            4,
            400,
            &conflicting_fields,
            &changed_song,
            &presentation,
        )
        .unwrap();
    assert!(
        output
            .core_reducer
            .test_music_select_resolver()
            .selected()
            .is_none()
    );
    output
        .publish_screen_change(
            &RunEvent {
                schema: RUN_EVENT_SCHEMA.to_owned(),
                kind: RunEventKind::ScreenChanged {
                    session_id: Some(session_id),
                    capture_generation: Some(1),
                    screen_episode_id: 10,
                    sequence: 5,
                    monotonic_start_ms: 500,
                    monotonic_end_ms: 500,
                    screen: "play".to_owned(),
                },
            },
            true,
        )
        .unwrap();
    let events = output.take_headless_events();
    let lifecycle = events
        .iter()
        .filter_map(|event| match &event.kind {
            RunEventKind::MusicSelectionChanged {
                revision, state, ..
            } => Some((*revision, state)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(lifecycle.len(), 3);
    assert!(matches!(
        lifecycle[0],
        (1, MusicSelectionState::Selected { .. })
    ));
    assert_eq!(
        lifecycle[1],
        (
            2,
            &MusicSelectionState::Unresolved {
                reason: MusicSelectionUnresolvedReason::EvidenceUnresolved,
            }
        )
    );
    assert_eq!(
        lifecycle[2],
        (
            3,
            &MusicSelectionState::Unresolved {
                reason: MusicSelectionUnresolvedReason::EpisodeEnded,
            }
        )
    );
    assert!(!events.iter().any(|event| matches!(
        event.kind,
        RunEventKind::ResolverStateChanged {
            state: ResolverResolutionState::AcceptedJoint,
            ..
        }
    )));
}

fn assert_double_play_selection(output: &RoutineOutput) {
    assert!(matches!(
        output.core_reducer.test_music_select_resolver().selected(),
        Some(MusicSelectionState::Selected {
            play_side: PlaySide::TwoPlayer,
            ..
        })
    ));
}

#[test]
fn music_selection_requires_two_equal_play_sides_and_rejects_a_conflict() {
    let mut resolver = MusicSelectResolver::default();
    let (mut fields, mut evidence, _) = music_selection_test_observation();
    fields["play_type"]["state"]["value"] = json!("single");
    evidence.candidates[0].chart.key.play_type = PlayType::Single;
    resolver.observe(
        1,
        100,
        &evidence,
        selected_difficulty(&fields),
        selected_play_type(&fields),
        selected_play_side(&fields),
    );
    assert!(resolver.selected().is_none());
    resolver.observe(
        2,
        200,
        &evidence,
        selected_difficulty(&fields),
        selected_play_type(&fields),
        selected_play_side(&fields),
    );
    assert!(matches!(
        resolver.selected(),
        Some(MusicSelectionState::Selected {
            play_side: PlaySide::TwoPlayer,
            ..
        })
    ));

    fields["play_side"]["state"]["value"] = json!("one_player");
    resolver.observe(
        3,
        300,
        &evidence,
        selected_difficulty(&fields),
        selected_play_type(&fields),
        selected_play_side(&fields),
    );
    assert!(resolver.selected().is_none());
}

#[test]
fn double_play_selection_requires_a_footer_play_side() {
    let mut resolver = MusicSelectResolver::default();
    let (fields, evidence, _) = music_selection_test_observation();
    for (sequence, monotonic_ms) in [(1, 100), (2, 200)] {
        resolver.observe(
            sequence,
            monotonic_ms,
            &evidence,
            selected_difficulty(&fields),
            selected_play_type(&fields),
            if sequence == 1 {
                None
            } else {
                selected_play_side(&fields)
            },
        );
    }
    assert!(resolver.selected().is_none());
    resolver.observe(
        3,
        300,
        &evidence,
        selected_difficulty(&fields),
        selected_play_type(&fields),
        selected_play_side(&fields),
    );
    assert!(matches!(
        resolver.selected(),
        Some(MusicSelectionState::Selected {
            play_side: PlaySide::TwoPlayer,
            play_type: PlayType::Double,
            ..
        })
    ));
}

#[test]
fn result_panel_side_requires_two_fresh_matches_and_two_opposites_conflict() {
    let mut side = ResultPanelSideAccumulator::default();
    assert_eq!(
        side.observe(7, 10, ResultPanelSide::Right),
        Some((
            ResultPanelSideEpisodeState::Pending,
            ResultPanelSideTransitionReason::CandidateStarted,
        ))
    );
    assert_eq!(side.stable(), None);
    assert_eq!(side.observe(7, 10, ResultPanelSide::Right), None);
    assert_eq!(
        side.observe(7, 11, ResultPanelSide::Right),
        Some((
            ResultPanelSideEpisodeState::Stable {
                side: ResultPanelSide::Right,
            },
            ResultPanelSideTransitionReason::Accepted,
        ))
    );
    assert_eq!(side.stable(), Some(ResultPanelSide::Right));
    assert_eq!(
        side.observe(7, 12, ResultPanelSide::Left),
        Some((
            ResultPanelSideEpisodeState::Stable {
                side: ResultPanelSide::Right,
            },
            ResultPanelSideTransitionReason::OppositeObserved,
        ))
    );
    assert_eq!(side.stable(), Some(ResultPanelSide::Right));
    assert_eq!(side.observe(7, 13, ResultPanelSide::Right), None);
    assert_eq!(
        side.observe(7, 14, ResultPanelSide::Left),
        Some((
            ResultPanelSideEpisodeState::Conflicted {
                stable_side: ResultPanelSide::Right,
            },
            ResultPanelSideTransitionReason::Conflict,
        ))
    );
    assert_eq!(side.stable(), None);
    side.start_episode(8);
    assert_eq!(side.stable(), None);
}

#[test]
fn pre_stable_candidates_do_not_count_as_opposite_evidence() {
    let mut side = ResultPanelSideAccumulator::default();
    assert!(side.observe(7, 10, ResultPanelSide::Left).is_some());
    assert!(side.observe(7, 11, ResultPanelSide::Right).is_some());
    assert_eq!(
        side.observe(7, 12, ResultPanelSide::Right),
        Some((
            ResultPanelSideEpisodeState::Stable {
                side: ResultPanelSide::Right,
            },
            ResultPanelSideTransitionReason::Accepted,
        ))
    );
    assert_eq!(
        side.observe(7, 13, ResultPanelSide::Left),
        Some((
            ResultPanelSideEpisodeState::Stable {
                side: ResultPanelSide::Right,
            },
            ResultPanelSideTransitionReason::OppositeObserved,
        ))
    );
    assert_eq!(side.stable(), Some(ResultPanelSide::Right));
    assert!(matches!(
        side.observe(7, 14, ResultPanelSide::Left),
        Some((
            ResultPanelSideEpisodeState::Conflicted { .. },
            ResultPanelSideTransitionReason::Conflict
        ))
    ));
}

#[test]
fn panel_side_conflict_retracts_the_provisional_result() {
    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    prepare_accepted_attempt(&mut output);
    output.publish(&accepted_result_event(1)).unwrap();
    output.publish(&accepted_result_event(2)).unwrap();
    assert!(
        output
            .core_reducer
            .test_active_provisional_result()
            .is_some()
    );

    prime_result_panel(&mut output, ResultPanelSide::Right, 3);
    assert!(
        output
            .core_reducer
            .test_active_provisional_result()
            .is_none()
    );

    assert!(
        output
            .core_reducer
            .test_active_provisional_result()
            .is_none()
    );
    assert!(output.headless_events.iter().any(|event| matches!(
        event.kind,
        RunEventKind::ResultChanged {
            state: ResultState::Retracted {
                reason: ResultRetractionReason::PanelSideConflict,
                ..
            },
            ..
        }
    )));
}

#[test]
fn side_mismatch_before_result_play_type_stabilizes_detaches_only_the_attempt_linkage() {
    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    output
        .core_reducer
        .test_engine_mut()
        .play_attempt
        .observe_selection_screen();
    output
        .core_reducer
        .test_engine_mut()
        .play_attempt
        .observe_screen(PlayAttemptScreen::Play, 0);
    output
        .core_reducer
        .test_engine_mut()
        .play_attempt
        .observe_screen(PlayAttemptScreen::Result, 0);
    output
        .core_reducer
        .test_engine_mut()
        .retained_select
        .select_play_sides
        .insert(PlaySide::OnePlayer, 2);
    output
        .core_reducer
        .test_engine_mut()
        .retained_select
        .select_play_types
        .insert(PlayType::Double, 2);
    prime_result_panel(&mut output, ResultPanelSide::Right, 0);

    for sequence in [1, 2] {
        let mut event = accepted_double_result_event(sequence);
        let RunEventKind::FieldObservation { fields, .. } = &mut event.kind else {
            unreachable!();
        };
        fields["panel_side"] = json!(ResultPanelSide::Right);
        output.publish(&event).unwrap();
    }

    assert!(output.core_reducer.test_result_select_context_detached());
    assert_eq!(
        output
            .core_reducer
            .test_engine()
            .retained_select
            .observation_count,
        0
    );
    assert!(
        output
            .core_reducer
            .test_active_provisional_result()
            .is_some()
    );
    assert!(output.headless_events.iter().any(|event| matches!(
        event.kind,
        RunEventKind::ResultSelectContextMismatch {
            source_sequence: 1,
            select_play_side: PlaySide::OnePlayer,
            result_play_side: PlaySide::TwoPlayer,
            ..
        }
    )));
    output
        .publish(&semantic_episode_event(
            3,
            "result",
            SemanticEpisodePhase::Finalized,
        ))
        .unwrap();
    assert!(output.headless_events.iter().any(|event| matches!(
        &event.kind,
        RunEventKind::ResultChanged {
            state: ResultState::Confirmed { result, .. },
            ..
        } if result.play_side == PlaySide::TwoPlayer
    )));
}

#[test]
fn result_play_side_serializes_plain_side_for_sp_and_dp() {
    assert_eq!(
        serde_json::to_value(result_play_side(Some(ResultPanelSide::Left)).unwrap()).unwrap(),
        json!("one_player")
    );
    assert_eq!(
        serde_json::to_value(result_play_side(Some(ResultPanelSide::Right)).unwrap()).unwrap(),
        json!("two_player")
    );
}

#[test]
fn session_finish_ends_selection_once_even_when_field_drain_did_not_finalize() {
    for finalize in [false, true] {
        let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
        let session_id = "invocation-1-session-1".to_owned();
        let episode = |phase, sequence| RunEvent {
            schema: RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::SemanticScreenEpisodeChanged {
                session_id: Some(session_id.clone()),
                capture_generation: Some(1),
                screen_episode_id: 9,
                sequence,
                monotonic_end_ms: sequence * 100,
                screen: "music_select".to_owned(),
                phase,
            },
        };
        output
            .publish(&episode(SemanticEpisodePhase::Started, 1))
            .unwrap();
        let (fields, evidence, presentation) = music_selection_test_observation();
        for sequence in [2, 3] {
            output
                .reduce_music_select_observation(
                    Some(&session_id),
                    Some(1),
                    sequence,
                    sequence * 100,
                    &fields,
                    &evidence,
                    &presentation,
                )
                .unwrap();
        }
        assert!(matches!(
            output.core_reducer.test_active_music_selection(),
            Some(MusicSelectionState::Selected { .. })
        ));
        output
            .publish(&episode(SemanticEpisodePhase::Closing, 4))
            .unwrap();
        if finalize {
            output
                .publish(&episode(SemanticEpisodePhase::Finalized, 4))
                .unwrap();
        }
        output
            .publish(&RunEvent {
                schema: RUN_EVENT_SCHEMA.to_owned(),
                kind: RunEventKind::SessionFinished {
                    session_id: session_id.clone(),
                    capture_generation: 1,
                    outcome: if finalize { "complete" } else { "failed" }.to_owned(),
                    report: json!({}),
                },
            })
            .unwrap();
        assert!(!output.core_reducer.test_music_selection_episode_active());
        let ended = MusicSelectionState::Unresolved {
            reason: MusicSelectionUnresolvedReason::EpisodeEnded,
        };
        assert_eq!(
            output.core_reducer.test_active_music_selection(),
            Some(&ended)
        );
        assert_eq!(
            output.state.lock().unwrap().latest_music_selection,
            Some(ended.clone())
        );
        let events = output.take_headless_events();
        let endings = events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| match &event.kind {
                RunEventKind::MusicSelectionChanged {
                    screen_episode_id: 9,
                    source_sequence: 4,
                    revision: 2,
                    state,
                    ..
                } if *state == ended => Some(index),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(endings.len(), 1);
        let finished = events
            .iter()
            .position(|event| matches!(event.kind, RunEventKind::SessionFinished { .. }))
            .unwrap();
        assert!(endings[0] < finished);
    }
}

#[test]
fn pending_marker_is_visible_before_any_song_evidence() {
    let shared = state();
    shared.lock().unwrap().current_screen = Some("music_select".to_owned());
    let mut output = test_output(Arc::clone(&shared), disconnected_test_channel());
    output.publish(&screen_event(0, "music_select")).unwrap();
    let fields = json!({
        "selected_difficulty": {
            "state": { "status": "known", "value": "normal" },
            "winner_score_ppm": 500_000,
            "margin_ppm": 250_000
        }
    });
    output
        .reduce_music_select_observation(
            Some(&"invocation-1-session-1".to_owned()),
            Some(1),
            10,
            1_000,
            &fields,
            &JointEvidenceObservation {
                catalog_song_count: 2,
                candidates: Vec::new(),
            },
            &SongResolutionPresentation::Unknown {
                reason: json!("test"),
                selected: None,
                runner_up: None,
                evidence_summary: None,
            },
        )
        .unwrap();
    let snapshot = shared.lock().unwrap().resolver.clone();
    assert_eq!(
        snapshot.selection_difficulty_target,
        Some(SelectionDifficultyTarget::Pending)
    );
    assert_eq!(
        snapshot.selection_difficulty.unwrap().difficulty,
        Difficulty::Normal
    );
    let rendered = music_select_best::lines(&shared.lock().unwrap().music_select, 80)
        .iter()
        .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
        .collect::<String>();
    assert!(rendered.contains("NORMAL streak=1 target=pending"));
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the regression verifies ordering across the complete admitted-field drain"
)]
fn music_select_handoff_waits_for_admitted_field_drain() {
    let shared = state();
    let mut output = test_output(Arc::clone(&shared), disconnected_test_channel());
    let semantic = |sequence, phase| RunEvent {
        schema: RUN_EVENT_SCHEMA.to_owned(),
        kind: RunEventKind::SemanticScreenEpisodeChanged {
            session_id: Some("invocation-1-session-1".to_owned()),
            capture_generation: Some(1),
            screen_episode_id: 7,
            sequence,
            monotonic_end_ms: sequence * 100,
            screen: "music_select".to_owned(),
            phase,
        },
    };
    output
        .publish(&semantic(70, SemanticEpisodePhase::Started))
        .unwrap();
    output
        .publish(&semantic(71, SemanticEpisodePhase::Closing))
        .unwrap();
    assert_eq!(
        output
            .core_reducer
            .test_engine()
            .retained_select
            .observation_count,
        0
    );

    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000061\"").unwrap();
    output
        .publish(&RunEvent {
            schema: RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::FieldObservation {
                session_id: Some("invocation-1-session-1".to_owned()),
                capture_generation: Some(1),
                screen_episode_id: 7,
                sequence: 70,
                monotonic_start_ms: 6_900,
                monotonic_end_ms: 7_050,
                screen: "music_select".to_owned(),
                fields: json!({
                    "active_list_title": "A",
                    "artist": "ARTIST",
                    "selected_difficulty": { "state": { "status": "known", "value": "hyper" } }
                }),
                result_song_resolution: Value::Null,
                music_select_song_resolution: Value::Null,
                parsed_result_fields: None,
                result_chart_resolution: None,
                result_performance_resolution: None,
                current_score_ocr_resolution: None,
                numeric_batch: None,
                joint_evidence: JointEvidenceObservation {
                    catalog_song_count: 2,
                    candidates: vec![JointEvidenceCandidate {
                        song_id,
                        chart: scorepeek_core::catalog::Chart {
                            key: scorepeek_core::catalog::ChartKey {
                                play_type: PlayType::Single,
                                difficulty: Difficulty::Hyper,
                            },
                            level: 10,
                            notes: 1_136,
                        },
                        display_titles: vec!["∀".to_owned()],
                        artist: "ARTIST".to_owned(),
                        family_support: BTreeMap::from([(EvidenceFamily::SelectArtist, 300)]),
                        support: 300,
                    }],
                },
                processing_timing: Value::Null,
                song_resolution_presentation: Box::new(SongResolutionPresentation::Unknown {
                    reason: json!("test"),
                    selected: None,
                    runner_up: None,
                    evidence_summary: None,
                }),
            },
        })
        .unwrap();
    assert_eq!(
        output
            .core_reducer
            .test_engine()
            .selection_epochs
            .incumbent
            .observation_count,
        1
    );
    assert_eq!(
        output
            .core_reducer
            .test_engine()
            .retained_select
            .observation_count,
        0
    );
    output
        .publish(&semantic(72, SemanticEpisodePhase::Finalized))
        .unwrap();
    assert_eq!(
        output
            .core_reducer
            .test_engine()
            .retained_select
            .observation_count,
        1
    );
    assert_eq!(
        output
            .core_reducer
            .test_engine()
            .retained_select
            .summary()
            .selected
            .unwrap()
            .song_id,
        song_id
    );
}

#[test]
fn selection_epoch_hands_off_only_the_latest_unfinished_successor() {
    let song = |suffix: u8| {
        serde_json::from_str(&format!("\"00000000-0000-0000-0000-{suffix:012}\"")).unwrap()
    };
    let observation = |song_id, support| JointEvidenceObservation {
        catalog_song_count: 100,
        candidates: vec![JointEvidenceCandidate {
            song_id,
            chart: scorepeek_core::catalog::Chart {
                key: scorepeek_core::catalog::ChartKey {
                    play_type: PlayType::Single,
                    difficulty: Difficulty::Hyper,
                },
                level: 8,
                notes: 764,
            },
            display_titles: vec![format!("SONG {song_id:?}")],
            artist: "ARTIST".to_owned(),
            family_support: BTreeMap::from([(EvidenceFamily::SelectTitleLexical, support)]),
            support,
        }],
    };
    let incumbent = song(1);
    let successor = song(2);
    let mut epochs = SelectionEpochTracker::default();
    epochs.observe(100, &observation(incumbent, 300), None);
    epochs.observe(200, &observation(successor, 70), None);
    assert_eq!(
        epochs.handoff().summary().selected.unwrap().song_id,
        successor
    );

    epochs.observe(300, &observation(incumbent, 300), None);
    assert!(epochs.successor.candidates.is_empty());
    assert_eq!(
        epochs.handoff().summary().selected.unwrap().song_id,
        incumbent
    );
}

#[test]
fn markerless_successor_is_active_without_inheriting_incumbent_difficulty() {
    let song = |suffix: u8| {
        serde_json::from_str(&format!("\"00000000-0000-0000-0000-{suffix:012}\"")).unwrap()
    };
    let observation = |song_id, support| JointEvidenceObservation {
        catalog_song_count: 100,
        candidates: vec![JointEvidenceCandidate {
            song_id,
            chart: scorepeek_core::catalog::Chart {
                key: scorepeek_core::catalog::ChartKey {
                    play_type: PlayType::Single,
                    difficulty: Difficulty::Hyper,
                },
                level: 8,
                notes: 764,
            },
            display_titles: vec!["TEST".to_owned()],
            artist: "ARTIST".to_owned(),
            family_support: BTreeMap::from([(EvidenceFamily::SelectTitle, support)]),
            support,
        }],
    };
    let incumbent = song(1);
    let successor = song(2);
    let mut epochs = SelectionEpochTracker::default();
    epochs.observe_at(
        100,
        1_000,
        &observation(incumbent, 300),
        Some(Difficulty::Hyper),
    );
    let successor_started = epochs.observe_at(200, 2_000, &observation(successor, 70), None);
    assert_eq!(
        epochs.active_difficulty_state(),
        Some((SelectionDifficultyTarget::Successor, None))
    );
    assert!(successor_started.iter().any(|transition| {
        transition.target == SelectionDifficultyTarget::Successor
            && transition.reason == SelectionDifficultyTransitionReason::TargetSwitch
            && transition.current.is_none()
    }));

    let incumbent_resumed = epochs.observe_at(300, 3_000, &observation(incumbent, 300), None);
    assert_eq!(
        epochs
            .active_difficulty_state()
            .unwrap()
            .1
            .unwrap()
            .difficulty,
        Difficulty::Hyper
    );
    assert!(incumbent_resumed.iter().any(|transition| {
        transition.target == SelectionDifficultyTarget::Incumbent
            && transition.reason == SelectionDifficultyTransitionReason::TargetSwitch
            && transition.current.is_some()
    }));
}

#[test]
fn accepted_hypothesis_can_return_to_conflict_on_new_contradictory_evidence() {
    let first_song = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
    let second_song = serde_json::from_str("\"00000000-0000-0000-0000-000000000002\"").unwrap();
    let candidate = |song_id, family| JointEvidenceCandidate {
        song_id,
        chart: scorepeek_core::catalog::Chart {
            key: scorepeek_core::catalog::ChartKey {
                play_type: PlayType::Single,
                difficulty: Difficulty::Hyper,
            },
            level: 8,
            notes: 764,
        },
        display_titles: vec!["TEST".to_owned()],
        artist: "ARTIST".to_owned(),
        family_support: BTreeMap::from([(family, 300)]),
        support: 300,
    };
    let mut accumulator = HypothesisAccumulator::default();
    accumulator.observe(
        100,
        &JointEvidenceObservation {
            catalog_song_count: 0,
            candidates: vec![candidate(first_song, EvidenceFamily::ResultTitle)],
        },
        None,
        None,
    );
    for monotonic_ms in [101, 102] {
        accumulator.observe(
            monotonic_ms,
            &JointEvidenceObservation {
                catalog_song_count: 0,
                candidates: Vec::new(),
            },
            None,
            Some(ResultChartFactor {
                play_type: Some(PlayType::Single),
                difficulty: None,
                notes: None,
                level: None,
            }),
        );
    }
    assert_eq!(
        accumulator.summary().state,
        ResolverResolutionState::AcceptedJoint
    );
    accumulator.observe(
        200,
        &JointEvidenceObservation {
            catalog_song_count: 0,
            candidates: vec![candidate(second_song, EvidenceFamily::ResultArtist)],
        },
        None,
        None,
    );
    assert_eq!(
        accumulator.summary().state,
        ResolverResolutionState::Conflict
    );
}

#[test]
fn resolver_transition_records_raw_and_normalized_family_contributions() {
    let mut output = RoutineOutput::start_headless("invocation-1".into(), "a".repeat(64));
    prime_left_result_panel(&mut output);
    output.take_headless_events();

    output.publish(&accepted_result_event(1)).unwrap();
    let events = output.take_headless_events();
    assert!(matches!(
        events[0].kind,
        RunEventKind::FieldObservation { .. }
    ));
    let transition = events[1].to_value().unwrap();
    assert_eq!(transition["event"], "resolver_state_changed");
    assert_eq!(transition["scope"], "result");
    assert_eq!(transition["state"], "song_projected");
    assert_eq!(
        transition["selected_family_support"]["result_title"]["raw"],
        120
    );
    assert_eq!(
        transition["selected_family_support"]["result_title"]["normalized"],
        120
    );
    assert_eq!(transition["observation_count"], 1);

    output.publish(&accepted_result_event(2)).unwrap();
    let transition = output
        .take_headless_events()
        .into_iter()
        .map(|event| event.to_value().unwrap())
        .find(|event| {
            event["event"] == "resolver_state_changed"
                && event["scope"] == "result"
                && event["selected_family_support"]["result_play_type"]["normalized"] == 100
        })
        .expect("result play-type authority transition");
    assert_eq!(transition["event"], "resolver_state_changed");
    assert_eq!(
        transition["selected_family_support"]["result_play_type"]["normalized"],
        100
    );
    assert_eq!(transition["observation_count"], 2);
}
