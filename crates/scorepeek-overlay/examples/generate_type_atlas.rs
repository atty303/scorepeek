//! Regenerate the embedded metallic Latin glyph sheets using the pinned native renderer.
use anyrender::ImageRenderer;
use anyrender_vello::VelloImageRenderer;
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};
use dioxus::prelude::*;
use dioxus_native_dom::DioxusDocument;
use image::{Rgba, RgbaImage};
use scorepeek_overlay_ui::typography::{CELL_HEIGHT, CELL_WIDTH, GLYPHS};

fn glyphs() -> Element {
    rsx! {
        style { "html,body{{margin:0;background:black}} span{{position:absolute;color:white;font-family:Oxanium;font-size:72px;font-weight:600;line-height:{CELL_HEIGHT}px;text-align:center;transform:scaleX(.86);width:{CELL_WIDTH}px;height:{CELL_HEIGHT}px}}" }
        for (glyph, index) in GLYPHS.chars().zip(0_u32..) {
            span { style: "left:{index * CELL_WIDTH}px;top:0", "{glyph}" }
        }
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args()
        .nth(1)
        .ok_or("usage: generate_type_atlas NEW_OUTPUT_DIRECTORY")?;
    let width = CELL_WIDTH * u32::try_from(GLYPHS.len())?;
    let height = CELL_HEIGHT;
    let mut document = DioxusDocument::new(
        VirtualDom::new(glyphs),
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
    let mut mask = RgbaImage::from_raw(width * 3, height * 3, pixels)
        .ok_or("invalid native image dimensions")?;
    // White glyph coverage on black avoids dependence on the root canvas alpha policy.
    for pixel in mask.pixels_mut() {
        pixel[3] = pixel[0];
    }
    let ink: Vec<_> = mask
        .enumerate_pixels()
        .filter(|(_, _, p)| p[3] > 128)
        .map(|(_, y, _)| y)
        .collect();
    let top = *ink.iter().min().ok_or("font sheet has no painted glyphs")?;
    let bottom = *ink.iter().max().ok_or("font sheet has no painted glyphs")?;
    std::fs::create_dir(&output)?;
    for (name, stops) in [
        (
            "silver",
            [
                (0_u32, [255, 255, 255]),
                (360, [204, 224, 239]),
                (490, [111, 142, 166]),
                (520, [249, 254, 255]),
                (1000, [153, 180, 200]),
            ],
        ),
        (
            "gold",
            [
                (0_u32, [255, 251, 212]),
                (360, [255, 221, 115]),
                (490, [165, 106, 30]),
                (520, [255, 243, 178]),
                (1000, [218, 159, 51]),
            ],
        ),
    ] {
        let mut atlas = mask.clone();
        for (x, y, pixel) in atlas.enumerate_pixels_mut() {
            let t = (y.saturating_sub(top) * 1000 / (bottom - top)).min(1000);
            let pair = stops
                .windows(2)
                .find(|p| t <= p[1].0)
                .unwrap_or(&stops[3..]);
            let distance = i32::try_from(t - pair[0].0)?;
            let span = i32::try_from(pair[1].0 - pair[0].0)?;
            let above = mask.get_pixel(x, y.saturating_sub(2))[3];
            let below = mask.get_pixel(x, (y + 2).min(height * 3 - 1))[3];
            let relief = (i32::from(below) - i32::from(above)) * 45 / 255;
            let rgb = std::array::from_fn::<_, 3, _>(|c| {
                u8::try_from(
                    (pair[0].1[c] + (pair[1].1[c] - pair[0].1[c]) * distance / span + relief)
                        .clamp(0, 255),
                )
                .expect("clamped color channel")
            });
            *pixel = Rgba([rgb[0], rgb[1], rgb[2], pixel[3]]);
        }
        atlas.save(format!("{output}/type-{name}.png"))?;
    }
    Ok(())
}
