//! Public snapshot/live fold. No recognition or score-writing authority.
use scorepeek_overlay_ui::{
    Chart, Confirmation, Field, FieldRole, History, OverlayState, ResultView,
};
use serde_json::Value;

#[derive(Default)]
pub struct Consumer {
    pub view: OverlayState,
    invocation: String,
    next_sequence: Option<u64>,
    confirmation_key: Option<(Value, u64)>,
    confirmation_sequence: Option<u64>,
    selection_record: Option<Value>,
}

impl Consumer {
    /// Replaces state on snapshot; live records must follow its sequence boundary.
    /// # Errors
    /// Rejects malformed envelopes, unsupported schemas and sequence gaps.
    pub fn apply(&mut self, record: &Value, expected_invocation: &str) -> Result<(), String> {
        let invocation = text(record, "invocation_id")?;
        if invocation != expected_invocation {
            return Err("overlay invocation mismatch".into());
        }
        match text(record, "schema")? {
            "scorepeek-event-snapshot-v1" => {
                let next = number(record, "next_sequence")?;
                let mut replacement = Self {
                    invocation: invocation.into(),
                    next_sequence: Some(next),
                    ..Self::default()
                };
                let active = record["status"]["watcher"] == "session_active";
                let selection = &record["music_selection"];
                // An unresolved slot cannot reconstruct a selection missed during a gap.
                // Retain it only when this is the exact selection event already observed.
                if active
                    && self.invocation == invocation
                    && self.selection_record.as_ref() == Some(selection)
                    && selection["capture"] == record["status"]["capture"]
                {
                    replacement.view.chart.clone_from(&self.view.chart);
                    replacement.view.history.clone_from(&self.view.history);
                }
                let mut slots: Vec<_> = [
                    "latest_result",
                    "music_selection",
                    "music_select_best",
                    "provisional_result",
                ]
                .into_iter()
                .filter_map(|slot| record.get(slot).filter(|value| !value.is_null()))
                .collect();
                slots.sort_by_key(|value| value["sequence"].as_u64());
                for value in slots {
                    replacement.event(value)?;
                    if value["event"] == "result_detected"
                        && (!active || value["capture"] != record["status"]["capture"])
                    {
                        replacement.view.result = None;
                    }
                }
                if !active {
                    replacement.view.chart = None;
                    replacement.view.result = None;
                    replacement.view.selecting = false;
                    replacement.view.best_received = [false; 3];
                    replacement.view.history = History::default();
                } else if selection["state"]["status"] == "unresolved"
                    && selection["sequence"].as_u64() > record["latest_result"]["sequence"].as_u64()
                    && record["provisional_result"].is_null()
                {
                    replacement.view.result = None;
                }
                if replacement.view.chart == self.view.chart {
                    replacement.view.history.clone_from(&self.view.history);
                }
                // Withdrawal removes the producer's provisional slot, not the
                // latest play already observed by this consumer.
                if self.invocation == invocation
                    && self.confirmation_sequence > replacement.confirmation_sequence
                {
                    replacement
                        .view
                        .confirmation
                        .clone_from(&self.view.confirmation);
                    replacement
                        .confirmation_key
                        .clone_from(&self.confirmation_key);
                    replacement.confirmation_sequence = self.confirmation_sequence;
                    replacement.view.result = None;
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
                        let chart = chart(state, &state["presentation"])?;
                        if self.view.chart.as_ref() != Some(&chart) {
                            self.view.history = History::default();
                            self.view.best_received = [false; 3];
                        }
                        self.view.chart = Some(chart);
                        self.view.result = None;
                        self.view.selecting = false;
                    }
                    "unresolved" => {
                        self.view.selecting = state["reason"] == "evidence_unresolved";
                        if self.view.selecting {
                            self.view.result = None;
                        }
                    }
                    _ => return Err("unsupported selection status".into()),
                }
            }
            "music_select_best_observed" => {
                let snapshot = &record["snapshot"];
                self.view.best_received = [false; 3];
                if snapshot.is_null() {
                    return Ok(());
                }
                let snapshot_chart = &snapshot["chart"];
                if self.view.chart.as_ref().is_some_and(|chart| {
                    snapshot_chart["scorepeek_song_id"] == chart.song_id
                        && snapshot_chart["play_type"] == chart.play_type
                        && snapshot_chart["difficulty"] == chart.difficulty
                }) {
                    self.view.best_received = ["score", "miss_count", "clear_type"].map(|key| {
                        matches!(
                            snapshot["values"][key]["status"].as_str(),
                            Some("known" | "no_record")
                        )
                    });
                }
            }
            "result_provisional_changed" => {
                let state = &record["state"];
                match text(state, "status")? {
                    "resolved" => self.result(record, &state["result"], &state["song"], false)?,
                    "withdrawn" => self.view.result = None,
                    _ => return Err("unsupported provisional status".into()),
                }
            }
            "result_detected" => self.result(record, &record["result"], &record["song"], true)?,
            "status_changed" => {
                if matches!(
                    record["status"]["watcher"].as_str(),
                    Some("session_active" | "session_finished" | "stopped")
                ) {
                    self.view.chart = None;
                    self.view.result = None;
                    self.view.history = History::default();
                    self.view.selecting = false;
                    self.view.best_received = [false; 3];
                }
            }
            _ => return Err("unsupported public event".into()),
        }
        Ok(())
    }

    fn result(
        &mut self,
        envelope: &Value,
        result: &Value,
        song: &Value,
        confirmed: bool,
    ) -> Result<(), String> {
        if text(result, "contract")? != "scorepeek-result-detected-v2" {
            return Err("unsupported result contract".into());
        }
        let key = (envelope["capture"].clone(), number(result, "attempt_id")?);
        let title = title(song).unwrap_or_else(|| {
            result["scorepeek_song_id"]
                .as_str()
                .unwrap_or("結果")
                .into()
        });
        let mut fields = Vec::new();
        for (role, label, key) in [
            (FieldRole::Score, "EX SCORE", "current_score"),
            (FieldRole::Clear, "CLEAR", "clear_type"),
            (FieldRole::Miss, "MISS COUNT", "miss_count"),
        ] {
            if let Some(value) = display_value(&result[key]) {
                fields.push(Field {
                    role,
                    label: label.into(),
                    value,
                });
            }
        }
        for (label, key) in [
            ("判定", "judgments"),
            ("FAST / SLOW", "timing"),
            ("COMBO BREAK", "combo_break"),
            ("OPTIONS", "play_options"),
        ] {
            if let Some(value) = display_value(&result[key]) {
                fields.push(Field {
                    role: FieldRole::Detail,
                    label: label.into(),
                    value,
                });
            }
        }
        if let (Some(score), Some(notes)) =
            (result["current_score"].as_u64(), result["notes"].as_u64())
            && notes > 0
        {
            let rank = ["F", "E", "D", "C", "B", "A", "AA", "AAA"][usize::try_from(
                (score.saturating_mul(9) / (notes * 2))
                    .saturating_sub(1)
                    .min(7),
            )
            .unwrap_or(0)];
            fields.push(Field {
                role: FieldRole::Rank,
                label: "DJ RANK".into(),
                value: rank.into(),
            });
        }
        // A delayed confirmation must not replace a newer provisional result.
        if confirmed
            && self
                .confirmation_key
                .as_ref()
                .is_some_and(|current| current != &key)
            && self
                .view
                .confirmation
                .as_ref()
                .is_some_and(|current| !current.confirmed)
        {
            return Ok(());
        }
        let already_confirmed = self.confirmation_key.as_ref() == Some(&key)
            && self
                .view
                .confirmation
                .as_ref()
                .is_some_and(|current| current.confirmed);
        if !confirmed && already_confirmed {
            return Ok(());
        }
        if !confirmed || self.view.result.is_some() || self.confirmation_key.is_none() {
            self.view.result = Some(ResultView {
                title: title.clone(),
                artist: song["artist"].as_str().unwrap_or_default().into(),
                play_type: result["play_type"].as_str().unwrap_or_default().into(),
                difficulty: result["difficulty"].as_str().unwrap_or_default().into(),
                fields,
            });
            self.view.selecting = false;
        }
        self.view.confirmation = Some(Confirmation { title, confirmed });
        self.confirmation_key = Some(key);
        self.confirmation_sequence = envelope["sequence"].as_u64();
        Ok(())
    }
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("missing event field: {key}"))
}
fn number(value: &Value, key: &str) -> Result<u64, String> {
    value[key]
        .as_u64()
        .ok_or_else(|| format!("missing event integer: {key}"))
}
fn title(presentation: &Value) -> Option<String> {
    presentation["display_titles"]
        .as_array()?
        .first()?
        .as_str()
        .map(str::to_owned)
}
fn chart(value: &Value, presentation: &Value) -> Result<Chart, String> {
    Ok(Chart {
        song_id: text(value, "scorepeek_song_id")?.into(),
        play_type: text(value, "play_type")?.into(),
        difficulty: text(value, "difficulty")?.into(),
        title: title(presentation).unwrap_or_else(|| {
            value["scorepeek_song_id"]
                .as_str()
                .unwrap_or_default()
                .into()
        }),
        artist: presentation["artist"].as_str().unwrap_or_default().into(),
    })
}
fn display_value(value: &Value) -> Option<String> {
    match value {
        Value::Null | Value::Bool(_) => None,
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Array(values) => {
            let parts: Vec<_> = values.iter().filter_map(display_value).collect();
            (!parts.is_empty()).then(|| parts.join(" / "))
        }
        Value::Object(fields) => {
            if let Some(status) = fields.get("status") {
                return (status == "known")
                    .then(|| {
                        display_value(
                            fields
                                .get("value")
                                .or_else(|| fields.get("values"))
                                .unwrap_or(&Value::Null),
                        )
                    })
                    .flatten();
            }
            let parts: Vec<_> = fields
                .iter()
                .filter_map(|(key, value)| {
                    display_value(value).map(|value| format!("{key}: {value}"))
                })
                .collect();
            (!parts.is_empty()).then(|| parts.join(" / "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn selected(sequence: u64) -> Value {
        json!({"event":"music_selection_changed","sequence":sequence,"state":{"status":"selected","scorepeek_song_id":"song","play_type":"single","difficulty":"hyper","presentation":{"display_titles":["選曲"]}}})
    }

    fn result_event(attempt: u64, confirmed: bool) -> Value {
        let result = json!({"contract":"scorepeek-result-detected-v2","attempt_id":attempt,"scorepeek_song_id":"song","difficulty":"hyper","current_score":1200,"notes":1000,"miss_count":{"status":"unknown"},"play_options":{"status":"known","values":["random"]}});
        let mut event = json!({"capture":{"session_id":"s","capture_generation":1},"sequence":10});
        if confirmed {
            event["event"] = json!("result_detected");
            event["result"] = result;
        } else {
            event["event"] = json!("result_provisional_changed");
            event["state"] = json!({"status":"resolved","result":result});
        }
        event
    }

    #[test]
    fn provisional_confirmation_withdrawal_and_next_selection() {
        let mut consumer = Consumer::default();
        consumer.event(&selected(1)).unwrap();
        consumer.event(&result_event(1, false)).unwrap();
        assert!(!consumer.view.confirmation.as_ref().unwrap().confirmed);
        let fields = &consumer.view.result.as_ref().unwrap().fields;
        assert!(!fields.iter().any(|field| field.role == FieldRole::Miss));
        assert!(
            fields
                .iter()
                .any(|field| field.label == "OPTIONS" && field.value == "random")
        );
        consumer.event(&result_event(1, true)).unwrap();
        consumer.event(&selected(20)).unwrap();
        assert!(consumer.view.result.is_none());
        assert!(consumer.view.confirmation.as_ref().unwrap().confirmed);
        consumer.event(&result_event(2, false)).unwrap();
        consumer.event(&result_event(1, true)).unwrap();
        assert!(!consumer.view.confirmation.as_ref().unwrap().confirmed);
        consumer
            .event(&json!({"event":"result_provisional_changed","state":{"status":"withdrawn"}}))
            .unwrap();
        assert!(consumer.view.result.is_none());
        assert!(consumer.view.chart.is_some());
    }

    #[test]
    fn best_receipt_is_not_a_score_and_clear_removes_indicators() {
        let mut consumer = Consumer::default();
        consumer.event(&selected(1)).unwrap();
        consumer.event(&json!({"event":"music_select_best_observed","snapshot":{"chart":{"scorepeek_song_id":"song","play_type":"single","difficulty":"hyper"},"values":{"score":{"status":"no_record"},"miss_count":{"status":"unknown"},"clear_type":{"status":"known","value":"hard_clear"}}}})).unwrap();
        assert_eq!(consumer.view.best_received, [true, false, true]);
        consumer
            .event(&json!({"event":"music_select_best_observed","snapshot":null}))
            .unwrap();
        assert_eq!(consumer.view.best_received, [false; 3]);
    }

    #[test]
    fn retained_snapshot_follows_original_sequence_order() {
        let mut consumer = Consumer::default();
        consumer.apply(&json!({"schema":"scorepeek-event-snapshot-v1","invocation_id":"a","next_sequence":11,"status":{"watcher":"session_active","capture":result_event(1,true)["capture"]},"latest_result":result_event(1,true),"music_selection":selected(1)}), "a").unwrap();
        assert!(consumer.view.result.is_some());
        assert!(consumer.view.confirmation.unwrap().confirmed);
    }

    #[test]
    fn reconnect_after_session_end_retains_confirmation_not_live_result() {
        let mut consumer = Consumer::default();
        consumer.event(&result_event(1, true)).unwrap();
        consumer
            .event(&json!({"event":"status_changed","status":{"watcher":"session_finished"}}))
            .unwrap();
        consumer.apply(&json!({"schema":"scorepeek-event-snapshot-v1","invocation_id":"a","next_sequence":12,"status":{"watcher":"session_finished","capture":null},"latest_result":result_event(1,true)}), "a").unwrap();
        assert!(consumer.view.result.is_none());
        assert!(consumer.view.chart.is_none());
        assert!(consumer.view.confirmation.as_ref().unwrap().confirmed);
    }

    #[test]
    fn reconnect_after_withdrawal_does_not_roll_back_latest_play() {
        for previous in [Value::Null, result_event(1, true)] {
            let mut consumer = Consumer::default();
            let mut snapshot = json!({"schema":"scorepeek-event-snapshot-v1","invocation_id":"a","next_sequence":20,"status":{"watcher":"session_active","capture":result_event(1,true)["capture"]},"latest_result":previous});
            consumer.apply(&snapshot, "a").unwrap();
            let mut pending = result_event(2, false);
            pending["sequence"] = json!(20);
            consumer.event(&pending).unwrap();
            consumer.event(&json!({"event":"result_provisional_changed","sequence":21,"state":{"status":"withdrawn"}})).unwrap();
            let expected = consumer.view.confirmation.clone();
            let expected_key = consumer.confirmation_key.clone();
            snapshot["next_sequence"] = json!(22);
            consumer.apply(&snapshot, "a").unwrap();
            assert_eq!(consumer.view.confirmation, expected);
            assert_eq!(consumer.confirmation_key, expected_key);
            assert!(consumer.view.result.is_none());
            // A later retained confirmation of this attempt still advances state.
            let mut confirmed = result_event(2, true);
            confirmed["sequence"] = json!(23);
            snapshot["latest_result"] = confirmed;
            snapshot["next_sequence"] = json!(24);
            consumer.apply(&snapshot, "a").unwrap();
            assert!(consumer.view.confirmation.as_ref().unwrap().confirmed);
            assert_eq!(consumer.confirmation_sequence, Some(23));
        }
    }

    #[test]
    fn reconnect_in_play_retains_only_an_already_observed_selection() {
        let mut consumer = Consumer::default();
        let capture = result_event(1, true)["capture"].clone();
        consumer.apply(&json!({"schema":"scorepeek-event-snapshot-v1","invocation_id":"a","next_sequence":20,"status":{"watcher":"session_active","capture":capture},"latest_result":result_event(1,true)}), "a").unwrap();
        let mut selection = selected(20);
        selection["capture"] = capture.clone();
        selection["state"]["scorepeek_song_id"] = json!("next-song");
        consumer.event(&selection).unwrap();
        let ended = json!({"event":"music_selection_changed","sequence":21,"capture":capture,"state":{"status":"unresolved","reason":"episode_ended"}});
        consumer.event(&ended).unwrap();
        consumer.view.history.status = "更新停止".into();
        let snapshot = json!({"schema":"scorepeek-event-snapshot-v1","invocation_id":"a","next_sequence":22,"status":{"watcher":"session_active","capture":capture},"latest_result":result_event(1,true),"music_selection":ended});
        consumer.apply(&snapshot, "a").unwrap();
        assert_eq!(consumer.view.chart.as_ref().unwrap().song_id, "next-song");
        assert_eq!(consumer.view.history.status, "更新停止");
        assert!(consumer.view.result.is_none());
        let mut fresh = Consumer::default();
        fresh.apply(&snapshot, "a").unwrap();
        assert!(fresh.view.chart.is_none());
        assert!(fresh.view.result.is_none());
        // A later unresolved event may hide a missed chart switch: don't relabel old history.
        let mut gap = snapshot;
        gap["music_selection"]["sequence"] = json!(30);
        gap["next_sequence"] = json!(31);
        consumer.apply(&gap, "a").unwrap();
        assert!(consumer.view.chart.is_none());
    }

    #[test]
    fn snapshot_sequence_selection_and_retention() {
        let mut consumer = Consumer::default();
        consumer.apply(&json!({"schema":"scorepeek-event-snapshot-v1","invocation_id":"a","next_sequence":2}), "a").unwrap();
        consumer.apply(&json!({"schema":"scorepeek-event-v1","invocation_id":"a","sequence":2,"event":"music_selection_changed","state":{"status":"selected","scorepeek_song_id":"song","play_type":"single","difficulty":"hyper","presentation":{"display_titles":["曲"]}}}), "a").unwrap();
        consumer.apply(&json!({"schema":"scorepeek-event-v1","invocation_id":"a","sequence":3,"event":"music_selection_changed","state":{"status":"unresolved","reason":"episode_ended"}}), "a").unwrap();
        assert_eq!(consumer.view.chart.as_ref().unwrap().title, "曲");
        assert!(!consumer.view.selecting);
        assert!(
            consumer
                .apply(
                    &json!({"schema":"scorepeek-event-v1","invocation_id":"a","sequence":5}),
                    "a"
                )
                .is_err()
        );
    }
}
