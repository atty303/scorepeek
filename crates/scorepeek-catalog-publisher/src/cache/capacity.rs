//! Shared source-cache capacity admission.

#[must_use]
pub fn admits(current: u64, incoming: u64, maximum: u64) -> bool {
    current
        .checked_add(incoming)
        .is_some_and(|total| total <= maximum)
}
