//! Image-backed panel materials and semantic chart/status fittings.
use crate::{Skin, WidgetKind, WidgetLayout};
use dioxus::prelude::*;

fn contour(width: f64, height: f64, inset: f64, cut: f64) -> String {
    let right = width - inset;
    let bottom = height - inset;
    let near = inset + cut;
    let far_x = right - cut;
    let far_y = bottom - cut;
    format!(
        "M{near} {inset}H{far_x}L{right} {near}V{far_y}L{far_x} {bottom}H{near}L{inset} {far_y}V{near}Z"
    )
}

// The artwork is a square source. Only the middle strips stretch: corners retain
// their aspect ratio, and metal thickness is independent of the widget size.
struct FrameMaterial {
    image: &'static str,
    corner_x: f64,
    corner_y: f64,
    scale: f64,
}

impl FrameMaterial {
    fn for_skin(skin: Skin) -> Self {
        match skin.name() {
            Skin::CYAN_SYSTEM_ID => Self {
                image: "cyan-system-frame.png",
                corner_x: 340.0,
                corner_y: 200.0,
                scale: 0.16,
            },
            Skin::RESULT_AURORA_ID => Self {
                image: "result-aurora-frame.png",
                corner_x: 160.0,
                corner_y: 160.0,
                scale: 0.24,
            },
            _ => Self {
                image: "dj-blackbox-frame.png",
                corner_x: 260.0,
                corner_y: 120.0,
                scale: 0.22,
            },
        }
    }
}

pub fn render(widget: &WidgetLayout, skin: Skin) -> Element {
    let material = FrameMaterial::for_skin(skin);
    let edge = f64::from(widget.settings.frame_width.pixels());
    let offset = 8.0 - edge;
    let width = f64::from(widget.width) - 16.0 + edge * 2.0;
    let height = f64::from(widget.height) - 16.0 + edge * 2.0;
    let aperture_scale = if widget.kind == WidgetKind::Empty {
        0.55
    } else {
        1.0
    };
    let scale = (material.scale * aperture_scale)
        .min(width / (2.0 * material.corner_x))
        .min(height / (2.0 * material.corner_y));
    let source_x = [0.0, material.corner_x, 1254.0 - material.corner_x, 1254.0];
    let source_y = [0.0, material.corner_y, 1254.0 - material.corner_y, 1254.0];
    let corner_x = (material.corner_x * scale + edge - 8.0)
        .max(1.0)
        .min(width / 2.0);
    let corner_y = (material.corner_y * scale + edge - 8.0)
        .max(1.0)
        .min(height / 2.0);
    let target_x = [0.0, corner_x, width - corner_x, width];
    let target_y = [0.0, corner_y, height - corner_y, height];
    let mut pieces = Vec::with_capacity(9);
    for row in 0..3 {
        for column in 0..3 {
            let w = target_x[column + 1] - target_x[column];
            let h = target_y[row + 1] - target_y[row];
            if w <= 0.0 || h <= 0.0 {
                continue;
            }
            let sx = w / (source_x[column + 1] - source_x[column]);
            let sy = h / (source_y[row + 1] - source_y[row]);
            let style = format!(
                "left:{}px;top:{}px;width:{w}px;height:{h}px;background-image:url('/skins/{}');background-size:{}px {}px;background-position:{}px {}px;",
                target_x[column],
                target_y[row],
                material.image,
                1254.0 * sx,
                1254.0 * sy,
                -source_x[column] * sx,
                -source_y[row] * sy
            );
            pieces.push(rsx! { div { class: "material-slice", style } });
        }
    }
    let edge = match skin.name() {
        Skin::CYAN_SYSTEM_ID => "#13dcef",
        Skin::RESULT_AURORA_ID => "#c2a660",
        _ => "#687067",
    };
    rsx! { div { class: "material-frame", style:format!("left:{offset}px;top:{offset}px;width:{width}px;height:{height}px"), {pieces.into_iter()} }
        if skin == Skin::ResultAurora && widget.kind == WidgetKind::Selection {
            div { class: "material-illumination" }
        }
        if widget.kind == WidgetKind::Status {
            svg { class: "panel-frame", width: "{width}", height: "{height}", view_box: "0 0 {width} {height}",
                path { d: format!("M{} 12l-30 {}h-6l30 -{}",width * 0.43,height-24.0,height-24.0), fill: "none", stroke: edge, stroke_width: "0.7" }
            }
        }
    }
}

pub fn chart_rail(width: u32, skin: Skin) -> Element {
    let width = f64::from(width);
    let edge = match skin.name() {
        Skin::CYAN_SYSTEM_ID => "#13dcef",
        Skin::RESULT_AURORA_ID => "#c2a660",
        _ => "#8c918c",
    };
    let inner = match skin.name() {
        Skin::CYAN_SYSTEM_ID => "#086f83",
        Skin::RESULT_AURORA_ID => "#b383cc",
        _ => "#444943",
    };
    let badge = "M34 4H88L97 13V21L88 30H34L25 21V13Z";
    rsx! { svg { class: "rail-frame", width: "{width}", height: "34", view_box: "0 0 {width} 34",
        path { d: contour(width,34.0,0.5,9.0), fill: "#020508", stroke: edge, stroke_width: "1" }
        path { d: contour(width,34.0,3.0,7.0), fill: "none", stroke: inner, stroke_width: "0.7" }
        path { d: badge, fill: "#0b1115", stroke: edge, stroke_width: "1" }
        path { d: format!("M{} 8l5 5v8l-5 5",width-14.0), fill: "none", stroke: edge, stroke_width: "1" }
    } }
}

pub fn lamp(state: crate::LampState, vertical: bool, skin: Skin, role: &str) -> Element {
    let accent = match skin.name() {
        Skin::CYAN_SYSTEM_ID => "#10dcfa",
        Skin::RESULT_AURORA_ID => "#c16aff",
        _ => "#c5e819",
    };
    let light = match state {
        crate::LampState::Active if vertical => accent,
        crate::LampState::Active => "#54e56b",
        crate::LampState::Error => "#ff5369",
        crate::LampState::Inactive => "#36434a",
    };
    let id = format!("lamp-{role}");
    let glow = format!("url(#{id})");
    let (width, height) = if vertical { (16, 64) } else { (24, 24) };
    rsx! { svg { width: "{width}", height: "{height}", view_box: "0 0 {width} {height}",
        defs { radialGradient { id: "{id}", cx: "40%", cy: "35%", r: "65%",
            stop { offset: "0", stop_color: if state == crate::LampState::Inactive { "#6c7981" } else { "#ffffff" } }
            stop { offset: "0.4", stop_color: light }
            stop { offset: "1", stop_color: "#051015" }
        } }
        if vertical {
            rect { x: "0.5", y: "0.5", width: "15", height: "63", fill: "#020709", stroke: accent, stroke_width: "1" }
            rect { x: "3", y: "3", width: "10", height: "58", fill: light }
            path { d: "M5 5V59", stroke: "#d5faff", stroke_width: "1", opacity: if state == crate::LampState::Inactive { "0.15" } else { "0.8" } }
        } else {
            circle { cx: "12", cy: "12", r: "11", fill: "#030a0d", stroke: light, stroke_width: "1" }
            circle { cx: "12", cy: "12", r: "8", fill: glow, stroke: "#87949b", stroke_width: "0.8" }
        }
    } }
}
