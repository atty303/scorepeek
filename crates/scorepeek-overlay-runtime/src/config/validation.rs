//! Pure overlay configuration validation.

use std::collections::BTreeSet;

use scorepeek_overlay::geometry::MAX_DIMENSION;
use scorepeek_overlay::{AspectRatio, Backend};

use super::PENDING_WAYLAND_OUTPUT_ID;
use super::layout::Canvas;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigIssue {
    pub canvas_id: String,
    pub message: String,
}

/// Returns individually valid canvases and their validation issues.
#[must_use]
pub fn validate_canvases(canvases: &[Canvas]) -> (Vec<Canvas>, Vec<ConfigIssue>) {
    let mut canvas_ids = BTreeSet::new();
    let mut canvas_names = BTreeSet::new();
    let mut valid = Vec::new();
    let mut issues = Vec::new();
    for canvas in canvases {
        match validate_canvas(canvas, &mut canvas_ids, &mut canvas_names)
            .and_then(|()| scorepeek_overlay::skin::validate_id(canvas.skin.name()))
        {
            Ok(()) => valid.push(canvas.clone()),
            Err(message) => issues.push(ConfigIssue {
                canvas_id: canvas.id.clone(),
                message,
            }),
        }
    }
    (valid, issues)
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
    if !(32..=MAX_DIMENSION).contains(&canvas.width)
        || !(32..=MAX_DIMENSION).contains(&canvas.height)
    {
        return Err(format!(
            "canvas dimensions must be between 32 and {MAX_DIMENSION}"
        ));
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
        if !(16..=MAX_DIMENSION).contains(&widget.width)
            || !(16..=MAX_DIMENSION).contains(&widget.height)
        {
            return Err(format!(
                "widget {} dimensions must be between 16 and {MAX_DIMENSION}",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_geometry_allows_offscreen_and_non_grid_values_but_bounds_dimensions() {
        let skin = "dev.example.skin".parse().unwrap();
        let mut canvas = super::super::empty_canvas("canvas-1".into(), Backend::Obs, skin);
        canvas.x = -101;
        canvas.y = 20_001;
        canvas.width = MAX_DIMENSION;
        canvas.height = 33;
        canvas.widgets.push(super::super::layout::Widget {
            id: "widget-1".into(),
            kind: scorepeek_overlay::WidgetKind::Empty,
            x: -29,
            y: 33_333,
            width: 17,
            height: 16,
            settings: scorepeek_overlay::WidgetSettings::default(),
            skin_properties: std::collections::BTreeMap::default(),
        });
        assert!(validate_canvases(&[canvas.clone()]).1.is_empty());

        canvas.width = MAX_DIMENSION + 1;
        assert!(!validate_canvases(&[canvas.clone()]).1.is_empty());
        canvas.width = MAX_DIMENSION;
        canvas.widgets[0].height = MAX_DIMENSION + 1;
        assert!(!validate_canvases(&[canvas]).1.is_empty());
    }
}
