//! Score-writer completion and health observations.

use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Completion {
    pub event_id: String,
    pub outcome: CompletionOutcome,
    pub chart: Option<ChartIdentity>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ChartIdentity {
    pub scorepeek_song_id: String,
    pub play_type: String,
    pub difficulty: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionOutcome {
    Persisted,
    Failed,
}

/// A host-owned diagnostic sample; no exporter or output sink is installed by this crate.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Health {
    pub accepted: u64,
    pub committed: u64,
    pub duplicates: u64,
    pub rejected: u64,
    pub failed: u64,
    pub pending: u64,
    pub queued_bytes: usize,
    pub last_committed_event_id: Option<String>,
    pub failure: Option<String>,
    pub cause: Option<String>,
    pub flush: Option<String>,
    pub recovered_provisional: u64,
    pub migration_unavailable_details: u64,
    pub migration_backup: Option<String>,
}

impl Health {
    pub(super) fn fail(&mut self, kind: &str, cause: &(impl ToString + ?Sized)) {
        if self.failure.is_none() {
            self.failure = Some(kind.to_owned());
            self.cause = Some(cause.to_string());
        }
    }
}
