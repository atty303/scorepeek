//! Backend-neutral, wasm-compatible overlay UI authority.

pub mod action;
pub mod config;
pub mod data;
pub mod editor;
pub mod geometry;
pub mod skin;
pub mod style;
pub mod view;

pub use data::*;
pub use editor::effect as editor_surface;
pub use editor::model as editor_model;
pub use editor::runtime as editor_runtime;
pub use skin::Skin;

pub(crate) use data::validate_skin_id;
