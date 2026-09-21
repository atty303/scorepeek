//! Persistable overlay document and pure validation.

mod document;
pub mod layout;
pub mod validation;

pub use document::*;
pub use layout::{Canvas, Widget};
pub use validation::ConfigIssue;
