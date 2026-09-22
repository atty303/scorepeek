//! Canonical-frame recording policy and writer.

pub(crate) mod artifact;
pub mod policy;
pub mod retention;
pub mod simulation;
pub(crate) mod source;
pub mod writer;

use std::process::ExitCode;

/// Runs the standalone recording simulation profile author.
#[must_use]
pub fn simulation_profile_author_main() -> ExitCode {
    crate::service::dispatch::development_operation_main("recording-simulation-profile-author")
}

/// Runs the standalone recording simulation.
#[must_use]
pub fn simulation_main() -> ExitCode {
    crate::service::dispatch::development_operation_main("recording-simulation")
}

/// Runs the standalone recording recognition evidence exporter.
#[must_use]
pub fn recognition_evidence_main() -> ExitCode {
    crate::service::dispatch::development_operation_main("recording-recognition-evidence")
}

/// Runs the standalone recording recognition simulation.
#[must_use]
pub fn recognition_simulation_main() -> ExitCode {
    crate::service::dispatch::development_operation_main("recording-recognition-simulation")
}
