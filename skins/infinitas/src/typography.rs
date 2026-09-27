use crate::{CHARS, element, glyphs, live};
use scorepeek_skin_sdk::Node;
pub fn letters(key: &str, value: &str, size: f64, tone: &str) -> Node {
    if !value.chars().all(|c| CHARS.contains(c)) {
        return live(key, "dynamic", value);
    }
    let colors = colors(tone);
    let gradient = format!("{key}-face-gradient");
    let stops = if tone == "gold" {
        vec![
            ("0", colors[0]),
            ("0.35", colors[1]),
            ("0.49", colors[2]),
            ("0.51", colors[0]),
            ("0.7", colors[3]),
            ("1", colors[2]),
        ]
    } else {
        vec![
            ("0", colors[0]),
            ("0.42", colors[1]),
            ("0.46", colors[2]),
            ("0.50", colors[3]),
            ("1", colors[2]),
        ]
    };
    let mut paths = vec![element(
        &format!("{key}-defs"),
        "defs",
        &[],
        vec![element(
            &gradient,
            "linearGradient",
            &[
                ("id", &gradient),
                ("gradientUnits", "userSpaceOnUse"),
                ("x1", "0"),
                ("y1", "4"),
                ("x2", "0"),
                ("y2", "60"),
            ],
            stops
                .into_iter()
                .enumerate()
                .map(|(i, (offset, color))| {
                    element(
                        &format!("{key}-stop-{i}"),
                        "stop",
                        &[("offset", offset), ("stop-color", color)],
                        vec![],
                    )
                })
                .collect(),
        )],
    )];
    let mut advance = 0.0;
    for (i, c) in value.chars().enumerate() {
        let (width, path) = glyphs::GLYPHS[CHARS.find(c).unwrap_or(0)];
        paths.push(element(
            &format!("{key}-glyph-{i}"),
            "path",
            &[
                ("d", path),
                ("transform", &format!("translate({advance} 0)")),
                ("fill", &format!("url(#{gradient})")),
                ("stroke", colors[1]),
                ("stroke-width", "0.35"),
                ("stroke-linejoin", "round"),
            ],
            vec![],
        ));
        advance += width;
    }
    let width = (advance + 4.0) * size / 80.0;
    element(
        key,
        "span",
        &[
            ("class", &format!("lettering tone-{tone}")),
            (
                "style",
                &format!("font-size:{size}px;width:{width}px;height:{size}px;"),
            ),
        ],
        vec![
            live(&format!("{key}-semantic"), "semantic", value),
            element(
                &format!("{key}-drawing"),
                "svg",
                &[
                    ("class", "glyph-drawing"),
                    ("width", &width.to_string()),
                    ("height", &size.to_string()),
                    ("viewBox", &format!("0 0 {} 80", advance + 4.0)),
                    ("aria-hidden", "true"),
                ],
                paths,
            ),
        ],
    )
}

fn colors(tone: &str) -> [&str; 4] {
    match tone {
        "gold" => ["#fff9ce", "#f4cc65", "#a9731d", "#ffe590"],
        "prism" => ["#ffffff", "#e9e9ff", "#a6b7ed", "#e1fdff"],
        "ice" => ["#f2ffff", "#b0edff", "#67b9d8", "#c0f9ff"],
        "label" => ["#f2fdff", "#c2dfed", "#7196b1", "#e7f8ff"],
        "red" => ["#ffe1e7", "#ff9aaa", "#de546d", "#ffb4bf"],
        "blue" => ["#d5f6ff", "#86e3ff", "#39b5e5", "#a6ebff"],
        "green" => ["#e6ffeb", "#a5edc0", "#63c989", "#c1f7d3"],
        "purple" => ["#faeaff", "#dbb0ff", "#af7fe5", "#e8caff"],
        "yellow" => ["#fffbd9", "#ffe79c", "#ceae50", "#fff0bb"],
        "neutral" => ["#b5c4d0", "#9aaeba", "#788c9b", "#a7b9c7"],
        _ => ["#ffffff", "#e2f3ff", "#7aa4c0", "#eaf9ff"],
    }
}
