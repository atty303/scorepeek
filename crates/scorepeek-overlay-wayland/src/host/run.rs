//! Production native host entrypoint.

use crate::bridge::data::Config;
use scorepeek_overlay::editor::EditorAction;
use std::time::Duration;

/// A timed editor action used by the checked-in nested compositor scenario.
#[doc(hidden)]
pub struct NativeEditorScenarioStep {
    pub after: Duration,
    pub target_canvas: &'static str,
    pub name: &'static str,
    pub action: EditorAction,
}

/// Runs until the parent's lifetime lease closes.
/// # Errors
/// Returns Wayland, GPU, or event-loop failures.
pub fn run(config: Config, input: impl std::io::Read + Send + 'static) -> Result<(), String> {
    crate::render::dioxus_dom::run_with_editor_scenario(config, input, Vec::new())
}

/// Runs the production host with deterministic scenario input.
#[doc(hidden)]
pub fn run_with_editor_scenario(
    config: Config,
    input: impl std::io::Read + Send + 'static,
    scenario: Vec<NativeEditorScenarioStep>,
) -> Result<(), String> {
    crate::render::dioxus_dom::run_with_editor_scenario(config, input, scenario)
}
