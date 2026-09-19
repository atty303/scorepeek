mod acquisition;
mod adapter;
mod sync;
mod tachi_acquisition;
mod textage_acquisition;
mod textage_adapter;

mod federation {
    pub use scorepeek_catalog::*;
}

mod store {
    pub use scorepeek::catalog::{CatalogStore, CatalogStoreError};
}

#[cfg(test)]
pub use adapter::{
    AdapterError, DqnLiveAdapter, SourceRevision, TachiFixtureAdapter, TachiLiveAdapter,
    TextageFixtureAdapter,
};
pub use federation::*;
#[cfg(test)]
pub use store::CatalogStore;
pub use sync::{CatalogSync, CatalogSyncSource};

#[cfg(test)]
mod tests;
