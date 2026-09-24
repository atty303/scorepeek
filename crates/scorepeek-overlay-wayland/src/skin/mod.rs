//! Wayland DOM adapter for the shared native skin runtime.

pub mod runtime;
pub use runtime::{NativeTree, RenderTiming, Runtime, RuntimeTiming};

/// Installs a skin after native Wasm validation and smoke execution.
/// # Errors
/// Returns package, validation, or durable storage errors.
pub fn install(store: &StoreRoot, source: &std::path::Path) -> Result<InstallOutcome, String> {
    store.install_with(source, runtime::validate_and_smoke_test)
}

pub use scorepeek_overlay_runtime::skin::*;
