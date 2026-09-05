use crate::state::Consumer;
use scorepeek_overlay_ui::{BestView, GraphPlay, History, HistoryPlay, OverlayState, ResultDetail};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead as _, BufReader, Read as _},
    net::SocketAddr,
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Wayland,
    Obs,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    pub backend: Backend,
    pub canvases: Vec<crate::config::Canvas>,
    pub config_path: PathBuf,
    pub control_socket: PathBuf,
    pub socket: PathBuf,
    pub invocation: String,
    pub scores_db: Option<PathBuf>,
    pub listen: SocketAddr,
    pub unknown_grace_ms: u32,
    pub settings_revision: u64,
}

pub struct Feed {
    pub state: Arc<Mutex<OverlayState>>,
    pub stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Feed {
    /// Starts only an event reader and a read-only score query worker.
    /// # Errors
    /// Returns thread creation errors.
    #[allow(clippy::too_many_lines)]
    pub fn start(config: Config, wake: Arc<dyn Fn() + Send + Sync>) -> std::io::Result<Self> {
        let state = Arc::new(Mutex::new(OverlayState::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::clone(&state);
        let stopping = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("overlay-feed".into())
            .spawn(move || {
                let mut consumer = Consumer::default();
                let mut stream = None;
                let mut pending = Vec::new();
                let mut next_connect = Instant::now();
                let mut next_query = Instant::now();
                let mut queried = None;
                let mut queried_revision = 0;
                let mut received = 0_u64;
                let mut updates = 0_u64;
                let mut history_checks = 0_u64;
                let mut unknown_grace_ms = config.unknown_grace_ms;
                let mut next_config_reload = Instant::now() + Duration::from_secs(1);
                while !stopping.load(Ordering::Acquire) {
                    if Instant::now() >= next_config_reload {
                        if let Ok((loaded, _)) = crate::config::load_or_create(&config.config_path) {
                            unknown_grace_ms = loaded.unknown_grace_ms;
                        }
                        next_config_reload = Instant::now() + Duration::from_secs(1);
                    }
                    if stream.is_none() && Instant::now() >= next_connect {
                        stream = UnixStream::connect(&config.socket)
                            .ok()
                            .and_then(|stream| stream.set_nonblocking(true).ok().map(|()| stream));
                        next_connect = Instant::now() + Duration::from_secs(1);
                    }
                    let mut disconnected = false;
                    if let Some(socket) = stream.as_mut() {
                        let mut bytes = [0; 8192];
                        match socket.read(&mut bytes) {
                            Ok(0) => disconnected = true,
                            Ok(count) => {
                            pending.extend_from_slice(&bytes[..count]);
                                while let Some(end) = pending.iter().position(|byte| *byte == b'\n')
                                {
                                let record: Vec<_> = pending.drain(..=end).collect();
                                received = received.saturating_add(1);
                                    if record.len() > 1024 * 1024
                                        || serde_json::from_slice(&record)
                                            .map_err(|error| error.to_string())
                                            .and_then(|value| {
                                                consumer.apply(&value, &config.invocation)
                                            })
                                            .is_err()
                                    {
                                        disconnected = true;
                                        break;
                                    }
                                }
                                disconnected |= pending.len() > 1024 * 1024;
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                            Err(_) => disconnected = true,
                        }
                    }
                    if disconnected {
                        stream = None;
                        pending.clear();
                        consumer.disconnect(unix_now_ms());
                    }
                    consumer.expire_screen(unix_now_ms(), unknown_grace_ms);
                    if consumer.view.chart != queried
                        || consumer.query_revision != queried_revision
                        || Instant::now() >= next_query
                    {
                        queried.clone_from(&consumer.view.chart);
                        queried_revision = consumer.query_revision;
                        next_query = Instant::now() + Duration::from_secs(5);
                        refresh_history(
                            &mut consumer.view,
                            config.scores_db.as_deref(),
                            50,
                        );
                        history_checks = history_checks.saturating_add(1);
                    }
                    {
                        let mut current = shared
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if *current != consumer.view {
                            current.clone_from(&consumer.view);
                            updates = updates.saturating_add(1);
                            crate::diagnostics::emit("state_applied", &serde_json::json!({
                                "connected": current.connected,
                                "updates": updates,
                                "received_records": received,
                                "history_recorded": current.history.recorded,
                                "history_rows": current.history.plays.len(),
                            }));
                            wake();
                        }
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                crate::diagnostics::emit("feed_summary", &serde_json::json!({"received_records": received, "state_updates": updates, "history_checks": history_checks}));
            })?;
        Ok(Self {
            state,
            stop,
            worker: Some(worker),
        })
    }
}

fn unix_now_ms() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
        Err(error) => -i64::try_from(error.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[allow(clippy::cast_precision_loss, clippy::single_match_else)]
fn refresh_history(view: &mut OverlayState, path: Option<&std::path::Path>, history_count: u32) {
    let Some(path) = path else {
        view.history = History::default();
        view.best = BestView::default();
        view.detail = ResultDetail::default();
        return;
    };
    let Some(chart) = view.chart.clone() else {
        return;
    };
    let local_now = chrono::Local::now();
    let now = local_now.timestamp_millis();
    let graph_starts = graph_starts(local_now);
    let since = graph_starts[3];
    match scorepeek_scores::query::chart_dashboard(
        path,
        &chart.song_id,
        &chart.play_type,
        &chart.difficulty,
        usize::try_from(history_count).unwrap_or(5),
        since,
    ) {
        Ok(history) => {
            let dj = dj_level(history.best.score, chart.notes);
            view.best = BestView {
                score: optional(history.best.score),
                dj_level: dj,
                miss: optional(history.best.miss),
                clear: clear(history.best.clear),
            };
            view.detail = history
                .representative
                .as_ref()
                .map(result_detail)
                .unwrap_or_default();
            view.history = History {
                recorded: history.recorded,
                plays: history
                    .recent
                    .iter()
                    .map(|play| HistoryPlay {
                        notified_at: notification_time(play.received_unix_ms),
                        score: play.score.to_string(),
                        dj_level: dj_level(Some(play.score), chart.notes),
                        miss: optional(play.miss),
                        clear: clear(Some(play.clear)),
                    })
                    .collect(),
                graph: history
                    .graph
                    .iter()
                    .filter_map(|play| {
                        let notes = chart.notes?;
                        (notes > 0).then_some(GraphPlay {
                            received_unix_ms: play.received_unix_ms,
                            score_ratio: play.score as f64 / (f64::from(notes) * 2.0),
                            miss_ratio: play.miss.map(|v| v as f64 / f64::from(notes)),
                        })
                    })
                    .collect(),
                graph_start_unix_ms: graph_starts,
                graph_end_unix_ms: now,
            };
        }
        Err(_) => {
            view.history = History::default();
            view.best = BestView::default();
            view.detail = ResultDetail::default();
        }
    }
}

fn graph_starts(local_now: chrono::DateTime<chrono::Local>) -> [i64; 4] {
    let now = local_now.timestamp_millis();
    [1_u32, 3, 6, 12].map(|months| {
        local_now
            .checked_sub_months(chrono::Months::new(months))
            .map_or_else(
                || now.saturating_sub(i64::from(months) * 31 * 24 * 60 * 60 * 1000),
                |value| value.timestamp_millis(),
            )
    })
}

fn dj_level(score: Option<i64>, notes: Option<u32>) -> String {
    let (Some(score), Some(notes)) = (score, notes) else {
        return "—".into();
    };
    if notes == 0 {
        return "—".into();
    }
    let band = (score.max(0).cast_unsigned() * 9 / (u64::from(notes) * 2)).min(8);
    match band {
        8 => "AAA",
        7 => "AA",
        6 => "A",
        5 => "B",
        4 => "C",
        3 => "D",
        2 => "E",
        _ => "F",
    }
    .into()
}
fn result_detail(raw: &serde_json::Value) -> ResultDetail {
    let r = &raw["result"];
    let judgments = &r["judgments"];
    let value = |v: &serde_json::Value| display_value(v).unwrap_or_else(|| "—".into());
    ResultDetail {
        pgreat: value(&judgments["pgreat"]),
        great: value(&judgments["great"]),
        good: value(&judgments["good"]),
        bad: value(&judgments["bad"]),
        poor: value(&judgments["poor"]),
        fast: value(&r["timing"]["fast"]),
        slow: value(&r["timing"]["slow"]),
        combo_break: value(&r["combo_break"]),
        play_options: value(&r["play_options"]),
    }
}
fn display_value(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(v) => Some(v.clone()),
        serde_json::Value::Number(v) => Some(v.to_string()),
        serde_json::Value::Array(v) => {
            let p: Vec<_> = v.iter().filter_map(display_value).collect();
            (!p.is_empty()).then(|| p.join(" / "))
        }
        serde_json::Value::Object(v) => {
            if let Some(status) = v.get("status") {
                return (status == "known")
                    .then(|| {
                        display_value(
                            v.get("value")
                                .or_else(|| v.get("values"))
                                .unwrap_or(&serde_json::Value::Null),
                        )
                    })
                    .flatten();
            }
            None
        }
        _ => None,
    }
}
fn optional(value: Option<i64>) -> String {
    value.map_or_else(|| "—".into(), |value| value.to_string())
}
fn clear(value: Option<i64>) -> String {
    value
        .and_then(|value| usize::try_from(value).ok())
        .and_then(|index| {
            [
                "NO PLAY",
                "FAILED",
                "ASSIST",
                "EASY",
                "CLEAR",
                "HARD",
                "EX HARD",
                "FULL COMBO",
            ]
            .get(index)
        })
        .map_or_else(|| "—".into(), |value| (*value).into())
}

/// Reads one configuration line; the remaining stdin pipe is the parent lifetime lease.
/// # Errors
/// Returns malformed configuration or stdin errors.
pub fn read_config() -> Result<(Config, BufReader<std::io::Stdin>), String> {
    let mut input = BufReader::new(std::io::stdin());
    let mut line = String::new();
    input
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    let config =
        serde_json::from_str(&line).map_err(|error| format!("overlay configuration: {error}"))?;
    Ok((config, input))
}

fn notification_time(unix_ms: i64) -> String {
    use chrono::TimeZone as _;
    chrono::Local
        .timestamp_millis_opt(unix_ms)
        .single()
        .map_or_else(
            || "—".into(),
            |time| time.format("%Y-%m-%d %H:%M:%S").to_string(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notification_dates_use_local_time_and_not_play_start_times() {
        use chrono::TimeZone as _;
        assert_ne!(notification_time(0), "");
        assert_eq!(
            notification_time(951_782_400_000),
            chrono::Local
                .timestamp_millis_opt(951_782_400_000)
                .single()
                .unwrap()
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        );
    }

    #[test]
    fn graph_months_follow_local_calendar_boundaries() {
        use chrono::TimeZone as _;
        let now = chrono::Local
            .with_ymd_and_hms(2024, 3, 31, 12, 0, 0)
            .single()
            .unwrap();
        let starts = graph_starts(now);
        assert_eq!(
            starts[0],
            chrono::Local
                .with_ymd_and_hms(2024, 2, 29, 12, 0, 0)
                .single()
                .unwrap()
                .timestamp_millis()
        );
    }
}
