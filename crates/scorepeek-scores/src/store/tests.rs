use super::*;
use crate::query;
use serde_json::json;

fn select(sequence: u64, score: &Value, miss: &Value) -> Value {
    json!({"schema":"scorepeek-event-v5","invocation_id":"run-a","sequence":sequence,"event_id":format!("run-a:{sequence}"),"emitted_monotonic_ms":sequence,"emitted_unix_ms":1000+sequence,"capture":{"session_id":"session"},"event":"music_select_best_observed","snapshot":{"contract":"scorepeek-music-select-best-snapshot-v4","revision":sequence,"observation_id":format!("select:{sequence}"),"chart":{"scorepeek_song_id":"song-a","play_side":"one_player","play_type":"single","difficulty":"hyper","presentation":{"display_titles":["Synthetic song"]}},"values":{"score":score,"miss_count":miss,"clear_type":{"status":"known","value":"hard_clear"}}}})
}
fn result(sequence: u64, score: u32) -> Value {
    json!({"schema":"scorepeek-event-v5","invocation_id":"run-a","sequence":sequence,"event_id":format!("run-a:{sequence}"),"emitted_monotonic_ms":sequence,"emitted_unix_ms":1000+sequence,"capture":{"session_id":"session"},"event":"result_changed","source_sequence":sequence,"state":{"status":"provisional","song":{"scorepeek_song_id":"song-a","display_titles":["Synthetic song"],"artist":"Synthetic artist"},"result":{"contract":"scorepeek-result-detected-v4","attempt_id":sequence,"scorepeek_song_id":"song-a","play_side":"one_player","play_mode":"sp","play_type":"single","difficulty":"hyper","level":10,"notes":1000,"current_score":score,"clear_type":"EXH-CLEAR","judgments":{"pgreat":50,"great":20,"good":3,"bad":2,"poor":1},"miss_count":{"status":"known","value":20},"timing":{"fast":{"status":"known","value":4},"slow":{"status":"known","value":5}},"combo_break":{"status":"known","value":6},"previous_best":{"score":{"status":"known","value":180},"miss_count":{"status":"unknown","reason":"empty"},"clear_type":{"status":"not_played"}},"play_options":{"status":"known","values":[]}}}})
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

fn downgrade_database_to_v3(path: &std::path::Path) {
    let mut connection = Connection::open(path).unwrap();
    let tx = connection.transaction().unwrap();
    let rows = {
        let mut statement = tx
            .prepare("SELECT rowid,play_type,event_json FROM play_results ORDER BY rowid")
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    for (rowid, play_type, json) in rows {
        let mut stored: Value = serde_json::from_str(&json).unwrap();
        stored["schema"] = json!("scorepeek-stored-result-v1");
        stored["result"]["contract"] = json!("scorepeek-result-detected-v3");
        stored["result"]["play_side"] = if play_type == "double" {
            json!({"status":"not_applicable"})
        } else {
            let side = stored["result"]["play_side"].clone();
            json!({"status":"known","value":side})
        };
        tx.execute(
            "UPDATE play_results SET event_json=?1 WHERE rowid=?2",
            params![serde_json::to_string(&stored).unwrap(), rowid],
        )
        .unwrap();
    }
    tx.execute_batch(
        "DROP TABLE result_attributes;
         DROP TABLE play_result_facts;
         DROP INDEX plays_chart;
         DROP INDEX plays_attempt;
         ALTER TABLE play_results RENAME TO play_results_v4;
         CREATE TABLE play_results (event_id TEXT PRIMARY KEY, session_id TEXT, attempt_id INTEGER, state TEXT NOT NULL, latest_event_id TEXT NOT NULL, recovery_confirmed INTEGER NOT NULL DEFAULT 0, song_id TEXT NOT NULL, play_type TEXT NOT NULL, difficulty TEXT NOT NULL, emitted_unix_ms INTEGER NOT NULL, received_unix_ms INTEGER NOT NULL, score INTEGER NOT NULL, miss INTEGER, clear INTEGER NOT NULL, event_json TEXT NOT NULL);
         INSERT INTO play_results(event_id,session_id,attempt_id,state,latest_event_id,recovery_confirmed,song_id,play_type,difficulty,emitted_unix_ms,received_unix_ms,score,miss,clear,event_json) SELECT event_id,session_id,attempt_id,state,latest_event_id,recovery_confirmed,song_id,play_type,difficulty,emitted_unix_ms,received_unix_ms,score,miss,clear,event_json FROM play_results_v4;
         DROP TABLE play_results_v4;
         CREATE INDEX plays_chart ON play_results(song_id,play_type,difficulty);
         CREATE UNIQUE INDEX plays_attempt ON play_results(session_id,attempt_id);
         PRAGMA user_version=3;",
    )
    .unwrap();
    tx.commit().unwrap();
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
    assert_eq!(history.plays[0].play_side, PlaySide::OnePlayer);
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
fn opening_v3_migrates_play_side_and_stored_results_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scores.sqlite3");
    let mut store = Store::open(&path).unwrap();
    let mut sp = result_state(1, 1, 200, "confirmed");
    sp["state"]["result"]["play_side"] = json!("two_player");
    assert!(apply(&mut store, &sp));
    let mut dp = result_state(2, 2, 300, "confirmed");
    dp["state"]["result"]["play_type"] = json!("double");
    dp["state"]["result"]["play_mode"] = json!("dp");
    dp["state"]["result"]["play_side"] = json!("two_player");
    assert!(apply(&mut store, &dp));
    drop(store);
    downgrade_database_to_v3(&path);

    let store = Store::open(&path).unwrap();
    let version: i64 = store
        .connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 5);
    let play_side_not_null: i64 = store
        .connection
        .query_row(
            "SELECT \"notnull\" FROM pragma_table_info('play_results') WHERE name='play_side'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(play_side_not_null, 1);
    let mut statement = store
        .connection
        .prepare("SELECT play_type,play_side,event_json FROM play_results ORDER BY attempt_id")
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows.len(), 2);
    for (play_type, play_side, json) in rows {
        let expected = if play_type == "single" {
            "two_player"
        } else {
            "one_player"
        };
        assert_eq!(play_side, expected);
        let stored: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(stored["schema"], "scorepeek-stored-result-v1");
        assert_eq!(stored["result"]["contract"], "scorepeek-result-detected-v3");
        assert_ne!(stored["result"]["play_side"], serde_json::Value::Null);
    }
    drop(statement);
    drop(store);
    assert_eq!(
        query::chart_history(&path, "song-a", "single", "hyper")
            .unwrap()
            .plays[0]
            .play_side,
        PlaySide::TwoPlayer
    );
    assert_eq!(
        query::chart_history(&path, "song-a", "double", "hyper")
            .unwrap()
            .plays[0]
            .play_side,
        PlaySide::OnePlayer
    );
    let representative = query::chart_dashboard(&path, "song-a", "single", "hyper", 5, 0)
        .unwrap()
        .representative
        .unwrap();
    assert_eq!(
        representative["result"]["previous_best"]["miss_count"]["reason"],
        "empty"
    );
}

#[test]
fn malformed_v3_row_rolls_back_the_schema_migration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scores.sqlite3");
    let mut store = Store::open(&path).unwrap();
    assert!(apply(&mut store, &result_state(1, 1, 200, "confirmed")));
    drop(store);
    downgrade_database_to_v3(&path);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute("UPDATE play_results SET event_json='{}'", [])
        .unwrap();
    drop(connection);

    assert!(matches!(Store::open(&path), Err(Error::Migration(_, _))));
    let connection = Connection::open(&path).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 3);
    let play_side_columns: i64 = connection
        .query_row(
            "SELECT count(*) FROM pragma_table_info('play_results') WHERE name='play_side'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(play_side_columns, 0);
}

