//! Shared concrete rendering implementation for bundled skins.

mod entry;
pub mod motion;
pub mod primitive;
pub mod render;
pub mod theme;
pub mod value;

pub use entry::{allocate, deallocate, render};
pub use theme::Theme;
