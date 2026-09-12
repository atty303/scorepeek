use super::*;
use serde_json::json;

fn select(sequence: u64, score: &Value, miss: &Value) -> Value {
    json!({"schema":"scorepeek-event-v2","invocation_id":"run-a","sequence":sequence,"event_id":format!("run-a:{sequence}"),"emitted_monotonic_ms":sequence,"emitted_unix_ms":1000+sequence,"capture":{"session_id":"session","capture_generation":1,"binding":null},"event":"music_select_best_observed","snapshot":{"contract":"scorepeek-music-select-best-snapshot-v1","revision":sequence,"observation_id":format!("select:{sequence}"),"chart":{"scorepeek_song_id":"song-a","play_type":"single","difficulty":"hyper","presentation":{"display_titles":["Synthetic song"]}},"values":{"score":score,"miss_count":miss,"clear_type":{"status":"known","value":"hard_clear"}}}})
}
fn result(sequence: u64, score: u32) -> Value {
    json!({"schema":"scorepeek-event-v2","invocation_id":"run-a","sequence":sequence,"event_id":format!("run-a:{sequence}"),"emitted_monotonic_ms":sequence,"emitted_unix_ms":1000+sequence,"capture":{"session_id":"session","capture_generation":1,"binding":null},"event":"result_changed","source_sequence":sequence,"state":{"status":"provisional","song":{"scorepeek_song_id":"song-a","display_titles":["Synthetic song"],"artist":"Synthetic artist"},"result":{"contract":"scorepeek-result-detected-v2","attempt_id":sequence,"scorepeek_song_id":"song-a","play_side":"1p","play_mode":"sp","play_type":"single","difficulty":"hyper","level":10,"notes":1000,"current_score":score,"clear_type":"EXH-CLEAR","judgments":{"pgreat":50,"great":20,"good":3,"bad":2,"poor":1},"miss_count":{"status":"known","value":20},"timing":{"fast":{"status":"known","value":4},"slow":{"status":"known","value":5}},"combo_break":{"status":"known","value":6},"previous_best":{"score":{"status":"known","value":180},"miss_count":{"status":"unknown","reason":"empty"},"clear_type":{"status":"not_played"}},"play_options":{"status":"known","values":[]}}}})
}
fn result_state(sequence: u64, attempt_id: u64, score: u32, status: &str) -> Value {
    let mut value = result(sequence, score);
    value["state"]["status"] = json!(status);
    value["state"]["result"]["attempt_id"] = json!(attempt_id);
    if status == "retracted" {
        value["state"]["reason"] = json!("evidence_unresolved");
    }
    value
}
fn apply(store: &mut Store, value: &Value) -> bool {
    store
        .consume(&serde_json::to_vec(value).unwrap(), 2000)
        .unwrap()
}
fn values(store: &Store) -> (Option<i64>, Option<i64>, Option<i64>) {
    store
        .connection
        .query_row("SELECT score,miss,clear FROM chart_bests", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap()
}

#[test]
fn read_only_history_never_creates_and_tracks_committed_latest_five() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite3");
    assert!(query::chart_history(&path, "song-a", "single", "hyper").is_err());
    assert!(!path.exists());
    let mut store = Store::open(&path).unwrap();
    for sequence in 1..=7 {
        apply(
            &mut store,
            &result(sequence, u32::try_from(sequence).unwrap() * 100),
        );
    }
    let history = query::chart_history(&path, "song-a", "single", "hyper").unwrap();
    assert_eq!(history.plays.len(), 5);
    assert_eq!(history.plays[0].event_id, "run-a:7");
    assert_eq!(history.plays[4].event_id, "run-a:3");
    assert_eq!(history.best.score, Some(700));
    assert!(
        query::chart_history(&path, "song-b", "single", "hyper")
            .unwrap()
            .plays
            .is_empty()
    );
    apply(&mut store, &result(8, 800));
    assert_eq!(
        query::chart_history(&path, "song-a", "single", "hyper")
            .unwrap()
            .plays[0]
            .score,
        800
    );
}
#[test]
fn select_only_updates_without_history_and_result_later_supports_best() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/scores.db");
    let mut store = Store::open(&path).unwrap();
    apply(
        &mut store,
        &select(
            1,
            &json!({"status":"known","value":900}),
            &json!({"status":"known","value":0}),
        ),
    );
    assert_eq!(values(&store), (Some(900), Some(0), Some(5)));
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM play_results", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    apply(
        &mut store,
        &select(
            2,
            &json!({"status":"known","value":100}),
            &json!({"status":"known","value":30}),
        ),
    );
    assert_eq!(values(&store), (Some(100), Some(30), Some(5)));
    apply(
        &mut store,
        &select(
            3,
            &json!({"status":"unknown"}),
            &json!({"status":"not_displayed"}),
        ),
    );
    assert_eq!(values(&store), (Some(100), Some(30), Some(5)));
    apply(&mut store, &result(4, 150));
    assert_eq!(values(&store), (Some(180), Some(20), Some(6)));
    apply(
        &mut store,
        &select(
            5,
            &json!({"status":"no_record"}),
            &json!({"status":"no_record"}),
        ),
    );
    assert_eq!(values(&store), (Some(180), Some(20), Some(6)));
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM chart_bests", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let stored: String = store
        .connection
        .query_row("SELECT select_score FROM chart_bests", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&stored).unwrap()["value"],
        Value::Null
    );
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(values(&store), (Some(180), Some(20), Some(6)));
    assert!(apply(&mut store, &result(4, 150)));
    assert!(apply(&mut store, &result(6, 150)));
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM play_results", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}
#[test]
fn order_clear_unknown_and_new_invocation() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("db")).unwrap();
    let mut event = select(
        5,
        &json!({"status":"known","value":900}),
        &json!({"status":"known","value":1}),
    );
    apply(&mut store, &event);
    assert!(!apply(
        &mut store,
        &select(
            4,
            &json!({"status":"known","value":100}),
            &json!({"status":"no_record"})
        )
    ));
    let clear = json!({"schema":"scorepeek-event-v2","invocation_id":"run-a","sequence":6,"event_id":"run-a:6","emitted_monotonic_ms":6,"emitted_unix_ms":1006,"capture":null,"event":"music_select_best_observed","snapshot":null});
    assert!(!apply(&mut store, &clear));
    assert_eq!(values(&store), (Some(900), Some(1), Some(5)));
    event["invocation_id"] = json!("run-b");
    event["event_id"] = json!("run-b:1");
    event["sequence"] = json!(1);
    event["emitted_unix_ms"] = json!(0);
    event["snapshot"]["revision"] = json!(1);
    event["snapshot"]["values"]["score"] = json!({"status":"known","value":100});
    apply(&mut store, &event);
    assert_eq!(values(&store), (Some(100), Some(1), Some(5)));
    for kind in ["music_selection_changed", "status_changed"] {
        event["event"] = json!(kind);
        assert!(!apply(&mut store, &event));
    }
}
#[test]
fn transaction_rollback_and_database_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut store = Store::open(&path).unwrap();
    store.connection.execute_batch("CREATE TRIGGER fail_update BEFORE UPDATE ON chart_bests BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").unwrap();
    assert!(
        store
            .consume(&serde_json::to_vec(&result(1, 100)).unwrap(), 2000)
            .is_err()
    );
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM play_results", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM chart_bests", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    store
        .connection
        .pragma_update(None, "user_version", 99)
        .unwrap();
    drop(store);
    assert!(matches!(
        Store::open(&path),
        Err(Error::UnsupportedDatabase(99))
    ));
}
#[test]
fn worker_drains_and_database_instances_are_separate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.db");
    let other = dir.path().join("b.db");
    let mut worker = Worker::start(&path);
    worker.offer(&serde_json::to_vec(&result(1, 100)).unwrap());
    let health = worker.finish();
    assert!(health.failure.is_none(), "{health:?}");
    assert_eq!(health.committed, 1);
    assert_eq!(health.pending, 0);
    let completions = worker.take_completions();
    assert_eq!(completions.len(), 1);
    assert_eq!(
        completions[0].chart,
        Some(ChartIdentity {
            scorepeek_song_id: "song-a".into(),
            play_type: "single".into(),
            difficulty: "hyper".into(),
        })
    );
    let store = Store::open(&path).unwrap();
    assert_eq!(values(&store).0, Some(180));
    let other = Store::open(&other).unwrap();
    assert_eq!(
        other
            .connection
            .query_row("SELECT count(*) FROM chart_bests", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let mut worker = Worker::start(dir.path());
    worker.offer(&serde_json::to_vec(&result(2, 200)).unwrap());
    assert_eq!(worker.finish().failure.as_deref(), Some("database_open"));
}
#[test]
fn unsupported_payload_is_not_saved() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("db")).unwrap();
    let mut value = result(1, 100);
    value["state"]["result"]["contract"] = json!("future");
    assert!(matches!(
        store.consume(&serde_json::to_vec(&value).unwrap(), 0),
        Err(Error::UnsupportedContract)
    ));
}

