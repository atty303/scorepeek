//! Installed skin package and resource access.

mod package;
mod resources;

pub use package::*;
pub use resources::{InstallOutcome, InstalledSkin, StoreRoot};
