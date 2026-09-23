//! Public snapshot/live fold. Recognition and score-writing authority remain upstream.
use crate::{Chart, History, LampState, OverlayState, ScreenKind};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct PublicEnvelope {
    schema: String,
    invocation_id: String,
    sequence: u64,
    event_id: String,
    emitted_monotonic_ms: u64,
    emitted_unix_ms: i64,
    capture: Value,
    event: String,
}

#[derive(Default)]
pub struct Consumer {
    pub view: OverlayState,
    pub query_revision: u64,
    invocation: String,
    next_sequence: Option<u64>,
    selection_record: Option<Value>,
    session_id: Option<String>,
}

impl Consumer {
    /// Applies one validated snapshot or live record to the display projection.
    /// # Errors
    /// Returns an envelope, invocation, sequence, or known-event field error.
    pub fn apply(&mut self, record: &Value, expected_invocation: &str) -> Result<(), String> {
        let invocation = text(record, "invocation_id")?;
        if invocation != expected_invocation {
            return Err("overlay invocation mismatch".into());
        }
        match text(record, "schema")? {
            "scorepeek-event-snapshot-v5" => {
                validate_status(&record["status"])?;
                if record.get("result").is_none_or(Value::is_null) {
                    return Err("missing snapshot result state".into());
                }
                let snapshot_next_sequence = number(record, "next_sequence")?;
                let mut replacement = Self {
                    invocation: invocation.into(),
                    next_sequence: Some(snapshot_next_sequence),
                    ..Self::default()
                };
                let active = record["status"]["watcher"] == "session_active";
                replacement.apply_status(&record["status"]);
                let mut slots: Vec<_> = [
                    "result",
                    "screen_state",
                    "music_selection",
                    "music_select_best",
                ]
                .into_iter()
                .filter_map(|key| record.get(key).filter(|v| !v.is_null()))
                .collect();
                slots.sort_by_key(|v| v["sequence"].as_u64());
                for (key, expected_event) in [
                    ("result", "result_changed"),
                    ("screen_state", "screen_state_changed"),
                    ("music_selection", "music_selection_changed"),
                    ("music_select_best", "music_select_best_observed"),
                ] {
                    let Some(slot) = record.get(key).filter(|value| !value.is_null()) else {
                        continue;
                    };
                    if slot["event"].as_str() != Some(expected_event)
                        || slot["sequence"]
                            .as_u64()
                            .is_none_or(|sequence| sequence >= snapshot_next_sequence)
                    {
                        return Err("invalid snapshot retained slot".into());
                    }
                    validate_public_record(slot, expected_invocation)?;
                }
                for slot in slots {
                    replacement.event(slot)?;
                }
                if active
                    && replacement.view.chart.is_none()
                    && self.invocation == invocation
                    && self.selection_record.as_ref() == record.get("music_selection")
                {
                    replacement.view.chart.clone_from(&self.view.chart);
                    replacement.view.history.clone_from(&self.view.history);
                }
                if !active {
                    replacement.view.chart = None;
                    replacement.view.history = History::default();
                }
                replacement.view.connected = true;
                *self = replacement;
            }
            "scorepeek-event-v5" => {
                validate_public_record(record, expected_invocation)?;
                let sequence = number(record, "sequence")?;
                if !self.view.connected
                    || self.invocation != invocation
                    || self.next_sequence != Some(sequence)
                {
                    return Err("overlay public sequence gap".into());
                }
                self.event(record)?;
                self.next_sequence = sequence.checked_add(1);
                self.view.connected = true;
            }
            _ => return Err("unsupported overlay event schema".into()),
        }
        Ok(())
    }

