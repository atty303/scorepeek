//! Projection cursor for deterministic event consumers.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProjectionCursor {
    pub next_sequence: u64,
}
