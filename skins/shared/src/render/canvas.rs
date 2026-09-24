use crate::primitive::{aperture_mask, el};
use scorepeek_skin_sdk::{Node, Widget};
pub(crate) fn canvas_background(mode: &str, widgets: &[Widget]) -> Node {
    let mask = aperture_mask(widgets.iter().filter(|widget| widget.kind == "empty").map(
        |widget| {
            (
                i64::from(widget.x),
                i64::from(widget.y),
                widget.width,
                widget.height,
            )
        },
    ));
    el(
        "background",
        "div",
        &[
            ("class", "canvas-background".into()),
            ("data-motion", mode.into()),
            ("aria-hidden", "true".into()),
            ("style", mask),
        ],
        vec![
            el(
                "background:art",
                "div",
                &[
                    ("class", "canvas-background-art".into()),
                    ("style", "background-image:url('background.png')".into()),
                ],
                vec![],
            ),
            el(
                "background:light",
                "div",
                &[("class", "canvas-background-light".into())],
                vec![],
            ),
        ],
    )
}