#[test]
fn invalid_retraction_keeps_the_provisional_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut store = Store::open(&path).unwrap();
    assert!(apply(&mut store, &result_state(1, 9, 200, "provisional")));

    for mut invalid in [
        result_state(2, 9, 200, "retracted"),
        result_state(3, 9, 200, "retracted"),
        result_state(4, 9, 200, "retracted"),
    ] {
        match invalid["sequence"].as_u64().unwrap() {
            2 => invalid["state"]["result"]["contract"] = json!("wrong"),
            3 => {
                invalid["state"].as_object_mut().unwrap().remove("reason");
            }
            4 => invalid["state"]["result"]["clear_type"] = json!("INVALID"),
            _ => unreachable!(),
        }
        assert!(matches!(
            store.consume(&serde_json::to_vec(&invalid).unwrap(), 2000),
            Err(Error::UnsupportedContract | Error::Json(_))
        ));
        assert_eq!(
            store
                .connection
                .query_row("SELECT state FROM play_results", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "provisional"
        );
    }
}

#[test]
fn incomplete_public_result_is_rejected_before_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut store = Store::open(&path).unwrap();
    for mut invalid in [result(1, 200), result(2, 200), result(3, 200)] {
        match invalid["sequence"].as_u64().unwrap() {
            1 => {
                invalid
                    .as_object_mut()
                    .unwrap()
                    .remove("emitted_monotonic_ms");
            }
            2 => {
                invalid["capture"]
                    .as_object_mut()
                    .unwrap()
                    .remove("capture_generation");
            }
            3 => {
                invalid["state"]["result"]["judgments"]
                    .as_object_mut()
                    .unwrap()
                    .remove("poor");
            }
            _ => unreachable!(),
        }
        assert!(
            store
                .consume(&serde_json::to_vec(&invalid).unwrap(), 2000)
                .is_err()
        );
    }
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM play_results", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn result_identity_and_presentation_are_required_before_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("db")).unwrap();
    for mut invalid in [
        result(1, 200),
        result(2, 200),
        result(3, 200),
        result(4, 200),
    ] {
        match invalid["sequence"].as_u64().unwrap() {
            1 => {
                invalid["state"].as_object_mut().unwrap().remove("song");
            }
            2 => invalid["state"]["song"]["scorepeek_song_id"] = json!("other-song"),
            3 => invalid["capture"] = Value::Null,
            4 => invalid["state"]["song"]["display_titles"] = json!([]),
            _ => unreachable!(),
        }
        assert!(
            store
                .consume(&serde_json::to_vec(&invalid).unwrap(), 2000)
                .is_err()
        );
    }
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM play_results", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn valid_unknown_event_advances_but_malformed_unknown_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("db")).unwrap();
    let unknown = json!({"schema":"scorepeek-event-v2","invocation_id":"run-a","sequence":1,"event_id":"run-a:1","emitted_monotonic_ms":1,"emitted_unix_ms":1001,"capture":null,"event":"future_event","payload":{"additive":true}});
    assert!(!apply(&mut store, &unknown));
    assert!(!apply(&mut store, &result(1, 100)));
    assert!(apply(&mut store, &result(2, 200)));

    let mut malformed = unknown;
    malformed["sequence"] = json!(3);
    malformed.as_object_mut().unwrap().remove("event_id");
    assert!(matches!(
        store.consume(&serde_json::to_vec(&malformed).unwrap(), 2000),
        Err(Error::UnsupportedContract)
    ));
}

