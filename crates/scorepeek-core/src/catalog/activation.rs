//! Content-addressed activation types shared by publisher and runtime.

use std::fs::File;

use serde::{Deserialize, Serialize};

use super::model::Catalog;
use super::store::CatalogStore;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveCatalog {
    pub digest: String,
    pub catalog: Catalog,
    pub origin: Option<CatalogOrigin>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogOrigin {
    pub source_url_sha256: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub last_success_unix_seconds: u64,
}

pub struct CatalogUpdate {
    pub(super) store: CatalogStore,
    pub(super) lock: File,
    pub(super) base_digest: Option<String>,
}
