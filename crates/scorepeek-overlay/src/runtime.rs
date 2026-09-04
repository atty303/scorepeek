use crate::state::Consumer;
use scorepeek_overlay_ui::{Field, History, OverlayState};
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
    pub socket: PathBuf,
    pub invocation: String,
    pub scores_db: Option<PathBuf>,
    pub output: Option<String>,
    pub listen: SocketAddr,
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
                let mut received = 0_u64;
                let mut updates = 0_u64;
                let mut history_checks = 0_u64;
                while !stopping.load(Ordering::Acquire) {
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
                        consumer.view.connected = false;
                    }
                    if consumer.view.chart != queried || Instant::now() >= next_query {
                        queried.clone_from(&consumer.view.chart);
                        next_query = Instant::now() + Duration::from_secs(1);
                    refresh_history(&mut consumer.view, config.scores_db.as_deref());
                    history_checks = history_checks.saturating_add(1);
                    }
                    {
                        let mut current = shared
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if *current != consumer.view {
                        current.clone_from(&consumer.view);
                        updates = updates.saturating_add(1);
                        crate::diagnostics::emit("state_applied", &serde_json::json!({"connected": current.connected, "updates": updates, "received_records": received, "history_status": current.history.status}));
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

impl Drop for Feed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn refresh_history(view: &mut OverlayState, path: Option<&std::path::Path>) {
    let Some(path) = path else {
        view.history = History {
            status: "履歴無効".into(),
            ..History::default()
        };
        return;
    };
    let Some(chart) = &view.chart else {
        return;
    };
    match scorepeek_scores::query::chart_history(
        path,
        &chart.song_id,
        &chart.play_type,
        &chart.difficulty,
    ) {
        Ok(history) => {
            view.history = History {
                status: "保存済み · bestは項目別".into(),
                best: [
                    Field {
                        label: "EX".into(),
                        value: optional(history.best.score),
                    },
                    Field {
                        label: "MISS".into(),
                        value: optional(history.best.miss),
                    },
                    Field {
                        label: "CLEAR".into(),
                        value: clear(history.best.clear),
                    },
                ]
                .into(),
                plays: history
                    .plays
                    .iter()
                    .map(|play| {
                        format!(
                            "{} · EX {} / MISS {} / {}",
                            notification_time(play.emitted_unix_ms),
                            play.score,
                            optional(play.miss),
                            clear(Some(play.clear))
                        )
                    })
                    .collect(),
            };
        }
        Err(_) => {
            view.history.status = if path.exists() {
                "履歴読取不可 · 更新停止"
            } else {
                "履歴DBなし"
            }
            .into();
        }
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
    // Civil date from an epoch day; UTC avoids a new platform time-zone dependency.
    let seconds = unix_ms.div_euclid(1000);
    let days = seconds.div_euclid(86400) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = month_index + if month_index < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let time = seconds.rem_euclid(86400);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notification_dates_are_utc_and_not_play_start_times() {
        assert_eq!(notification_time(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(
            notification_time(951_782_400_000),
            "2000-02-29 00:00:00 UTC"
        );
    }
}