#[test]
fn chart_keys_and_tie_provenance_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("db")).unwrap();
    apply(
        &mut store,
        &select(
            1,
            &json!({"status":"known","value":180}),
            &json!({"status":"known","value":20}),
        ),
    );
    apply(&mut store, &result(2, 180));
    let source: String = store
        .connection
        .query_row("SELECT score_origin FROM chart_bests", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&source).unwrap()["source"],
        "select"
    );
    apply(
        &mut store,
        &select(
            3,
            &json!({"status":"known","value":100}),
            &json!({"status":"no_record"}),
        ),
    );
    let source: String = store
        .connection
        .query_row("SELECT score_origin FROM chart_bests", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&source).unwrap()["source"],
        "result"
    );
    let mut dp = result(4, 200);
    dp["state"]["result"]["play_type"] = json!("double");
    apply(&mut store, &dp);
    let mut another = result(5, 300);
    another["state"]["result"]["difficulty"] = json!("another");
    apply(&mut store, &another);
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM chart_bests", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        3
    );
}

#[test]
fn database_lock_is_bounded_and_does_not_partially_insert() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut store = Store::open(&path).unwrap();
    let lock = Connection::open(&path).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let started = std::time::Instant::now();
    assert!(
        store
            .consume(&serde_json::to_vec(&result(1, 100)).unwrap(), 2000)
            .is_err()
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    lock.execute_batch("ROLLBACK;").unwrap();
    assert!(apply(&mut store, &result(1, 100)));
}

