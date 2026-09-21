//! Native editor and surface geometry policy.

pub(crate) fn upper_right_x(output_width: i32, surface_width: u32, inset: i32) -> Option<i32> {
    output_width
        .checked_sub(i32::try_from(surface_width).ok()?)?
        .checked_sub(inset.max(0))
}

#[cfg(test)]
pub(crate) fn editor_geometry(
    position: [i32; 2],
    canvas_size: [u32; 2],
    output_size: Option<[u32; 2]>,
) -> ([i32; 2], [u32; 2]) {
    if let Some(output) = output_size {
        return ([0, 0], output);
    }
    (position, canvas_size)
}

#[cfg(test)]
pub(crate) fn editor_panel_width(output_width: Option<u32>) -> u32 {
    match output_width {
        Some(width) => (width / 5).clamp(360, 480),
        None => 400,
    }
}

#[cfg(test)]
mod tests {
    use super::upper_right_x;

    #[test]
    fn upper_right_position_uses_logical_output_width_and_inset() {
        assert_eq!(upper_right_x(1920, 560, 20), Some(1340));
        assert_eq!(upper_right_x(1920, 560, -20), Some(1360));
        assert_eq!(upper_right_x(100, 560, 20), Some(-480));
    }
}
