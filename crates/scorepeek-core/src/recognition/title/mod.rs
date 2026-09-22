//! Title observation, preprocessing, decoding, and resolution.

pub(in crate::recognition) mod decode;
pub(in crate::recognition) mod observe;
pub(in crate::recognition) mod preprocess;
pub(in crate::recognition) mod resolve;

pub use decode::*;
pub use observe::*;
pub use preprocess::*;
pub use resolve::*;

/// Produces the catalog comparison key used by title observation and resolution.
#[must_use]
pub fn normalized_title_key(value: &str) -> String {
    observe::folded_comparison_key(value)
}
