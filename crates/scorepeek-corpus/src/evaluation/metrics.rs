//! Deterministic evaluation metric primitives.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counts {
    pub accepted: u64,
    pub rejected: u64,
}