#[test]
fn backup_creation_failure_leaves_legacy_database_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scores.sqlite3");
    let mut store = Store::open(&path).unwrap();
    assert!(apply(&mut store, &result_state(1, 1, 200, "confirmed")));
    drop(store);
    downgrade_database_to_v3(&path);
    let original = std::fs::read(&path).unwrap();
    let connection = Connection::open(&path).unwrap();
    let nonexistent = dir.path().join("missing/scores.sqlite3");
    assert!(crate::migration::backup_database(&connection, &nonexistent, 3).is_err());
    drop(connection);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(
        Connection::open(&path)
            .unwrap()
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        3
    );
}

#[test]
fn missing_legacy_detail_is_marked_without_losing_score_or_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scores.sqlite3");
    let mut store = Store::open(&path).unwrap();
    assert!(apply(&mut store, &result_state(1, 1, 200, "confirmed")));
    drop(store);
    downgrade_database_to_v3(&path);
    let connection = Connection::open(&path).unwrap();
    let json: String = connection
        .query_row("SELECT event_json FROM play_results", [], |row| row.get(0))
        .unwrap();
    let mut raw: Value = serde_json::from_str(&json).unwrap();
    raw["result"].as_object_mut().unwrap().remove("judgments");
    let original = serde_json::to_string(&raw).unwrap();
    connection
        .execute("UPDATE play_results SET event_json=?1", [&original])
        .unwrap();
    drop(connection);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.migration_unavailable_details(), 1);
    let saved: String = store
        .connection
        .query_row("SELECT event_json FROM play_results", [], |row| row.get(0))
        .unwrap();
    assert_eq!(saved, original);
    let dashboard = query::chart_dashboard(&path, "song-a", "single", "hyper", 5, 0).unwrap();
    assert_eq!(dashboard.best.score, Some(200));
    assert_eq!(
        dashboard.representative.unwrap()["detail_state"],
        "unavailable_at_migration"
    );
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert!(apply(&mut store, &result_state(2, 1, 300, "confirmed")));
    let updated = query::chart_dashboard(&path, "song-a", "single", "hyper", 5, 0).unwrap();
    assert_eq!(
        updated.representative.unwrap()["result"]["current_score"],
        300
    );
}

