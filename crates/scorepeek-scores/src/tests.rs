use super::*;
use serde_json::json;

fn select(sequence: u64, score: &Value, miss: &Value) -> Value {
    json!({"schema":"scorepeek-event-v1","invocation_id":"run-a","sequence":sequence,"event_id":format!("run-a:{sequence}"),"emitted_unix_ms":1000+sequence,"capture":{"session_id":"session"},"event":"music_select_best_observed","snapshot":{"contract":"scorepeek-music-select-best-snapshot-v1","revision":sequence,"observation_id":format!("select:{sequence}"),"chart":{"scorepeek_song_id":"song-a","play_type":"single","difficulty":"hyper","presentation":{"display_titles":["Synthetic song"]}},"values":{"score":score,"miss_count":miss,"clear_type":{"status":"known","value":"hard_clear"}}}})
}
fn result(sequence: u64, score: u32) -> Value {
    json!({"schema":"scorepeek-event-v1","invocation_id":"run-a","sequence":sequence,"event_id":format!("run-a:{sequence}"),"emitted_unix_ms":1000+sequence,"capture":null,"event":"result_detected","song":null,"result":{"contract":"scorepeek-result-detected-v2","scorepeek_song_id":"song-a","play_type":"single","difficulty":"hyper","current_score":score,"clear_type":"EXH-CLEAR","miss_count":{"status":"known","value":20},"previous_best":{"score":{"status":"known","value":180},"miss_count":{"status":"unknown","reason":"empty"},"clear_type":{"status":"not_played"}},"judgments":{"synthetic":true}}})
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
    assert!(!apply(&mut store, &result(4, 150)));
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
    let clear = json!({"schema":"scorepeek-event-v1","invocation_id":"run-a","sequence":6,"event_id":"run-a:6","emitted_unix_ms":1006,"event":"music_select_best_observed","snapshot":null});
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
    for kind in [
        "result_provisional_changed",
        "music_selection_changed",
        "status_changed",
    ] {
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
    value["result"]["contract"] = json!("future");
    assert!(matches!(
        store.consume(&serde_json::to_vec(&value).unwrap(), 0),
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
    dp["result"]["play_type"] = json!("double");
    apply(&mut store, &dp);
    let mut another = result(5, 300);
    another["result"]["difficulty"] = json!("another");
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
    assert!(!apply(&mut store, &result(10, 100)));
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
fn concurrent_creators_both_open_and_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let writers = (1..=2)
        .map(|sequence| {
            let path = path.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                let mut store = Store::open(&path).unwrap();
                assert!(apply(&mut store, &result(sequence, 100)));
            })
        })
        .collect::<Vec<_>>();
    for writer in writers {
        writer.join().unwrap();
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM play_results", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}
