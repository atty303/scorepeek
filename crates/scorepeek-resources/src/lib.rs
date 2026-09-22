//! Catalog artifact verification and content addressed local activation.

pub mod activation;
pub mod artifact;
pub mod model;
pub mod recognition;
mod store;

pub use activation::{ActiveCatalog, CatalogOrigin, CatalogUpdate};
pub use store::{CatalogStore, CatalogStoreError};
