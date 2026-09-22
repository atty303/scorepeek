//! Native host lifecycle facade.

pub mod event_loop;
pub mod lifecycle;
pub mod run;

pub use run::{NativeEditorScenarioStep, run, run_with_editor_scenario};
