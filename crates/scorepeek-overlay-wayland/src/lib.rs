//! Independent consumers of the public live API and committed scores.
#![forbid(unsafe_code)]
extern crate self as scorepeek_overlay_wayland_handles;
pub mod bridge;
pub mod config;
pub mod control;
pub mod diagnostics;
pub mod host;
pub use host as native;
pub mod input;
pub mod render;
pub use host::event_loop::{TextCommand, TextInputState, TextUpdate};
pub mod skin;
pub mod window;

pub use scorepeek_overlay::Skin;
