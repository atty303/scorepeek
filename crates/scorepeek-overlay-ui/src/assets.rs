//! Fixed, embedded artwork shared by both renderers.
pub const SKIN_ASSETS: &[(&str, &[u8])] = &[
    (
        "/skins/cyan-system-frame.png",
        include_bytes!("../assets/skins/cyan-system-frame.png"),
    ),
    (
        "/skins/result-aurora-frame.png",
        include_bytes!("../assets/skins/result-aurora-frame.png"),
    ),
    (
        "/skins/dj-blackbox-frame.png",
        include_bytes!("../assets/skins/dj-blackbox-frame.png"),
    ),
    (
        "/skins/result-aurora-header.png",
        include_bytes!("../assets/skins/result-aurora-header.png"),
    ),
];

#[must_use]
pub fn skin_asset(path: &str) -> Option<&'static [u8]> {
    SKIN_ASSETS
        .iter()
        .find_map(|(name, bytes)| (*name == path).then_some(*bytes))
}
