use std::io::{BufRead as _, BufReader, Write};
use std::os::unix::fs::symlink;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use scorepeek_core::recognition::result::ResultFieldValue;

use super::*;

#[test]
fn diagnostic_timing_summary_counts_only_measured_values() {
    let mut timings = TimingAccumulator::default();
    timings.observe(&Value::Null);
    timings.observe(&json!(12));
    timings.observe(&json!(7));
    timings.observe(&json!("invalid"));
    assert_eq!(timings.count, 2);
    assert_eq!(timings.total_us, 19);
    assert_eq!(timings.max_us, 12);
}

fn frontend_snapshot(output: &RoutineOutput) -> Value {
    serde_json::from_slice(&snapshot_bytes(&output.state, &ChannelHealth::default()).unwrap())
        .unwrap()
}

fn frontend_select_best(output: &RoutineOutput) -> Value {
    frontend_snapshot(output)["music_select_best"]["snapshot"].clone()
}

fn current_provisional_result(output: &RoutineOutput) -> Option<&ResultDomainEvent> {
    output
        .headless_events
        .iter()
        .rev()
        .find_map(|event| match &event.kind {
            RunEventKind::ResultChanged { state, .. } => Some(state),
            _ => None,
        })
        .and_then(|state| match state {
            ResultState::Provisional { result, .. } => Some(result.as_ref()),
            _ => None,
        })
}

#[test]
fn live_output_uses_one_ordered_core_coordinator_across_two_sessions() {
    let mut output = RoutineOutput::start_headless("invocation-1".into(), "a".repeat(64));
    let event = |kind| RunEvent {
        schema: RUN_EVENT_SCHEMA.into(),
        kind,
    };
    output
        .publish(&event(RunEventKind::WatcherStarted {
            invocation_id: "invocation-1".into(),
        }))
        .unwrap();
    for generation in [1, 2] {
        let session_id = format!("session-{generation}");
        output
            .publish(&event(RunEventKind::SessionStarted {
                session_id: Some(session_id.clone()),
            }))
            .unwrap();
        output
            .publish(&event(RunEventKind::SessionFinished {
                session_id,
                outcome: "complete".into(),
                report: Value::Null,
            }))
            .unwrap();
    }
    output
        .publish(&event(RunEventKind::WatcherStopped {
            invocation_id: "invocation-1".into(),
            reason: "complete".into(),
        }))
        .unwrap();
    assert!(output.core_reducer.state().is_finished());
    assert_eq!(output.core_reducer.state().last_input_sequence(), Some(5));
}

