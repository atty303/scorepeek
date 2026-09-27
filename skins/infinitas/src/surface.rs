use crate::{element, palette::Palette};
use scorepeek_skin_sdk::Node;
fn outline(width: f64, height: f64, inset: f64, chamfer: f64) -> String {
    let (left, top, right, bottom) = (inset, inset, width - inset, height - inset);
    format!(
        "M {} {top} H {} L {right} {} V {} L {} {bottom} H {} L {left} {} V {} Z",
        left + chamfer,
        right - chamfer,
        top + chamfer,
        bottom - chamfer,
        right - chamfer,
        left + chamfer,
        bottom - chamfer,
        top + chamfer
    )
}
pub fn frame(id: &str, width: u32, height: u32, p: Palette, open: bool) -> Node {
    let (w, h) = (f64::from(width), f64::from(height));
    let metal_id = format!("{id}-metal");
    let face_id = format!("{id}-glass");
    let mut nodes = vec![element(
        &format!("{id}-defs"),
        "defs",
        &[],
        vec![
            gradient(
                &metal_id,
                &[
                    ("0", "#ffffff"),
                    ("0.18", "#90a9c3"),
                    ("0.35", "#f3fbff"),
                    ("0.49", "#6983a0"),
                    ("0.52", "#c9dce9"),
                    ("0.80", "#7386a5"),
                    ("1", "#f5fdff"),
                ],
            ),
            gradient(
                &face_id,
                &[
                    ("0", p.middle),
                    ("0.12", p.dark),
                    ("0.65", p.dark),
                    ("1", p.middle),
                ],
            ),
        ],
    )];
    for (n, inset, chamfer, fill, stroke, thickness) in [
        (
            "shell",
            2.0,
            11.0,
            format!("url(#{metal_id})"),
            "#d4eafa",
            1.0,
        ),
        ("recess", 6.0, 10.0, p.dark.into(), "#233c58", 1.0),
        ("lip", 8.0, 9.0, format!("url(#{face_id})"), p.light, 1.0),
        ("inner", 10.0, 8.0, "none".into(), p.middle, 0.7),
    ] {
        let path = if open && n == "shell" {
            format!(
                "{} {}",
                outline(w, h, inset, chamfer),
                outline(w, h, 6.0, 10.0)
            )
        } else {
            outline(w, h, inset, chamfer)
        };
        let fill = if open && n != "shell" {
            "none".into()
        } else {
            fill
        };
        nodes.push(element(
            &format!("{id}-{n}"),
            "path",
            &[
                ("d", &path),
                ("fill-rule", "evenodd"),
                ("fill", &fill),
                ("stroke", stroke),
                ("stroke-width", &thickness.to_string()),
            ],
            vec![],
        ));
    }
    nodes.extend(details(id, w, h, p, open));
    element(
        &format!("{id}-surface"),
        "svg",
        &[
            ("class", "surface"),
            ("width", &width.to_string()),
            ("height", &height.to_string()),
            ("viewBox", &format!("0 0 {width} {height}")),
            ("aria-hidden", "true"),
        ],
        nodes,
    )
}

fn gradient(key: &str, colors: &[(&str, &str)]) -> Node {
    element(
        key,
        "linearGradient",
        &[
            ("id", key),
            ("x1", "0"),
            ("y1", "0"),
            ("x2", "0"),
            ("y2", "1"),
        ],
        colors
            .iter()
            .enumerate()
            .map(|(i, (offset, color))| {
                element(
                    &format!("{key}-{i}"),
                    "stop",
                    &[("offset", offset), ("stop-color", color)],
                    vec![],
                )
            })
            .collect(),
    )
}

fn details(id: &str, w: f64, h: f64, p: Palette, open: bool) -> Vec<Node> {
    let mut nodes = vec![];
    if !open {
        nodes.push(element(
            &format!("{id}-reflection"),
            "path",
            &[
                (
                    "d",
                    &format!(
                        "M 20 11 H {} L {} {} L {} {} Z",
                        w * 0.58,
                        w * 0.35,
                        h - 12.0,
                        w * 0.28,
                        h - 12.0
                    ),
                ),
                ("fill", p.light),
                ("opacity", "0.055"),
            ],
            vec![],
        ));
        nodes.push(element(
            &format!("{id}-arc"),
            "path",
            &[
                (
                    "d",
                    &format!(
                        "M {} 12 Q {} {} {} {}",
                        w * 0.88,
                        w * 0.35,
                        h * 0.4,
                        w * 0.72,
                        h - 12.0
                    ),
                ),
                ("fill", "none"),
                ("stroke", p.light),
                ("stroke-width", "0.8"),
                ("opacity", "0.15"),
            ],
            vec![],
        ));
    }
    for (i, (x, y, sx, sy)) in [
        (3.0, 3.0, 1.0, 1.0),
        (w - 3.0, 3.0, -1.0, 1.0),
        (3.0, h - 3.0, 1.0, -1.0),
        (w - 3.0, h - 3.0, -1.0, -1.0),
    ]
    .into_iter()
    .enumerate()
    {
        nodes.push(element(
            &format!("{id}-joint-{i}"),
            "path",
            &[
                ("d", "M 0 11 L 11 0 H 30 L 20 3 H 13 L 3 13 V 22 L 0 29 Z"),
                ("transform", &format!("translate({x} {y}) scale({sx} {sy})")),
                ("fill", "#dfeffb"),
                ("stroke", p.middle),
                ("stroke-width", "0.7"),
            ],
            vec![],
        ));
    }
    nodes
}