    fn event(&mut self, record: &Value) -> Result<(), String> {
        match text(record, "event")? {
            "screen_state_changed" => {
                self.view.screen.revision = self.view.screen.revision.saturating_add(1);
                let Some(state) = record.get("state").filter(|value| !value.is_null()) else {
                    self.view.screen.kind = None;
                    self.view.screen.suspended_since_unix_ms = None;
                    return Ok(());
                };
                let screen = screen_kind(text(state, "screen")?)?;
                self.view.screen.kind = Some(screen);
                self.view.screen.suspended_since_unix_ms = state["suspended"]
                    .as_bool()
                    .ok_or("missing screen suspended flag")?
                    .then(|| record["emitted_unix_ms"].as_i64())
                    .flatten();
            }
            "music_selection_changed" => {
                self.selection_record = Some(record.clone());
                let state = &record["state"];
                match text(state, "status")? {
                    "selected" => {
                        let next = chart(state, &state["presentation"])?;
                        if self.view.chart.as_ref() != Some(&next) {
                            self.view.history = History::default();
                        }
                        self.view.chart = Some(next);
                    }
                    "unresolved" => {}
                    _ => return Err("unsupported selection status".into()),
                }
            }
            "result_changed" => {
                self.view.result_signal = match text(&record["state"], "status")? {
                    "inactive" => LampState::Inactive,
                    "provisional" | "confirmed" => LampState::Active,
                    "retracted" => LampState::Error,
                    _ => return Err("unsupported result state".into()),
                };
            }
            "score_store_changed" => {
                let _ = number(record, "revision")?;
                let chart = &record["chart"];
                if self.view.chart.as_ref().is_some_and(|current| {
                    chart["scorepeek_song_id"].as_str() == Some(current.song_id.as_str())
                        && chart["play_type"].as_str() == Some(current.play_type.as_str())
                        && chart["difficulty"].as_str() == Some(current.difficulty.as_str())
                }) {
                    self.query_revision = self.query_revision.saturating_add(1);
                }
            }
            "status_changed" => {
                self.apply_status(&record["status"]);
                if matches!(
                    record["status"]["watcher"].as_str(),
                    Some("session_finished" | "stopped")
                ) {
                    self.view.chart = None;
                    self.view.history = History::default();
                    self.view.screen.kind = None;
                    self.view.screen.suspended_since_unix_ms = None;
                    self.view.screen.revision = self.view.screen.revision.saturating_add(1);
                }
            }
            // Additive v2 events are intentionally skippable after envelope validation.
            _ => {}
        }
        Ok(())
    }
    fn apply_status(&mut self, status: &Value) {
        let session_id = status["capture"]["session_id"].as_str().map(str::to_owned);
        self.session_id = session_id;
        let dependencies_ready = ["catalog", "model"]
            .into_iter()
            .all(|key| status[key] == "ready")
            && ["scores", "recording"]
                .into_iter()
                .all(|key| status[key].is_null() || status[key] == "ready");
        self.view.system = match status["watcher"].as_str() {
            Some("session_active") if dependencies_ready => LampState::Active,
            Some("starting" | "waiting_for_source" | "session_finished" | "stopped") => {
                LampState::Inactive
            }
            _ => LampState::Error,
        };
    }

    pub fn disconnect(&mut self, now_unix_ms: i64) {
        self.view.connected = false;
        if self.view.screen.kind.is_some() && self.view.screen.suspended_since_unix_ms.is_none() {
            self.view.screen.suspended_since_unix_ms = Some(now_unix_ms);
            self.view.screen.revision = self.view.screen.revision.saturating_add(1);
        }
    }

    pub fn expire_screen(&mut self, now_unix_ms: i64, grace_ms: u32) {
        if self
            .view
            .screen
            .suspended_since_unix_ms
            .is_some_and(|started| now_unix_ms.saturating_sub(started) >= i64::from(grace_ms))
        {
            self.view.screen.kind = None;
            self.view.screen.suspended_since_unix_ms = None;
            self.view.screen.revision = self.view.screen.revision.saturating_add(1);
        }
    }
}

fn validate_public_record(record: &Value, expected_invocation: &str) -> Result<(), String> {
    let envelope: PublicEnvelope =
        serde_json::from_value(record.clone()).map_err(|_| "invalid public event envelope")?;
    if envelope.schema != "scorepeek-event-v5" || envelope.invocation_id != expected_invocation {
        return Err("invalid public event envelope".into());
    }
    let _ = (
        envelope.sequence,
        envelope.emitted_monotonic_ms,
        envelope.emitted_unix_ms,
    );
    if envelope.event_id.is_empty() {
        return Err("invalid public event id".into());
    }
    validate_capture(&envelope.capture)?;
    match envelope.event.as_str() {
        "screen_state_changed" => validate_screen(record),
        "result_changed" => validate_result(record),
        "music_selection_changed" => validate_selection(record),
        "music_select_best_observed" => validate_select_best(record),
        "status_changed" => validate_status(&record["status"]),
        "score_store_changed" => {
            number(record, "revision")?;
            validate_chart(&record["chart"])
        }
        _ => Ok(()),
    }
}