#[test]
fn operational_events_share_channel_order_without_consuming_core_input_numbers() {
    let temporary = tempfile::tempdir().unwrap();
    let diagnostics = RunDiagnostics::start(temporary.path(), "ordered-inputs");
    let mut output = RoutineOutput::start_headless_with_diagnostics(
        "invocation-ordered".into(),
        "a".repeat(64),
        diagnostics,
    );
    let event = |kind| RunEvent {
        schema: RUN_EVENT_SCHEMA.into(),
        kind,
    };
    output
        .publish(&event(RunEventKind::WatcherStarted {
            invocation_id: "invocation-ordered".into(),
        }))
        .unwrap();
    output
        .publish(&event(RunEventKind::OverlayObserved {
            observation: json!({"source":"test"}),
        }))
        .unwrap();
    output
        .publish(&event(RunEventKind::SessionStarted {
            session_id: Some("session-1".into()),
        }))
        .unwrap();
    output
        .publish(&event(RunEventKind::SessionFinished {
            session_id: "session-1".into(),
            outcome: "complete".into(),
            report: json!({"field_rejected": 2}),
        }))
        .unwrap();
    output
        .publish(&event(RunEventKind::WatcherStopped {
            invocation_id: "invocation-ordered".into(),
            reason: "complete".into(),
        }))
        .unwrap();
    output.finish_diagnostics("success");
    assert_eq!(output.core_reducer.state().last_input_sequence(), Some(3));

    let stream =
        fs::read_to_string(temporary.path().join("ordered-inputs/diagnostics.ndjson")).unwrap();
    let events: Vec<Value> = stream
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| {
            matches!(
                record["operation"].as_str(),
                Some("runtime_event" | "domain_transition")
            )
        })
        .map(|record| record["data"].clone())
        .collect();
    let find = |kind| events.iter().find(|value| value["event"] == kind).unwrap();
    assert!(find("watcher_started").get("input_sequence").is_none());
    assert!(find("overlay_observed").get("input_sequence").is_none());
    assert_eq!(find("session_started")["input_sequence"], 1);
    assert_eq!(find("session_finished")["input_sequence"], 2);
    assert_eq!(find("watcher_stopped")["input_sequence"], 3);
    assert_eq!(find("session_finished")["report"]["field_rejected"], 2);
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0]["channel_sequence"].as_u64().unwrap()
                < pair[1]["channel_sequence"].as_u64().unwrap())
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the test covers one complete ordered diagnostic stream"
)]
fn diagnostic_trace_counts_no_op_ticks_without_repeating_them() {
    let temporary = tempfile::tempdir().unwrap();
    let diagnostics = RunDiagnostics::start(temporary.path(), "run-trace-test");
    let mut output = RoutineOutput::start_headless_with_diagnostics(
        "invocation-trace".into(),
        "a".repeat(64),
        diagnostics,
    );
    let event = |kind| RunEvent {
        schema: RUN_EVENT_SCHEMA.into(),
        kind,
    };
    output
        .publish(&event(RunEventKind::WatcherStarted {
            invocation_id: "invocation-trace".into(),
        }))
        .unwrap();
    output
        .publish(&event(RunEventKind::SessionStarted {
            session_id: Some("session-1".into()),
        }))
        .unwrap();
    for (sequence, timestamp) in [(1, 100), (3, 300)] {
        output
            .publish(&event(RunEventKind::RawScreenObserved {
                session_id: Some("session-1".into()),
                semantic_episode_id: Some(1),
                sequence,
                monotonic_start_ms: timestamp,
                monotonic_end_ms: timestamp,
                screen: "play".into(),
                result_presence: None,
                play_presence: None,
                unknown_reason: None,
            }))
            .unwrap();
    }
    output
        .publish(&event(RunEventKind::ScreenChanged {
            session_id: Some("session-1".into()),
            screen_episode_id: 1,
            sequence: 1,
            monotonic_start_ms: 100,
            monotonic_end_ms: 100,
            screen: "play".into(),
        }))
        .unwrap();
    output
        .publish(&event(RunEventKind::ScreenTick {
            screen_episode_id: 1,
            sequence: 2,
            monotonic_end_ms: 200,
            screen: "play".into(),
        }))
        .unwrap();
    output
        .publish(&event(RunEventKind::SessionFinished {
            session_id: "session-1".into(),
            outcome: "complete".into(),
            report: Value::Null,
        }))
        .unwrap();
    output.finish_diagnostics("success");
    let stream =
        fs::read_to_string(temporary.path().join("run-trace-test/diagnostics.ndjson")).unwrap();
    assert!(!stream.contains("\"event\":\"screen_tick\""));
    assert!(stream.contains("\"operation\":\"domain_summary\""));
    let records = stream
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| {
            matches!(
                record["operation"].as_str(),
                Some("runtime_event" | "domain_transition")
            )
        })
        .collect::<Vec<_>>();
    assert!(
        records
            .iter()
            .all(|record| match record["operation"].as_str() {
                Some("runtime_event") => record["data"]["schema"] == RUN_EVENT_SCHEMA,
                Some("domain_transition") =>
                    record["data"]["schema"] == super::super::schema::DOMAIN_TRANSITION_SCHEMA,
                _ => false,
            })
    );
    let screen_changed = records
        .iter()
        .position(|record| record["data"]["event"] == "screen_changed")
        .unwrap();
    assert!(
        records[screen_changed]["data"]["channel_sequence"]
            .as_u64()
            .unwrap()
            > records[screen_changed - 1]["data"]["channel_sequence"]
                .as_u64()
                .unwrap()
                + 1
    );
    assert_eq!(
        records
            .iter()
            .find(|record| record["data"]["event"] == "session_finished")
            .unwrap()["data"]["input_sequence"],
        6
    );
    assert!(
        records
            .windows(2)
            .all(|pair| pair[0]["data"]["channel_sequence"].as_u64().unwrap()
                < pair[1]["data"]["channel_sequence"].as_u64().unwrap())
    );
    let summary = stream
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .rfind(|record| record["operation"] == "domain_summary")
        .unwrap();
    assert_eq!(summary["data"]["admitted_frames"], 2);
    assert_eq!(summary["data"]["source_sequence_gaps"], 1);
    assert_eq!(summary["data"]["screen_counts"]["play"], 2);
    assert!(summary["data"]["no_op_inputs"].as_u64().unwrap() >= 2);
    assert!(stream.contains("\"recording_publication\":\"disabled\""));
    assert!(
        stream.contains(
            "\"canonical_recording_schema\":\"scorepeek-canonical-session-recording-v5\""
        )
    );
    assert!(
        stream.contains("\"canonical_frame_contract\":\"scorepeek-canonical-rgb8-1920x1080-v1\"")
    );
}

