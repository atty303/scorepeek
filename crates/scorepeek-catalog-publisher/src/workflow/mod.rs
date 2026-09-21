mod build;
pub mod select;
pub mod verify;

mod domain {
    pub use scorepeek_core::catalog::*;
}

mod store {
    pub use scorepeek_core::catalog::{CatalogStore, CatalogStoreError};
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
pub use store::CatalogStore;

#[cfg(test)]
mod tests;
