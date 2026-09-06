//! Regenerate material typography from the bundled fonts through the production native renderer.
use anyrender::ImageRenderer;
use anyrender_vello::VelloImageRenderer;
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};
use dioxus::prelude::*;
use dioxus_native_dom::DioxusDocument;
use image::{Rgba, RgbaImage};
use scorepeek_overlay_ui::{
    Skin,
    typography::{
        CELL_HEIGHT, CELL_WIDTH, GLYPHS, LABEL_HEIGHT, LABEL_WIDTH, LABELS, Tone, label_font,
    },
};

const SOURCE_CELL_WIDTH: u32 = 80;

fn sheet((skin, labels): (Skin, bool)) -> Element {
    let font = if labels {
        label_font(skin)
    } else {
        match skin {
            Skin::CyanSystem => "Orbitron",
            Skin::ResultAurora => "Oxanium",
            Skin::DjBlackbox => "Rajdhani",
        }
    };
    rsx! {
        style { "html,body{{margin:0;background:black}} span{{position:absolute;color:white;font-family:{font};font-weight:600;white-space:nowrap}}" }
        if labels {
            for ((text, _), index) in LABELS.iter().zip(0_u32..) {
                span { style: "left:0;top:{index * LABEL_HEIGHT}px;font-size:20px;line-height:{LABEL_HEIGHT}px;letter-spacing:.04em", "{text}" }
            }
        } else {
            for (glyph, index) in GLYPHS.chars().zip(0_u32..) {
                span { style: "left:{index * SOURCE_CELL_WIDTH}px;top:0;font-size:72px;line-height:{CELL_HEIGHT}px;width:{SOURCE_CELL_WIDTH}px;text-align:center;font-weight:700", "{glyph}" }
            }
        }
    }
}

fn mask(skin: Skin, labels: bool) -> Result<RgbaImage, Box<dyn std::error::Error>> {
    let (width, height) = if labels {
        (LABEL_WIDTH, LABEL_HEIGHT * u32::try_from(LABELS.len())?)
    } else {
        (
            SOURCE_CELL_WIDTH * u32::try_from(GLYPHS.len())?,
            CELL_HEIGHT,
        )
    };
    let mut document = DioxusDocument::new(
        VirtualDom::new_with_props(sheet, (skin, labels)),
        scorepeek_overlay::native::document_config(),
    );
    document.initial_build();
    let mut dom = document.inner.borrow_mut();
    dom.set_viewport(Viewport::new(width, height, 3.0, ColorScheme::Dark));
    dom.resolve(0.0);
    let mut renderer = VelloImageRenderer::new(width * 3, height * 3);
    let mut pixels = Vec::new();
    renderer.render_to_vec(
        |scene| paint_scene(scene, &mut dom, 3.0, width * 3, height * 3, 0, 0),
        &mut pixels,
    );
    let mut mask =
        RgbaImage::from_raw(width * 3, height * 3, pixels).ok_or("invalid mask dimensions")?;
    for pixel in mask.pixels_mut() {
        *pixel = Rgba([255, 255, 255, pixel[0]]);
    }
    Ok(mask)
}

/// Fit a font uniformly, preserving narrow glyphs and transparent material padding.
fn fit_glyphs(mask: &RgbaImage) -> RgbaImage {
    let cells: Vec<_> = GLYPHS
        .chars()
        .zip(0_u32..)
        .map(|(_, index)| {
            let cell = image::imageops::crop_imm(
                mask,
                index * SOURCE_CELL_WIDTH * 3,
                0,
                SOURCE_CELL_WIDTH * 3,
                CELL_HEIGHT * 3,
            )
            .to_image();
            let xs: Vec<_> = cell
                .enumerate_pixels()
                .filter(|(_, _, p)| p[3] != 0)
                .map(|(x, _, _)| x)
                .collect();
            let left = *xs.iter().min().expect("painted glyph");
            let right = *xs.iter().max().expect("painted glyph");
            image::imageops::crop_imm(&cell, left, 0, right - left + 1, CELL_HEIGHT * 3).to_image()
        })
        .collect();
    let widest = cells
        .iter()
        .map(image::GenericImageView::width)
        .max()
        .expect("glyphs");
    let cell_width = CELL_WIDTH * 3;
    let mut output = RgbaImage::new(
        cell_width * u32::try_from(cells.len()).expect("glyph count"),
        CELL_HEIGHT * 3,
    );
    for (cell, index) in cells.iter().zip(0_u32..) {
        let width = (cell.width() * (cell_width - 12) / widest).max(1);
        let glyph = image::imageops::resize(
            cell,
            width,
            CELL_HEIGHT * 3,
            image::imageops::FilterType::Lanczos3,
        );
        image::imageops::replace(
            &mut output,
            &glyph,
            i64::from(index * cell_width + (cell_width - width) / 2),
            0,
        );
    }
    output
}

fn alpha(mask: &RgbaImage, x: i32, y: i32) -> i32 {
    let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) else {
        return 0;
    };
    mask.get_pixel_checked(x, y).map_or(0, |p| i32::from(p[3]))
}

