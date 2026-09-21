//! Title observation, preprocessing, decoding, and resolution.

pub(in crate::recognition) mod decode;
pub(in crate::recognition) mod observe;
pub(in crate::recognition) mod preprocess;
pub(in crate::recognition) mod resolve;

pub use decode::*;
pub use observe::*;
pub use preprocess::*;
pub use resolve::*;
