//! Presentation-only motion shared by native and browser drivers.
use serde::Deserialize;

pub const SPEC: &str = include_str!("../assets/motion.json");
pub const BROWSER_DRIVER: &str = include_str!("../motion.js");
#[derive(Deserialize)]
pub struct Track {
    pub selector: String,
    pub property: String,
    pub from: f64,
    pub to: f64,
    pub period: f64,
    pub phase: f64,
    pub unit: String,
    pub wave: Wave,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wave {
    Sine,
    Ramp,
}
impl Track {
    #[must_use]
    pub fn value(&self, seconds: f64) -> String {
        let phase = (seconds / self.period + self.phase).rem_euclid(1.0);
        let blend = match self.wave {
            Wave::Sine => 0.5 - 0.5 * (phase * std::f64::consts::TAU).cos(),
            Wave::Ramp => phase,
        };
        format!(
            "{:.4}{}",
            self.from + (self.to - self.from) * blend,
            self.unit
        )
    }
}
/// Visual identity only; unknown values never acquire an achievement treatment.
#[must_use]
pub fn clear_role(value: &str) -> &'static str {
    match value {
        "FULL COMBO" | "FULL COMBO CLEAR" | "FC" => "full-combo",
        "EX HARD CLEAR" | "EX HARD" | "EXH" => "ex-hard",
        "HARD CLEAR" | "HARD" => "hard",
        "CLEAR" => "clear",
        "EASY CLEAR" | "EASY" => "easy",
        "ASSIST CLEAR" | "ASSIST" => "assist",
        "FAILED" => "failed",
        _ => "unknown",
    }
}
