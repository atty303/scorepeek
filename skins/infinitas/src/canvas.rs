use crate::{element, group, palette::Palette};
use scorepeek_skin_sdk::{Input, Node};
#[derive(Clone, Copy)]
struct Rect {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}
impl Rect {
    fn subtract(self, hole: Self) -> Vec<Self> {
        let left = self.left.max(hole.left);
        let top = self.top.max(hole.top);
        let right = self.right.min(hole.right);
        let bottom = self.bottom.min(hole.bottom);
        if left >= right || top >= bottom {
            return vec![self];
        }
        [
            Self {
                bottom: top,
                ..self
            },
            Self {
                top: bottom,
                ..self
            },
            Self {
                left: self.left,
                top,
                right: left,
                bottom,
            },
            Self {
                left: right,
                top,
                right: self.right,
                bottom,
            },
        ]
        .into_iter()
        .filter(|r| r.left < r.right && r.top < r.bottom)
        .collect()
    }
}
pub fn background(input: &Input, palette: Palette, drift: f64) -> Vec<Node> {
    let mut regions = vec![Rect {
        left: 0.0,
        top: 0.0,
        right: f64::from(input.canvas.width),
        bottom: f64::from(input.canvas.height),
    }];
    for widget in input.widgets.iter().filter(|w| w.kind == "empty") {
        let hole = Rect {
            left: f64::from(widget.x) + 10.0,
            top: f64::from(widget.y) + 10.0,
            right: f64::from(widget.x) + f64::from(widget.width) - 10.0,
            bottom: f64::from(widget.y) + f64::from(widget.height) - 10.0,
        };
        regions = regions.into_iter().flat_map(|r| r.subtract(hole)).collect();
    }
    regions
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            group(
                &format!("backdrop-{i}"),
                "backdrop",
                &format!(
                    "left:{}px;top:{}px;width:{}px;height:{}px;background:{};",
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    palette.dark
                ),
                vec![
                    element(
                        &format!("backdrop-image-{i}"),
                        "img",
                        &[
                            ("class", "backdrop-image"),
                            ("src", "optical-glass.png"),
                            ("alt", ""),
                            (
                                "style",
                                &format!(
                                    "left:{}px;top:{}px;width:{}px;height:{}px;",
                                    -r.left - 20.0 + drift,
                                    -r.top - 20.0,
                                    f64::from(input.canvas.width) + 40.0,
                                    f64::from(input.canvas.height) + 40.0
                                ),
                            ),
                        ],
                        vec![],
                    ),
                    group(
                        &format!("backdrop-tint-{i}"),
                        "backdrop-tint",
                        &format!("background:{};", palette.middle),
                        vec![],
                    ),
                ],
            )
        })
        .collect()
}
