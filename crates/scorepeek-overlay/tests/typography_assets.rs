// Verify embedded glyph coverage at the native image decoding boundary.
#[test]
fn metallic_atlases_preserve_glyph_coverage_and_transparency() {
    use scorepeek_overlay_ui::typography::{CELL_HEIGHT, CELL_WIDTH, GLYPHS};
    for name in ["cyan-system", "result-aurora", "dj-blackbox"] {
        let path = format!("/skins/type-{name}.png");
        let image = image::load_from_memory(scorepeek_overlay_ui::skin_asset(&path).unwrap())
            .unwrap()
            .into_rgba8();
        assert_eq!(
            image.dimensions(),
            (
                CELL_WIDTH * 3 * u32::try_from(GLYPHS.len()).unwrap(),
                CELL_HEIGHT * 3
            )
        );
        for index in 0..u32::try_from(GLYPHS.len()).unwrap() {
            let x = index * CELL_WIDTH * 3;
            assert_eq!(
                image.get_pixel(x, 0)[3],
                0,
                "{path}: transparent cell padding"
            );
            for y in 0..CELL_HEIGHT * 3 {
                assert_eq!(
                    image.get_pixel(x, y)[3],
                    0,
                    "{path}: glyph {index} left edge must remain transparent"
                );
                assert_eq!(
                    image.get_pixel(x + CELL_WIDTH * 3 - 1, y)[3],
                    0,
                    "{path}: glyph {index} right edge must remain transparent"
                );
            }
            let colors: std::collections::BTreeSet<_> = (x..x + CELL_WIDTH * 3)
                .flat_map(|x| (0..CELL_HEIGHT * 3).map(move |y| (x, y)))
                .map(|(x, y)| image.get_pixel(x, y))
                .filter(|p| p[3] == 255)
                .map(|p| [p[0], p[1], p[2]])
                .collect();
            assert!(
                colors.len() > 8,
                "{path}: glyph {index} must contain a painted gradient"
            );
        }
    }
}

#[test]
fn label_atlases_paint_every_semantic_label_without_bleeding_between_rows() {
    use scorepeek_overlay_ui::typography::{LABEL_HEIGHT, LABEL_WIDTH, LABELS};
    for name in ["cyan-system", "result-aurora", "dj-blackbox"] {
        let path = format!("/skins/labels-{name}.png");
        let image = image::load_from_memory(scorepeek_overlay_ui::skin_asset(&path).unwrap())
            .unwrap()
            .into_rgba8();
        assert_eq!(
            image.dimensions(),
            (
                LABEL_WIDTH * 3,
                LABEL_HEIGHT * 3 * u32::try_from(LABELS.len()).unwrap()
            )
        );
        for ((label, _), index) in LABELS.iter().zip(0_u32..) {
            let row = image::imageops::crop_imm(
                &image,
                0,
                index * LABEL_HEIGHT * 3,
                LABEL_WIDTH * 3,
                LABEL_HEIGHT * 3,
            );
            let colors: std::collections::BTreeSet<_> = row
                .to_image()
                .pixels()
                .filter(|p| p[3] > 240)
                .map(|p| [p[0], p[1], p[2]])
                .collect();
            assert!(
                colors.len() > 8,
                "{path}: {label} must be painted with material shading"
            );
            for x in 0..LABEL_WIDTH * 3 {
                assert_eq!(
                    image.get_pixel(x, (index + 1) * LABEL_HEIGHT * 3 - 1)[3],
                    0,
                    "{path}: {label} must not bleed into the next row"
                );
            }
        }
    }
}
