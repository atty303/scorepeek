//! Portable canvas and widget layout and validation.

pub mod layout;
pub mod validation;

pub use layout::{Canvas, Widget};
pub use validation::{ConfigIssue, validate_canvases};

use crate::{Backend, Skin};
use layout::{default_height, default_width};
use std::collections::BTreeMap;

pub const OBS_OUTPUT_ID: &str = "obs-output";
pub const PENDING_WAYLAND_OUTPUT_ID: &str = "__pending-wayland-output__";

#[must_use]
pub fn empty_canvas(id: String, backend: Backend, skin: Skin) -> Canvas {
    Canvas {
        name: id.clone(),
        id,
        backend,
        skin,
        skin_properties: BTreeMap::new(),
        show_on: Some(Vec::new()),
        opacity_percent: 100,
        output: if backend == Backend::Obs {
            OBS_OUTPUT_ID.into()
        } else {
            PENDING_WAYLAND_OUTPUT_ID.into()
        },
        x: 20,
        y: 20,
        width: default_width(),
        height: default_height(),
        widgets: Vec::new(),
    }
}
