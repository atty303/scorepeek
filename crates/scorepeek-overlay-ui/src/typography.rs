//! Shared semantic text and its skin-specific material decoration.
use crate::Skin;
use dioxus::prelude::*;

pub const GLYPHS: &str = "0123456789ABCDEFG-";
pub const CELL_WIDTH: u32 = 44;
pub const CELL_HEIGHT: u32 = 80;
pub const LABEL_WIDTH: u32 = 256;
pub const LABEL_HEIGHT: u32 = 24;

#[derive(Clone, Copy)]
pub enum Tone {
    Heading,
    Metric,
    Silver,
    Gold,
    Cyan,
    Purple,
    Lime,
    Red,
    Pink,
}

pub const LABELS: &[(&str, Tone)] = &[
    ("SYSTEM", Tone::Silver),
    ("RESULT", Tone::Silver),
    ("BEST", Tone::Heading),
    ("RESULT DETAIL", Tone::Heading),
    ("HISTORY", Tone::Heading),
    ("HISTORY GRAPH", Tone::Heading),
    ("EX SCORE", Tone::Metric),
    ("DJ LEVEL", Tone::Metric),
    ("MISS COUNT", Tone::Metric),
    ("LV", Tone::Heading),
    ("NOTES", Tone::Heading),
    ("SP", Tone::Silver),
    ("DP", Tone::Silver),
    ("NORMAL", Tone::Cyan),
    ("HYPER", Tone::Gold),
    ("ANOTHER", Tone::Red),
    ("LEGGENDARIA", Tone::Purple),
    ("PGREAT", Tone::Purple),
    ("GREAT", Tone::Cyan),
    ("GOOD", Tone::Lime),
    ("BAD", Tone::Gold),
    ("POOR", Tone::Red),
    ("FAST", Tone::Cyan),
    ("SLOW", Tone::Pink),
    ("COMBO BREAK", Tone::Heading),
    ("PLAY OPTIONS", Tone::Heading),
    ("DATE", Tone::Heading),
    ("MISS", Tone::Heading),
    ("CLEAR", Tone::Heading),
    ("MISS RATE", Tone::Gold),
    ("NO PLAY", Tone::Silver),
    ("FAILED", Tone::Red),
    ("ASSIST", Tone::Purple),
    ("EASY", Tone::Lime),
    ("HARD", Tone::Heading),
    ("EX HARD", Tone::Gold),
    ("ASSIST CLEAR", Tone::Purple),
    ("EASY CLEAR", Tone::Lime),
    ("HARD CLEAR", Tone::Heading),
    ("EX HARD CLEAR", Tone::Gold),
    ("FULL COMBO", Tone::Gold),
    ("AAA", Tone::Heading),
    ("AA", Tone::Heading),
    ("A", Tone::Heading),
    ("B", Tone::Heading),
    ("C", Tone::Heading),
    ("D", Tone::Heading),
    ("E", Tone::Heading),
    ("100%", Tone::Gold),
    ("75%", Tone::Gold),
    ("50%", Tone::Gold),
    ("25%", Tone::Gold),
    ("0%", Tone::Gold),
];

#[must_use]
pub const fn label_font(skin: Skin) -> &'static str {
    match skin {
        Skin::DjBlackbox => "Rajdhani",
        _ => "Oxanium",
    }
}

pub(crate) fn metallic(value: &str, skin: Skin, height: u32, rank: bool) -> Element {
    if value.is_empty() || !value.chars().all(|c| GLYPHS.contains(c)) {
        return rsx! { "{value}" };
    }
    let width = f64::from(CELL_WIDTH * height) / f64::from(CELL_HEIGHT);
    let sheet_width = width * f64::from(GLYPHS.chars().map(|_| 1_u32).sum::<u32>());
    let material = if rank && skin == Skin::CyanSystem {
        Skin::ResultAurora
    } else {
        skin
    }
    .name();
    rsx! { span { class: "metal-type",
        span { class: "type-value", "{value}" }
        for glyph in value.chars() {
            span { class: "metal-glyph", aria_hidden: "true", style: "width:{width}px;height:{height}px;background-image:url('/skins/type-{material}.png');background-size:{sheet_width}px {height}px;background-position:-{f64::from(GLYPHS.chars().zip(0_u32..).find(|(c, _)| *c == glyph).map_or(0, |(_, index)| index)) * width}px 0" }
        }
    } }
}

/// Whole words preserve proportional spacing. The ordinary text reserves layout and the accessible name.
pub(crate) fn label(value: &str, skin: Skin, height: u32) -> Element {
    let Some((_, index)) = LABELS
        .iter()
        .zip(0_u32..)
        .find(|((text, _), _)| *text == value)
    else {
        return rsx! { "{value}" };
    };
    let scale = f64::from(height) / f64::from(LABEL_HEIGHT);
    let width = f64::from(LABEL_WIDTH) * scale;
    let sheet_height = f64::from(LABELS.iter().map(|_| LABEL_HEIGHT).sum::<u32>()) * scale;
    let font = label_font(skin);
    let material = skin.name();
    rsx! { span { class: "material-label", style: "font-family:{font};font-size:{scale * 20.0}px;line-height:{height}px;height:{height}px",
        span { class: "label-value", "{value}" }
        span { class: "label-art", aria_hidden: "true", style: "width:{width}px;height:{height}px;background-image:url('/skins/labels-{material}.png');background-size:{width}px {sheet_height}px;background-position:0 -{index * height}px" }
    } }
}
