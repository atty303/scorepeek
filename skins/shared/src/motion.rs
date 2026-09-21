pub(crate) fn motion_value(from: f64, to: f64, period: f64, phase: f64, ramp: bool) -> Option<f64> {
    MOTION.with(|motion| {
        let (native, milliseconds) = motion.get();
        if !native {
            return None;
        }
        let seconds = std::time::Duration::from_millis(milliseconds).as_secs_f64();
        let phase = (seconds / period + phase).rem_euclid(1.0);
        let blend = if ramp {
            phase
        } else {
            0.5 - 0.5 * (phase * std::f64::consts::TAU).cos()
        };
        Some(from + (to - from) * blend)
    })
}

pub(crate) fn background_motion_style(mode: &str) -> String {
    if mode != "animated" {
        return String::new();
    }
    motion_value(0.3, 0.85, 16.0, 0.0, false)
        .map_or_else(String::new, |value| format!("opacity:{value:.4}"))
}

pub(crate) fn energy_motion_style() -> String {
    let Some(opacity) = motion_value(0.06, 0.16, 7.0, 0.0, false) else {
        return String::new();
    };
    let left = motion_value(-5.0, 1.0, 11.0, 0.0, false).unwrap_or(-5.0);
    format!("opacity:{opacity:.4};left:{left:.4}%")
}

pub(crate) fn glint_motion_style(kind: &str) -> String {
    let phase = match kind {
        "selection" => 0.14,
        "score" => 0.28,
        "history-list" => 0.42,
        "history-graph" => 0.56,
        _ => 0.0,
    };
    motion_value(-20.0, 100.0, 6.0, phase, true)
        .map_or_else(String::new, |value| format!("left:{value:.4}%"))
}

pub(crate) fn clear_motion_style(clear: &str) -> String {
    let track = match clear_role(clear) {
        "full-combo" => Some((0.72, 0.82)),
        "ex-hard" => Some((1.1, 0.82)),
        _ => None,
    };
    track
        .and_then(|(period, from)| motion_value(from, 1.0, period, 0.0, false))
        .map_or_else(String::new, |value| format!("opacity:{value:.4}"))
}

pub(crate) fn lamp_motion_style(state: &str) -> String {
    let track = match state {
        "active" | "persisted" => Some((1.8, 0.68)),
        "processing" => Some((0.8, 0.4)),
        _ => None,
    };
    track
        .and_then(|(period, from)| motion_value(from, 1.0, period, 0.0, false))
        .map_or_else(String::new, |value| format!("opacity:{value:.4}"))
}
pub(crate) fn dot_style(value: &Value, ratio: f64, start: i64, end: i64) -> String {
    format!(
        "left:{:.2}%;top:{:.2}%",
        time_ratio(integer(value, "received_unix_ms"), start, end) * 100.0,
        100.0 - ratio.clamp(0.0, 1.0) * 100.0
    )
}
pub(crate) fn time_ratio(time: i64, start: i64, end: i64) -> f64 {
    let elapsed = time.saturating_sub(start).max(0).cast_unsigned();
    let span = end.saturating_sub(start).max(1).cast_unsigned();
    std::time::Duration::from_millis(elapsed).as_secs_f64()
        / std::time::Duration::from_millis(span).as_secs_f64()
}
use serde_json::Value;

use crate::entry::MOTION;
use crate::value::{clear_role, integer};