#[test]
fn session_finish_trace_records_completed_publication_result() {
    let temporary = tempfile::tempdir().unwrap();
    let diagnostics = RunDiagnostics::start(temporary.path(), "run-publication-test");
    let mut output = RoutineOutput::start_headless_with_diagnostics(
        "invocation-publication".into(),
        "a".repeat(64),
        diagnostics,
    );
    output.state.lock().unwrap().recording = "enabled";
    output
        .publish(&RunEvent {
            schema: RUN_EVENT_SCHEMA.into(),
            kind: RunEventKind::SessionFinished {
                session_id: "session-1".into(),
                outcome: "complete".into(),
                report: json!({
                    "canonical_recording_completeness": "complete",
                    "canonical_recording_manifest_published": true,
                    "recognition_busy_skips": 3,
                    "field_rejected": 2
                }),
            },
        })
        .unwrap();
    output.finish_diagnostics("success");
    let stream = fs::read_to_string(
        temporary
            .path()
            .join("run-publication-test/diagnostics.ndjson"),
    )
    .unwrap();
    assert!(stream.contains("\"recording_publication\":\"published\""));
    assert!(stream.contains("\"recording_locator\":\"sessions/session-1/canonical\""));
    let summary = stream
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|record| record["operation"] == "domain_summary")
        .unwrap();
    assert_eq!(
        summary["data"]["runtime_scheduling"]["recognition_busy_skips"],
        3
    );
    assert_eq!(summary["data"]["runtime_scheduling"]["field_rejected"], 2);
}

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
    let diagnostics = RunDiagnostics::start(temporary.path(), "score-shutdown-order");
    let mut output = RoutineOutput::start_headless_with_diagnostics(
        "invocation-1".into(),
        "a".repeat(64),
        diagnostics,
    );
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
    output.finish_diagnostics("success");
    let stream = fs::read_to_string(
        temporary
            .path()
            .join("score-shutdown-order/diagnostics.ndjson"),
    )
    .unwrap();
    let shutdown_events: Vec<Value> = stream
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| {
            record["operation"] == "public_event"
                && record["data"]["public_event_sequence"]
                    .as_u64()
                    .is_some_and(|sequence| sequence >= before_stop)
        })
        .map(|record| record["data"]["public_event"].clone())
        .collect();
    assert_eq!(
        shutdown_events
            .iter()
            .map(|event| event["event"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["score_store_changed", "status_changed"]
    );
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
        next_core_input_sequence: 1,
        active_core_input_sequence: None,
        timing_active: false,
        output_us: 0,
        headless_events: Vec::new(),
        core_reducer: DomainCoordinator::new(CoordinatorPolicy::default()).unwrap(),
        trace_counters: TraceCounters::default(),
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

fn detected_result_event(
    session_id: &str,
    source_sequence: u64,
    result: ResultDomainEvent,
) -> RunEvent {
    RunEvent {
        schema: RUN_EVENT_SCHEMA.to_owned(),
        kind: RunEventKind::ResultChanged {
            session_id: session_id.to_owned(),
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
                    semantic_episode_id: Some(0),
                    sequence,
                    monotonic_start_ms: sequence,
                    monotonic_end_ms: sequence,
                    screen: "result".to_owned(),
                    result_presence: Some(
                        scorepeek_core::recognition::screen::ResultPresenceEvidence {
                            warm_pixels: 0,
                            warm_pixels_min: 0,
                            panel_side:
                                scorepeek_core::recognition::screen::ResultPanelSideState::Known(
                                    side,
                                ),
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
                    ),
                    play_presence: Some(
                        scorepeek_core::recognition::screen::PlayPresenceEvidence {
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
                    ),
                    unknown_reason: None,
                },
            })
            .unwrap();
    }
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
    assert_eq!(snapshot["schema"], "scorepeek-event-snapshot-v5");
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
            assert!(current_provisional_result(&output).is_some());
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

    let provisional = current_provisional_result(&output).unwrap();
    assert_eq!(provisional.clear_type, "HARD CLEAR");
    assert_eq!(
        provisional.miss_count,
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
    let provisional = current_provisional_result(&output).unwrap();
    assert_eq!(
        provisional.miss_count,
        SupplementalResultValue::Known { value: 5 }
    );
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
    assert!(current_provisional_result(&output).is_some());
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
fn run_view_tracks_each_result_state_without_falling_back_after_retraction() {
    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    prepare_accepted_attempt(&mut output);
    output.publish(&accepted_result_event(1)).unwrap();
    output.publish(&accepted_result_event(2)).unwrap();
    let (provisional_song, provisional_result) = output
        .headless_events
        .iter()
        .rev()
        .find_map(|event| match &event.kind {
            RunEventKind::ResultChanged {
                state: ResultState::Provisional { song, result },
                ..
            } => Some((song.clone(), result.clone())),
            _ => None,
        })
        .unwrap();
    output
        .publish(&semantic_episode_event(
            3,
            "result",
            SemanticEpisodePhase::Finalized,
        ))
        .unwrap();
    assert!(output.state.lock().unwrap().result_history.back().is_some());

    let resolved = RunEvent {
        schema: RUN_EVENT_SCHEMA.to_owned(),
        kind: RunEventKind::ResultChanged {
            session_id: "invocation-1-session-1".to_owned(),
            source_sequence: 4,
            state: ResultState::Provisional {
                song: provisional_song.clone(),
                result: provisional_result.clone(),
            },
        },
    };
    output.publish(&resolved).unwrap();
    assert_eq!(
        output.state.lock().unwrap().latest_result_label,
        Some("PROVISIONAL")
    );

    let withdrawn = RunEvent {
        schema: RUN_EVENT_SCHEMA.to_owned(),
        kind: RunEventKind::ResultChanged {
            session_id: "invocation-1-session-1".to_owned(),
            source_sequence: 5,
            state: ResultState::Retracted {
                song: provisional_song,
                result: provisional_result,
                reason: ResultRetractionReason::EvidenceUnresolved,
            },
        },
    };
    output.publish(&withdrawn).unwrap();
    assert_eq!(
        output.state.lock().unwrap().latest_result_label,
        Some("RETRACTED")
    );
    output
        .publish(&RunEvent {
            schema: RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::ResultChanged {
                session_id: "invocation-1-session-1".to_owned(),
                source_sequence: 6,
                state: ResultState::Inactive,
            },
        })
        .unwrap();
    assert_eq!(
        output.state.lock().unwrap().latest_result_label,
        Some("INACTIVE")
    );
    assert_eq!(output.state.lock().unwrap().result_count, 1);
}

#[test]
fn linkage_deficient_attempt_is_provisional_then_withdrawn_on_rejection() {
    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    output.publish(&screen_event(0, "music_select")).unwrap();
    output
        .publish(&screen_event(0, "decide_transition"))
        .unwrap();
    output.publish(&screen_event(0, "result")).unwrap();
    prime_left_result_panel(&mut output);
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

    assert!(!output.headless_events.iter().any(|event| matches!(
        event.kind,
        RunEventKind::ResultChanged {
            state: ResultState::Confirmed { .. },
            ..
        }
    )));
    assert!(output.headless_events.iter().any(|event| matches!(
        &event.kind,
        RunEventKind::PlayAttemptChanged {
            state: PlayAttemptState::Attempt { attempt },
            ..
        } if attempt.result_relation == scorepeek_core::session::attempt::PlayAttemptResultRelation::Conflict && attempt.reasons.contains(&PlayAttemptReason::ResultEvidenceUnresolved)
    )));
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

    assert!(!output.headless_events.iter().any(|event| matches!(
        event.kind,
        RunEventKind::ResultChanged {
            state: ResultState::Confirmed { .. },
            ..
        }
    )));
    assert!(output.headless_events.iter().any(|event| matches!(
        &event.kind,
        RunEventKind::PlayAttemptChanged {
            state: PlayAttemptState::Attempt { attempt },
            ..
        } if attempt.phase == scorepeek_core::session::attempt::PlayAttemptPhase::Abandoned && attempt.reasons.contains(&PlayAttemptReason::SessionEnded)
    )));
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
        assert!(snapshot.contains("scorepeek-event-snapshot-v5"));
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
    assert_eq!(output.core_reducer.state().last_input_sequence(), Some(1));
    assert!(
        !output
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
    let mut state = RunViewState::new("invocation-1".to_owned(), true);
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
    let mut state = RunViewState::new("invocation-1".to_owned(), true);
    let started = RunEvent::from_value(json!({
        "schema": RUN_EVENT_SCHEMA,
        "event": "session_started",
        "session_id": "invocation-1-session-1",
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
    let mut state = RunViewState::new("invocation-1".to_owned(), true);
    assert_eq!(state.status_recording, "armed");
    let health = RunEvent::from_value(json!({
        "schema": RUN_EVENT_SCHEMA,
        "event": "recording_health_changed",
        "session_id": "session-1",
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
    let mut state = RunViewState::new("invocation-1".to_owned(), true);
    state.stable_result_song = Some(SongPresentation {
        scorepeek_song_id: song_id,
        display_titles: vec!["TITLE".to_owned()],
        artist: "ARTIST".to_owned(),
    });
    for source_sequence in 1..=(RESULT_HISTORY_CAPACITY as u64 + 1) {
        let result = detected_result_event(
            "session-1",
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
    assert_eq!(snapshot["latest_field_sequence"], 42);
    assert_eq!(
        snapshot["local"]["top_candidates"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    output
        .publish(&RunEvent {
            schema: RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::SemanticScreenEpisodeChanged {
                session_id: Some("invocation-1-session-1".to_owned()),
                screen_episode_id: 43,
                sequence: 43,
                monotonic_end_ms: 4_300,
                screen: "play".to_owned(),
                phase: SemanticEpisodePhase::Started,
            },
        })
        .unwrap();
    let snapshot = shared.lock().unwrap().resolver.clone();
    assert!(snapshot["latest_field_sequence"].is_null());
    assert!(snapshot["latest_field_ms"].is_null());
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
                sequence,
                sequence * 100,
                &fields,
                &evidence,
                &presentation,
            )
            .unwrap();
    }
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
                    sequence,
                    sequence * 100,
                    &fields,
                    &evidence,
                    &presentation,
                )
                .unwrap();
        }
        let first = frontend_select_best(&output);
        assert!(!first.is_null());
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
                    sequence,
                    sequence * 100,
                    &missing_fields,
                    &missing_evidence,
                    &presentation,
                )
                .unwrap();
            assert_eq!(frontend_select_best(&output), first, "{missing}");
        }
        for sequence in 9..=10 {
            output
                .reduce_music_select_observation(
                    Some(&session),
                    sequence,
                    sequence * 100,
                    &fields,
                    &evidence,
                    &presentation,
                )
                .unwrap();
            assert_eq!(frontend_select_best(&output), first, "{missing}");
        }
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
                    sequence,
                    sequence * 100,
                    &fields,
                    &evidence,
                    &presentation,
                )
                .unwrap();
        }
        let first = frontend_select_best(&output);
        assert!(!first.is_null());
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
                6,
                600,
                &changed_fields,
                &changed_evidence,
                &presentation,
            )
            .unwrap();
        assert!(frontend_select_best(&output).is_null(), "{conflict}");
        for sequence in 7..=15 {
            output
                .reduce_music_select_observation(
                    Some(&session),
                    sequence,
                    sequence * 100,
                    &fields,
                    &evidence,
                    &presentation,
                )
                .unwrap();
        }
        if conflict == "mode" {
            assert!(frontend_select_best(&output).is_null(), "{conflict}");
            continue;
        }
        let revisit = frontend_select_best(&output);
        assert!(!revisit.is_null(), "{conflict}");
        assert_ne!(
            first["selection_interval"], revisit["selection_interval"],
            "{conflict}"
        );
        assert_eq!(first["values"], revisit["values"], "{conflict}");
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
                    sequence,
                    sequence * 100,
                    &fields,
                    &evidence,
                    &presentation,
                )
                .unwrap();
        }
        assert!(output.headless_events.iter().any(|event| matches!(
            event.kind,
            RunEventKind::MusicSelectionChanged {
                state: MusicSelectionState::Selected { .. },
                ..
            }
        )));
        assert!(frontend_select_best(&output).is_null());
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
                sequence,
                sequence * 100,
                &fields,
                &evidence,
                &presentation,
            )
            .unwrap();
    }
    let mut changed_song = evidence.clone();
    changed_song.candidates[0].song_id =
        serde_json::from_str("\"00000000-0000-0000-0000-000000000047\"").unwrap();
    let mut conflicting_fields = fields.clone();
    conflicting_fields["play_type"]["state"]["value"] = json!("single");
    output
        .reduce_music_select_observation(
            Some(&session_id),
            4,
            400,
            &conflicting_fields,
            &changed_song,
            &presentation,
        )
        .unwrap();
    output
        .publish_screen_change(
            &RunEvent {
                schema: RUN_EVENT_SCHEMA.to_owned(),
                kind: RunEventKind::ScreenChanged {
                    session_id: Some(session_id),
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

#[test]
fn panel_side_conflict_retracts_the_provisional_result() {
    let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
    prepare_accepted_attempt(&mut output);
    output.publish(&accepted_result_event(1)).unwrap();
    output.publish(&accepted_result_event(2)).unwrap();
    assert!(current_provisional_result(&output).is_some());

    prime_result_panel(&mut output, ResultPanelSide::Right, 3);
    assert!(current_provisional_result(&output).is_none());

    assert!(current_provisional_result(&output).is_none());
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
fn session_finish_ends_selection_once_even_when_field_drain_did_not_finalize() {
    for finalize in [false, true] {
        let mut output = RoutineOutput::start_headless("invocation-1".to_owned(), "a".repeat(64));
        let session_id = "invocation-1-session-1".to_owned();
        let episode = |phase, sequence| RunEvent {
            schema: RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::SemanticScreenEpisodeChanged {
                session_id: Some(session_id.clone()),
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
                    sequence,
                    sequence * 100,
                    &fields,
                    &evidence,
                    &presentation,
                )
                .unwrap();
        }
        assert!(output.headless_events.iter().any(|event| matches!(
            event.kind,
            RunEventKind::MusicSelectionChanged {
                state: MusicSelectionState::Selected { .. },
                ..
            }
        )));
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
                    outcome: if finalize { "complete" } else { "failed" }.to_owned(),
                    report: json!({}),
                },
            })
            .unwrap();
        let ended = MusicSelectionState::Unresolved {
            reason: MusicSelectionUnresolvedReason::EpisodeEnded,
        };
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
    assert_eq!(snapshot["selection_difficulty_target"], "pending");
    assert_eq!(snapshot["selection_difficulty"]["difficulty"], "normal");
}

#[test]
fn resolver_transition_records_raw_and_normalized_family_contributions() {
    let mut output = RoutineOutput::start_headless("invocation-1".into(), "a".repeat(64));
    prime_left_result_panel(&mut output);
    output.take_headless_events();

    output.publish(&accepted_result_event(1)).unwrap();
    let events = output.take_headless_events();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, RunEventKind::FieldObservation { .. }))
    );
    let transition = events[0].to_value().unwrap();
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