#[test]
fn reopened_duplicate_result_advances_order_before_reversed_select() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut store = Store::open(&path).unwrap();
    apply(
        &mut store,
        &select(
            8,
            &json!({"status":"known","value":300}),
            &json!({"status":"known","value":10}),
        ),
    );
    apply(&mut store, &result(10, 100));
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert!(apply(&mut store, &result(10, 100)));
    assert!(!apply(
        &mut store,
        &select(
            9,
            &json!({"status":"known","value":200}),
            &json!({"status":"known","value":30})
        )
    ));
    assert_eq!(values(&store).0, Some(300));
}

#[test]
fn second_writer_cannot_recover_a_live_provisional() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut first = Store::open(&path).unwrap();
    assert!(apply(&mut first, &result_state(1, 10, 100, "provisional")));
    assert!(matches!(
        Store::open(&path),
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock
    ));
    assert_eq!(
        first
            .connection
            .query_row("SELECT state FROM play_results", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "provisional"
    );
    drop(first);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.recovered_provisional_count(), 1);
    assert_eq!(
        store
            .connection
            .query_row("SELECT state FROM play_results", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "confirmed"
    );
}

#[test]
fn provisional_update_retraction_and_confirmation_share_one_attempt_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut store = Store::open(&path).unwrap();
    apply(
        &mut store,
        &select(
            1,
            &json!({"status":"known","value":100}),
            &json!({"status":"known","value":30}),
        ),
    );
    assert!(apply(&mut store, &result_state(2, 44, 200, "provisional")));
    assert!(apply(&mut store, &result_state(3, 44, 220, "provisional")));
    assert_eq!(values(&store).0, Some(220));
    assert_eq!(
        query::chart_history(&path, "song-a", "single", "hyper")
            .unwrap()
            .plays
            .len(),
        1
    );
    assert!(apply(&mut store, &result_state(4, 44, 220, "retracted")));
    assert_eq!(values(&store).0, Some(100));
    assert!(
        query::chart_history(&path, "song-a", "single", "hyper")
            .unwrap()
            .plays
            .is_empty()
    );
    assert!(apply(&mut store, &result_state(5, 44, 230, "provisional")));
    assert!(apply(&mut store, &result_state(6, 44, 230, "confirmed")));
    let play = query::chart_history(&path, "song-a", "single", "hyper")
        .unwrap()
        .plays
        .pop()
        .unwrap();
    assert_eq!(play.event_id, "run-a:2");
    assert_eq!(play.emitted_unix_ms, 1002);
    let (state, latest): (String, String) = store
        .connection
        .query_row(
            "SELECT state,latest_event_id FROM play_results",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((state.as_str(), latest.as_str()), ("confirmed", "run-a:6"));

    assert!(apply(&mut store, &result_state(7, 44, 230, "retracted")));
    let (state, latest): (String, String) = store
        .connection
        .query_row(
            "SELECT state,latest_event_id FROM play_results",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((state.as_str(), latest.as_str()), ("confirmed", "run-a:6"));
    assert_eq!(
        query::chart_history(&path, "song-a", "single", "hyper")
            .unwrap()
            .plays
            .len(),
        1
    );
}

#[test]
fn reopening_promotes_provisional_with_recovery_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut store = Store::open(&path).unwrap();
    assert!(apply(&mut store, &result_state(1, 7, 200, "provisional")));
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.recovered_provisional_count(), 1);
    let recovered: (String, i64) = store
        .connection
        .query_row(
            "SELECT state,recovery_confirmed FROM play_results",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(recovered, ("confirmed".into(), 1));
}

