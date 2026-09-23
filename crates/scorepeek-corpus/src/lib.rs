//! Canonical recording import, review, and deterministic semantic regression.

extern crate self as scorepeek_corpus;

pub mod canonical;
pub mod oracle;
pub mod replay;
mod resources;
pub mod store;

// Cargo excludes `test = false` custom targets from `clippy --all-targets` on the pinned
// toolchain. Typecheck the exact private entry-point source in ordinary lib-test compilation.
#[cfg(test)]
#[allow(dead_code)]
#[path = "../tests/full_replay.rs"]
mod full_replay_entry_point_compile;
