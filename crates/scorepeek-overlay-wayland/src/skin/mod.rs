//! Wasmtime skin execution and package resources.

pub mod resources;
pub mod runtime;

pub use resources::{InstallOutcome, InstalledSkin, StoreRoot};
pub use runtime::*;
