//! Bundled-skin theme constants.

#[derive(Clone, Copy)]
pub struct Theme {
    pub graph_score: &'static str,
    pub graph_miss: &'static str,
    pub frame_source: [f64; 2],
    pub frame_factor: f64,
    pub selection_illumination: bool,
    pub status_edge: &'static str,
    pub lamp_accent: &'static str,
    pub rail_edge: &'static str,
    pub rail_inner: &'static str,
    pub rank_material: &'static str,
    pub label_font: &'static str,
    pub label_descent: f64,
}

pub(crate) type Skin = &'static Theme;
