//! Wayland DOM adapter for the shared native skin runtime.

pub mod runtime;
mod style_scope;
pub use runtime::{NativeTree, RenderTiming, Runtime, RuntimeTiming};

pub use scorepeek_overlay_runtime::skin::*;
