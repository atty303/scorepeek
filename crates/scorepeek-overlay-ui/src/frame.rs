//! Chamfered panel geometry measured against the original design sheets.
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

pub fn render(widget: &WidgetLayout, skin: Skin) -> Element {
    if skin == Skin::DjBlackbox {
        return rsx! { div { class: "hardware-frame",
            for edge in ["top", "bottom", "left", "right"] { div { class: "hardware-edge hardware-{edge}" } }
            for corner in ["nw", "ne", "sw", "se"] { div { class: "hardware-corner hardware-{corner}" } }
        } };
    }
    let width = f64::from(widget.width);
    let height = f64::from(widget.height);
    let outer = contour(width, height, 1.5, 10.0);
    let inner = contour(width, height, 10.0, 9.0);
    let band = format!("{outer} {inner}");
    let fine = contour(width, height, 5.0, 10.0);
    let (edge, trim, material) = match skin {
        Skin::CyanSystem => ("#0de0f6", "#117b8e", "#04141c"),
        Skin::ResultAurora => ("#d2cadb", "#bc78e5", "#393440"),
        Skin::DjBlackbox => ("#898d88", "#171b18", "#303430"),
    };
    let gradient_id = format!("panel-metal-{}", widget.id);
    let fill = if skin == Skin::CyanSystem {
        material.to_owned()
    } else {
        format!("url(#{gradient_id})")
    };
    let divider = format!("M{} 8l-40 {}", width * 0.45, height - 16.0);
    let accents = format!(
        "M3 18V12L13 2H28 M{} 2h21l10 10v6 M3 {}v6l10 10h15 M{} {}h21l10 -10v-6",
        width - 34.0,
        height - 19.0,
        width - 34.0,
        height - 3.0
    );
    // Explicit attributes also paint in Blitz's standalone SVG image renderer.
    rsx! { svg { class: "panel-frame", width: "{width}", height: "{height}", view_box: "0 0 {width} {height}",
        defs { linearGradient { id: "{gradient_id}", x1: "0", y1: "0", x2: "0", y2: "1",
            stop { offset: "0", stop_color: "#e0dce2" }
            stop { offset: "0.04", stop_color: material }
            stop { offset: "0.96", stop_color: "#181b1b" }
            stop { offset: "1", stop_color: "#767a77" }
        } }
        path { d: band, fill: fill, fill_rule: "evenodd", stroke: edge, stroke_width: "1" }
        path { d: fine, fill: "none", stroke: trim, stroke_width: "0.75" }
        if skin != Skin::CyanSystem {
            path { d: contour(width,height,3.0,10.0), fill: "none", stroke: "#111318", stroke_width: "1" }
            path { d: contour(width,height,7.5,9.0), fill: "none", stroke: "#85828a", stroke_width: "0.7" }
        }
        if widget.kind == WidgetKind::Score {
            path { d: format!("M{} 10l-5 5v{}l5 5",20.0+(width-40.0)*0.54,height-30.0), fill: "none", stroke: edge, stroke_width: "0.7" }
        }
        if skin != Skin::DjBlackbox {
            path { d: accents, fill: "none", stroke: edge, stroke_width: "2.5" }
        }
        if widget.kind == WidgetKind::Status {
            path { d: divider, fill: "none", stroke: edge, stroke_width: "1" }
        }
        if skin == Skin::ResultAurora {
            path { d: contour(width, height, 9.5, 7.0), fill: "none", stroke: "#b69b60", stroke_width: "0.6" }
        }

    } }
}

pub fn chart_rail(width: u32, skin: Skin) -> Element {
    let width = f64::from(width);
    let edge = match skin {
        Skin::CyanSystem => "#13dcef",
        Skin::ResultAurora => "#c2a660",
        Skin::DjBlackbox => "#8c918c",
    };
    let inner = match skin {
        Skin::CyanSystem => "#086f83",
        Skin::ResultAurora => "#b383cc",
        Skin::DjBlackbox => "#444943",
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
    let accent = match skin {
        Skin::CyanSystem => "#10dcfa",
        Skin::ResultAurora => "#c16aff",
        Skin::DjBlackbox => "#c5e819",
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
