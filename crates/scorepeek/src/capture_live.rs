use std::ffi::OsStr;
use std::time::{Duration, Instant};

use scorepeek::capture::{
    CaptureDiagnosticFact, CaptureDiagnosticSink, CaptureErrorType, acquire_gamescope_source,
    start_uncalibrated_gamescope_receiver,
};
use serde::Serialize;

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);
const RECEIVER_START_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_GATE_DURATION_MS: u64 = 60_000;
const MAX_DIAGNOSTIC_FACTS: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum LiveGateStatus {
    Success,
    Error,
}

#[derive(Debug, Serialize)]
pub struct GamescopeLiveGateReport {
    schema: &'static str,
    status: LiveGateStatus,
    requested_duration_ms: u64,
    consumed_frames: u64,
    first_sequence: Option<u64>,
    last_sequence: Option<u64>,
    error_type: Option<CaptureErrorType>,
    diagnostic_facts: Vec<CaptureDiagnosticFact>,
    dropped_diagnostic_facts: u64,
}

impl GamescopeLiveGateReport {
    pub const fn succeeded(&self) -> bool {
        matches!(self.status, LiveGateStatus::Success)
    }
}

#[derive(Default)]
struct BoundedDiagnosticSink {
    facts: Vec<CaptureDiagnosticFact>,
    dropped: u64,
}

impl CaptureDiagnosticSink for BoundedDiagnosticSink {
    fn record(&mut self, fact: CaptureDiagnosticFact) {
        if self.facts.len() < MAX_DIAGNOSTIC_FACTS {
            self.facts.push(fact);
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }
}

pub fn parse_duration_ms(value: &OsStr) -> Result<u64, String> {
    let value = value
        .to_str()
        .ok_or_else(|| "capture live gate duration must be UTF-8".to_owned())?;
    let duration = value
        .parse::<u64>()
        .map_err(|_| "capture live gate duration must be an integer".to_owned())?;
    if !(1..=MAX_GATE_DURATION_MS).contains(&duration) {
        return Err(format!(
            "capture live gate duration must be between 1 and {MAX_GATE_DURATION_MS} ms"
        ));
    }
    Ok(duration)
}

pub fn run_gamescope_live_gate(duration_ms: u64) -> GamescopeLiveGateReport {
    let mut sink = BoundedDiagnosticSink::default();
    let mut consumed_frames = 0_u64;
    let mut first_sequence = None;
    let mut last_sequence = None;

    let lease = match acquire_gamescope_source(DISCOVERY_TIMEOUT, &mut sink) {
        Ok(lease) => lease,
        Err(error) => {
            return report(
                duration_ms,
                consumed_frames,
                first_sequence,
                last_sequence,
                Some(error.error_type()),
                sink,
            );
        }
    };
    let mut receiver =
        match start_uncalibrated_gamescope_receiver(lease, RECEIVER_START_TIMEOUT, &mut sink) {
            Ok(receiver) => receiver,
            Err(error) => {
                return report(
                    duration_ms,
                    consumed_frames,
                    first_sequence,
                    last_sequence,
                    Some(error.error_type()),
                    sink,
                );
            }
        };

    consume_latest(
        &mut receiver,
        &mut consumed_frames,
        &mut first_sequence,
        &mut last_sequence,
    );
    let steady_started = Instant::now();
    let requested_duration = Duration::from_millis(duration_ms);
    let mut terminal = None;
    while steady_started.elapsed() < requested_duration {
        let remaining = requested_duration.saturating_sub(steady_started.elapsed());
        if let Err(error) = receiver.poll(remaining, &mut sink) {
            terminal = Some(error.error_type());
            break;
        }
        consume_latest(
            &mut receiver,
            &mut consumed_frames,
            &mut first_sequence,
            &mut last_sequence,
        );
    }

    if let Err(error) = receiver.shutdown(&mut sink) {
        terminal.get_or_insert(error.error_type());
    }
    report(
        duration_ms,
        consumed_frames,
        first_sequence,
        last_sequence,
        terminal,
        sink,
    )
}

fn consume_latest(
    receiver: &mut scorepeek::capture::UncalibratedPipeWireReceiver,
    consumed_frames: &mut u64,
    first_sequence: &mut Option<u64>,
    last_sequence: &mut Option<u64>,
) {
    let Some(frame) = receiver.take_latest_frame() else {
        return;
    };
    *consumed_frames = consumed_frames.saturating_add(1);
    first_sequence.get_or_insert(frame.sequence());
    *last_sequence = Some(frame.sequence());
}

fn report(
    duration_ms: u64,
    consumed_frames: u64,
    first_sequence: Option<u64>,
    last_sequence: Option<u64>,
    error_type: Option<CaptureErrorType>,
    sink: BoundedDiagnosticSink,
) -> GamescopeLiveGateReport {
    GamescopeLiveGateReport {
        schema: "scorepeek-gamescope-live-gate-v1",
        status: if error_type.is_some() {
            LiveGateStatus::Error
        } else {
            LiveGateStatus::Success
        },
        requested_duration_ms: duration_ms,
        consumed_frames,
        first_sequence,
        last_sequence,
        error_type,
        diagnostic_facts: sink.facts,
        dropped_diagnostic_facts: sink.dropped,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use scorepeek::capture::{
        CaptureDiagnosticDetail, CaptureDiagnosticFact, CaptureDiagnosticOperation,
        CaptureDiagnosticSink, CaptureDiagnosticStatus, CaptureSourceKind,
    };

    use super::{BoundedDiagnosticSink, MAX_DIAGNOSTIC_FACTS, parse_duration_ms};

    #[test]
    fn duration_is_explicitly_bounded() {
        assert_eq!(parse_duration_ms(OsStr::new("1")).unwrap(), 1);
        assert_eq!(parse_duration_ms(OsStr::new("60000")).unwrap(), 60_000);
        assert!(parse_duration_ms(OsStr::new("0")).is_err());
        assert!(parse_duration_ms(OsStr::new("60001")).is_err());
        assert!(parse_duration_ms(OsStr::new("forever")).is_err());
    }

    #[test]
    fn diagnostic_sink_drops_new_facts_at_capacity() {
        let mut sink = BoundedDiagnosticSink::default();
        for sequence in 0..=MAX_DIAGNOSTIC_FACTS as u64 {
            sink.record(CaptureDiagnosticFact {
                sequence,
                monotonic_start_ms: 0,
                monotonic_end_ms: 0,
                operation: CaptureDiagnosticOperation::Shutdown,
                status: CaptureDiagnosticStatus::Success,
                error_type: None,
                detail: CaptureDiagnosticDetail::Shutdown {
                    source: CaptureSourceKind::GamescopeDefaultRemote,
                },
            });
        }
        assert_eq!(sink.facts.len(), MAX_DIAGNOSTIC_FACTS);
        assert_eq!(sink.dropped, 1);
    }
}
