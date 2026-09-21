use crate::entry::{GLYPHS, LABELS};
use crate::motion::{energy_motion_style, glint_motion_style, lamp_motion_style};
use crate::primitive::{el, frame_width, label_class, node_text, shape, text};
use crate::theme::Skin;
use scorepeek_skin_sdk::{Node, Widget};
pub(crate) fn chrome(key: &str, widget: &Widget, skin: Skin) -> Node {
    el(
        &format!("{key}:chrome"),
        "div",
        &[
            ("class", "skin-frame".into()),
            ("aria-hidden", "true".into()),
        ],
        vec![
            frame(&format!("{key}:frame"), widget, skin),
            el(
                &format!("{key}:energy"),
                "div",
                &[
                    ("class", "skin-energy".into()),
                    ("style", energy_motion_style()),
                ],
                vec![],
            ),
            el(
                &format!("{key}:glint"),
                "div",
                &[
                    ("class", "skin-glint".into()),
                    ("style", glint_motion_style(&widget.kind)),
                ],
                vec![],
            ),
        ],
    )
}
pub(crate) fn frame(key: &str, widget: &Widget, skin: Skin) -> Node {
    let image = "frame.png";
    let [source_x, source_y] = skin.frame_source;
    let factor = skin.frame_factor;
    let edge = f64::from(frame_width(widget));
    let width = f64::from(widget.width) - 16.0 + edge * 2.0;
    let height = f64::from(widget.height) - 16.0 + edge * 2.0;
    let aperture_scale = if widget.kind == "empty" { 0.55 } else { 1.0 };
    let scale = (factor * aperture_scale)
        .min(width / (2.0 * source_x))
        .min(height / (2.0 * source_y));
    let sx = [0.0, source_x, 1254.0 - source_x, 1254.0];
    let sy = [0.0, source_y, 1254.0 - source_y, 1254.0];
    let corner_x = (source_x * scale + edge - 8.0).max(1.0).min(width / 2.0);
    let corner_y = (source_y * scale + edge - 8.0).max(1.0).min(height / 2.0);
    let tx = [0.0, corner_x, width - corner_x, width];
    let ty = [0.0, corner_y, height - corner_y, height];
    let mut pieces = Vec::new();
    for row in 0..3 {
        for column in 0..3 {
            let w = tx[column + 1] - tx[column];
            let h = ty[row + 1] - ty[row];
            if w > 0.0 && h > 0.0 {
                let xs = w / (sx[column + 1] - sx[column]);
                let ys = h / (sy[row + 1] - sy[row]);
                pieces.push(el(&format!("{key}:slice:{row}:{column}"), "div", &[("class", "material-slice".into()), ("style", format!("left:{}px;top:{}px;width:{w}px;height:{h}px;background-image:url('{image}');background-size:{}px {}px;background-position:{}px {}px", tx[column], ty[row], 1254.0 * xs, 1254.0 * ys, -sx[column] * xs, -sy[row] * ys))], vec![]));
            }
        }
    }
    let mut nodes = vec![el(
        &format!("{key}:material"),
        "div",
        &[
            ("class", "material-frame".into()),
            (
                "style",
                format!(
                    "left:{}px;top:{}px;width:{width}px;height:{height}px",
                    8.0 - edge,
                    8.0 - edge
                ),
            ),
        ],
        pieces,
    )];
    if skin.selection_illumination && widget.kind == "selection" {
        nodes.push(el(
            &format!("{key}:illumination"),
            "div",
            &[("class", "material-illumination".into())],
            vec![],
        ));
    }
    if widget.kind == "status" {
        let edge = skin.status_edge;
        nodes.push(el(
            &format!("{key}:panel-frame"),
            "svg",
            &[
                ("class", "panel-frame".into()),
                ("width", width.to_string()),
                ("height", height.to_string()),
                ("viewBox", format!("0 0 {width} {height}")),
            ],
            vec![shape(
                &format!("{key}:panel-frame:path"),
                "path",
                &[
                    (
                        "d",
                        &format!(
                            "M{} 12l-30 {}h-6l30 -{}",
                            width * 0.43,
                            height - 24.0,
                            height - 24.0
                        ),
                    ),
                    ("fill", "none"),
                    ("stroke", edge),
                    ("stroke-width", "0.7"),
                ],
            )],
        ));
    }
    el(key, "div", &[], nodes)
}

