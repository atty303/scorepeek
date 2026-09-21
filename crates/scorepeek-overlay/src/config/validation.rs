//! Pure overlay configuration validation.

use std::collections::BTreeSet;

use crate::{AspectRatio, Backend};

use super::document::{OverlayConfig, PENDING_WAYLAND_OUTPUT_ID, SCHEMA_VERSION};
use super::layout::Canvas;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigIssue {
    pub canvas_id: String,
    pub message: String,
}

impl OverlayConfig {
    /// Validates global invariants and returns individually valid canvases.
    /// # Errors
    /// Returns an unsupported schema or a backend without a valid canvas.
    pub fn validated(&self) -> Result<(Vec<Canvas>, Vec<ConfigIssue>), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!("overlay schema_version must be {SCHEMA_VERSION}"));
        }
        if self.unknown_grace_ms > 10_000 {
            return Err("overlay unknown_grace_ms must be at most 10000".into());
        }
        let listen = self
            .obs_listen
            .parse::<std::net::SocketAddr>()
            .map_err(|error| format!("overlay obs_listen: {error}"))?;
        if !listen.ip().is_loopback() {
            return Err("overlay obs_listen must use a loopback address".into());
        }
        let mut canvas_ids = BTreeSet::new();
        let mut canvas_names = BTreeSet::new();
        let mut valid = Vec::new();
        let mut issues = Vec::new();
        for canvas in &self.canvases {
            match validate_canvas(canvas, &mut canvas_ids, &mut canvas_names)
                .and_then(|()| crate::validate_skin_id(canvas.skin.name()))
            {
                Ok(()) => valid.push(canvas.clone()),
                Err(message) => issues.push(ConfigIssue {
                    canvas_id: canvas.id.clone(),
                    message,
                }),
            }
        }
        Ok((valid, issues))
    }
}

fn validate_canvas(
    canvas: &Canvas,
    canvas_ids: &mut BTreeSet<(Backend, String)>,
    canvas_names: &mut BTreeSet<(Backend, String)>,
) -> Result<(), String> {
    if canvas.id.is_empty()
        || !canvas
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("id must use ASCII letters, digits, '-' or '_'".into());
    }
    if !canvas_ids.insert((canvas.backend, canvas.id.clone())) {
        return Err("duplicate canvas id within backend workspace".into());
    }
    if canvas.name.trim().is_empty() {
        return Err("canvas name must be non-empty".into());
    }
    if !canvas_names.insert((canvas.backend, canvas.name.clone())) {
        return Err("duplicate canvas name within backend workspace".into());
    }
    if canvas.output.is_empty() || canvas.output == PENDING_WAYLAND_OUTPUT_ID {
        return Err("output must be assigned".into());
    }
    if canvas.width < 32 || canvas.height < 32 {
        return Err("canvas dimensions must be at least 32x32".into());
    }
    if canvas.opacity_percent == 0 || canvas.opacity_percent > 100 {
        return Err("canvas opacity_percent must be between 1 and 100".into());
    }
    let mut widget_ids = BTreeSet::new();
    for widget in &canvas.widgets {
        if widget.id.is_empty() || !widget_ids.insert(widget.id.clone()) {
            return Err("widget ids must be non-empty and unique per canvas".into());
        }
        if let AspectRatio::Current([width, height]) = widget.settings.aspect_ratio
            && (width == 0 || height == 0)
        {
            return Err(format!(
                "widget {} locked aspect ratio dimensions must be positive",
                widget.id
            ));
        }
        if widget.width < 16 || widget.height < 16 {
            return Err(format!("widget {} must be at least 16x16", widget.id));
        }
        if widget.x % 4 != 0
            || widget.y % 4 != 0
            || !widget.width.is_multiple_of(4)
            || !widget.height.is_multiple_of(4)
        {
            return Err(format!(
                "widget {} position and dimensions must align to the 4px grid",
                widget.id
            ));
        }
        if !matches!(widget.settings.history_count, 5 | 10 | 20 | 50) {
            return Err(format!(
                "widget {} history_count must be 5, 10, 20 or 50",
                widget.id
            ));
        }
        if !matches!(widget.settings.graph_months, 1 | 3 | 6 | 12) {
            return Err(format!(
                "widget {} graph_months must be 1, 3, 6 or 12",
                widget.id
            ));
        }
    }
    Ok(())
}
