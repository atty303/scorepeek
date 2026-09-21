use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
pub struct Input {
    pub schema: String,
    pub backend: String,
    #[serde(default)]
    pub monotonic_ms: u64,
    pub canvas: Canvas,
    #[serde(default)]
    pub widgets: Vec<Widget>,
    pub state: serde_json::Value,
}

#[derive(Deserialize)]
pub struct Canvas {
    pub id: String,
    pub skin: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub properties: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
pub struct Widget {
    pub id: String,
    pub kind: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub settings: serde_json::Value,
    #[serde(default)]
    pub properties: BTreeMap<String, serde_json::Value>,
}