pub(crate) fn lamp(
    key: &str,
    state: &str,
    caption: Option<&str>,
    vertical: bool,
    skin: Skin,
) -> Node {
    let mut children = vec![el(
        &format!("{key}:lamp"),
        "span",
        &[
            ("class", "lamp".into()),
            ("data-state", state.into()),
            ("aria-hidden", "true".into()),
            ("style", lamp_motion_style(state)),
        ],
        vec![lamp_svg(key, state, vertical, skin)],
    )];
    if let Some(value) = caption {
        children.push(label_class(
            &format!("{key}:caption"),
            "lamp-label",
            value,
            skin,
            15,
        ));
    }
    let role = if caption == Some("SYSTEM") {
        "system"
    } else if caption == Some("RESULT") {
        "result"
    } else {
        "recorded"
    };
    el(
        key,
        "div",
        &[("class", format!("lamp-group {role}-lamp"))],
        children,
    )
}

pub(crate) fn lamp_svg(key: &str, state: &str, vertical: bool, skin: Skin) -> Node {
    let accent = skin.lamp_accent;
    let light = match (state, vertical) {
        ("active", true) => accent,
        ("active", false) => "#54e56b",
        ("error", _) => "#ff5369",
        _ => "#36434a",
    };
    let (width, height) = if vertical { (16, 64) } else { (24, 24) };
    let gradient_id = format!("lamp-gradient-{}", fragment_id(key));
    let glow = format!("url(#{gradient_id})");
    let mut art = vec![lamp_gradient(key, state, light, &gradient_id)];
    if vertical {
        art.extend([
            shape(
                &format!("{key}:outer"),
                "rect",
                &[
                    ("x", "0.5"),
                    ("y", "0.5"),
                    ("width", "15"),
                    ("height", "63"),
                    ("fill", "#020709"),
                    ("stroke", accent),
                    ("stroke-width", "1"),
                ],
            ),
            shape(
                &format!("{key}:inner"),
                "rect",
                &[
                    ("x", "3"),
                    ("y", "3"),
                    ("width", "10"),
                    ("height", "58"),
                    ("fill", light),
                ],
            ),
            shape(
                &format!("{key}:highlight"),
                "path",
                &[
                    ("d", "M5 5V59"),
                    ("stroke", "#d5faff"),
                    ("stroke-width", "1"),
                    ("opacity", if state == "inactive" { "0.15" } else { "0.8" }),
                ],
            ),
        ]);
    } else {
        art.extend([
            shape(
                &format!("{key}:outer"),
                "circle",
                &[
                    ("cx", "12"),
                    ("cy", "12"),
                    ("r", "11"),
                    ("fill", "#030a0d"),
                    ("stroke", light),
                    ("stroke-width", "1"),
                ],
            ),
            shape(
                &format!("{key}:inner"),
                "circle",
                &[
                    ("cx", "12"),
                    ("cy", "12"),
                    ("r", "8"),
                    ("fill", &glow),
                    ("stroke", "#87949b"),
                    ("stroke-width", "0.8"),
                ],
            ),
        ]);
    }
    el(
        &format!("{key}:svg"),
        "svg",
        &[
            ("width", width.to_string()),
            ("height", height.to_string()),
            ("viewBox", format!("0 0 {width} {height}")),
        ],
        art,
    )
}

