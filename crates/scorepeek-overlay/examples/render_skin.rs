//! Render a selected skin through the production native DOM/paint stack, without Wayland.
//! Input: JSON {appearance, state}; output: create-only RGBA PAM image.
use anyrender::ImageRenderer;
use anyrender_vello::VelloImageRenderer;
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};
use dioxus_core::{Element, VirtualDom};
use dioxus_native_dom::DioxusDocument;
use scorepeek_overlay_ui::{Appearance, OverlayState, overlay_panel};
use serde::Deserialize;
use std::io::Write as _;

#[derive(Clone, Deserialize)]
struct Preview {
    appearance: Appearance,
    state: OverlayState,
}
fn app(Preview { state, appearance }: Preview) -> Element {
    overlay_panel(&state, appearance)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: render_skin INPUT.json OUTPUT.pam".into());
    }
    let preview: Preview = serde_json::from_slice(&std::fs::read(&args[0])?)?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[1])?;
    let mut document = DioxusDocument::new(
        VirtualDom::new_with_props(app, preview),
        scorepeek_overlay::native::document_config(),
    );
    document.initial_build();
    let mut inner = document.inner.borrow_mut();
    inner.set_viewport(Viewport::new(600, 1080, 1.0, ColorScheme::Dark));
    inner.resolve(0.0);
    inner.resolve(1.0);
    assert!(!inner.is_animating(), "skin must settle after confirmation");
    let mut renderer = VelloImageRenderer::new(600, 1080);
    let mut pixels = Vec::new();
    renderer.render_to_vec(
        |scene| paint_scene(scene, &mut inner, 1.0, 600, 1080, 0, 0),
        &mut pixels,
    );
    output.write_all(
        b"P7\nWIDTH 600\nHEIGHT 1080\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n",
    )?;
    output.write_all(&pixels)?;
    Ok(())
}