fn validate_capture(capture: &Value) -> Result<(), String> {
    if capture.is_null() {
        return Ok(());
    }
    let object = capture.as_object().ok_or("invalid capture context")?;
    if object.len() != 1
        || object
            .get("session_id")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err("invalid capture session".into());
    }
    Ok(())
}

fn validate_screen(record: &Value) -> Result<(), String> {
    let state = record.get("state").ok_or("missing screen state")?;
    if state.is_null() {
        return Ok(());
    }
    number(state, "screen_episode_id")?;
    screen_kind(text(state, "screen")?)?;
    state["suspended"]
        .as_bool()
        .ok_or_else(|| "missing screen suspended flag".to_owned())?;
    Ok(())
}

fn validate_result(record: &Value) -> Result<(), String> {
    number(record, "source_sequence")?;
    let state = &record["state"];
    match text(state, "status")? {
        "inactive" => Ok(()),
        "provisional" | "confirmed" => validate_result_identity(record, state),
        "retracted" => {
            match text(state, "reason")? {
                "evidence_unresolved"
                | "panel_side_conflict"
                | "attempt_rejected"
                | "session_ended" => {}
                _ => return Err("unsupported result retraction reason".into()),
            }
            validate_result_identity(record, state)
        }
        _ => Err("unsupported result state".into()),
    }
}