fn tint(skin: Skin, tone: Tone) -> [i32; 3] {
    match tone {
        Tone::Heading => match skin {
            Skin::CyanSystem => [35, 224, 255],
            Skin::ResultAurora => [224, 163, 255],
            Skin::DjBlackbox => [206, 230, 107],
        },
        Tone::Metric => match skin {
            Skin::CyanSystem => [70, 222, 255],
            Skin::ResultAurora => [244, 236, 252],
            Skin::DjBlackbox => [235, 205, 121],
        },
        Tone::Silver => [235, 242, 253],
        Tone::Gold => [255, 215, 98],
        Tone::Cyan => [90, 226, 255],
        Tone::Purple => [236, 184, 249],
        Tone::Lime => [198, 238, 114],
        Tone::Red => [255, 119, 113],
        Tone::Pink => [255, 93, 177],
    }
}

fn mix(pixel: &mut Rgba<u8>, rgb: [i32; 3], opacity: i32) {
    let a = opacity.clamp(0, 255);
    let old = i32::from(pixel[3]) * (255 - a) / 255;
    let total = a + old;
    if total == 0 {
        return;
    }
    for c in 0..3 {
        pixel[c] = u8::try_from(
            ((rgb[c].clamp(0, 255) * a + i32::from(pixel[c]) * old) / total).clamp(0, 255),
        )
        .expect("clamped channel");
    }
    pixel[3] = u8::try_from(total).expect("alpha composition");
}

fn material(mask: &RgbaImage, skin: Skin, tone: Tone, large: bool) -> RgbaImage {
    let mut output = RgbaImage::new(mask.width(), mask.height());
    let ys: Vec<_> = mask
        .enumerate_pixels()
        .filter(|(_, _, p)| p[3] > 128)
        .map(|(_, y, _)| y)
        .collect();
    let top = *ys.iter().min().unwrap_or(&0);
    let bottom = *ys.iter().max().unwrap_or(&1);
    let color = tint(skin, tone);
    for (x, y, pixel) in output.enumerate_pixels_mut() {
        let ix = i32::try_from(x).expect("atlas width");
        let iy = i32::try_from(y).expect("atlas height");
        let coverage = alpha(mask, ix, iy);
        let radius = if large { 3 } else { 1 };
        let mut outer = coverage;
        let mut inner = coverage;
        for (dx, dy) in [
            (-radius, 0),
            (radius, 0),
            (0, -radius),
            (0, radius),
            (-radius, -radius),
            (radius, radius),
            (-radius, radius),
            (radius, -radius),
        ] {
            let sample = alpha(mask, ix + dx, iy + dy);
            outer = outer.max(sample);
            inner = inner.min(sample);
        }
        let relief = alpha(mask, ix + 2, iy + 2) - alpha(mask, ix - 2, iy - 2);
        // Separate the silhouette, reflected bevel, and face: a gradient alone has no material edge.
        mix(pixel, [3, 5, 9], alpha(mask, ix - 3, iy - 4) * 3 / 4);
        if skin == Skin::CyanSystem {
            mix(pixel, [25, 126, 200], outer / 5);
        }
        let edge = if skin == Skin::ResultAurora {
            color.map(|v| v * 3 / 4 + relief / 5)
        } else {
            color.map(|v| v * 2 / 3 + relief / 4)
        };
        mix(pixel, edge, outer);
        mix(pixel, [8, 9, 14], coverage);
        let t = i32::try_from(y.saturating_sub(top) * 1000 / (bottom - top).max(1))
            .expect("face position")
            .min(1000);
        let light = match skin {
            Skin::CyanSystem => 1020 - t * 12 / 100,
            Skin::ResultAurora => {
                if t < 440 {
                    1100 - t * 45 / 100
                } else if t < 580 {
                    750 + (t - 440) * 2
                } else {
                    1030 - (t - 580) * 30 / 100
                }
            }
            Skin::DjBlackbox => 970 - t * 14 / 100,
        };
        let grain = if skin == Skin::DjBlackbox {
            i32::try_from((x * 17 + y * 131 + (x * y) % 19) % 13).expect("grain") - 6
        } else {
            0
        };
        let face = color.map(|v| v * light / 1000 + grain + relief / 7);
        mix(pixel, face, inner);
        // A narrow upper-left specular line remains visible after downsampling.
        let bevel = (coverage - inner).max(0);
        mix(pixel, color.map(|v| v + relief / 3 + 25), bevel * 3 / 4);
    }
    output
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args()
        .nth(1)
        .ok_or("usage: generate_type_atlas NEW_OUTPUT_DIRECTORY")?;
    std::fs::create_dir(&output)?;
    for skin in [Skin::CyanSystem, Skin::ResultAurora, Skin::DjBlackbox] {
        let glyph_mask = mask(skin, false)?;
        let glyph_mask = fit_glyphs(&glyph_mask);
        let tone = if skin == Skin::ResultAurora {
            Tone::Gold
        } else {
            Tone::Silver
        };
        material(&glyph_mask, skin, tone, true)
            .save(format!("{output}/type-{}.png", skin.name()))?;
        let label_mask = mask(skin, true)?;
        let mut labels = RgbaImage::new(label_mask.width(), label_mask.height());
        for ((_, tone), index) in LABELS.iter().zip(0_u32..) {
            let row = image::imageops::crop_imm(
                &label_mask,
                0,
                index * LABEL_HEIGHT * 3,
                LABEL_WIDTH * 3,
                LABEL_HEIGHT * 3,
            )
            .to_image();
            image::imageops::replace(
                &mut labels,
                &material(&row, skin, *tone, false),
                0,
                i64::from(index * LABEL_HEIGHT * 3),
            );
        }
        labels.save(format!("{output}/labels-{}.png", skin.name()))?;
    }
    Ok(())
}