#[test]
fn malformed_stored_evidence_does_not_block_new_scores_after_migration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scores.sqlite3");
    let mut store = Store::open(&path).unwrap();
    assert!(apply(&mut store, &result_state(1, 1, 200, "confirmed")));
    drop(store);
    downgrade_database_to_v3(&path);
    let mut store = Store::open(&path).unwrap();
    store
        .connection
        .execute(
            "UPDATE play_results SET event_json='unreadable legacy evidence'",
            [],
        )
        .unwrap();
    assert!(apply(&mut store, &result_state(2, 2, 300, "confirmed")));
    let dashboard = query::chart_dashboard(&path, "song-a", "single", "hyper", 5, 0).unwrap();
    assert_eq!(dashboard.best.score, Some(300));
    assert_eq!(dashboard.recent.len(), 2);
    assert_eq!(
        dashboard.representative.unwrap()["result"]["current_score"],
        300
    );
}

#[test]
fn isolated_legacy_database_preserves_evidence_and_scores() {
    type BestRow = (
        String,
        String,
        String,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    );
    let Ok(source) = std::env::var("SCOREPEEK_LEGACY_DB_COPY") else {
        return;
    };
    let original = std::fs::read(&source).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scores.sqlite3");
    std::fs::write(&path, &original).unwrap();
    let before = Connection::open(&path).unwrap();
    let plays_before: Vec<(String, String, i64, Option<i64>, i64)> = before
        .prepare("SELECT event_id,event_json,score,miss,clear FROM play_results ORDER BY event_id")
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let bests_before: Vec<BestRow> = before.prepare("SELECT song_id,play_type,difficulty,score,miss,clear FROM chart_bests ORDER BY song_id,play_type,difficulty").unwrap().query_map([], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).unwrap().collect::<Result<_,_>>().unwrap();
    drop(before);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.migration_unavailable_details(), 0);
    let backup = store.migration_backup().unwrap();
    let saved = Connection::open(backup).unwrap();
    assert_eq!(
        saved
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        saved
            .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    drop(saved);
    let plays_after: Vec<(String, String, i64, Option<i64>, i64)> = store
        .connection
        .prepare("SELECT event_id,event_json,score,miss,clear FROM play_results ORDER BY event_id")
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let bests_after: Vec<BestRow> = store.connection.prepare("SELECT song_id,play_type,difficulty,score,miss,clear FROM chart_bests ORDER BY song_id,play_type,difficulty").unwrap().query_map([], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).unwrap().collect::<Result<_,_>>().unwrap();
    assert_eq!(plays_before, plays_after);
    assert_eq!(bests_before, bests_after);
    assert_eq!(std::fs::read(&source).unwrap(), original);
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
    let clear = json!({"schema":"scorepeek-event-v5","invocation_id":"run-a","sequence":6,"event_id":"run-a:6","emitted_monotonic_ms":6,"emitted_unix_ms":1006,"capture":null,"event":"music_select_best_observed","snapshot":null});
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
fn play_side_is_a_plain_enum_for_both_play_types() {
    let mut invalid_result = result(1, 100);
    invalid_result["state"]["result"]["play_side"] = json!({"status":"known","value":"one_player"});
    let select_score = json!({"status":"known", "value":100});
    let select_miss = json!({"status":"known", "value":1});
    let mut legacy_select = select(1, &select_score, &select_miss);
    legacy_select["snapshot"]["chart"]["play_side"] =
        json!({"status":"known","value":"one_player"});

    for invalid in [invalid_result, legacy_select] {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        assert!(
            store
                .consume(&serde_json::to_vec(&invalid).unwrap(), 0)
                .is_err()
        );
    }

    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("db")).unwrap();
    let mut dp = result(1, 100);
    dp["state"]["result"]["play_type"] = json!("double");
    dp["state"]["result"]["play_mode"] = json!("dp");
    dp["state"]["result"]["play_side"] = json!("two_player");
    assert!(apply(&mut store, &dp));
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
                    .remove("session_id");
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
    let unknown = json!({"schema":"scorepeek-event-v5","invocation_id":"run-a","sequence":1,"event_id":"run-a:1","emitted_monotonic_ms":1,"emitted_unix_ms":1001,"capture":null,"event":"future_event","payload":{"additive":true}});
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
    dp["state"]["result"]["play_mode"] = json!("dp");
    dp["state"]["result"]["play_side"] = json!("two_player");
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
