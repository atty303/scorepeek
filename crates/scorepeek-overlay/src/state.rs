//! Public snapshot/live fold. Recognition and score-writing authority remain upstream.
use scorepeek_overlay_ui::{Chart, History, LampState, OverlayState};
use serde_json::Value;

#[derive(Default)]
pub struct Consumer {
    pub view: OverlayState,
    pub query_revision: u64,
    invocation: String,
    next_sequence: Option<u64>,
    selection_record: Option<Value>,
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
                replacement.apply_status(&record["status"]);
                let mut slots: Vec<_> = ["music_selection", "music_select_best", "result_ingest"]
                    .into_iter()
                    .filter_map(|key| record.get(key).filter(|v| !v.is_null()))
                    .collect();
                slots.sort_by_key(|v| v["sequence"].as_u64());
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
                self.view.result_ingest = match record
                    .get("ingest")
                    .filter(|v| !v.is_null())
                    .and_then(|v| v["state"].as_str())
                {
                    None => LampState::Inactive,
                    Some("processing") => LampState::Processing,
                    Some("persisted") => LampState::Persisted,
                    Some("failed") => LampState::Failed,
                    _ => return Err("unsupported result ingest state".into()),
                };
                if self.view.result_ingest == LampState::Persisted {
                    self.query_revision = self.query_revision.saturating_add(1);
                }
            }
            "music_select_best_observed" => {
                self.query_revision = self.query_revision.saturating_add(1);
            }
            "status_changed" => {
                self.apply_status(&record["status"]);
                if matches!(
                    record["status"]["watcher"].as_str(),
                    Some("session_finished" | "stopped")
                ) {
                    self.view.chart = None;
                    self.view.history = History::default();
                }
            }
            // Additive v1 events are intentionally skippable after envelope validation.
            _ => {}
        }
        Ok(())
    }
    fn apply_status(&mut self, status: &Value) {
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
    fn committed_db_inputs_trigger_a_readback() {
        let mut c = Consumer::default();
        c.event(&json!({"event":"music_select_best_observed","snapshot":null}))
            .unwrap();
        c.event(&json!({"event":"result_ingest_changed","ingest":{"state":"persisted"}}))
            .unwrap();
        assert_eq!(c.query_revision, 2);
    }
}
