//! Widget-specific bundled-skin renderers.

pub mod canvas;
pub mod chrome;
pub mod empty;
pub mod history;
pub mod score;
pub mod selection;
pub mod status;

pub(crate) use canvas::*;
pub(crate) use chrome::*;
pub(crate) use empty::*;
pub(crate) use history::*;
pub(crate) use score::*;
pub(crate) use selection::*;
pub(crate) use status::*;
