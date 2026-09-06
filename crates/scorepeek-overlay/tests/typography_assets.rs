// Verify embedded glyph coverage at the native image decoding boundary.
#[test]
fn metallic_atlases_preserve_glyph_coverage_and_transparency() {
    use scorepeek_overlay_ui::typography::{CELL_HEIGHT, CELL_WIDTH, GLYPHS};
    for name in ["gold", "silver"] {
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
