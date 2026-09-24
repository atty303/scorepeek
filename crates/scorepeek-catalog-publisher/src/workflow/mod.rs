mod build;
pub mod verify;

mod domain {
    pub use crate::federation::*;
    pub use scorepeek_core::catalog::*;
}

#[cfg(test)]
pub use crate::source::common::{AdapterError, SourceRevision};
#[cfg(test)]
pub use crate::source::dqn::decode::DqnLiveAdapter;
#[cfg(test)]
pub use crate::source::tachi::decode::{TachiFixtureAdapter, TachiLiveAdapter};
#[cfg(test)]
pub use crate::source::textage::decode::TextageFixtureAdapter;
pub use build::{CatalogSync, CatalogSyncSource};
pub use domain::*;

#[cfg(test)]
mod tests;
