use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use serde::Serialize;

use super::acquisition::{DqnAcquisitionError, DqnTransport, UreqDqnTransport, acquire_dqn};
use super::federation::{Catalog, FederationInput, QuarantineEntry, QuarantineReason, SourceId};
use super::store::{CatalogStore, CatalogStoreError};
use super::tachi_acquisition::{
    TachiAcquisitionError, TachiTransport, UreqTachiTransport, acquire_tachi,
};
use super::textage_acquisition::{
    TextageAcquisitionError, TextageTransport, UreqTextageTransport, acquire_textage,
};

#[derive(Clone, Debug)]
pub struct CatalogSync {
    store: CatalogStore,
    cache_root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogSyncResult {
    pub activated: bool,
    pub active_catalog_digest: Option<String>,
    pub sources: BTreeMap<SourceId, CatalogSyncSource>,
    pub quarantine: Vec<QuarantineEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CatalogSyncSource {
    pub revision: String,
    pub content_sha256: String,
    pub record_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CatalogSyncSummary {
    pub activated: bool,
    pub active_catalog_digest: Option<String>,
    pub sources: BTreeMap<SourceId, CatalogSyncSource>,
    pub quarantine_counts: BTreeMap<QuarantineReason, usize>,
}

#[derive(Debug)]
pub enum CatalogSyncError {
    Store(CatalogStoreError),
    TachiAcquisition(TachiAcquisitionError),
    TextageAcquisition(TextageAcquisitionError),
    DqnAcquisition(DqnAcquisitionError),
}

impl fmt::Display for CatalogSyncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::TachiAcquisition(error) => error.fmt(formatter),
            Self::TextageAcquisition(error) => error.fmt(formatter),
            Self::DqnAcquisition(error) => error.fmt(formatter),
        }
    }
}

impl Error for CatalogSyncError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::TachiAcquisition(error) => Some(error),
            Self::TextageAcquisition(error) => Some(error),
            Self::DqnAcquisition(error) => Some(error),
        }
    }
}

