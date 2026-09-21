//! Recognition primitives shared by screen-specific authorities.

mod candidates;
pub(in crate::recognition) mod confidence;
pub(in crate::recognition) mod ctc;

pub use candidates::*;
pub use confidence::*;