fn validate_result_identity(record: &Value, state: &Value) -> Result<(), String> {
    let _session_id = record["capture"]["session_id"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or("missing result capture session")?;
    let song_id = validate_song(state.get("song").ok_or("missing result song")?)?;
    let result_id = validate_result_payload(&state["result"])?;
    if song_id != result_id {
        return Err("result song identity mismatch".into());
    }
    Ok(())
}

fn validate_song(song: &Value) -> Result<&str, String> {
    if song.is_null() {
        return Err("invalid result song".into());
    }
    let song_id = text(song, "scorepeek_song_id")?;
    text(song, "artist")?;
    if !song["display_titles"].as_array().is_some_and(|titles| {
        !titles.is_empty()
            && titles
                .iter()
                .all(|title| title.as_str().is_some_and(|value| !value.is_empty()))
    }) {
        return Err("invalid result song titles".into());
    }
    Ok(song_id)
}

fn validate_result_payload(result: &Value) -> Result<&str, String> {
    if text(result, "contract")? != "scorepeek-result-detected-v4" {
        return Err("unsupported result payload".into());
    }
    for key in ["attempt_id", "level", "notes", "current_score"] {
        number(result, key)?;
    }
    let song_id = text(result, "scorepeek_song_id")?;
    text(result, "play_type")?;
    validate_play_side(&result["play_side"])?;
    for key in ["play_mode", "difficulty", "clear_type"] {
        text(result, key)?;
    }
    for (object, fields) in [
        (
            &result["judgments"],
            &["pgreat", "great", "good", "bad", "poor"][..],
        ),
        (&result["timing"], &["fast", "slow"] as &[&str]),
        (
            &result["previous_best"],
            &["score", "miss_count", "clear_type"] as &[&str],
        ),
    ] {
        let object = object.as_object().ok_or("invalid result object")?;
        if fields.iter().any(|field| !object.contains_key(*field)) {
            return Err("incomplete result payload".into());
        }
    }
    for key in ["pgreat", "great", "good", "bad", "poor"] {
        number(&result["judgments"], key)?;
    }
    validate_supplemental(&result["miss_count"], false, false)?;
    validate_supplemental(&result["timing"]["fast"], false, false)?;
    validate_supplemental(&result["timing"]["slow"], false, false)?;
    validate_supplemental(&result["combo_break"], false, false)?;
    validate_supplemental(&result["previous_best"]["score"], true, false)?;
    validate_supplemental(&result["previous_best"]["miss_count"], true, false)?;
    validate_supplemental(&result["previous_best"]["clear_type"], true, true)?;
    match text(&result["play_options"], "status")? {
        "known" => {
            if !result["play_options"]["values"]
                .as_array()
                .is_some_and(|values| values.iter().all(Value::is_string))
            {
                return Err("invalid play options".into());
            }
        }
        "unknown" => {
            text(&result["play_options"], "reason")?;
        }
        _ => return Err("invalid play options".into()),
    }
    Ok(song_id)
}

fn validate_supplemental(value: &Value, previous: bool, known_string: bool) -> Result<(), String> {
    match text(value, "status")? {
        "known" => {
            if (known_string && !value["value"].is_string())
                || (!known_string && !value["value"].is_u64())
            {
                return Err("invalid known result field".into());
            }
        }
        "unknown" => {
            text(value, "reason")?;
        }
        "not_displayed" => {}
        "not_played" if previous => {}
        _ => return Err("invalid result field status".into()),
    }
    Ok(())
}

fn validate_play_side(value: &Value) -> Result<(), String> {
    match value.as_str() {
        Some("one_player" | "two_player") => Ok(()),
        _ => Err("invalid play side".into()),
    }
}

fn validate_selection(record: &Value) -> Result<(), String> {
    for key in ["screen_episode_id", "source_sequence", "revision"] {
        number(record, key)?;
    }
    let state = &record["state"];
    match text(state, "status")? {
        "selected" => {
            validate_chart(state)?;
            validate_play_side(&state["play_side"])?;
            if !state["presentation"].is_object() {
                return Err("missing selection presentation".into());
            }
            Ok(())
        }
        "unresolved" => text(state, "reason").map(|_| ()),
        _ => Err("unsupported selection status".into()),
    }
}

fn validate_select_best(record: &Value) -> Result<(), String> {
    let snapshot = record
        .get("snapshot")
        .ok_or("missing music select best snapshot")?;
    if snapshot.is_null() {
        return Ok(());
    }
    if text(snapshot, "contract")? != "scorepeek-music-select-best-snapshot-v4" {
        return Err("unsupported music select best snapshot".into());
    }
    number(snapshot, "revision")?;
    text(snapshot, "observation_id")?;
    validate_chart(&snapshot["chart"])?;
    validate_play_side(&snapshot["chart"]["play_side"])?;
    for key in ["score", "miss_count", "clear_type"] {
        if !snapshot["values"].get(key).is_some_and(Value::is_object) {
            return Err("incomplete music select best snapshot".into());
        }
    }
    Ok(())
}

fn validate_status(status: &Value) -> Result<(), String> {
    let object = status.as_object().ok_or("invalid status payload")?;
    for key in [
        "watcher",
        "capture",
        "catalog",
        "model",
        "scores",
        "recording",
        "last_session_outcome",
    ] {
        if !object.contains_key(key) {
            return Err("incomplete status payload".into());
        }
    }
    if !status["capture"].is_null() {
        validate_capture(&status["capture"])?;
    }
    match text(status, "watcher")? {
        "starting"
        | "waiting_for_source"
        | "ambiguous_sources"
        | "remote_unavailable"
        | "catalog_unavailable"
        | "admission_rejected"
        | "session_active"
        | "session_finished"
        | "stopped" => {}
        _ => return Err("invalid watcher status".into()),
    }
    for key in ["catalog", "model"] {
        if !matches!(
            status[key].as_str(),
            Some("not_ready" | "ready" | "unavailable")
        ) {
            return Err("invalid readiness".into());
        }
    }
    for key in ["scores", "recording"] {
        if !status[key].is_null()
            && !matches!(
                status[key].as_str(),
                Some("not_ready" | "ready" | "unavailable")
            )
        {
            return Err("invalid optional readiness".into());
        }
    }
    if !status["last_session_outcome"].is_null()
        && !matches!(
            status["last_session_outcome"].as_str(),
            Some("stopped" | "source_ended" | "error")
        )
    {
        return Err("invalid session outcome".into());
    }
    Ok(())
}

fn validate_chart(chart: &Value) -> Result<(), String> {
    for key in ["scorepeek_song_id", "play_type", "difficulty"] {
        text(chart, key)?;
    }
    Ok(())
}

fn screen_kind(value: &str) -> Result<ScreenKind, String> {
    match value {
        "music_select" => Ok(ScreenKind::MusicSelect),
        "mode_select" => Ok(ScreenKind::ModeSelect),
        "decide_transition" => Ok(ScreenKind::DecideTransition),
        "play" => Ok(ScreenKind::Play),
        "result" => Ok(ScreenKind::Result),
        _ => Err(format!("unsupported semantic screen: {value}")),
    }
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v[key]
        .as_str()
        .ok_or_else(|| format!("missing event field: {key}"))
}
fn number(v: &Value, key: &str) -> Result<u64, String> {
    v[key]
        .as_u64()
        .ok_or_else(|| format!("missing event integer: {key}"))
}
fn chart(value: &Value, presentation: &Value) -> Result<Chart, String> {
    Ok(Chart {
        song_id: text(value, "scorepeek_song_id")?.into(),
        play_type: text(value, "play_type")?.into(),
        difficulty: text(value, "difficulty")?.into(),
        title: presentation["display_titles"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        artist: presentation["artist"].as_str().unwrap_or_default().into(),
        level: value["level"].as_u64().and_then(|v| u32::try_from(v).ok()),
        notes: value["notes"].as_u64().and_then(|v| u32::try_from(v).ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn status() -> Value {
        json!({"watcher":"session_active","capture":{"session_id":"session"},"catalog":"ready","model":"ready","scores":"ready","recording":null,"last_session_outcome":null})
    }

    fn result_payload() -> Value {
        json!({"contract":"scorepeek-result-detected-v4","attempt_id":1,"scorepeek_song_id":"song","play_side":"one_player","play_mode":"sp","play_type":"single","difficulty":"hyper","level":10,"notes":1000,"current_score":100,"clear_type":"CLEAR","judgments":{"pgreat":1,"great":2,"good":3,"bad":4,"poor":5},"miss_count":{"status":"known","value":9},"timing":{"fast":{"status":"known","value":4},"slow":{"status":"known","value":5}},"combo_break":{"status":"known","value":6},"previous_best":{"score":{"status":"known","value":90},"miss_count":{"status":"known","value":10},"clear_type":{"status":"known","value":"FAILED"}},"play_options":{"status":"known","values":[]}})
    }

    #[test]
    fn play_side_is_a_plain_enum_for_both_play_types() {
        assert!(validate_play_side(&json!("one_player")).is_ok());
        assert!(validate_play_side(&json!("two_player")).is_ok());
        assert!(validate_play_side(&json!({"status":"not_applicable"})).is_err());
    }

    fn result_state(state: &str) -> Value {
        match state {
            "inactive" => json!({"status":"inactive"}),
            "retracted" => {
                json!({"status":"retracted","song":{"scorepeek_song_id":"song","display_titles":["Synthetic song"],"artist":"Synthetic artist"},"result":result_payload(),"reason":"evidence_unresolved"})
            }
            _ => {
                json!({"status":state,"song":{"scorepeek_song_id":"song","display_titles":["Synthetic song"],"artist":"Synthetic artist"},"result":result_payload()})
            }
        }
    }

    fn wire(sequence: u64, event: &Value) -> Value {
        let mut record = json!({"schema":"scorepeek-event-v5","invocation_id":"a","sequence":sequence,"event_id":format!("a:{sequence}"),"emitted_monotonic_ms":sequence,"emitted_unix_ms":1000+i64::try_from(sequence).unwrap(),"capture":{"session_id":"session"}});
        record
            .as_object_mut()
            .unwrap()
            .extend(event.as_object().unwrap().clone());
        record
    }

    fn snapshot(next_sequence: u64, result: &Value, screen: Option<&Value>) -> Value {
        let result = json!({"event":"result_changed","source_sequence":0,"state":result});
        json!({"schema":"scorepeek-event-snapshot-v5","invocation_id":"a","next_sequence":next_sequence,"status":status(),"result":wire(0,&result),"screen_state":screen,"music_selection":null,"music_select_best":null})
    }

    #[test]
    fn unknown_v5_event_advances_sequence() {
        let mut c = Consumer::default();
        c.apply(&snapshot(2, &result_state("inactive"), None), "a")
            .unwrap();
        c.apply(&wire(2, &json!({"event":"future_event"})), "a")
            .unwrap();
        assert!(c.view.connected);
    }

    #[test]
    fn malformed_envelope_and_known_payload_do_not_advance_state() {
        let mut missing_result = snapshot(2, &result_state("inactive"), None);
        missing_result["result"] = Value::Null;
        assert!(Consumer::default().apply(&missing_result, "a").is_err());
        let wrong_slot = wire(1, &json!({"event":"future_event"}));
        assert!(
            Consumer::default()
                .apply(
                    &snapshot(2, &result_state("inactive"), Some(&wrong_slot)),
                    "a"
                )
                .is_err()
        );

        let mut c = Consumer::default();
        c.apply(&snapshot(2, &result_state("inactive"), None), "a")
            .unwrap();
        let mut missing_id = wire(2, &json!({"event":"future_event"}));
        missing_id.as_object_mut().unwrap().remove("event_id");
        assert!(c.apply(&missing_id, "a").is_err());
        let incomplete = wire(
            2,
            &json!({"event":"result_changed","source_sequence":2,"state":{"status":"provisional"}}),
        );
        assert!(c.apply(&incomplete, "a").is_err());
        let mut wrong_nested = wire(
            2,
            &json!({"event":"result_changed","source_sequence":2,"state":result_state("provisional")}),
        );
        wrong_nested["state"]["result"]["judgments"]["pgreat"] = json!("1");
        assert!(c.apply(&wrong_nested, "a").is_err());
        let mut missing_song = wire(
            2,
            &json!({"event":"result_changed","source_sequence":2,"state":result_state("provisional")}),
        );
        missing_song["state"]
            .as_object_mut()
            .unwrap()
            .remove("song");
        assert!(c.apply(&missing_song, "a").is_err());
        let mut mismatched_song = wire(
            2,
            &json!({"event":"result_changed","source_sequence":2,"state":result_state("provisional")}),
        );
        mismatched_song["state"]["song"]["scorepeek_song_id"] = json!("other-song");
        assert!(c.apply(&mismatched_song, "a").is_err());
        let mut empty_title = wire(
            2,
            &json!({"event":"result_changed","source_sequence":2,"state":result_state("provisional")}),
        );
        empty_title["state"]["song"]["display_titles"] = json!([""]);
        assert!(c.apply(&empty_title, "a").is_err());
        let mut missing_capture = wire(
            2,
            &json!({"event":"result_changed","source_sequence":2,"state":result_state("provisional")}),
        );
        missing_capture["capture"] = Value::Null;
        assert!(c.apply(&missing_capture, "a").is_err());
        assert_eq!(c.view.result_signal, LampState::Inactive);
        c.apply(&wire(2, &json!({"event":"future_event"})), "a")
            .unwrap();
    }
    #[test]
    fn selection_carries_chart_attributes() {
        let mut c = Consumer::default();
        c.event(&json!({"event":"music_selection_changed","state":{"status":"selected","scorepeek_song_id":"s","play_type":"single","difficulty":"hyper","level":12,"notes":1877,"presentation":{"display_titles":["T"],"artist":"A"}}})).unwrap();
        let v = c.view.chart.unwrap();
        assert_eq!((v.level, v.notes), (Some(12), Some(1877)));
    }

    #[test]
    fn ended_selection_retains_chart_until_another_selection() {
        let mut c = Consumer::default();
        c.event(&json!({"event":"music_selection_changed","state":{"status":"selected","scorepeek_song_id":"s","play_type":"single","difficulty":"hyper","presentation":{"display_titles":["T"]}}})).unwrap();
        c.event(&json!({"event":"music_selection_changed","state":{"status":"unresolved","reason":"episode_ended"}})).unwrap();
        assert_eq!(
            c.view.chart.as_ref().map(|chart| chart.song_id.as_str()),
            Some("s")
        );
    }

    #[test]
    fn only_matching_store_commits_trigger_a_readback() {
        let mut c = Consumer::default();
        c.event(&json!({"event":"music_selection_changed","state":{"status":"selected","scorepeek_song_id":"s","play_type":"single","difficulty":"hyper","presentation":{"display_titles":["T"]}}})).unwrap();
        c.event(&json!({"event":"music_select_best_observed","snapshot":null}))
            .unwrap();
        c.event(&json!({"event":"score_store_changed","revision":1,"chart":{"scorepeek_song_id":"other","play_type":"single","difficulty":"hyper"}})).unwrap();
        assert_eq!(c.query_revision, 0);
        c.event(&json!({"event":"score_store_changed","revision":2,"chart":{"scorepeek_song_id":"s","play_type":"single","difficulty":"hyper"}})).unwrap();
        assert_eq!(c.query_revision, 1);
    }

    #[test]
    fn result_signal_tracks_only_explicit_result_states() {
        let mut c = Consumer::default();
        c.apply_status(&json!({"watcher":"session_active","capture":{"session_id":"session"},"catalog":"ready","model":"ready","scores":"ready","recording":null}));
        c.event(&json!({"event":"screen_state_changed","emitted_unix_ms":100,"state":{"screen_episode_id":7,"screen":"result","suspended":false}})).unwrap();
        assert_eq!(c.view.result_signal, LampState::Inactive);
        c.event(&json!({"event":"result_changed","state":{"status":"provisional"}}))
            .unwrap();
        assert_eq!(c.view.result_signal, LampState::Active);
        c.event(&json!({"event":"result_changed","state":{"status":"retracted"}}))
            .unwrap();
        assert_eq!(c.view.result_signal, LampState::Error);
        c.event(&json!({"event":"result_changed","state":{"status":"confirmed"}}))
            .unwrap();
        c.event(&json!({"event":"screen_state_changed","emitted_unix_ms":200,"state":null}))
            .unwrap();
        assert_eq!(c.view.result_signal, LampState::Active);
        c.event(&json!({"event":"screen_state_changed","emitted_unix_ms":300,"state":{"screen_episode_id":8,"screen":"result","suspended":false}})).unwrap();
        assert_eq!(c.view.result_signal, LampState::Active);
        c.event(&json!({"event":"screen_state_changed","emitted_unix_ms":400,"state":null}))
            .unwrap();
        assert_eq!(c.view.result_signal, LampState::Active);
        c.event(&json!({"event":"status_changed","status":{"watcher":"session_finished","capture":null}})).unwrap();
        assert_eq!(c.view.result_signal, LampState::Active);
    }

    #[test]
    fn reconnect_restores_the_explicit_result_state() {
        let mut c = Consumer::default();
        c.apply(&snapshot(4, &result_state("provisional"), None), "a")
            .unwrap();
        assert_eq!(c.view.result_signal, LampState::Active);
    }

    #[test]
    fn first_snapshot_restores_a_retracted_result() {
        let mut c = Consumer::default();
        let screen = wire(
            2,
            &json!({"event":"screen_state_changed","state":{"screen_episode_id":7,"screen":"result","suspended":false}}),
        );
        c.apply(&snapshot(4, &result_state("retracted"), Some(&screen)), "a")
            .unwrap();
        assert_eq!(c.view.result_signal, LampState::Error);
        c.event(&json!({
            "event":"screen_state_changed",
            "emitted_unix_ms":200,
            "state":null
        }))
        .unwrap();
        assert_eq!(c.view.result_signal, LampState::Error);
    }

    #[test]
    fn suspended_and_disconnected_screens_expire_after_the_configured_grace() {
        let mut c = Consumer::default();
        c.event(&json!({"event":"screen_state_changed","emitted_unix_ms":100,"state":{"screen_episode_id":1,"screen":"music_select","suspended":false}})).unwrap();
        assert_eq!(c.view.screen.kind, Some(ScreenKind::MusicSelect));
        c.event(&json!({"event":"screen_state_changed","emitted_unix_ms":200,"state":{"screen_episode_id":1,"screen":"music_select","suspended":true}})).unwrap();
        c.expire_screen(1_199, 1_000);
        assert_eq!(c.view.screen.kind, Some(ScreenKind::MusicSelect));
        c.expire_screen(1_200, 1_000);
        assert_eq!(c.view.screen.kind, None);

        c.event(&json!({"event":"screen_state_changed","emitted_unix_ms":300,"state":{"screen_episode_id":2,"screen":"play","suspended":false}})).unwrap();
        c.disconnect(400);
        c.expire_screen(1_400, 1_000);
        assert_eq!(c.view.screen.kind, None);
    }

    #[test]
    fn a_new_known_screen_replaces_a_suspended_screen_without_waiting() {
        let mut c = Consumer::default();
        c.event(&json!({"event":"screen_state_changed","emitted_unix_ms":100,"state":{"screen_episode_id":1,"screen":"music_select","suspended":true}})).unwrap();
        c.event(&json!({"event":"screen_state_changed","emitted_unix_ms":101,"state":{"screen_episode_id":2,"screen":"result","suspended":false}})).unwrap();
        assert_eq!(c.view.screen.kind, Some(ScreenKind::Result));
        assert_eq!(c.view.screen.suspended_since_unix_ms, None);
    }
}
