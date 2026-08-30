//! Deterministic application-level recognition cadence.

/// Production recognition interval. Capture and decode may run faster; only the latest frame at
/// each due instant enters the recognition pipeline.
pub const RECOGNITION_INTERVAL_MS: u64 = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CadenceDecision {
    Process { tick_sequence: u64 },
    SkipCadence,
}

/// A source-timestamp scheduler shared by live capture and deterministic replay.
#[derive(Clone, Debug, Default)]
pub struct RecognitionCadence {
    next_due_ms: Option<u64>,
    next_tick_sequence: u64,
    processed: u64,
}

impl RecognitionCadence {
    #[must_use]
    pub fn observe(&mut self, source_timestamp_ms: u64) -> CadenceDecision {
        let next_due = self.next_due_ms.get_or_insert(source_timestamp_ms);
        if source_timestamp_ms < *next_due {
            return CadenceDecision::SkipCadence;
        }

        let tick_sequence = self.next_tick_sequence;
        self.next_tick_sequence = self.next_tick_sequence.saturating_add(1);
        let elapsed_ticks = source_timestamp_ms
            .saturating_sub(*next_due)
            .checked_div(RECOGNITION_INTERVAL_MS)
            .unwrap_or(0)
            .saturating_add(1);
        *next_due = next_due.saturating_add(elapsed_ticks.saturating_mul(RECOGNITION_INTERVAL_MS));

        self.processed = self.processed.saturating_add(1);
        CadenceDecision::Process { tick_sequence }
    }

    #[must_use]
    pub const fn processed(&self) -> u64 {
        self.processed
    }
}

#[cfg(test)]
mod tests {
    use super::{CadenceDecision, RecognitionCadence};

    #[test]
    fn faster_sources_enter_recognition_at_ten_hertz_without_backlog() {
        for source_fps in [60_u64, 120] {
            let mut cadence = RecognitionCadence::default();
            let processed = (0..source_fps)
                .map(|frame| frame.saturating_mul(1_000) / source_fps)
                .filter(|timestamp| {
                    matches!(cadence.observe(*timestamp), CadenceDecision::Process { .. })
                })
                .count();
            assert_eq!(processed, 10);
        }
    }

    #[test]
    fn due_ticks_are_never_suppressed_by_downstream_work() {
        let mut cadence = RecognitionCadence::default();
        assert_eq!(
            cadence.observe(0),
            CadenceDecision::Process { tick_sequence: 0 }
        );
        assert_eq!(
            cadence.observe(100),
            CadenceDecision::Process { tick_sequence: 1 }
        );
        assert_eq!(cadence.observe(150), CadenceDecision::SkipCadence);
        assert_eq!(cadence.processed(), 2);
    }
}
