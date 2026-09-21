//! Bounded diagnostic recording policy.

use serde::{Deserialize, Serialize};

pub const DEFAULT_SAMPLE_INTERVAL_MS: u64 = 100;
pub const DEFAULT_AGGREGATE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const NORMAL_RETENTION_HOURS: u32 = 24;
pub const PRIORITY_RETENTION_HOURS: u32 = 7 * 24;

#[derive(Clone, Debug)]
pub struct DiagnosticPolicy {
    pub enabled: bool,
    pub sample_interval_ms: u64,
    pub maximum_run_bytes: u64,
    pub retention: DiagnosticRetention,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticRetention {
    CompleteCadence,
    ForegroundFailureWindowV1,
    FactsOnly,
}

impl Default for DiagnosticPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            sample_interval_ms: DEFAULT_SAMPLE_INTERVAL_MS,
            maximum_run_bytes: DEFAULT_AGGREGATE_BYTES,
            retention: DiagnosticRetention::CompleteCadence,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_preserves_the_production_recording_contract() {
        let policy = DiagnosticPolicy::default();
        assert!(policy.enabled);
        assert_eq!(policy.sample_interval_ms, 100);
        assert_eq!(policy.maximum_run_bytes, 8 * 1024 * 1024 * 1024);
        assert_eq!(policy.retention, DiagnosticRetention::CompleteCadence);
        assert_eq!(NORMAL_RETENTION_HOURS, 24);
        assert_eq!(PRIORITY_RETENTION_HOURS, 7 * 24);
    }
}
