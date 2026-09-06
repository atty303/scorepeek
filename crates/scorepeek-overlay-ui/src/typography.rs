//! Material typography; values remain semantic text independently of their decoration.
use dioxus::prelude::*;

pub const GLYPHS: &str = "0123456789ABCDEFG-";
pub const CELL_WIDTH: u32 = 44;
pub const CELL_HEIGHT: u32 = 80;

pub(crate) fn metallic(value: &str, gold: bool, height: u32) -> Element {
    if value.is_empty() || !value.chars().all(|c| GLYPHS.contains(c)) {
        return rsx! { "{value}" };
    }
    let width = f64::from(CELL_WIDTH * height) / f64::from(CELL_HEIGHT);
    let sheet_width = width * f64::from(GLYPHS.chars().map(|_| 1_u32).sum::<u32>());
    let material = if gold { "gold" } else { "silver" };
    rsx! { span { class: "metal-type",
        span { class: "type-value", "{value}" }
        for glyph in value.chars() {
            span { class: "metal-glyph", aria_hidden: "true", style: "width:{width}px;height:{height}px;background-image:url('/skins/type-{material}.png');background-size:{sheet_width}px {height}px;background-position:-{f64::from(GLYPHS.chars().zip(0_u32..).find(|(c, _)| *c == glyph).map_or(0, |(_, index)| index)) * width}px 0" }
        }
    } }
}

/// Clip identically shaped text into one-pixel bands; no glyph coverage or system font is replaced.
pub(crate) fn sheen(value: &str, height: u32, tint: [u8; 3]) -> Element {
    rsx! { span { class: "sheen-type", style: "height:{height}px;line-height:{height}px",
        span { class: "type-value", "{value}" }
        for row in 0..height {
            span { class: "sheen-band", aria_hidden: "true", style: "top:{row}px;color:{sheen_color(row, height, tint)}",
                span { style: "top:-{row}px", "{value}" }
            }
        }
    } }
}
fn sheen_color(row: u32, height: u32, tint: [u8; 3]) -> String {
    let t = row * 1000 / height;
    // A soft upper face, a narrow reflected horizon, then a shaded lower face.
    let light = if t < 450 {
        980 - t * 6 / 10
    } else if t < 560 {
        1000
    } else {
        1000 - (t - 560) * 7 / 10
    };
    let [r, g, b] = tint.map(|v| u32::from(v) * light / 1000);
    format!("rgb({r},{g},{b})")
}
