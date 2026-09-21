//! Vello scene construction and embedded-image atlas retention.

use anyrender::PaintScene;
use blitz_paint::paint_scene;
use std::sync::Arc;

pub(crate) fn paint_native_scene(
    scene: &mut impl PaintScene,
    document: &mut blitz_dom::BaseDocument,
    scale: f64,
    width: u32,
    height: u32,
) {
    paint_scene(scene, document, scale, width, height, 0, 0);
    retain_native_image_atlas(scene);
}

pub(crate) fn retain_native_image_atlas(scene: &mut impl PaintScene) {
    // Vello keeps image residency metadata separately from its persistent atlas.
    // A frame without image patches otherwise replaces the atlas with a 1x1 texture
    // without invalidating that metadata, so later raster images are not uploaded.
    static ATLAS_KEEPALIVE: std::sync::LazyLock<peniko::ImageBrush> =
        std::sync::LazyLock::new(|| {
            peniko::ImageBrush::new(peniko::ImageData {
                data: peniko::Blob::new(Arc::new(vec![0_u8; 4])),
                format: peniko::ImageFormat::Rgba8,
                alpha_type: peniko::ImageAlphaType::Alpha,
                width: 1,
                height: 1,
            })
        });
    scene.fill(
        peniko::Fill::NonZero,
        peniko::kurbo::Affine::IDENTITY,
        ATLAS_KEEPALIVE.as_ref(),
        None,
        &peniko::kurbo::Rect::new(0.0, 0.0, 1.0, 1.0),
    );
}

pub(crate) fn resolve_with_loaded_resources(document: &mut blitz_dom::BaseDocument, seconds: f64) {
    document.resolve(seconds);
    // Embedded images complete synchronously during resolve. Ingest their messages, then
    // resolve again so their layers are available to the current paint.
    document.handle_messages();
    document.resolve(seconds);
}
