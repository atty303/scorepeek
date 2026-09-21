pub(crate) fn aperture_mask(holes: impl IntoIterator<Item = (i64, i64, u32, u32)>) -> String {
    let mut images = vec!["linear-gradient(white,white)".to_owned()];
    let mut sizes = vec!["100% 100%".to_owned()];
    let mut positions = vec!["0px 0px".to_owned()];
    for (x, y, width, height) in holes {
        let cut = (width / 4).min(12).min(height / 4);
        let right = width.saturating_sub(cut);
        let bottom = height.saturating_sub(cut);
        images.push(format!(
            "url(\"data:image/svg+xml,%3Csvg%20xmlns=%27http://www.w3.org/2000/svg%27%20width=%27{width}%27%20height=%27{height}%27%3E%3Cpath%20fill=%27white%27%20d=%27M{cut}%200H{right}L{width}%20{cut}V{bottom}L{right}%20{height}H{cut}L0%20{bottom}V{cut}Z%27/%3E%3C/svg%3E\")"
        ));
        sizes.push(format!("{width}px {height}px"));
        positions.push(format!("{x}px {y}px"));
    }
    if images.len() == 1 {
        return String::new();
    }
    let mut composites = vec!["subtract"; images.len()];
    composites[1..].fill("add");
    format!(
        "mask-image:{};mask-size:{};mask-position:{};mask-repeat:no-repeat;mask-composite:{};",
        images.join(","),
        sizes.join(","),
        positions.join(","),
        composites.join(",")
    )
}
