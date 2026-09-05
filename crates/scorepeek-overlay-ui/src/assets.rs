//! Fixed, embedded artwork shared by both renderers.
pub const SKIN_ASSETS: &[(&str, &[u8])] = &[
    (
        "/skins/cyan-circuit.png",
        include_bytes!("../assets/skins/cyan-circuit.png"),
    ),
    (
        "/skins/aurora-energy.png",
        include_bytes!("../assets/skins/aurora-energy.png"),
    ),
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

/// Fonts and their licenses are delivered from the executable in both renderers.
pub const FONT_ASSETS: &[(&str, &[u8])] = &[
    (
        "orbitron.ttf",
        include_bytes!("../assets/fonts/Orbitron.ttf"),
    ),
    (
        "rajdhani.ttf",
        include_bytes!("../assets/fonts/Rajdhani-SemiBold.ttf"),
    ),
];
pub const FONT_CSS: &str = "@font-face{font-family:Orbitron;src:url('/fonts/orbitron.ttf');font-weight:400 900}@font-face{font-family:Rajdhani;src:url('/fonts/rajdhani.ttf');font-weight:600}";

/// Redistributed font licenses, including attribution and reserved-name terms.
pub const FONT_LICENSES: &[(&str, &str)] = &[
    (
        "orbitron-OFL.txt",
        include_str!("../assets/fonts/orbitron-OFL.txt"),
    ),
    (
        "rajdhani-OFL.txt",
        include_str!("../assets/fonts/rajdhani-OFL.txt"),
    ),
];
