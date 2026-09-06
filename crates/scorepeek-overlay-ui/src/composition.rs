//! Shared inner geometry and image-backed canvas composition.
use crate::{Skin, WidgetKind, WidgetLayout};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Background {
    #[default]
    None,
    Static,
    Animated,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameWidth {
    S,
    #[default]
    M,
    L,
}
impl FrameWidth {
    #[must_use]
    pub const fn pixels(self) -> u32 {
        match self {
            Self::S => 4,
            Self::M => 8,
            Self::L => 16,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AspectRatio {
    #[default]
    Free,
    Wide,
    Standard,
    Current([u32; 2]),
}

impl WidgetKind {
    #[must_use]
    pub const fn class(self) -> &'static str {
        match self {
            Self::Status => "status-widget",
            Self::Selection => "selection-widget",
            Self::Score => "score-widget",
            Self::HistoryList => "history-list-widget",
            Self::HistoryGraph => "history-graph-widget",
            Self::Empty => "empty-widget",
        }
    }
}

fn hole_image(width: u32, height: u32) -> String {
    format!("url('/skins/aperture-{width}-{height}.svg')")
}

const fn aperture_cut(width: u32, height: u32) -> u32 {
    let cut = if width / 4 < 12 { width / 4 } else { 12 };
    if height / 4 < cut { height / 4 } else { cut }
}

#[must_use]
pub fn aperture_asset(path: &str) -> Option<String> {
    let dimensions = path
        .strip_prefix("/skins/aperture-")?
        .strip_suffix(".svg")?;
    let (width, height) = dimensions.split_once('-')?;
    let width = width.parse::<u32>().ok()?;
    let height = height.parse::<u32>().ok()?;
    let cut = aperture_cut(width, height);
    Some(format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{width}' height='{height}'><path fill='white' d='M{cut} 0H{}L{width} {cut}V{}L{} {height}H{cut}L0 {}V{cut}Z'/></svg>",
        width - cut,
        height - cut,
        width - cut,
        height - cut
    ))
}

/// Alpha subtraction gives a union of apertures, including overlapping widgets.
#[must_use]
pub fn aperture_mask(holes: impl IntoIterator<Item = (i32, i32, u32, u32)>) -> String {
    let mut images = vec!["linear-gradient(white,white)".to_owned()];
    let mut sizes = vec!["100% 100%".to_owned()];
    let mut positions = vec!["0px 0px".to_owned()];
    for (x, y, w, h) in holes {
        images.push(hole_image(w, h));
        sizes.push(format!("{w}px {h}px"));
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

pub(crate) fn background(mode: Background, widgets: &[WidgetLayout], skin: Skin) -> Element {
    if mode == Background::None {
        return rsx! {};
    }
    let mask = aperture_mask(
        widgets
            .iter()
            .filter(|w| w.kind == WidgetKind::Empty)
            .map(|w| (w.x, w.y, w.width, w.height)),
    );
    let image = match skin {
        Skin::CyanSystem => "cyan-system-background.png",
        Skin::ResultAurora => "result-aurora-background.png",
        Skin::DjBlackbox => "dj-blackbox-background.png",
    };
    rsx! { div { key:"{mode:?}", class:"canvas-background", style:mask, "data-motion":if mode == Background::Animated { "animated" } else { "static" }, aria_hidden:"true",
        div { class:"canvas-background-art", style:format!("background-image:url('/skins/{image}')") }
        div { class:"canvas-background-light" }
    } }
}

pub(crate) fn empty_widget(widget: &WidgetLayout, skin: Skin) -> Element {
    let cut = aperture_cut(widget.width, widget.height);
    let edge = widget.settings.frame_width.pixels();
    let mut frame = widget.clone();
    frame.width += 16;
    frame.height += 16;
    let mask = aperture_mask([(
        i32::try_from(edge).unwrap_or(8),
        i32::try_from(edge).unwrap_or(8),
        widget.width,
        widget.height,
    )]);
    rsx! {
        div { class:"empty-frame", style:format!("position:absolute;left:-{edge}px;top:-{edge}px;width:{}px;height:{}px;{mask}",widget.width+2*edge,widget.height+2*edge),
            div { style:format!("position:absolute;left:{}px;top:{}px;width:{}px;height:{}px",i64::from(edge)-8,i64::from(edge)-8,frame.width,frame.height),
                {crate::frame::render(&frame,skin)}
            }
        }
        div { class:"empty-fill", style:format!("position:absolute;inset:0;background:rgba(0,0,0,{});clip-path:polygon({cut}px 0,calc(100% - {cut}px) 0,100% {cut}px,100% calc(100% - {cut}px),calc(100% - {cut}px) 100%,{cut}px 100%,0 calc(100% - {cut}px),0 {cut}px)",f64::from(widget.settings.fill_opacity_percent)/100.0) }
        if !widget.settings.title.is_empty() { div { class:"empty-title", "{widget.settings.title}" } }
    }
}
