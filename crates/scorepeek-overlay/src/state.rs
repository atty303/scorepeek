//! Public snapshot/live fold. Recognition and score-writing authority remain upstream.
use scorepeek_overlay_ui::{Chart, History, LampState, OverlayState, ScreenKind};
use serde_json::Value;

#[derive(Default)]
pub struct Consumer {
    pub view: OverlayState,
    pub query_revision: u64,
    invocation: String,
    next_sequence: Option<u64>,
    selection_record: Option<Value>,
    session_id: Option<String>,
    result_episode_id: Option<u64>,
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
            "scorepeek-event-snapshot-v1" => {
                let mut replacement = Self {
                    invocation: invocation.into(),
                    next_sequence: Some(number(record, "next_sequence")?),
                    ..Self::default()
                };
                let active = record["status"]["watcher"] == "session_active";
                let previous_session = self.session_id.clone();
                let previous_result = self.view.result_signal;
                let previous_episode = self.result_episode_id;
                replacement.apply_status(&record["status"]);
                let mut slots: Vec<_> = [
                    "latest_result",
                    "provisional_result",
                    "screen_state",
                    "music_selection",
                    "music_select_best",
                    "result_ingest",
                ]
                .into_iter()
                .filter_map(|key| record.get(key).filter(|v| !v.is_null()))
                .collect();
                slots.sort_by_key(|v| v["sequence"].as_u64());
                for slot in slots {
                    replacement.event(slot)?;
                }
                if active
                    && replacement.session_id == previous_session
                    && self.invocation == invocation
                {
                    replacement.retain_result_signal(previous_result, previous_episode);
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
            "scorepeek-event-v1" => {
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
                    self.finish_result_episode();
                    self.view.screen.kind = None;
                    self.view.screen.suspended_since_unix_ms = None;
                    return Ok(());
                };
                let screen = screen_kind(text(state, "screen")?)?;
                if screen == ScreenKind::Result {
                    let episode = number(state, "screen_episode_id")?;
                    if self.result_episode_id != Some(episode) {
                        self.result_episode_id = Some(episode);
                        self.view.result_signal = LampState::Inactive;
                    }
                } else {
                    self.finish_result_episode();
                }
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
            "result_ingest_changed" => {
                validate_result_ingest(record)?;
            }
            "result_provisional_changed" => {
                let episode = number(record, "screen_episode_id")?;
                if self.result_episode_id == Some(episode) {
                    self.view.result_signal = match text(&record["state"], "status")? {
                        "resolved" => LampState::Active,
                        "withdrawn" => LampState::Error,
                        _ => return Err("unsupported provisional result state".into()),
                    };
                }
            }
            "result_detected" => {
                if record["capture"]["session_id"].as_str() == self.session_id.as_deref() {
                    self.view.result_signal = LampState::Active;
                }
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
                    self.view.result_signal = LampState::Inactive;
                    self.result_episode_id = None;
                    self.view.chart = None;
                    self.view.history = History::default();
                    self.view.screen.kind = None;
                    self.view.screen.suspended_since_unix_ms = None;
                    self.view.screen.revision = self.view.screen.revision.saturating_add(1);
                }
            }
            // Additive v1 events are intentionally skippable after envelope validation.
            _ => {}
        }
        Ok(())
    }
    fn apply_status(&mut self, status: &Value) {
        let session_id = status["capture"]["session_id"].as_str().map(str::to_owned);
        if self.session_id.is_some() && self.session_id != session_id {
            self.view.result_signal = LampState::Inactive;
            self.result_episode_id = None;
        }
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

    fn finish_result_episode(&mut self) {
        if self.result_episode_id.take().is_some() && self.view.result_signal == LampState::Inactive
        {
            self.view.result_signal = LampState::Error;
        }
    }

    fn retain_result_signal(&mut self, previous: LampState, previous_episode: Option<u64>) {
        match (previous_episode, self.result_episode_id) {
            (Some(previous_episode), Some(current_episode))
                if previous_episode == current_episode =>
            {
                if self.view.result_signal == LampState::Inactive {
                    self.view.result_signal = previous;
                }
            }
            (Some(_), None) => {
                self.view.result_signal = if previous == LampState::Inactive {
                    LampState::Error
                } else {
                    previous
                };
            }
            (None, None) => self.view.result_signal = previous,
            _ => {}
        }
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

fn validate_result_ingest(record: &Value) -> Result<(), String> {
    let Some(ingest) = record.get("ingest").filter(|value| !value.is_null()) else {
        return Ok(());
    };
    match text(ingest, "state")? {
        "processing" | "persisted" | "failed" => Ok(()),
        _ => Err("unsupported result ingest state".into()),
    }
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
    #[test]
    fn unknown_v1_event_advances_sequence() {
        let mut c = Consumer::default();
        c.apply(&json!({"schema":"scorepeek-event-snapshot-v1","invocation_id":"a","next_sequence":2,"status":{"watcher":"session_active"}}),"a").unwrap();
        c.apply(&json!({"schema":"scorepeek-event-v1","invocation_id":"a","sequence":2,"event":"future_event"}),"a").unwrap();
        assert!(c.view.connected);
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
        c.event(&json!({"event":"result_ingest_changed","ingest":{"state":"persisted"}}))
            .unwrap();
        c.event(&json!({"event":"score_store_changed","revision":1,"chart":{"scorepeek_song_id":"other","play_type":"single","difficulty":"hyper"}})).unwrap();
        assert_eq!(c.query_revision, 0);
        c.event(&json!({"event":"score_store_changed","revision":2,"chart":{"scorepeek_song_id":"s","play_type":"single","difficulty":"hyper"}})).unwrap();
        assert_eq!(c.query_revision, 1);
    }

    #[test]
    fn result_signal_tracks_provisional_readiness_until_the_next_result() {
        let mut c = Consumer::default();
        c.apply_status(&json!({"watcher":"session_active","capture":{"session_id":"session"},"catalog":"ready","model":"ready","scores":"ready","recording":null}));
        c.event(&json!({"event":"screen_state_changed","emitted_unix_ms":100,"state":{"screen_episode_id":7,"screen":"result","suspended":false}})).unwrap();
        assert_eq!(c.view.result_signal, LampState::Inactive);
        c.event(&json!({"event":"result_provisional_changed","screen_episode_id":7,"state":{"status":"resolved"}})).unwrap();
        assert_eq!(c.view.result_signal, LampState::Active);
        c.event(&json!({"event":"result_provisional_changed","screen_episode_id":7,"state":{"status":"withdrawn"}})).unwrap();
        assert_eq!(c.view.result_signal, LampState::Error);
        c.event(&json!({"event":"result_provisional_changed","screen_episode_id":7,"state":{"status":"resolved"}})).unwrap();
        c.event(&json!({"event":"screen_state_changed","emitted_unix_ms":200,"state":null}))
            .unwrap();
        assert_eq!(c.view.result_signal, LampState::Active);
        c.event(&json!({"event":"screen_state_changed","emitted_unix_ms":300,"state":{"screen_episode_id":8,"screen":"result","suspended":false}})).unwrap();
        assert_eq!(c.view.result_signal, LampState::Inactive);
        c.event(&json!({"event":"screen_state_changed","emitted_unix_ms":400,"state":null}))
            .unwrap();
        assert_eq!(c.view.result_signal, LampState::Error);
        c.event(&json!({"event":"status_changed","status":{"watcher":"session_finished","capture":null}})).unwrap();
        assert_eq!(c.view.result_signal, LampState::Inactive);
    }

    #[test]
    fn same_session_reconnect_retains_result_signal_and_new_session_clears_it() {
        let status = json!({
            "watcher":"session_active",
            "capture":{"session_id":"session"},
            "catalog":"ready",
            "model":"ready",
            "scores":"ready",
            "recording":null
        });
        let screen = json!({
            "event":"screen_state_changed",
            "sequence":2,
            "emitted_unix_ms":100,
            "state":{"screen_episode_id":7,"screen":"result","suspended":false}
        });
        let mut c = Consumer::default();
        c.apply(
            &json!({
                "schema":"scorepeek-event-snapshot-v1",
                "invocation_id":"a",
                "next_sequence":3,
                "status":status,
                "screen_state":screen
            }),
            "a",
        )
        .unwrap();
        c.apply(
            &json!({
                "schema":"scorepeek-event-v1",
                "invocation_id":"a",
                "sequence":3,
                "event":"result_provisional_changed",
                "screen_episode_id":7,
                "state":{"status":"resolved"}
            }),
            "a",
        )
        .unwrap();
        c.disconnect(200);
        c.expire_screen(1_200, 1_000);
        assert_eq!(c.view.result_signal, LampState::Active);

        c.apply(
            &json!({
                "schema":"scorepeek-event-snapshot-v1",
                "invocation_id":"a",
                "next_sequence":4,
                "status":status,
                "screen_state":screen
            }),
            "a",
        )
        .unwrap();
        assert_eq!(c.view.result_signal, LampState::Active);

        c.apply(
            &json!({
                "schema":"scorepeek-event-snapshot-v1",
                "invocation_id":"a",
                "next_sequence":5,
                "status":{
                    "watcher":"session_active",
                    "capture":{"session_id":"next-session"},
                    "catalog":"ready",
                    "model":"ready",
                    "scores":"ready",
                    "recording":null
                }
            }),
            "a",
        )
        .unwrap();
        assert_eq!(c.view.result_signal, LampState::Inactive);
    }

    #[test]
    fn first_snapshot_restores_a_resolved_provisional_result() {
        let mut c = Consumer::default();
        c.apply(
            &json!({
                "schema":"scorepeek-event-snapshot-v1",
                "invocation_id":"a",
                "next_sequence":4,
                "status":{
                    "watcher":"session_active",
                    "capture":{"session_id":"session"},
                    "catalog":"ready",
                    "model":"ready",
                    "scores":"ready",
                    "recording":null
                },
                "screen_state":{
                    "event":"screen_state_changed",
                    "sequence":2,
                    "emitted_unix_ms":100,
                    "state":{"screen_episode_id":7,"screen":"result","suspended":false}
                },
                "provisional_result":{
                    "event":"result_provisional_changed",
                    "sequence":3,
                    "screen_episode_id":7,
                    "state":{"status":"resolved"}
                }
            }),
            "a",
        )
        .unwrap();
        assert_eq!(c.view.result_signal, LampState::Active);
        c.event(&json!({
            "event":"screen_state_changed",
            "emitted_unix_ms":200,
            "state":null
        }))
        .unwrap();
        assert_eq!(c.view.result_signal, LampState::Active);
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
