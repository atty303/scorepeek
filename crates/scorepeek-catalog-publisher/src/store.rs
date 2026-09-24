//! Publication-side snapshot construction and activation.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Read as _;
use std::path::{Path, PathBuf};

use scorepeek_core::catalog::Catalog;
use scorepeek_resources::{ActiveCatalog, CatalogUpdate};
use sha2::{Digest as _, Sha256};

pub use scorepeek_resources::CatalogStoreError;

#[derive(Clone, Debug)]
pub struct CatalogStore {
    root: PathBuf,
    client: scorepeek_resources::CatalogStore,
}

pub struct PublisherUpdate {
    root: PathBuf,
    client: CatalogUpdate,
}

impl CatalogStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            client: scorepeek_resources::CatalogStore::new(&root),
            root,
        }
    }

    /// Acquires the writer lock before source acquisition and federation.
    /// # Errors
    /// Returns store I/O or manifest errors.
    pub fn begin_update(&self) -> Result<PublisherUpdate, CatalogStoreError> {
        let client = self.client.begin_update()?;
        recover_publisher_staging(&self.root)?;
        Ok(PublisherUpdate {
            root: self.root.clone(),
            client,
        })
    }

    /// Loads the published candidate through the ordinary runtime reader.
    /// # Errors
    /// Returns invalid snapshot or I/O errors.
    pub fn load_active(&self) -> Result<Option<ActiveCatalog>, CatalogStoreError> {
        self.client.load_active()
    }

    /// Resolves the content-addressed `SQLite` path for a published digest.
    /// # Errors
    /// Returns a malformed digest error.
    pub fn snapshot_path(&self, digest: &str) -> Result<PathBuf, CatalogStoreError> {
        self.client.snapshot_path(digest)
    }
}

impl PublisherUpdate {
    #[must_use]
    pub fn base_digest(&self) -> Option<&str> {
        self.client.base_digest()
    }

    /// Writes the publisher schema and atomically activates the resulting bytes.
    /// # Errors
    /// Returns schema, capacity, concurrency, or durability errors.
    pub fn publish(self, catalog: &Catalog) -> Result<ActiveCatalog, CatalogStoreError> {
        let staging = tempfile::Builder::new()
            .prefix(".publisher-snapshot-")
            .tempdir_in(&self.root)?;
        let path = staging.path().join("catalog.sqlite3");
        crate::snapshot::write_snapshot(&path, catalog)?;
        File::open(&path)?.sync_all()?;
        let digest = digest_file(&path)?;
        let digest = self.client.install_verified_snapshot(&path, &digest)?;
        Ok(ActiveCatalog {
            digest,
            catalog: catalog.clone(),
            origin: None,
        })
    }
}

fn digest_file(path: &Path) -> Result<String, CatalogStoreError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let digest = hasher.finalize();
    let mut value = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(value, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(value)
}

fn recover_publisher_staging(root: &Path) -> Result<(), CatalogStoreError> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(".publisher-snapshot-") {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            return Err(CatalogStoreError::InvalidSnapshot(
                "publisher staging path is not a directory".to_owned(),
            ));
        }
    }
    Ok(())
}
