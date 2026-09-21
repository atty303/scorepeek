//! Diagnostic-run bounded-record retention limits.

pub const MAX_FRAMES_PER_RUN: usize = 8_192;
pub const MAX_FACTS_PER_RUN: usize = 250_000;
pub const MAX_DEGRADATIONS_PER_RUN: usize = 4_096;
