use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use serde::Serialize;

use super::domain::{
    Catalog, CatalogFederationExt, FederationInput, QuarantineEntry, QuarantineReason, SourceId,
};
use crate::source::dqn::acquire::{
    DqnAcquisitionError, DqnTransport, UreqDqnTransport, acquire_dqn,
};
use crate::source::tachi::acquire::{
    TachiAcquisitionError, TachiTransport, UreqTachiTransport, acquire_tachi,
};
use crate::source::textage::acquire::{
    TextageAcquisitionError, TextageTransport, UreqTextageTransport, acquire_textage,
};
use crate::store::WorkspaceLock;

#[derive(Clone, Debug)]
pub struct CatalogSync {
    work_directory: PathBuf,
    cache_root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogSyncResult {
    pub catalog: Option<Catalog>,
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
    pub candidate_accepted: bool,
    pub sources: BTreeMap<SourceId, CatalogSyncSource>,
    pub quarantine_counts: BTreeMap<QuarantineReason, usize>,
}

#[derive(Debug)]
pub enum CatalogSyncError {
    Workspace(std::io::Error),
    TachiAcquisition(TachiAcquisitionError),
    TextageAcquisition(TextageAcquisitionError),
    DqnAcquisition(DqnAcquisitionError),
}

impl fmt::Display for CatalogSyncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Workspace(error) => error.fmt(formatter),
            Self::TachiAcquisition(error) => error.fmt(formatter),
            Self::TextageAcquisition(error) => error.fmt(formatter),
            Self::DqnAcquisition(error) => error.fmt(formatter),
        }
    }
}

impl Error for CatalogSyncError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Workspace(error) => Some(error),
            Self::TachiAcquisition(error) => Some(error),
            Self::TextageAcquisition(error) => Some(error),
            Self::DqnAcquisition(error) => Some(error),
        }
    }
}

impl From<std::io::Error> for CatalogSyncError {
    fn from(error: std::io::Error) -> Self {
        Self::Workspace(error)
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
    pub fn new(work_directory: impl Into<PathBuf>, cache_root: impl Into<PathBuf>) -> Self {
        Self {
            work_directory: work_directory.into(),
            cache_root: cache_root.into(),
        }
    }

    /// Acquires, validates, caches, and federates all live catalog inputs.
    ///
    /// # Errors
    /// Returns an error when locking, acquisition, or cache persistence fails.
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
        let _lock = WorkspaceLock::acquire(&self.work_directory)?;
        let tachi = acquire_tachi(tachi_transport, &self.cache_root)?;
        let textage = acquire_textage(textage_transport, &self.cache_root)?;
        let dqn = acquire_dqn(dqn_transport, &self.cache_root)?;
        // Every candidate is rebuilt from the three current snapshots.
        let output = Catalog::default().federate(FederationInput {
            tachi: Some(tachi.snapshot),
            textage: Some(textage.snapshot),
            dqn: Some(dqn.snapshot),
        });
        let blocked = output.quarantine.iter().any(|entry| {
            !matches!(
                entry.reason,
                QuarantineReason::ProvisionalWithoutTachiAnchor
                    | QuarantineReason::AmbiguousIdentity
                    | QuarantineReason::ConflictingChart
            )
        });
        let catalog = (!blocked).then_some(output.catalog);

        Ok(CatalogSyncResult {
            catalog,
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
            candidate_accepted: self.catalog.is_some(),
            sources: self.sources,
            quarantine_counts,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::path::Path;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::source::common::MAX_SOURCE_BYTES;
    use crate::source::dqn::acquire::DqnHttpResponse;
    use crate::source::tachi::acquire::{TachiHttpResponse, TachiResource};
    use crate::source::textage::acquire::{TextageHttpResponse, TextageResource};

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
    fn healthy_dqn_response_is_cached_and_federated() {
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

        assert!(result.catalog.is_some());
        assert_eq!(result.sources[&SourceId::DqnIidxapi].record_count, 1);
        assert!(result.quarantine.is_empty());
        let dqn_digest = &result.sources[&SourceId::DqnIidxapi].content_sha256;
        let cache_file = roots.cache.join("dqn").join(format!("{dqn_digest}.json"));
        assert!(cache_file.is_file());
        let tachi = &result.sources[&SourceId::Tachi];
        let tachi_cache = roots
            .cache
            .join("tachi")
            .join(format!("{}-{}", tachi.revision, tachi.content_sha256));
        assert!(tachi_cache.is_dir());
        for filename in [
            "songs-iidx.json",
            "charts-iidx-sp.json",
            "charts-iidx-dp.json",
        ] {
            assert!(tachi_cache.join(filename).is_file());
        }
        assert!(!roots.store.join("catalog-store").exists());
        let textage = &result.sources[&SourceId::Textage];
        let textage_cache = roots.cache.join("textage").join(&textage.content_sha256);
        assert!(textage_cache.is_dir());
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
    fn transport_status_and_declared_or_actual_size_reject_candidates() {
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
        assert!(!roots.store.join("catalog-store").exists());
    }

    #[test]
    fn every_snapshot_is_zero_built_and_current_removals_change_candidate() {
        let roots = Roots::new();
        let alpha = dqn_bytes("ALPHA", "ARTIST A");
        let first = sync_with_dqn(&roots, &response(200, Some(alpha.len() as u64), alpha))
            .unwrap()
            .catalog
            .unwrap()
            .semantic_digest();
        let beta = dqn_bytes("BETA", "ARTIST B");
        let second = sync_with_dqn(&roots, &response(200, Some(beta.len() as u64), beta))
            .unwrap()
            .catalog
            .unwrap()
            .semantic_digest();
        assert_ne!(first, second);
        assert!(!roots.store.join("catalog-store").exists());
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
