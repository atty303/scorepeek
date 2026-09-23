use std::fmt;
use std::sync::Arc;

use scorepeek::capture::NormalizedCanonicalFrame;

#[derive(Clone)]
pub struct BoundCanonicalFrame {
    pub(crate) session_id: Arc<str>,
    pub(crate) source_sequence: u64,
    pub(crate) sequence: u64,
    pub(crate) monotonic_start_ms: u64,
    pub(crate) monotonic_end_ms: u64,
    pub(crate) pixels: Arc<Box<[u8]>>,
}

impl fmt::Debug for BoundCanonicalFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundCanonicalFrame")
            .field("session_id", &self.session_id)
            .field("source_sequence", &self.source_sequence)
            .field("sequence", &self.sequence)
            .field("monotonic_start_ms", &self.monotonic_start_ms)
            .field("monotonic_end_ms", &self.monotonic_end_ms)
            .finish_non_exhaustive()
    }
}

impl BoundCanonicalFrame {
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    #[must_use]
    pub(crate) const fn sequence(&self) -> u64 {
        self.sequence
    }

    pub(crate) fn shared_pixels(&self) -> Arc<Box<[u8]>> {
        Arc::clone(&self.pixels)
    }

    #[must_use]
    pub(crate) const fn source_sequence(&self) -> u64 {
        self.source_sequence
    }

    pub(crate) fn assign_tick_sequence(&mut self, tick_sequence: u64) {
        self.sequence = tick_sequence;
    }

    #[must_use]
    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }

    #[must_use]
    pub(crate) const fn monotonic_start_ms(&self) -> u64 {
        self.monotonic_start_ms
    }

    #[must_use]
    pub(crate) const fn monotonic_end_ms(&self) -> u64 {
        self.monotonic_end_ms
    }

    #[cfg(test)]
    pub(crate) fn for_test(generation: u64, sequence: u64, time: u64) -> Self {
        Self::for_test_pixels(
            generation,
            sequence,
            time,
            vec![7; scorepeek_core::frame::CANONICAL_BYTES].into_boxed_slice(),
        )
    }

    #[cfg(test)]
    pub(crate) fn for_test_session(mut self, session_id: &str) -> Self {
        self.session_id = Arc::from(session_id);
        self
    }

    #[cfg(test)]
    pub(crate) fn for_test_pixels(
        generation: u64,
        sequence: u64,
        time: u64,
        pixels: Box<[u8]>,
    ) -> Self {
        Self {
            session_id: Arc::from(format!("session-{generation}")),
            source_sequence: sequence,
            sequence,
            monotonic_start_ms: time,
            monotonic_end_ms: time + 16,
            pixels: Arc::new(pixels),
        }
    }
}

impl BoundCanonicalFrame {
    pub(crate) fn from_normalized(frame: NormalizedCanonicalFrame, session_id: Arc<str>) -> Self {
        let received_monotonic_ms = frame.received_monotonic_ns() / 1_000_000;
        let pixel_address = frame.pixels().as_ptr();
        let live = Self {
            session_id,
            source_sequence: frame.source_sequence(),
            sequence: frame.source_sequence(),
            monotonic_start_ms: received_monotonic_ms,
            monotonic_end_ms: received_monotonic_ms,
            pixels: Arc::new(frame.into_pixels()),
        };
        debug_assert_eq!(live.pixels.len(), scorepeek_core::frame::CANONICAL_BYTES);
        debug_assert_eq!(live.pixels.as_ptr(), pixel_address);
        live
    }
}
