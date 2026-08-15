use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use serde::Serialize;

use super::acquisition::{DqnAcquisitionError, DqnTransport, UreqDqnTransport, acquire_dqn};
use super::federation::{Catalog, FederationInput, QuarantineEntry, QuarantineReason};
use super::store::{CatalogStore, CatalogStoreError};

#[derive(Clone, Debug)]
pub struct CatalogSync {
    store: CatalogStore,
    cache_root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogSyncResult {
    pub activated: bool,
    pub active_catalog_digest: Option<String>,
    pub source_content_sha256: String,
    pub source_record_count: usize,
    pub quarantine: Vec<QuarantineEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CatalogSyncSummary {
    pub activated: bool,
    pub active_catalog_digest: Option<String>,
    pub source_content_sha256: String,
    pub source_record_count: usize,
    pub quarantine_counts: BTreeMap<QuarantineReason, usize>,
}

#[derive(Debug)]
pub enum CatalogSyncError {
    Store(CatalogStoreError),
    Acquisition(DqnAcquisitionError),
}

impl fmt::Display for CatalogSyncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Acquisition(error) => error.fmt(formatter),
        }
    }
}

impl Error for CatalogSyncError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Acquisition(error) => Some(error),
        }
    }
}

impl CatalogSyncError {
    #[must_use]
    pub fn redacted_message(&self) -> String {
        match self {
            Self::Store(_) => "catalog store operation failed".to_owned(),
            Self::Acquisition(DqnAcquisitionError::UnexpectedStatus(status)) => {
                format!("dqn acquisition returned unexpected HTTP status {status}")
            }
            Self::Acquisition(
                DqnAcquisitionError::DeclaredBodyTooLarge { .. }
                | DqnAcquisitionError::BodyTooLarge { .. },
            ) => "dqn response exceeded the configured size limit".to_owned(),
            Self::Acquisition(DqnAcquisitionError::Timeout) => {
                "dqn acquisition timed out".to_owned()
            }
            Self::Acquisition(DqnAcquisitionError::Transport(_)) => {
                "dqn transport failed".to_owned()
            }
            Self::Acquisition(DqnAcquisitionError::Adapter(_)) => {
                "dqn response validation failed".to_owned()
            }
            Self::Acquisition(
                DqnAcquisitionError::CacheIo(_)
                | DqnAcquisitionError::CacheConflict(_)
                | DqnAcquisitionError::CacheCapacityExceeded,
            ) => "dqn cache persistence failed".to_owned(),
        }
    }
}

impl From<CatalogStoreError> for CatalogSyncError {
    fn from(error: CatalogStoreError) -> Self {
        Self::Store(error)
    }
}

impl From<DqnAcquisitionError> for CatalogSyncError {
    fn from(error: DqnAcquisitionError) -> Self {
        Self::Acquisition(error)
    }
}

impl CatalogSync {
    #[must_use]
    pub fn new(store_root: impl Into<PathBuf>, cache_root: impl Into<PathBuf>) -> Self {
        Self {
            store: CatalogStore::new(store_root),
            cache_root: cache_root.into(),
        }
    }

    /// Acquires, validates, caches, federates, and conditionally activates the dqn catalog input.
    ///
    /// The per-host writer lock is acquired before network access and remains held through
    /// activation. Snapshot-wide health regressions leave the active catalog unchanged.
    ///
    /// # Errors
    ///
    /// Returns an error when locking, acquisition, validation, private cache persistence, or
    /// catalog activation fails.
    pub fn sync_dqn(&self) -> Result<CatalogSyncResult, CatalogSyncError> {
        self.sync_dqn_with(&UreqDqnTransport::new())
    }

    fn sync_dqn_with(
        &self,
        transport: &impl DqnTransport,
    ) -> Result<CatalogSyncResult, CatalogSyncError> {
        let update = self.store.begin_update()?;
        let acquired = acquire_dqn(transport, &self.cache_root)?;
        let base = self
            .store
            .load_active()?
            .map_or_else(Catalog::default, |active| active.catalog);
        let output = base.federate(FederationInput {
            dqn: Some(acquired.snapshot),
            ..FederationInput::default()
        });
        let blocked = output.quarantine.iter().any(|entry| {
            matches!(
                entry.reason,
                QuarantineReason::SourcePolicyMismatch
                    | QuarantineReason::DqnBindingRegression
                    | QuarantineReason::SourceHealthRegression
            )
        });
        let active_catalog_digest = if blocked {
            update.base_digest().map(str::to_owned)
        } else {
            Some(update.publish(&output.catalog)?.digest)
        };

        Ok(CatalogSyncResult {
            activated: !blocked,
            active_catalog_digest,
            source_content_sha256: acquired.content_sha256,
            source_record_count: acquired.record_count,
            quarantine: output.quarantine,
        })
    }
}