#[test]
fn version_one_migrates_transactionally_to_stored_results() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(&format!(
            "CREATE TABLE play_results (event_id TEXT PRIMARY KEY, song_id TEXT NOT NULL, play_type TEXT NOT NULL, difficulty TEXT NOT NULL, emitted_unix_ms INTEGER NOT NULL, received_unix_ms INTEGER NOT NULL, score INTEGER NOT NULL, miss INTEGER, clear INTEGER NOT NULL, event_json TEXT NOT NULL);
             CREATE INDEX plays_chart ON play_results(song_id,play_type,difficulty);
             CREATE TABLE chart_bests (song_id TEXT NOT NULL, play_type TEXT NOT NULL, difficulty TEXT NOT NULL, presentation TEXT, {}, score INTEGER, miss INTEGER, clear INTEGER, PRIMARY KEY(song_id,play_type,difficulty));
             PRAGMA user_version=1;",
            COLUMNS
                .iter()
                .map(|column| format!("{column} TEXT"))
                .collect::<Vec<_>>()
                .join(",")
        ))
        .unwrap();
    let mut legacy = result_state(1, 1, 100, "confirmed");
    legacy["schema"] = json!("scorepeek-event-v1");
    legacy["event"] = json!("result_detected");
    legacy["result"] = legacy["state"]["result"].take();
    legacy["song"] = Value::Null;
    legacy.as_object_mut().unwrap().remove("state");
    connection
        .execute(
            "INSERT INTO play_results VALUES ('legacy:1','song-a','single','hyper',10,11,100,20,4,?1)",
            [serde_json::to_string(&legacy).unwrap()],
        )
        .unwrap();
    drop(connection);

    assert_eq!(Store::migrate(&path).unwrap(), 1);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.recovered_provisional_count(), 0);
    let migrated: (i64, String, String) = store
        .connection
        .query_row(
            "SELECT (SELECT user_version FROM pragma_user_version),state,latest_event_id FROM play_results",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(migrated, (3, "confirmed".into(), "legacy:1".into()));
    let stored_schema: String = store
        .connection
        .query_row(
            "SELECT json_extract(event_json,'$.schema') FROM play_results",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_schema, "scorepeek-stored-result-v1");
    assert!(apply(&mut store, &result_state(2, 2, 250, "provisional")));
    assert_eq!(values(&store).0, Some(250));
    assert!(apply(&mut store, &result_state(3, 2, 250, "retracted")));
    assert_eq!(values(&store).0, Some(180));
}
