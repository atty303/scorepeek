//! Recognition facade grouped by screen and field authority.

mod screen;

pub mod music_select;
pub mod result;
pub mod shared;
pub mod title;

pub use crate::frame::{CanonicalFrame, CanonicalLayout, Roi};
pub use screen::*;