impl CatalogSyncResult {
    #[must_use]
    pub fn into_summary(self) -> CatalogSyncSummary {
        let mut quarantine_counts = BTreeMap::new();
        for entry in self.quarantine {
            *quarantine_counts.entry(entry.reason).or_default() += 1;
        }
        CatalogSyncSummary {
            activated: self.activated,
            active_catalog_digest: self.active_catalog_digest,
            source_content_sha256: self.source_content_sha256,
            source_record_count: self.source_record_count,
            quarantine_counts,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::fs::OpenOptions;
    use std::os::unix::fs::PermissionsExt as _;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::catalog::acquisition::DqnHttpResponse;
    use crate::catalog::adapter::MAX_SOURCE_BYTES;
    use crate::catalog::{SourceRevision, TachiFixtureAdapter};

    const GIT_REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

    struct FakeTransport(Result<DqnHttpResponse, DqnAcquisitionError>);

    impl DqnTransport for FakeTransport {
        fn get(&self) -> Result<DqnHttpResponse, DqnAcquisitionError> {
            match &self.0 {
                Ok(response) => Ok(DqnHttpResponse {
                    status: response.status,
                    content_length: response.content_length,
                    body: response.body.clone(),
                }),
                Err(DqnAcquisitionError::Timeout) => Err(DqnAcquisitionError::Timeout),
                Err(error) => panic!("unsupported fake error: {error}"),
            }
        }
    }

    struct LockCheckingTransport {
        lock_path: PathBuf,
        response: DqnHttpResponse,
    }

    impl DqnTransport for LockCheckingTransport {
        fn get(&self) -> Result<DqnHttpResponse, DqnAcquisitionError> {
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.lock_path)
                .unwrap();
            assert!(matches!(
                lock.try_lock(),
                Err(std::fs::TryLockError::WouldBlock)
            ));
            Ok(DqnHttpResponse {
                status: self.response.status,
                content_length: self.response.content_length,
                body: self.response.body.clone(),
            })
        }
    }

    #[test]
    fn healthy_dqn_response_is_cached_federated_and_activated() {
        let roots = Roots::new();
        seed_tachi(&roots.store);
        let bytes = dqn_bytes("ALPHA", "ARTIST A");
        let result = roots
            .sync()
            .sync_dqn_with(&LockCheckingTransport {
                lock_path: roots.store.join("catalog-sync.lock"),
                response: DqnHttpResponse {
                    status: 200,
                    content_length: Some(bytes.len() as u64),
                    body: bytes.clone(),
                },
            })
            .unwrap();

        assert!(result.activated);
        assert_eq!(result.source_record_count, 1);
        assert!(result.quarantine.is_empty());
        let cache_file = roots
            .cache
            .join("dqn")
            .join(format!("{}.json", result.source_content_sha256));
        assert!(cache_file.is_file());
        assert_eq!(
            fs::metadata(&cache_file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let active = CatalogStore::new(&roots.store)
            .load_active()
            .unwrap()
            .unwrap();
        assert_eq!(Some(active.digest), result.active_catalog_digest);
    }

    #[test]
    fn public_summary_aggregates_quarantine_without_source_keys() {
        let roots = Roots::new();
        let bytes = dqn_bytes("PRIVATE SOURCE TITLE", "PRIVATE SOURCE ARTIST");
        let summary = roots
            .sync()
            .sync_dqn_with(&response(200, Some(bytes.len() as u64), bytes))
            .unwrap()
            .into_summary();
        let json = serde_json::to_string(&summary).unwrap();

        assert_eq!(
            summary
                .quarantine_counts
                .get(&QuarantineReason::ProvisionalWithoutTachiAnchor),
            Some(&1)
        );
        assert!(!json.contains("PRIVATE SOURCE TITLE"));
        assert!(!json.contains("PRIVATE SOURCE ARTIST"));
        assert!(!json.contains("source_key"));
    }

    #[test]
    fn transport_status_and_declared_or_actual_size_fail_before_activation() {
        let roots = Roots::new();
        let bytes = dqn_bytes("ALPHA", "ARTIST A");
        let status = roots
            .sync()
            .sync_dqn_with(&response(503, Some(bytes.len() as u64), bytes.clone()))
            .unwrap_err();
        assert!(status.to_string().contains("HTTP status 503"));

        let redirect = roots
            .sync()
            .sync_dqn_with(&response(
                302,
                Some((MAX_SOURCE_BYTES + 1) as u64),
                Vec::new(),
            ))
            .unwrap_err();
        assert!(redirect.to_string().contains("HTTP status 302"));

        let declared = roots
            .sync()
            .sync_dqn_with(&response(200, Some((MAX_SOURCE_BYTES + 1) as u64), bytes))
            .unwrap_err();
        assert!(declared.to_string().contains("declares"));

        let actual = roots
            .sync()
            .sync_dqn_with(&response(200, None, vec![b' '; MAX_SOURCE_BYTES + 1]))
            .unwrap_err();
        assert!(actual.to_string().contains("maximum"));
        assert!(
            CatalogStore::new(&roots.store)
                .load_active()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn timeout_and_cache_write_failure_leave_active_catalog_unchanged() {
        let roots = Roots::new();
        seed_tachi(&roots.store);
        let before = CatalogStore::new(&roots.store)
            .load_active()
            .unwrap()
            .unwrap()
            .digest;
        let timeout = roots
            .sync()
            .sync_dqn_with(&FakeTransport(Err(DqnAcquisitionError::Timeout)))
            .unwrap_err();
        assert!(timeout.to_string().contains("timed out"));

        fs::write(&roots.cache, b"not a directory").unwrap();
        let bytes = dqn_bytes("ALPHA", "ARTIST A");
        let cache_error = roots
            .sync()
            .sync_dqn_with(&response(200, Some(bytes.len() as u64), bytes))
            .unwrap_err();
        assert!(cache_error.to_string().contains("cache write failed"));
        let after = CatalogStore::new(&roots.store)
            .load_active()
            .unwrap()
            .unwrap()
            .digest;
        assert_eq!(before, after);
    }

    #[test]
    fn snapshot_wide_regressions_do_not_activate_a_candidate() {
        let roots = Roots::new();
        seed_tachi(&roots.store);
        let alpha = dqn_bytes("ALPHA", "ARTIST A");
        let accepted = roots
            .sync()
            .sync_dqn_with(&response(200, Some(alpha.len() as u64), alpha))
            .unwrap();
        let accepted_digest = accepted.active_catalog_digest.unwrap();

        let beta = dqn_bytes("BETA", "ARTIST B");
        let binding_regression = roots
            .sync()
            .sync_dqn_with(&response(200, Some(beta.len() as u64), beta))
            .unwrap();
        assert!(!binding_regression.activated);
        assert_eq!(
            binding_regression.active_catalog_digest.as_deref(),
            Some(accepted_digest.as_str())
        );
        assert!(
            binding_regression
                .quarantine
                .iter()
                .all(|entry| entry.reason == QuarantineReason::DqnBindingRegression)
        );

        let two_records = serde_json::to_vec(&json!([
            { "title": "ALPHA", "artist": "ARTIST A", "packName": null },
            { "title": "MISSING", "artist": "ARTIST M", "packName": null }
        ]))
        .unwrap();
        let larger = roots
            .sync()
            .sync_dqn_with(&response(200, Some(two_records.len() as u64), two_records))
            .unwrap();
        assert!(larger.activated);
        let larger_digest = larger.active_catalog_digest.unwrap();

        let alpha = dqn_bytes("ALPHA", "ARTIST A");
        let health_regression = roots
            .sync()
            .sync_dqn_with(&response(200, Some(alpha.len() as u64), alpha))
            .unwrap();
        assert!(!health_regression.activated);
        assert_eq!(
            health_regression.active_catalog_digest.as_deref(),
            Some(larger_digest.as_str())
        );
        assert_eq!(
            health_regression.quarantine[0].reason,
            QuarantineReason::SourceHealthRegression
        );
    }

    struct Roots {
        _root: TempDir,
        store: PathBuf,
        cache: PathBuf,
    }

    impl Roots {
        fn new() -> Self {
            let root = TempDir::new().unwrap();
            Self {
                store: root.path().join("store"),
                cache: root.path().join("cache"),
                _root: root,
            }
        }

        fn sync(&self) -> CatalogSync {
            CatalogSync::new(&self.store, &self.cache)
        }
    }

    fn response(status: u16, content_length: Option<u64>, body: Vec<u8>) -> FakeTransport {
        FakeTransport(Ok(DqnHttpResponse {
            status,
            content_length,
            body,
        }))
    }

    fn seed_tachi(store: &PathBuf) {
        let fixture = json!({
            "schema": "scorepeek-tachi-fixture-v1",
            "records": [{
                "source_song_id": "anchor-1",
                "title": "ALPHA",
                "title_kind": "in_game_display",
                "artist": "ARTIST A",
                "version": "V1",
                "charts": [
                    { "play_type": "single", "difficulty": "normal", "level": 4,
                      "notes": 400, "source_chart_id": "spn",
                      "product_versions": ["synthetic-v1"], "primary": true },
                    { "play_type": "single", "difficulty": "hyper", "level": 8,
                      "notes": 800, "source_chart_id": "sph",
                      "product_versions": ["synthetic-v1"], "primary": true }
                ],
                "primary_infinitas": false
            }]
        });
        let bytes = serde_json::to_vec(&fixture).unwrap();
        let snapshot =
            TachiFixtureAdapter::parse(&bytes, SourceRevision::git_commit(GIT_REVISION).unwrap())
                .unwrap();
        let catalog = Catalog::default()
            .federate(FederationInput {
                tachi: Some(snapshot),
                ..FederationInput::default()
            })
            .catalog;
        CatalogStore::new(store)
            .begin_update()
            .unwrap()
            .publish(&catalog)
            .unwrap();
    }

    fn dqn_bytes(title: &str, artist: &str) -> Vec<u8> {
        serde_json::to_vec(&json!([{
            "title": title,
            "artist": artist,
            "packName": null
        }]))
        .unwrap()
    }
}
