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
    let width = f64::from(widget.width);
    let height = f64::from(widget.height);
    let outer = contour(width, height, 1.5, 10.0);
    let inner = contour(width, height, 8.0, 7.0);
    let band = format!("{outer} {inner}");
    let fine = contour(width, height, 5.0, 8.0);
    let (edge, trim, material) = match skin {
        Skin::CyanSystem => ("#0de0f6", "#117b8e", "#04141c"),
        Skin::ResultAurora => ("#d2cadb", "#bc78e5", "#393440"),
        Skin::DjBlackbox => ("#898d88", "#363b37", "#303430"),
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
    let screws = [
        (12.0, 12.0),
        (width - 12.0, 12.0),
        (12.0, height - 12.0),
        (width - 12.0, height - 12.0),
    ];
    // Explicit attributes also paint in Blitz's standalone SVG image renderer.
    rsx! { svg { class: "panel-frame", width: "{width}", height: "{height}", view_box: "0 0 {width} {height}",
        defs { linearGradient { id: "{gradient_id}", x1: "0", y1: "0", x2: "0", y2: "1",
            stop { offset: "0", stop_color: "#aaa7ad" }
            stop { offset: "0.07", stop_color: material }
            stop { offset: "0.92", stop_color: "#181b1b" }
            stop { offset: "1", stop_color: "#767a77" }
        } }
        path { d: band, fill: fill, fill_rule: "evenodd", stroke: edge, stroke_width: "1" }
        path { d: fine, fill: "none", stroke: trim, stroke_width: "0.75" }
        if skin != Skin::DjBlackbox {
            path { d: accents, fill: "none", stroke: edge, stroke_width: "2.5" }
        }
        if widget.kind == WidgetKind::Status {
            path { d: divider, fill: "none", stroke: edge, stroke_width: "1" }
        }
        if skin == Skin::ResultAurora {
            path { d: contour(width, height, 9.5, 7.0), fill: "none", stroke: "#b69b60", stroke_width: "0.6" }
        }
        if skin == Skin::DjBlackbox {
            for x in [26.0,30.0,34.0,38.0,width-38.0,width-34.0,width-30.0,width-26.0] {
                path { d: format!("M{x} 4v4"), fill: "none", stroke: "#c1d928", stroke_width: "1" }
            }
            for (x,y) in screws {
                circle { cx: "{x}", cy: "{y}", r: "5", fill: "#060807", stroke: "#999d97", stroke_width: "1" }
                circle { cx: "{x}", cy: "{y}", r: "2", fill: "#4b514b" }
            }
        }
    } }
}
