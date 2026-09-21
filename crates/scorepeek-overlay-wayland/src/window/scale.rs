/// Stable output metadata exposed without leaking a Wayland proxy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputDescription {
    pub name: String,
    pub model: String,
    pub logical_size: Option<[u32; 2]>,
}

pub(crate) fn scaled_size(logical: u32, scale_120: u32) -> u32 {
    u32::try_from((u64::from(logical) * u64::from(scale_120)).div_ceil(120)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::scaled_size;

    #[test]
    fn integer_and_fractional_buffers_round_up() {
        assert_eq!(scaled_size(1920, 120), 1920);
        assert_eq!(scaled_size(960, 240), 1920);
        assert_eq!(scaled_size(1001, 150), 1252);
    }
}
