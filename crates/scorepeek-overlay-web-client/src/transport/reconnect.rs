//! Reconnect compatibility state.

#[derive(Clone, Copy, PartialEq)]
pub enum Compatibility {
    Checking,
    Ready,
    Mismatch,
}