impl CatalogSyncError {
    #[must_use]
    pub fn redacted_message(&self) -> String {
        match self {
            Self::Store(_) => "catalog store operation failed".to_owned(),
            Self::TachiAcquisition(TachiAcquisitionError::UnexpectedStatus {
                resource,
                status,
            }) => format!("Tachi {resource} acquisition returned unexpected HTTP status {status}"),
            Self::TachiAcquisition(
                TachiAcquisitionError::DeclaredBodyTooLarge { .. }
                | TachiAcquisitionError::BodyTooLarge { .. },
            ) => "Tachi response exceeded the configured size limit".to_owned(),
            Self::TachiAcquisition(TachiAcquisitionError::Timeout(resource)) => {
                format!("Tachi {resource} acquisition timed out")
            }
            Self::TachiAcquisition(TachiAcquisitionError::Transport(resource, _)) => {
                format!("Tachi {resource} transport failed")
            }
            Self::TachiAcquisition(
                TachiAcquisitionError::InvalidRevisionResponse(_)
                | TachiAcquisitionError::InvalidRevision(_),
            ) => "Tachi revision validation failed".to_owned(),
            Self::TachiAcquisition(TachiAcquisitionError::Adapter(_)) => {
                "Tachi seed validation failed".to_owned()
            }
            Self::TachiAcquisition(
                TachiAcquisitionError::CacheIo(_)
                | TachiAcquisitionError::CacheConflict(_)
                | TachiAcquisitionError::CacheCapacityExceeded,
            ) => "Tachi cache persistence failed".to_owned(),
            Self::TextageAcquisition(TextageAcquisitionError::UnexpectedStatus {
                resource,
                status,
            }) => {
                format!("Textage {resource} acquisition returned unexpected HTTP status {status}")
            }
            Self::TextageAcquisition(
                TextageAcquisitionError::DeclaredBodyTooLarge { .. }
                | TextageAcquisitionError::BodyTooLarge { .. },
            ) => "Textage response exceeded the configured size limit".to_owned(),
            Self::TextageAcquisition(TextageAcquisitionError::Timeout(resource)) => {
                format!("Textage {resource} acquisition timed out")
            }
            Self::TextageAcquisition(TextageAcquisitionError::Transport(resource, _)) => {
                format!("Textage {resource} transport failed")
            }
            Self::TextageAcquisition(TextageAcquisitionError::Adapter(_)) => {
                "Textage response validation failed".to_owned()
            }
            Self::TextageAcquisition(
                TextageAcquisitionError::CacheIo(_)
                | TextageAcquisitionError::CacheConflict(_)
                | TextageAcquisitionError::CacheCapacityExceeded,
            ) => "Textage cache persistence failed".to_owned(),
            Self::DqnAcquisition(DqnAcquisitionError::UnexpectedStatus(status)) => {
                format!("dqn acquisition returned unexpected HTTP status {status}")
            }
            Self::DqnAcquisition(
                DqnAcquisitionError::DeclaredBodyTooLarge { .. }
                | DqnAcquisitionError::BodyTooLarge { .. },
            ) => "dqn response exceeded the configured size limit".to_owned(),
            Self::DqnAcquisition(DqnAcquisitionError::Timeout) => {
                "dqn acquisition timed out".to_owned()
            }
            Self::DqnAcquisition(DqnAcquisitionError::Transport(_)) => {
                "dqn transport failed".to_owned()
            }
            Self::DqnAcquisition(DqnAcquisitionError::Adapter(_)) => {
                "dqn response validation failed".to_owned()
            }
            Self::DqnAcquisition(
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
        Self::DqnAcquisition(error)
    }
}

impl From<TachiAcquisitionError> for CatalogSyncError {
    fn from(error: TachiAcquisitionError) -> Self {
        Self::TachiAcquisition(error)
    }
}

impl From<TextageAcquisitionError> for CatalogSyncError {
    fn from(error: TextageAcquisitionError) -> Self {
        Self::TextageAcquisition(error)
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

    /// Acquires, validates, caches, federates, and conditionally activates all live catalog inputs.
    ///
    /// The per-host writer lock is acquired before network access and remains held through
    /// activation. Snapshot-wide health regressions leave the active catalog unchanged.
    ///
    /// # Errors
    ///
    /// Returns an error when locking, acquisition, validation, private cache persistence, or
    /// catalog activation fails.
    pub fn sync(&self) -> Result<CatalogSyncResult, CatalogSyncError> {
        self.sync_with(
            &UreqTachiTransport::new(),
            &UreqTextageTransport::new(),
            &UreqDqnTransport::new(),
        )
    }

    fn sync_with(
        &self,
        tachi_transport: &impl TachiTransport,
        textage_transport: &impl TextageTransport,
        dqn_transport: &impl DqnTransport,
    ) -> Result<CatalogSyncResult, CatalogSyncError> {
        let update = self.store.begin_update()?;
        let tachi = acquire_tachi(tachi_transport, &self.cache_root)?;
        let textage = acquire_textage(textage_transport, &self.cache_root)?;
        let dqn = acquire_dqn(dqn_transport, &self.cache_root)?;
        let base = self
            .store
            .load_active()?
            .map_or_else(Catalog::default, |active| active.catalog);
        let output = base.federate(FederationInput {
            tachi: Some(tachi.snapshot),
            textage: Some(textage.snapshot),
            dqn: Some(dqn.snapshot),
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
            sources: BTreeMap::from([
                (
                    SourceId::Tachi,
                    CatalogSyncSource {
                        revision: tachi.revision,
                        content_sha256: tachi.content_sha256,
                        record_count: tachi.record_count,
                    },
                ),
                (
                    SourceId::Textage,
                    CatalogSyncSource {
                        revision: textage.content_sha256.clone(),
                        content_sha256: textage.content_sha256,
                        record_count: textage.record_count,
                    },
                ),
                (
                    SourceId::DqnIidxapi,
                    CatalogSyncSource {
                        revision: dqn.content_sha256.clone(),
                        content_sha256: dqn.content_sha256,
                        record_count: dqn.record_count,
                    },
                ),
            ]),
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
            sources: self.sources,
            quarantine_counts,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::fs::OpenOptions;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::catalog::acquisition::DqnHttpResponse;
    use crate::catalog::adapter::MAX_SOURCE_BYTES;
    use crate::catalog::tachi_acquisition::{TachiHttpResponse, TachiResource};
    use crate::catalog::textage_acquisition::{TextageHttpResponse, TextageResource};

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

    struct FakeTachiTransport {
        reference: TachiHttpResponse,
        seeds: BTreeMap<TachiResource, TachiHttpResponse>,
    }

    impl TachiTransport for FakeTachiTransport {
        fn get_ref(&self) -> Result<TachiHttpResponse, TachiAcquisitionError> {
            Ok(self.reference.clone())
        }

        fn get_seed(
            &self,
            revision: &str,
            resource: TachiResource,
        ) -> Result<TachiHttpResponse, TachiAcquisitionError> {
            assert_eq!(revision, GIT_REVISION);
            Ok(self.seeds.get(&resource).unwrap().clone())
        }
    }

    struct LockCheckingTachiTransport {
        lock_path: PathBuf,
        inner: FakeTachiTransport,
    }

    impl TachiTransport for LockCheckingTachiTransport {
        fn get_ref(&self) -> Result<TachiHttpResponse, TachiAcquisitionError> {
            assert_writer_lock(&self.lock_path);
            self.inner.get_ref()
        }

        fn get_seed(
            &self,
            revision: &str,
            resource: TachiResource,
        ) -> Result<TachiHttpResponse, TachiAcquisitionError> {
            self.inner.get_seed(revision, resource)
        }
    }

    struct LockCheckingTransport {
        lock_path: PathBuf,
        response: DqnHttpResponse,
    }

    struct FakeTextageTransport {
        responses: BTreeMap<TextageResource, TextageHttpResponse>,
        lock_path: Option<PathBuf>,
    }

    impl TextageTransport for FakeTextageTransport {
        fn get(
            &self,
            resource: TextageResource,
        ) -> Result<TextageHttpResponse, TextageAcquisitionError> {
            if let Some(lock_path) = &self.lock_path {
                assert_writer_lock(lock_path);
            }
            Ok(self.responses[&resource].clone())
        }
    }

    impl DqnTransport for LockCheckingTransport {
        fn get(&self) -> Result<DqnHttpResponse, DqnAcquisitionError> {
            assert_writer_lock(&self.lock_path);
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
        let bytes = dqn_bytes("ALPHA", "ARTIST A");
        let result = roots
            .sync()
            .sync_with(
                &LockCheckingTachiTransport {
                    lock_path: roots.store.join("catalog-sync.lock"),
                    inner: tachi_transport(),
                },
                &textage_transport(Some(roots.store.join("catalog-sync.lock"))),
                &LockCheckingTransport {
                    lock_path: roots.store.join("catalog-sync.lock"),
                    response: DqnHttpResponse {
                        status: 200,
                        content_length: Some(bytes.len() as u64),
                        body: bytes.clone(),
                    },
                },
            )
            .unwrap();

        assert!(result.activated);
        assert_eq!(result.sources[&SourceId::DqnIidxapi].record_count, 1);
        assert!(result.quarantine.is_empty());
        let dqn_digest = &result.sources[&SourceId::DqnIidxapi].content_sha256;
        let cache_file = roots.cache.join("dqn").join(format!("{dqn_digest}.json"));
        assert!(cache_file.is_file());
        assert_eq!(
            fs::metadata(&cache_file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let tachi = &result.sources[&SourceId::Tachi];
        let tachi_cache = roots
            .cache
            .join("tachi")
            .join(format!("{}-{}", tachi.revision, tachi.content_sha256));
        assert_eq!(
            fs::metadata(&tachi_cache).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for filename in [
            "songs-iidx.json",
            "charts-iidx-sp.json",
            "charts-iidx-dp.json",
        ] {
            assert_eq!(
                fs::metadata(tachi_cache.join(filename))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let active = CatalogStore::new(&roots.store)
            .load_active()
            .unwrap()
            .unwrap();
        assert_eq!(Some(active.digest), result.active_catalog_digest);
        let textage = &result.sources[&SourceId::Textage];
        let textage_cache = roots.cache.join("textage").join(&textage.content_sha256);
        assert_eq!(
            fs::metadata(textage_cache).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn public_summary_aggregates_quarantine_without_source_keys() {
        let roots = Roots::new();
        let bytes = dqn_bytes("PRIVATE SOURCE TITLE", "PRIVATE SOURCE ARTIST");
        let summary = sync_with_dqn(&roots, &response(200, Some(bytes.len() as u64), bytes))
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
        let status = sync_with_dqn(
            &roots,
            &response(503, Some(bytes.len() as u64), bytes.clone()),
        )
        .unwrap_err();
        assert!(status.to_string().contains("HTTP status 503"));

        let redirect = sync_with_dqn(
            &roots,
            &response(302, Some((MAX_SOURCE_BYTES + 1) as u64), Vec::new()),
        )
        .unwrap_err();
        assert!(redirect.to_string().contains("HTTP status 302"));

        let declared = sync_with_dqn(
            &roots,
            &response(200, Some((MAX_SOURCE_BYTES + 1) as u64), bytes),
        )
        .unwrap_err();
        assert!(declared.to_string().contains("declares"));

        let actual = sync_with_dqn(
            &roots,
            &response(200, None, vec![b' '; MAX_SOURCE_BYTES + 1]),
        )
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
        let initial = dqn_bytes("ALPHA", "ARTIST A");
        let before = sync_with_dqn(&roots, &response(200, Some(initial.len() as u64), initial))
            .unwrap()
            .active_catalog_digest
            .unwrap();
        let timeout =
            sync_with_dqn(&roots, &FakeTransport(Err(DqnAcquisitionError::Timeout))).unwrap_err();
        assert!(timeout.to_string().contains("timed out"));

        let dqn_cache = roots.cache.join("dqn");
        fs::remove_dir_all(&dqn_cache).unwrap();
        fs::write(&dqn_cache, b"not a directory").unwrap();
        let bytes = dqn_bytes("ALPHA", "ARTIST A");
        let cache_error =
            sync_with_dqn(&roots, &response(200, Some(bytes.len() as u64), bytes)).unwrap_err();
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
        let alpha = dqn_bytes("ALPHA", "ARTIST A");
        let accepted =
            sync_with_dqn(&roots, &response(200, Some(alpha.len() as u64), alpha)).unwrap();
        let accepted_digest = accepted.active_catalog_digest.unwrap();

        let beta = dqn_bytes("BETA", "ARTIST B");
        let binding_regression =
            sync_with_dqn(&roots, &response(200, Some(beta.len() as u64), beta)).unwrap();
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
        let larger = sync_with_dqn(
            &roots,
            &response(200, Some(two_records.len() as u64), two_records),
        )
        .unwrap();
        assert!(larger.activated);
        let larger_digest = larger.active_catalog_digest.unwrap();

        let alpha = dqn_bytes("ALPHA", "ARTIST A");
        let health_regression =
            sync_with_dqn(&roots, &response(200, Some(alpha.len() as u64), alpha)).unwrap();
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

    fn assert_writer_lock(lock_path: &Path) {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock_path)
            .unwrap();
        assert!(matches!(
            lock.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
    }

    fn sync_with_dqn(
        roots: &Roots,
        dqn: &impl DqnTransport,
    ) -> Result<CatalogSyncResult, CatalogSyncError> {
        roots
            .sync()
            .sync_with(&tachi_transport(), &textage_transport(None), dqn)
    }

    fn tachi_transport() -> FakeTachiTransport {
        let reference = serde_json::to_vec(&json!({
            "ref": "refs/heads/main",
            "node_id": "synthetic-node",
            "url": "https://example.invalid/ref",
            "object": {
                "sha": GIT_REVISION,
                "type": "commit",
                "url": "https://example.invalid/commit"
            }
        }))
        .unwrap();
        let songs = serde_json::to_vec(&json!([{
                "altTitles": [],
                "artist": "ARTIST A",
                "data": { "displayVersion": "1", "genre": "SYNTHETIC" },
                "id": "S0000000000000000001",
                "legacySongID": 1,
                "searchTerms": [],
                "title": "ALPHA"
        }]))
        .unwrap();
        let single_charts = serde_json::to_vec(&json!([
            {
                "data": { "notecount": 400 },
                "difficulty": "NORMAL",
                "id": "C0000000000000000001",
                "isPrimary": true,
                "legacyChartID": "synthetic-spn",
                "level": "4",
                "levelNum": 4,
                "songID": "S0000000000000000001",
                "versions": ["synthetic-v1"]
            },
            {
                "data": { "notecount": 800 },
                "difficulty": "HYPER",
                "id": "C0000000000000000002",
                "isPrimary": true,
                "legacyChartID": "synthetic-sph",
                "level": "8",
                "levelNum": 8,
                "songID": "S0000000000000000001",
                "versions": ["synthetic-v1"]
            }
        ]))
        .unwrap();
        let double_charts = b"[]".to_vec();
        FakeTachiTransport {
            reference: ok_tachi_response(reference),
            seeds: BTreeMap::from([
                (TachiResource::Songs, ok_tachi_response(songs)),
                (
                    TachiResource::SingleCharts,
                    ok_tachi_response(single_charts),
                ),
                (
                    TachiResource::DoubleCharts,
                    ok_tachi_response(double_charts),
                ),
            ]),
        }
    }

    fn ok_tachi_response(body: Vec<u8>) -> TachiHttpResponse {
        TachiHttpResponse {
            status: 200,
            content_length: Some(body.len() as u64),
            body,
        }
    }

    fn textage_transport(lock_path: Option<PathBuf>) -> FakeTextageTransport {
        let title = br#"VERINDEX=0;IDINDEX=1;OPTINDEX=2;GENREINDEX=3;ARTISTINDEX=4;TITLEINDEX=5;SUBTITLEINDEX=6;SS=0;titletbl={'alpha':[1,10,0,"GENRE","ARTIST A","ALPHA"]};"#.to_vec();
        let availability = br#"pspver="version";A=10,B=11,C=12,D=13,E=14,F=15;actbl={'alpha':[1,0,0,1,7,4,7,8,7,A,7,0,0,0,0,4,7,8,7,A,7,0,0]};"#.to_vec();
        let chart = br#"datatbl={'alpha':[0,100,400,800,1200,0,0,410,810,1210,0,"120"]};"#.to_vec();
        FakeTextageTransport {
            responses: [
                (TextageResource::Title, title),
                (TextageResource::Availability, availability),
                (TextageResource::Chart, chart),
            ]
            .into_iter()
            .map(|(resource, body)| {
                (
                    resource,
                    TextageHttpResponse {
                        status: 200,
                        content_length: Some(body.len() as u64),
                        body,
                    },
                )
            })
            .collect(),
            lock_path,
        }
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