pub(crate) fn lamp_gradient(key: &str, state: &str, light: &str, gradient_id: &str) -> Node {
    el(
        &format!("{key}:defs"),
        "defs",
        &[],
        vec![el(
            &format!("{key}:gradient"),
            "radialGradient",
            &[
                ("id", gradient_id.into()),
                ("cx", "40%".into()),
                ("cy", "35%".into()),
                ("r", "65%".into()),
            ],
            vec![
                shape(
                    &format!("{key}:gradient:0"),
                    "stop",
                    &[
                        ("offset", "0"),
                        (
                            "stop-color",
                            if state == "inactive" {
                                "#6c7981"
                            } else {
                                "#ffffff"
                            },
                        ),
                    ],
                ),
                shape(
                    &format!("{key}:gradient:1"),
                    "stop",
                    &[("offset", "0.4"), ("stop-color", light)],
                ),
                shape(
                    &format!("{key}:gradient:2"),
                    "stop",
                    &[("offset", "1"), ("stop-color", "#051015")],
                ),
            ],
        )],
    )
}
pub(crate) fn chart_rail(key: &str, width: u32, skin: Skin) -> Node {
    let edge = skin.rail_edge;
    let inner = skin.rail_inner;
    let width_f64 = f64::from(width);
    el(
        key,
        "svg",
        &[
            ("class", "rail-frame".into()),
            ("width", width.to_string()),
            ("height", "34".into()),
            ("viewBox", format!("0 0 {width} 34")),
        ],
        vec![
            shape(
                &format!("{key}:outer"),
                "path",
                &[
                    ("d", &contour(width_f64, 34.0, 0.5, 9.0)),
                    ("fill", "#020508"),
                    ("stroke", edge),
                    ("stroke-width", "1"),
                ],
            ),
            shape(
                &format!("{key}:inner"),
                "path",
                &[
                    ("d", &contour(width_f64, 34.0, 3.0, 7.0)),
                    ("fill", "none"),
                    ("stroke", inner),
                    ("stroke-width", "0.7"),
                ],
            ),
            shape(
                &format!("{key}:badge"),
                "path",
                &[
                    ("d", "M34 4H88L97 13V21L88 30H34L25 21V13Z"),
                    ("fill", "#0b1115"),
                    ("stroke", edge),
                    ("stroke-width", "1"),
                ],
            ),
            shape(
                &format!("{key}:end"),
                "path",
                &[
                    ("d", &format!("M{} 8l5 5v8l-5 5", width_f64 - 14.0)),
                    ("fill", "none"),
                    ("stroke", edge),
                    ("stroke-width", "1"),
                ],
            ),
        ],
    )
}

pub(crate) fn contour(width: f64, height: f64, inset: f64, cut: f64) -> String {
    let right = width - inset;
    let bottom = height - inset;
    let near = inset + cut;
    let far_x = right - cut;
    let far_y = bottom - cut;
    format!(
        "M{near} {inset}H{far_x}L{right} {near}V{far_y}L{far_x} {bottom}H{near}L{inset} {far_y}V{near}Z"
    )
}

pub(crate) fn fragment_id(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.bytes() {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

pub(crate) fn metallic(key: &str, value: &str, skin: Skin, height: u32, rank: bool) -> Node {
    if value.is_empty() || !value.chars().all(|c| GLYPHS.contains(c)) {
        return text(key, value);
    }
    let width = 44.0 * f64::from(height) / 80.0;
    let sheet = width * 18.0;
    let material = if rank { skin.rank_material } else { "type.png" };
    let mut nodes = vec![node_text(
        &format!("{key}:value"),
        "span",
        &[("class", "type-value".into())],
        value,
    )];
    for (i, glyph) in value.chars().enumerate() {
        let index = GLYPHS.chars().position(|v| v == glyph).unwrap_or(0);
        nodes.push(el(&format!("{key}:glyph:{i}"), "span", &[("class", "metal-glyph".into()), ("aria-hidden", "true".into()), ("style", format!("width:{width}px;height:{height}px;background-image:url('{material}');background-size:{sheet}px {height}px;background-position:-{}px 0", f64::from(u32::try_from(index).unwrap_or(0)) * width))], vec![]));
    }
    el(key, "span", &[("class", "metal-type".into())], nodes)
}
pub(crate) fn label(key: &str, value: &str, skin: Skin, height: u32) -> Node {
    let Some(index) = LABELS.iter().position(|item| *item == value) else {
        return text(key, value);
    };
    let scale = f64::from(height) / 24.0;
    let font = skin.label_font;
    el(
        key,
        "span",
        &[
            ("class", "material-label".into()),
            (
                "style",
                format!(
                    "margin-bottom:-{}px;font-family:{font};font-size:{}px;line-height:{height}px;height:{height}px",
                    scale * skin.label_descent,
                    scale * 20.0
                ),
            ),
        ],
        vec![
            node_text(
                &format!("{key}:value"),
                "span",
                &[("class", "label-value".into())],
                value,
            ),
            el(
                &format!("{key}:art"),
                "span",
                &[
                    ("class", "label-art".into()),
                    ("aria-hidden", "true".into()),
                    (
                        "style",
                        format!(
                            "width:100%;height:{height}px;background-image:url('labels.png');background-size:{}px {}px;background-position:0 -{}px",
                            256.0 * scale,
                            f64::from(u32::try_from(LABELS.len()).unwrap_or(0)) * 24.0 * scale,
                            u32::try_from(index).unwrap_or(0) * height
                        ),
                    ),
                ],
                vec![],
            ),
        ],
    )
}
