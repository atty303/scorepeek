//! Bounded diagnostic recording policy.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticPolicy {
    pub maximum_records: usize,
    pub maximum_record_bytes: usize,
}
