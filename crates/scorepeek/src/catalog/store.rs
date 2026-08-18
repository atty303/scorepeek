use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::Builder;
use uuid::Uuid;

use super::federation::{
    Catalog, CatalogSong, Chart, ChartAssertion, ChartKey, Completeness, Difficulty,
    DisplayVariant, DisplayVariantKind, DqnBinding, EvidenceId, ExactTitleArtist, InfinitasStatus,
    LineageId, PlayType, RevisionStrategy, ScorepeekSongId, SourceEvidence, SourceId, SourcePolicy,
};

const MANIFEST_SCHEMA: &str = "scorepeek-active-catalog-v1";
const SNAPSHOT_SCHEMA: &str = "scorepeek-catalog-snapshot-v1";
const SNAPSHOT_FILE: &str = "catalog.sqlite3";
const MAX_MANIFEST_BYTES: usize = 4 * 1024;
const MAX_SNAPSHOT_BYTES: u64 = 128 * 1024 * 1024;
const MAX_SNAPSHOT_GENERATIONS: usize = 32;
const MAX_SNAPSHOT_STORAGE_BYTES: u64 = 512 * 1024 * 1024;
const SNAPSHOT_STAGING_PREFIX: &str = ".catalog-staging-";
const MANIFEST_STAGING_PREFIX: &str = ".catalog-manifest-staging-";

#[derive(Clone, Debug)]
pub struct CatalogStore {
    root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveCatalog {
    pub digest: String,
    pub catalog: Catalog,
}

pub struct CatalogUpdate {
    store: CatalogStore,
    lock: File,
    base_digest: Option<String>,
}

#[derive(Debug)]
pub enum CatalogStoreError {
    Io(io::Error),
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    InvalidManifest(String),
    InvalidSnapshot(String),
    BaseDigestChanged {
        expected: Option<String>,
        actual: Option<String>,
    },
    CapacityExceeded,
}

impl fmt::Display for CatalogStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "catalog store I/O failed: {error}"),
            Self::Sqlite(error) => write!(formatter, "catalog SQLite operation failed: {error}"),
            Self::Json(error) => write!(formatter, "catalog manifest JSON failed: {error}"),
            Self::InvalidManifest(detail) => write!(formatter, "invalid active manifest: {detail}"),
            Self::InvalidSnapshot(detail) => {
                write!(formatter, "invalid catalog snapshot: {detail}")
            }
            Self::BaseDigestChanged { expected, actual } => write!(
                formatter,
                "active catalog changed while update was built: expected {expected:?}, found {actual:?}"
            ),
            Self::CapacityExceeded => formatter.write_str("catalog snapshot capacity is exhausted"),
        }
    }
}

impl Error for CatalogStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::InvalidManifest(_)
            | Self::InvalidSnapshot(_)
            | Self::BaseDigestChanged { .. }
            | Self::CapacityExceeded => None,
        }
    }
}

impl From<io::Error> for CatalogStoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for CatalogStoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<serde_json::Error> for CatalogStoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl CatalogStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Acquires the per-store writer lock before source acquisition or federation begins.
    ///
    /// # Errors
    ///
    /// Returns an error if the private store directories, lock, or current active manifest cannot
    /// be opened. The lock is held until the returned update is dropped.
    pub fn begin_update(&self) -> Result<CatalogUpdate, CatalogStoreError> {
        create_private_directory(&self.root)?;
        create_private_directory(&self.content_dir())?;
        create_private_directory(&self.manifest_dir())?;

        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.root.join("catalog-sync.lock"))?;
        lock.lock()?;
        recover_staging(&self.content_dir(), &self.manifest_dir())?;
        let base_digest = self
            .read_manifest()?
            .map(|manifest| manifest.catalog_digest);
        Ok(CatalogUpdate {
            store: self.clone(),
            lock,
            base_digest,
        })
    }

    /// Loads and verifies the active content-addressed catalog, if one is activated.
    ///
    /// # Errors
    ///
    /// Returns an error when the manifest or snapshot is malformed, missing, or has a digest that
    /// does not match its content-addressed path.
    pub fn load_active(&self) -> Result<Option<ActiveCatalog>, CatalogStoreError> {
        let Some(manifest) = self.read_manifest()? else {
            return Ok(None);
        };
        validate_digest(&manifest.catalog_digest)?;
        let expected_relative = format!("content/{}/{SNAPSHOT_FILE}", manifest.catalog_digest);
        if manifest.snapshot_path != expected_relative {
            return Err(CatalogStoreError::InvalidManifest(
                "snapshot_path does not match catalog_digest".to_owned(),
            ));
        }
        let snapshot_path = self.root.join(&manifest.snapshot_path);
        let actual_digest = digest_file(&snapshot_path)?;
        if actual_digest != manifest.catalog_digest {
            return Err(CatalogStoreError::InvalidSnapshot(format!(
                "content digest is {actual_digest}, expected {}",
                manifest.catalog_digest
            )));
        }
        let catalog = read_snapshot(&snapshot_path)?;
        Ok(Some(ActiveCatalog {
            digest: manifest.catalog_digest,
            catalog,
        }))
    }

    fn content_dir(&self) -> PathBuf {
        self.root.join("content")
    }

    fn manifest_dir(&self) -> PathBuf {
        self.root.join("manifests")
    }

    fn active_manifest_path(&self) -> PathBuf {
        self.manifest_dir().join("active.json")
    }

    fn read_manifest(&self) -> Result<Option<ActiveManifest>, CatalogStoreError> {
        let path = self.active_manifest_path();
        let metadata = match path.symlink_metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() || metadata.len() > MAX_MANIFEST_BYTES as u64 {
            return Err(CatalogStoreError::InvalidManifest(
                "active manifest is not a bounded regular file".to_owned(),
            ));
        }
        let capacity = usize::try_from(metadata.len()).map_err(|_| {
            CatalogStoreError::InvalidManifest(
                "active manifest size is not representable".to_owned(),
            )
        })?;
        let mut bytes = Vec::with_capacity(capacity);
        File::open(path)?
            .take((MAX_MANIFEST_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(CatalogStoreError::InvalidManifest(
                "active manifest exceeds the size limit while reading".to_owned(),
            ));
        }
        let manifest: ActiveManifest = serde_json::from_slice(&bytes)?;
        if manifest.schema != MANIFEST_SCHEMA {
            return Err(CatalogStoreError::InvalidManifest(format!(
                "schema is {:?}, expected {MANIFEST_SCHEMA:?}",
                manifest.schema
            )));
        }
        Ok(Some(manifest))
    }
}

impl CatalogUpdate {
    #[must_use]
    pub fn base_digest(&self) -> Option<&str> {
        self.base_digest.as_deref()
    }

    /// Publishes a complete snapshot and atomically activates its manifest.
    ///
    /// # Errors
    ///
    /// Returns an error if snapshot construction, durability synchronization, base-digest
    /// revalidation, or manifest replacement fails. A failed operation leaves either the previous
    /// active snapshot or a complete new snapshot visible.
    pub fn publish(self, catalog: &Catalog) -> Result<ActiveCatalog, CatalogStoreError> {
        self.publish_with(catalog, |_| Ok(()))
    }

    fn publish_with(
        self,
        catalog: &Catalog,
        mut checkpoint: impl FnMut(PublishPoint) -> io::Result<()>,
    ) -> Result<ActiveCatalog, CatalogStoreError> {
        let staging = Builder::new()
            .prefix(SNAPSHOT_STAGING_PREFIX)
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(self.store.content_dir())?;
        let staging_snapshot = staging.path().join(SNAPSHOT_FILE);
        write_snapshot(&staging_snapshot, catalog)?;
        let staging_size = staging_snapshot.metadata()?.len();
        if staging_size > MAX_SNAPSHOT_BYTES {
            return Err(CatalogStoreError::CapacityExceeded);
        }
        File::open(&staging_snapshot)?.sync_all()?;
        checkpoint(PublishPoint::SnapshotFileSynced)?;
        File::open(staging.path())?.sync_all()?;
        checkpoint(PublishPoint::StagingDirectorySynced)?;

        let digest = digest_file(&staging_snapshot)?;
        let destination = self.store.content_dir().join(&digest);
        if destination.exists() {
            let existing = destination.join(SNAPSHOT_FILE);
            if digest_file(&existing)? != digest {
                return Err(CatalogStoreError::InvalidSnapshot(
                    "existing content-addressed destination has different bytes".to_owned(),
                ));
            }
        } else {
            ensure_snapshot_capacity(&self.store.content_dir(), staging.path(), staging_size)?;
            fs::rename(staging.path(), &destination)?;
        }
        checkpoint(PublishPoint::SnapshotRenamed)?;
        File::open(self.store.content_dir())?.sync_all()?;
        checkpoint(PublishPoint::ContentParentSynced)?;

        let manifest = ActiveManifest {
            schema: MANIFEST_SCHEMA.to_owned(),
            catalog_digest: digest.clone(),
            snapshot_path: format!("content/{digest}/{SNAPSHOT_FILE}"),
        };
        let mut temporary = Builder::new()
            .prefix(MANIFEST_STAGING_PREFIX)
            .tempfile_in(self.store.manifest_dir())?;
        serde_json::to_writer(&mut temporary, &manifest)?;
        temporary.write_all(b"\n")?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        checkpoint(PublishPoint::ManifestFileSynced)?;
        let actual_base = self
            .store
            .read_manifest()?
            .map(|manifest| manifest.catalog_digest);
        if actual_base != self.base_digest {
            return Err(CatalogStoreError::BaseDigestChanged {
                expected: self.base_digest,
                actual: actual_base,
            });
        }
        temporary
            .persist(self.store.active_manifest_path())
            .map_err(|error| error.error)?;
        checkpoint(PublishPoint::ManifestRenamed)?;
        File::open(self.store.manifest_dir())?.sync_all()?;
        checkpoint(PublishPoint::ManifestParentSynced)?;

        drop(self.lock);
        Ok(ActiveCatalog {
            digest,
            catalog: catalog.clone(),
        })
    }
}

fn recover_staging(content_directory: &Path, manifest_directory: &Path) -> io::Result<()> {
    let mut removed_content = false;
    for entry in fs::read_dir(content_directory)? {
        let entry = entry?;
        let is_staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(SNAPSHOT_STAGING_PREFIX));
        if !is_staging {
            continue;
        }
        if !entry.path().symlink_metadata()?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "catalog snapshot staging entry is not a directory",
            ));
        }
        fs::remove_dir_all(entry.path())?;
        removed_content = true;
    }
    if removed_content {
        File::open(content_directory)?.sync_all()?;
    }

    let mut removed_manifest = false;
    for entry in fs::read_dir(manifest_directory)? {
        let entry = entry?;
        let is_staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(MANIFEST_STAGING_PREFIX));
        if !is_staging {
            continue;
        }
        if !entry.path().symlink_metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "catalog manifest staging entry is not a file",
            ));
        }
        fs::remove_file(entry.path())?;
        removed_manifest = true;
    }
    if removed_manifest {
        File::open(manifest_directory)?.sync_all()?;
    }
    Ok(())
}

fn ensure_snapshot_capacity(
    content_directory: &Path,
    current_staging: &Path,
    incoming_bytes: u64,
) -> Result<(), CatalogStoreError> {
    let mut generations = 0_usize;
    let mut total_bytes = 0_u64;
    for entry in fs::read_dir(content_directory)? {
        let entry = entry?;
        if entry.path() == current_staging {
            continue;
        }
        let metadata = entry.path().symlink_metadata()?;
        if !metadata.is_dir() {
            return Err(CatalogStoreError::CapacityExceeded);
        }
        let snapshot = entry.path().join(SNAPSHOT_FILE);
        let snapshot_metadata = snapshot.symlink_metadata()?;
        if !snapshot_metadata.is_file() || snapshot_metadata.len() > MAX_SNAPSHOT_BYTES {
            return Err(CatalogStoreError::CapacityExceeded);
        }
        let mut children = fs::read_dir(entry.path())?;
        let first = children.next().transpose()?;
        let second = children.next().transpose()?;
        if first.as_ref().map(std::fs::DirEntry::path) != Some(snapshot) || second.is_some() {
            return Err(CatalogStoreError::CapacityExceeded);
        }
        generations = generations.saturating_add(1);
        total_bytes = total_bytes.saturating_add(snapshot_metadata.len());
        if generations >= MAX_SNAPSHOT_GENERATIONS || total_bytes > MAX_SNAPSHOT_STORAGE_BYTES {
            return Err(CatalogStoreError::CapacityExceeded);
        }
    }
    if total_bytes.saturating_add(incoming_bytes) > MAX_SNAPSHOT_STORAGE_BYTES {
        return Err(CatalogStoreError::CapacityExceeded);
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ActiveManifest {
    schema: String,
    catalog_digest: String,
    snapshot_path: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PublishPoint {
    SnapshotFileSynced,
    StagingDirectorySynced,
    SnapshotRenamed,
    ContentParentSynced,
    ManifestFileSynced,
    ManifestRenamed,
    ManifestParentSynced,
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    let mut missing = Vec::new();
    let mut candidate = path;
    loop {
        match candidate.symlink_metadata() {
            Ok(metadata) if metadata.is_dir() => break,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "catalog ancestor is not a directory",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(candidate.to_owned());
                candidate = candidate.parent().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "directory has no existing ancestor",
                    )
                })?;
            }
            Err(error) => return Err(error),
        }
    }
    for directory in missing.into_iter().rev() {
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        sync_directory_and_parent(&directory)?;
    }
    sync_directory_and_parent(path)
}

fn sync_directory_and_parent(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn create_snapshot_schema(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch(
        "PRAGMA page_size = 4096;
         PRAGMA journal_mode = OFF;
         PRAGMA synchronous = OFF;
         PRAGMA application_id = 0x5343504b;
         PRAGMA user_version = 1;
         CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
         CREATE TABLE source_evidence (
             source_id TEXT NOT NULL,
             lineage_id TEXT NOT NULL,
             revision_strategy TEXT NOT NULL,
             revision TEXT NOT NULL,
             content_sha256 TEXT NOT NULL,
             byte_size INTEGER NOT NULL,
             record_count INTEGER NOT NULL,
             parser_version TEXT NOT NULL,
             declared_scope TEXT NOT NULL,
             completeness TEXT NOT NULL,
             freshness TEXT NOT NULL,
             rights_and_provenance TEXT NOT NULL,
             PRIMARY KEY (source_id, revision, content_sha256)
         ) WITHOUT ROWID;
         CREATE TABLE source_authority (
             source_id TEXT NOT NULL,
             revision TEXT NOT NULL,
             content_sha256 TEXT NOT NULL,
             field_name TEXT NOT NULL,
             PRIMARY KEY (source_id, revision, content_sha256, field_name)
         ) WITHOUT ROWID;
         CREATE TABLE latest_evidence (
             source_id TEXT PRIMARY KEY,
             revision TEXT NOT NULL,
             content_sha256 TEXT NOT NULL
         ) WITHOUT ROWID;",
    )?;
    create_song_schema(connection)
}

fn create_song_schema(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch(
        "CREATE TABLE songs (
             song_id TEXT PRIMARY KEY,
             tachi_source_id TEXT NOT NULL UNIQUE,
             artist TEXT NOT NULL,
             version TEXT NOT NULL,
             infinitas_status TEXT NOT NULL,
             tachi_primary_infinitas INTEGER NOT NULL CHECK (tachi_primary_infinitas IN (0, 1))
         ) WITHOUT ROWID;
         CREATE TABLE title_variants (
             song_id TEXT NOT NULL,
             source_id TEXT NOT NULL,
             evidence_digest TEXT NOT NULL,
             variant_kind TEXT NOT NULL,
             value TEXT NOT NULL,
             PRIMARY KEY (song_id, source_id, evidence_digest, variant_kind, value)
         ) WITHOUT ROWID;
         CREATE TABLE charts (
             song_id TEXT NOT NULL,
             play_type TEXT NOT NULL,
             difficulty TEXT NOT NULL,
             level INTEGER NOT NULL,
             notes INTEGER NOT NULL,
             PRIMARY KEY (song_id, play_type, difficulty)
         ) WITHOUT ROWID;
         CREATE TABLE chart_assertions (
             song_id TEXT NOT NULL,
             play_type TEXT NOT NULL,
             difficulty TEXT NOT NULL,
             source_id TEXT NOT NULL,
             evidence_digest TEXT NOT NULL,
             source_chart_id TEXT NOT NULL,
             is_primary INTEGER NOT NULL CHECK (is_primary IN (0, 1)),
             PRIMARY KEY (song_id, play_type, difficulty, source_id, evidence_digest,
                          source_chart_id)
         ) WITHOUT ROWID;
         CREATE TABLE chart_assertion_products (
             song_id TEXT NOT NULL,
             play_type TEXT NOT NULL,
             difficulty TEXT NOT NULL,
             source_id TEXT NOT NULL,
             evidence_digest TEXT NOT NULL,
             source_chart_id TEXT NOT NULL,
             product_version TEXT NOT NULL,
             PRIMARY KEY (song_id, play_type, difficulty, source_id, evidence_digest,
                          source_chart_id, product_version)
         ) WITHOUT ROWID;
         CREATE TABLE source_bindings (
             song_id TEXT NOT NULL,
             source_id TEXT NOT NULL,
             source_key TEXT NOT NULL,
             PRIMARY KEY (song_id, source_id, source_key),
             UNIQUE (source_id, source_key)
         ) WITHOUT ROWID;
         CREATE TABLE binding_evidence (
             song_id TEXT NOT NULL,
             source_id TEXT NOT NULL,
             source_key TEXT NOT NULL,
             evidence_digest TEXT NOT NULL,
             PRIMARY KEY (song_id, source_id, source_key, evidence_digest)
         ) WITHOUT ROWID;
         CREATE TABLE binding_attributes (
             song_id TEXT NOT NULL,
             source_id TEXT NOT NULL,
             source_key TEXT NOT NULL,
             evidence_digest TEXT NOT NULL,
             attribute_key TEXT NOT NULL,
             attribute_value TEXT NOT NULL,
             PRIMARY KEY (song_id, source_id, source_key, evidence_digest, attribute_key)
         ) WITHOUT ROWID;
         CREATE TABLE dqn_bindings (
             title TEXT NOT NULL,
             artist TEXT NOT NULL,
             song_id TEXT NOT NULL,
             PRIMARY KEY (title, artist)
         ) WITHOUT ROWID;
         CREATE TABLE dqn_binding_evidence (
             title TEXT NOT NULL,
             artist TEXT NOT NULL,
             evidence_digest TEXT NOT NULL,
             availability_kind TEXT NOT NULL CHECK (availability_kind IN ('base', 'pack')),
             pack_name TEXT NOT NULL,
             CHECK ((availability_kind = 'base' AND pack_name = '') OR
                    (availability_kind = 'pack' AND pack_name <> '')),
             PRIMARY KEY (title, artist, evidence_digest, availability_kind, pack_name)
         ) WITHOUT ROWID;",
    )
}

fn write_snapshot(path: &Path, catalog: &Catalog) -> Result<(), CatalogStoreError> {
    catalog
        .validate()
        .map_err(CatalogStoreError::InvalidSnapshot)?;
    let mut connection = Connection::open(path)?;
    create_snapshot_schema(&connection)?;
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT INTO metadata (key, value) VALUES ('schema', ?1)",
        [SNAPSHOT_SCHEMA],
    )?;
    write_catalog_rows(&transaction, catalog)?;
    transaction.commit()?;
    connection.close().map_err(|(_, error)| error)?;
    Ok(())
}

fn write_catalog_rows(
    transaction: &Transaction<'_>,
    catalog: &Catalog,
) -> Result<(), rusqlite::Error> {
    write_evidence_rows(transaction, catalog)?;
    write_song_rows(transaction, catalog)?;
    write_dqn_rows(transaction, catalog)
}

fn write_evidence_rows(
    transaction: &Transaction<'_>,
    catalog: &Catalog,
) -> Result<(), rusqlite::Error> {
    for evidence in catalog.source_evidence.values() {
        transaction.execute(
            "INSERT INTO source_evidence VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                source_id_label(evidence.source_id),
                lineage_id_label(evidence.lineage_id),
                revision_strategy_label(evidence.revision_strategy),
                evidence.revision,
                evidence.content_sha256,
                i64::try_from(evidence.byte_size).expect("bounded source size fits SQLite INTEGER"),
                i64::try_from(evidence.record_count)
                    .expect("bounded source count fits SQLite INTEGER"),
                evidence.parser_version,
                evidence.declared_scope,
                completeness_label(evidence.completeness),
                evidence.freshness,
                evidence.rights_and_provenance,
            ],
        )?;
        for field_name in &evidence.field_authority {
            transaction.execute(
                "INSERT INTO source_authority VALUES (?1, ?2, ?3, ?4)",
                params![
                    source_id_label(evidence.source_id),
                    evidence.revision,
                    evidence.content_sha256,
                    field_name
                ],
            )?;
        }
    }
    for (source_id, evidence_id) in &catalog.latest_evidence {
        transaction.execute(
            "INSERT INTO latest_evidence VALUES (?1, ?2, ?3)",
            params![
                source_id_label(*source_id),
                evidence_id.revision,
                evidence_id.content_sha256
            ],
        )?;
    }
    Ok(())
}

fn write_song_rows(
    transaction: &Transaction<'_>,
    catalog: &Catalog,
) -> Result<(), rusqlite::Error> {
    for song in catalog.songs.values() {
        let song_id = song.song_id.as_uuid().to_string();
        transaction.execute(
            "INSERT INTO songs VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                song_id,
                song.tachi_source_id,
                song.artist,
                song.version,
                infinitas_status_label(song.infinitas_status),
                song.tachi_primary_infinitas,
            ],
        )?;
        write_song_detail_rows(transaction, &song_id, song)?;
    }
    Ok(())
}

fn write_song_detail_rows(
    transaction: &Transaction<'_>,
    song_id: &str,
    song: &CatalogSong,
) -> Result<(), rusqlite::Error> {
    for variant in &song.title_variants {
        transaction.execute(
            "INSERT INTO title_variants VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                song_id,
                source_id_label(variant.source_id),
                evidence_key(&variant.evidence_id),
                display_variant_kind_label(variant.kind),
                variant.value
            ],
        )?;
    }
    for chart in song.charts.values() {
        transaction.execute(
            "INSERT INTO charts VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                song_id,
                play_type_label(chart.key.play_type),
                difficulty_label(chart.key.difficulty),
                chart.level,
                chart.notes,
            ],
        )?;
    }
    for (key, assertions) in &song.chart_assertions {
        for assertion in assertions {
            transaction.execute(
                "INSERT INTO chart_assertions VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    song_id,
                    play_type_label(key.play_type),
                    difficulty_label(key.difficulty),
                    source_id_label(assertion.evidence_id.source_id),
                    evidence_key(&assertion.evidence_id),
                    assertion.source_chart_id,
                    assertion.primary,
                ],
            )?;
            for product_version in &assertion.product_versions {
                transaction.execute(
                    "INSERT INTO chart_assertion_products VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        song_id,
                        play_type_label(key.play_type),
                        difficulty_label(key.difficulty),
                        source_id_label(assertion.evidence_id.source_id),
                        evidence_key(&assertion.evidence_id),
                        assertion.source_chart_id,
                        product_version,
                    ],
                )?;
            }
        }
    }
    for (source_id, bindings) in &song.source_bindings {
        for source_key in bindings {
            transaction.execute(
                "INSERT INTO source_bindings VALUES (?1, ?2, ?3)",
                params![song_id, source_id_label(*source_id), source_key],
            )?;
        }
    }
    for ((source_id, source_key), evidence_ids) in &song.binding_evidence {
        for evidence_id in evidence_ids {
            transaction.execute(
                "INSERT INTO binding_evidence VALUES (?1, ?2, ?3, ?4)",
                params![
                    song_id,
                    source_id_label(*source_id),
                    source_key,
                    evidence_key(evidence_id),
                ],
            )?;
        }
    }
    for ((source_id, source_key, evidence_id), attributes) in &song.binding_attributes {
        for (attribute_key, attribute_value) in attributes {
            transaction.execute(
                "INSERT INTO binding_attributes VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    song_id,
                    source_id_label(*source_id),
                    source_key,
                    evidence_key(evidence_id),
                    attribute_key,
                    attribute_value,
                ],
            )?;
        }
    }
    Ok(())
}

fn write_dqn_rows(transaction: &Transaction<'_>, catalog: &Catalog) -> Result<(), rusqlite::Error> {
    for (tuple, binding) in &catalog.dqn_bindings {
        transaction.execute(
            "INSERT INTO dqn_bindings VALUES (?1, ?2, ?3)",
            params![
                tuple.title,
                tuple.artist,
                binding.song_id.as_uuid().to_string()
            ],
        )?;
        for (evidence_id, packs) in &binding.evidence_packs {
            for pack in packs {
                let (availability_kind, pack_name) = match pack {
                    Some(pack) => ("pack", pack.as_str()),
                    None => ("base", ""),
                };
                transaction.execute(
                    "INSERT INTO dqn_binding_evidence VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        tuple.title,
                        tuple.artist,
                        evidence_key(evidence_id),
                        availability_kind,
                        pack_name
                    ],
                )?;
            }
        }
    }
    Ok(())
}

fn read_snapshot(path: &Path) -> Result<Catalog, CatalogStoreError> {
    validate_snapshot_file(path)?;
    let connection = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let application_id: u32 =
        connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    let user_version: u32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if application_id != 0x5343_504b || user_version != 1 {
        return Err(CatalogStoreError::InvalidSnapshot(format!(
            "SQLite identity is application_id={application_id:#x}, user_version={user_version}"
        )));
    }
    let quick_check: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if quick_check != "ok" {
        return Err(CatalogStoreError::InvalidSnapshot(format!(
            "SQLite quick_check failed: {quick_check}"
        )));
    }
    let schema: Option<String> = connection
        .query_row(
            "SELECT value FROM metadata WHERE key = 'schema'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if schema.as_deref() != Some(SNAPSHOT_SCHEMA) {
        return Err(CatalogStoreError::InvalidSnapshot(format!(
            "schema is {schema:?}, expected {SNAPSHOT_SCHEMA:?}"
        )));
    }

    let mut catalog = Catalog::default();
    read_source_evidence(&connection, &mut catalog)?;
    let mut statement = connection.prepare(
        "SELECT song_id, tachi_source_id, artist, version, infinitas_status,
                tachi_primary_infinitas
         FROM songs ORDER BY song_id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, bool>(5)?,
        ))
    })?;
    for row in rows {
        let (song_id, tachi_source_id, artist, version, status, primary) = row?;
        let song_id = parse_song_id(&song_id)?;
        catalog.songs.insert(
            song_id,
            CatalogSong {
                song_id,
                tachi_source_id,
                title_variants: BTreeSet::new(),
                artist,
                version,
                charts: BTreeMap::new(),
                chart_assertions: BTreeMap::new(),
                infinitas_status: parse_infinitas_status(&status)?,
                source_bindings: BTreeMap::new(),
                binding_evidence: BTreeMap::new(),
                binding_attributes: BTreeMap::new(),
                tachi_primary_infinitas: primary,
            },
        );
    }
    drop(statement);

    read_title_variants(&connection, &mut catalog)?;
    read_charts(&connection, &mut catalog)?;
    read_chart_assertions(&connection, &mut catalog)?;
    read_source_bindings(&connection, &mut catalog)?;
    read_binding_evidence(&connection, &mut catalog)?;
    read_binding_attributes(&connection, &mut catalog)?;
    read_dqn_bindings(&connection, &mut catalog)?;
    catalog
        .validate()
        .map_err(CatalogStoreError::InvalidSnapshot)?;
    Ok(catalog)
}

fn read_source_evidence(
    connection: &Connection,
    catalog: &mut Catalog,
) -> Result<(), CatalogStoreError> {
    let mut statement = connection.prepare(
        "SELECT source_id, lineage_id, revision_strategy, revision, content_sha256, byte_size,
                record_count, parser_version, declared_scope, completeness, freshness,
                rights_and_provenance
         FROM source_evidence ORDER BY source_id, content_sha256",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, String>(9)?,
            row.get::<_, String>(10)?,
            row.get::<_, String>(11)?,
        ))
    })?;
    for row in rows {
        let (
            source_id,
            lineage_id,
            revision_strategy,
            revision,
            content_sha256,
            byte_size,
            record_count,
            parser_version,
            declared_scope,
            completeness,
            freshness,
            rights_and_provenance,
        ) = row?;
        let source_id = parse_source_id(&source_id)?;
        let mut authority_statement = connection.prepare(
            "SELECT field_name FROM source_authority
             WHERE source_id = ?1 AND revision = ?2 AND content_sha256 = ?3
             ORDER BY field_name",
        )?;
        let authority_rows = authority_statement.query_map(
            params![source_id_label(source_id), revision, content_sha256],
            |authority| authority.get(0),
        )?;
        let field_authority = authority_rows.collect::<Result<Vec<String>, _>>()?;
        let evidence_id = EvidenceId {
            source_id,
            revision: revision.clone(),
            content_sha256: content_sha256.clone(),
        };
        catalog.source_evidence.insert(
            evidence_id,
            SourceEvidence {
                source_id,
                lineage_id: parse_lineage_id(&lineage_id)?,
                revision_strategy: parse_revision_strategy(&revision_strategy)?,
                revision,
                content_sha256,
                byte_size: usize::try_from(byte_size).map_err(|_| {
                    CatalogStoreError::InvalidSnapshot("negative source byte_size".to_owned())
                })?,
                record_count: usize::try_from(record_count).map_err(|_| {
                    CatalogStoreError::InvalidSnapshot("negative source record_count".to_owned())
                })?,
                parser_version,
                declared_scope,
                completeness: parse_completeness(&completeness)?,
                field_authority,
                freshness,
                rights_and_provenance,
            },
        );
    }
    let mut latest_statement = connection.prepare(
        "SELECT source_id, revision, content_sha256 FROM latest_evidence ORDER BY source_id",
    )?;
    let latest_rows = latest_statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in latest_rows {
        let (source_id, revision, content_sha256) = row?;
        let source_id = parse_source_id(&source_id)?;
        catalog.latest_evidence.insert(
            source_id,
            EvidenceId {
                source_id,
                revision,
                content_sha256,
            },
        );
    }
    Ok(())
}

fn read_title_variants(
    connection: &Connection,
    catalog: &mut Catalog,
) -> Result<(), CatalogStoreError> {
    let mut statement = connection.prepare(
        "SELECT song_id, source_id, evidence_digest, variant_kind, value FROM title_variants
         ORDER BY song_id, source_id, evidence_digest, variant_kind, value",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    for row in rows {
        let (song_id, source_id, evidence_digest, variant_kind, value) = row?;
        let source_id = parse_source_id(&source_id)?;
        song_mut(catalog, &song_id)?
            .title_variants
            .insert(DisplayVariant {
                value,
                source_id,
                kind: parse_display_variant_kind(&variant_kind)?,
                evidence_id: parse_evidence_key(source_id, &evidence_digest)?,
            });
    }
    Ok(())
}

fn read_charts(connection: &Connection, catalog: &mut Catalog) -> Result<(), CatalogStoreError> {
    let mut statement = connection.prepare(
        "SELECT song_id, play_type, difficulty, level, notes FROM charts
         ORDER BY song_id, play_type, difficulty",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, u8>(3)?,
            row.get::<_, u32>(4)?,
        ))
    })?;
    for row in rows {
        let (song_id, play_type, difficulty, level, notes) = row?;
        let key = ChartKey {
            play_type: parse_play_type(&play_type)?,
            difficulty: parse_difficulty(&difficulty)?,
        };
        song_mut(catalog, &song_id)?
            .charts
            .insert(key, Chart { key, level, notes });
    }
    Ok(())
}

fn read_chart_assertions(
    connection: &Connection,
    catalog: &mut Catalog,
) -> Result<(), CatalogStoreError> {
    let mut statement = connection.prepare(
        "SELECT song_id, play_type, difficulty, source_id, evidence_digest, source_chart_id,
                is_primary
         FROM chart_assertions
         ORDER BY song_id, play_type, difficulty, source_id, evidence_digest, source_chart_id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, bool>(6)?,
        ))
    })?;
    for row in rows {
        let (song_id, play_type, difficulty, source_id, evidence_digest, source_chart_id, primary) =
            row?;
        let key = ChartKey {
            play_type: parse_play_type(&play_type)?,
            difficulty: parse_difficulty(&difficulty)?,
        };
        let source_id = parse_source_id(&source_id)?;
        let mut products = connection.prepare(
            "SELECT product_version FROM chart_assertion_products
             WHERE song_id = ?1 AND play_type = ?2 AND difficulty = ?3 AND source_id = ?4
               AND evidence_digest = ?5 AND source_chart_id = ?6
             ORDER BY product_version",
        )?;
        let product_rows = products.query_map(
            params![
                song_id,
                play_type,
                difficulty,
                source_id_label(source_id),
                evidence_digest,
                source_chart_id,
            ],
            |row| row.get(0),
        )?;
        let product_versions = product_rows.collect::<Result<BTreeSet<String>, _>>()?;
        song_mut(catalog, &song_id)?
            .chart_assertions
            .entry(key)
            .or_default()
            .insert(ChartAssertion {
                source_chart_id,
                product_versions,
                primary,
                evidence_id: parse_evidence_key(source_id, &evidence_digest)?,
            });
    }
    Ok(())
}

fn read_source_bindings(
    connection: &Connection,
    catalog: &mut Catalog,
) -> Result<(), CatalogStoreError> {
    let mut statement = connection.prepare(
        "SELECT song_id, source_id, source_key FROM source_bindings
         ORDER BY song_id, source_id, source_key",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (song_id, source_id, source_key) = row?;
        song_mut(catalog, &song_id)?
            .source_bindings
            .entry(parse_source_id(&source_id)?)
            .or_default()
            .insert(source_key);
    }
    Ok(())
}

fn read_dqn_bindings(
    connection: &Connection,
    catalog: &mut Catalog,
) -> Result<(), CatalogStoreError> {
    let mut statement = connection
        .prepare("SELECT title, artist, song_id FROM dqn_bindings ORDER BY title, artist")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (title, artist, song_id) = row?;
        let mut evidence_statement = connection.prepare(
            "SELECT evidence_digest, availability_kind, pack_name FROM dqn_binding_evidence
             WHERE title = ?1 AND artist = ?2
             ORDER BY evidence_digest, availability_kind, pack_name",
        )?;
        let evidence_rows = evidence_statement.query_map(params![title, artist], |evidence| {
            Ok((
                evidence.get::<_, String>(0)?,
                evidence.get::<_, String>(1)?,
                evidence.get::<_, String>(2)?,
            ))
        })?;
        let mut evidence_packs = BTreeMap::new();
        for evidence in evidence_rows {
            let (content_sha256, availability_kind, pack_name) = evidence?;
            let pack = match (availability_kind.as_str(), pack_name.as_str()) {
                ("base", "") => None,
                ("pack", pack_name) if !pack_name.is_empty() => Some(pack_name.to_owned()),
                _ => {
                    return Err(CatalogStoreError::InvalidSnapshot(
                        "invalid dqn availability evidence".to_owned(),
                    ));
                }
            };
            evidence_packs
                .entry(parse_evidence_key(SourceId::DqnIidxapi, &content_sha256)?)
                .or_insert_with(BTreeSet::new)
                .insert(pack);
        }
        catalog.dqn_bindings.insert(
            ExactTitleArtist { title, artist },
            DqnBinding {
                song_id: parse_song_id(&song_id)?,
                evidence_packs,
            },
        );
    }
    Ok(())
}

fn read_binding_evidence(
    connection: &Connection,
    catalog: &mut Catalog,
) -> Result<(), CatalogStoreError> {
    let mut statement = connection.prepare(
        "SELECT song_id, source_id, source_key, evidence_digest FROM binding_evidence
         ORDER BY song_id, source_id, source_key, evidence_digest",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (song_id, source_id, source_key, evidence_digest) = row?;
        let source_id = parse_source_id(&source_id)?;
        song_mut(catalog, &song_id)?
            .binding_evidence
            .entry((source_id, source_key))
            .or_default()
            .insert(parse_evidence_key(source_id, &evidence_digest)?);
    }
    Ok(())
}

fn read_binding_attributes(
    connection: &Connection,
    catalog: &mut Catalog,
) -> Result<(), CatalogStoreError> {
    let mut statement = connection.prepare(
        "SELECT song_id, source_id, source_key, evidence_digest, attribute_key, attribute_value
         FROM binding_attributes
         ORDER BY song_id, source_id, source_key, evidence_digest, attribute_key",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    for row in rows {
        let (song_id, source_id, source_key, evidence_digest, key, value) = row?;
        let source_id = parse_source_id(&source_id)?;
        song_mut(catalog, &song_id)?
            .binding_attributes
            .entry((
                source_id,
                source_key,
                parse_evidence_key(source_id, &evidence_digest)?,
            ))
            .or_default()
            .insert(key, value);
    }
    Ok(())
}

fn song_mut<'a>(
    catalog: &'a mut Catalog,
    value: &str,
) -> Result<&'a mut CatalogSong, CatalogStoreError> {
    let song_id = parse_song_id(value)?;
    catalog.songs.get_mut(&song_id).ok_or_else(|| {
        CatalogStoreError::InvalidSnapshot(format!("row references unknown song {value:?}"))
    })
}

fn parse_song_id(value: &str) -> Result<ScorepeekSongId, CatalogStoreError> {
    Uuid::parse_str(value)
        .map(ScorepeekSongId::from_uuid)
        .map_err(|error| CatalogStoreError::InvalidSnapshot(format!("invalid song UUID: {error}")))
}

fn validate_snapshot_file(path: &Path) -> io::Result<()> {
    let metadata = path.symlink_metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_SNAPSHOT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "catalog snapshot is not a bounded regular file",
        ));
    }
    Ok(())
}

fn digest_file(path: &Path) -> io::Result<String> {
    validate_snapshot_file(path)?;
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > MAX_SNAPSHOT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "catalog snapshot exceeds the size limit while reading",
            ));
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(encoded)
}

fn evidence_key(evidence_id: &EvidenceId) -> String {
    format!("{}:{}", evidence_id.revision, evidence_id.content_sha256)
}

fn parse_evidence_key(source_id: SourceId, value: &str) -> Result<EvidenceId, CatalogStoreError> {
    let (revision, content_sha256) = value
        .split_once(':')
        .ok_or_else(|| CatalogStoreError::InvalidSnapshot("malformed evidence key".to_owned()))?;
    let revision_length = match SourcePolicy::for_id(source_id).revision_strategy {
        RevisionStrategy::GitCommit => 40,
        RevisionStrategy::ContentSha256 => 64,
    };
    if !is_lower_hex(revision, revision_length) || !is_lower_hex(content_sha256, 64) {
        return Err(CatalogStoreError::InvalidSnapshot(
            "malformed evidence key".to_owned(),
        ));
    }
    Ok(EvidenceId {
        source_id,
        revision: revision.to_owned(),
        content_sha256: content_sha256.to_owned(),
    })
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn validate_digest(value: &str) -> Result<(), CatalogStoreError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CatalogStoreError::InvalidManifest(
            "catalog_digest must be 64 hexadecimal characters".to_owned(),
        ));
    }
    Ok(())
}

const fn source_id_label(value: SourceId) -> &'static str {
    match value {
        SourceId::Tachi => "tachi",
        SourceId::Textage => "textage",
        SourceId::DqnIidxapi => "dqn_iidxapi",
    }
}

fn parse_source_id(value: &str) -> Result<SourceId, CatalogStoreError> {
    match value {
        "tachi" => Ok(SourceId::Tachi),
        "textage" => Ok(SourceId::Textage),
        "dqn_iidxapi" => Ok(SourceId::DqnIidxapi),
        _ => Err(invalid_enum("source_id", value)),
    }
}

const fn lineage_id_label(value: LineageId) -> &'static str {
    match value {
        LineageId::GameMdb => "game_mdb",
        LineageId::Textage => "textage",
        LineageId::OfficialInfinitasHtml => "official_infinitas_html",
    }
}

fn parse_lineage_id(value: &str) -> Result<LineageId, CatalogStoreError> {
    match value {
        "game_mdb" => Ok(LineageId::GameMdb),
        "textage" => Ok(LineageId::Textage),
        "official_infinitas_html" => Ok(LineageId::OfficialInfinitasHtml),
        _ => Err(invalid_enum("lineage_id", value)),
    }
}

const fn revision_strategy_label(value: RevisionStrategy) -> &'static str {
    match value {
        RevisionStrategy::GitCommit => "git_commit",
        RevisionStrategy::ContentSha256 => "content_sha256",
    }
}

fn parse_revision_strategy(value: &str) -> Result<RevisionStrategy, CatalogStoreError> {
    match value {
        "git_commit" => Ok(RevisionStrategy::GitCommit),
        "content_sha256" => Ok(RevisionStrategy::ContentSha256),
        _ => Err(invalid_enum("revision_strategy", value)),
    }
}

const fn completeness_label(value: Completeness) -> &'static str {
    match value {
        Completeness::NonExhaustive => "non_exhaustive",
    }
}

fn parse_completeness(value: &str) -> Result<Completeness, CatalogStoreError> {
    match value {
        "non_exhaustive" => Ok(Completeness::NonExhaustive),
        _ => Err(invalid_enum("completeness", value)),
    }
}

const fn play_type_label(value: PlayType) -> &'static str {
    match value {
        PlayType::Single => "single",
        PlayType::Double => "double",
    }
}

fn parse_play_type(value: &str) -> Result<PlayType, CatalogStoreError> {
    match value {
        "single" => Ok(PlayType::Single),
        "double" => Ok(PlayType::Double),
        _ => Err(invalid_enum("play_type", value)),
    }
}

const fn difficulty_label(value: Difficulty) -> &'static str {
    match value {
        Difficulty::Beginner => "beginner",
        Difficulty::Normal => "normal",
        Difficulty::Hyper => "hyper",
        Difficulty::Another => "another",
        Difficulty::Leggendaria => "leggendaria",
    }
}

fn parse_difficulty(value: &str) -> Result<Difficulty, CatalogStoreError> {
    match value {
        "beginner" => Ok(Difficulty::Beginner),
        "normal" => Ok(Difficulty::Normal),
        "hyper" => Ok(Difficulty::Hyper),
        "another" => Ok(Difficulty::Another),
        "leggendaria" => Ok(Difficulty::Leggendaria),
        _ => Err(invalid_enum("difficulty", value)),
    }
}

const fn display_variant_kind_label(value: DisplayVariantKind) -> &'static str {
    match value {
        DisplayVariantKind::InGameDisplay => "in_game_display",
        DisplayVariantKind::OfficialDisplay => "official_display",
        DisplayVariantKind::EamusementCsv => "eamusement_csv",
        DisplayVariantKind::AlternateDisplay => "alternate_display",
        DisplayVariantKind::SearchTerm => "search_term",
    }
}

fn parse_display_variant_kind(value: &str) -> Result<DisplayVariantKind, CatalogStoreError> {
    match value {
        "in_game_display" => Ok(DisplayVariantKind::InGameDisplay),
        "official_display" => Ok(DisplayVariantKind::OfficialDisplay),
        "eamusement_csv" => Ok(DisplayVariantKind::EamusementCsv),
        "alternate_display" => Ok(DisplayVariantKind::AlternateDisplay),
        "search_term" => Ok(DisplayVariantKind::SearchTerm),
        _ => Err(invalid_enum("display variant kind", value)),
    }
}

const fn infinitas_status_label(value: InfinitasStatus) -> &'static str {
    match value {
        InfinitasStatus::ConfirmedPresent => "confirmed_present",
        InfinitasStatus::Unknown => "unknown",
        InfinitasStatus::Conflicted => "conflicted",
    }
}

fn parse_infinitas_status(value: &str) -> Result<InfinitasStatus, CatalogStoreError> {
    match value {
        "confirmed_present" => Ok(InfinitasStatus::ConfirmedPresent),
        "unknown" => Ok(InfinitasStatus::Unknown),
        "conflicted" => Ok(InfinitasStatus::Conflicted),
        _ => Err(invalid_enum("infinitas_status", value)),
    }
}

fn invalid_enum(field: &str, value: &str) -> CatalogStoreError {
    CatalogStoreError::InvalidSnapshot(format!("unknown {field} value {value:?}"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::os::unix::fs::symlink;

    use rusqlite::Connection;
    use serde_json::json;
    use tempfile::TempDir;

    use super::{
        CatalogStore, CatalogStoreError, MANIFEST_STAGING_PREFIX, MAX_MANIFEST_BYTES,
        MAX_SNAPSHOT_BYTES, MAX_SNAPSHOT_GENERATIONS, MAX_SNAPSHOT_STORAGE_BYTES, PublishPoint,
        SNAPSHOT_FILE, SNAPSHOT_STAGING_PREFIX, digest_file, ensure_snapshot_capacity,
    };

    #[test]
    fn managed_catalog_directory_rejects_symlinks() {
        let root = TempDir::new().unwrap();
        let target = root.path().join("target");
        let alias = root.path().join("alias");
        fs::create_dir(&target).unwrap();
        symlink(&target, &alias).unwrap();
        assert!(super::create_private_directory(&alias).is_err());
    }
    use crate::catalog::{Catalog, FederationInput, SourceRevision, TachiFixtureAdapter};

    const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn active_manifest_rejects_oversized_files_and_symlinks_before_reading() {
        let root = TempDir::new().unwrap();
        let store = CatalogStore::new(root.path());
        drop(store.begin_update().unwrap());
        let manifest = store.active_manifest_path();
        fs::File::create(&manifest)
            .unwrap()
            .set_len((MAX_MANIFEST_BYTES + 1) as u64)
            .unwrap();
        let oversized = store.load_active().unwrap_err();
        assert!(matches!(oversized, CatalogStoreError::InvalidManifest(_)));

        fs::remove_file(&manifest).unwrap();
        let target = root.path().join("manifest-target.json");
        fs::write(&target, b"{}").unwrap();
        symlink(&target, &manifest).unwrap();
        let linked = store.load_active().unwrap_err();
        assert!(matches!(linked, CatalogStoreError::InvalidManifest(_)));
    }

    #[test]
    fn snapshot_generation_limit_rejects_new_content() {
        let root = TempDir::new().unwrap();
        let content = root.path().join("content");
        fs::create_dir(&content).unwrap();
        for index in 0..MAX_SNAPSHOT_GENERATIONS {
            let generation = content.join(format!("{index:064x}"));
            fs::create_dir(&generation).unwrap();
            fs::write(generation.join(SNAPSHOT_FILE), []).unwrap();
        }

        let error = ensure_snapshot_capacity(&content, &content.join("current"), 1).unwrap_err();
        assert!(matches!(error, CatalogStoreError::CapacityExceeded));
    }

    #[test]
    fn snapshot_storage_limit_rejects_new_content_at_exact_capacity() {
        let root = TempDir::new().unwrap();
        let content = root.path().join("content");
        fs::create_dir(&content).unwrap();
        let generation_bytes = MAX_SNAPSHOT_STORAGE_BYTES / 8;
        assert!(generation_bytes <= MAX_SNAPSHOT_BYTES);
        for index in 0..8 {
            let generation = content.join(format!("{index:064x}"));
            fs::create_dir(&generation).unwrap();
            fs::File::create(generation.join(SNAPSHOT_FILE))
                .unwrap()
                .set_len(generation_bytes)
                .unwrap();
        }

        let error = ensure_snapshot_capacity(&content, &content.join("current"), 1).unwrap_err();
        assert!(matches!(error, CatalogStoreError::CapacityExceeded));
    }

    #[test]
    fn bounded_digest_rejects_oversized_existing_snapshot_without_reading_it() {
        let root = TempDir::new().unwrap();
        let snapshot = root.path().join(SNAPSHOT_FILE);
        fs::File::create(&snapshot)
            .unwrap()
            .set_len(MAX_SNAPSHOT_BYTES + 1)
            .unwrap();

        let error = digest_file(&snapshot).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn begin_update_recovers_only_owned_staging_entries() {
        let root = TempDir::new().unwrap();
        let store = CatalogStore::new(root.path());
        drop(store.begin_update().unwrap());
        let stale_snapshot = root
            .path()
            .join("content")
            .join(format!("{SNAPSHOT_STAGING_PREFIX}interrupted"));
        fs::create_dir(&stale_snapshot).unwrap();
        fs::write(stale_snapshot.join(SNAPSHOT_FILE), b"partial").unwrap();
        let stale_manifest = root
            .path()
            .join("manifests")
            .join(format!("{MANIFEST_STAGING_PREFIX}interrupted"));
        fs::write(&stale_manifest, b"partial").unwrap();

        drop(store.begin_update().unwrap());

        assert!(!stale_snapshot.exists());
        assert!(!stale_manifest.exists());
    }

    #[test]
    fn existing_snapshot_remains_publishable_at_generation_capacity() {
        let root = TempDir::new().unwrap();
        let store = CatalogStore::new(root.path());
        let catalog = synthetic_catalog("ALPHA");
        let active = store.begin_update().unwrap().publish(&catalog).unwrap();
        let content = root.path().join("content");
        let mut created = 0;
        let mut index = 0_u64;
        while created < MAX_SNAPSHOT_GENERATIONS - 1 {
            let generation = content.join(format!("{index:064x}"));
            index += 1;
            if generation.exists() {
                continue;
            }
            fs::create_dir(&generation).unwrap();
            fs::write(generation.join(SNAPSHOT_FILE), []).unwrap();
            created += 1;
        }

        let republished = store.begin_update().unwrap().publish(&catalog).unwrap();
        assert_eq!(republished.digest, active.digest);
        let error = store
            .begin_update()
            .unwrap()
            .publish(&synthetic_catalog("BETA"))
            .unwrap_err();
        assert!(matches!(error, CatalogStoreError::CapacityExceeded));
        assert_eq!(store.load_active().unwrap().unwrap(), active);
    }

    #[test]
    fn snapshot_round_trip_is_content_deterministic() {
        let catalog = synthetic_catalog("ALPHA");
        let first_root = TempDir::new().unwrap();
        let second_root = TempDir::new().unwrap();
        let first = CatalogStore::new(first_root.path())
            .begin_update()
            .unwrap()
            .publish(&catalog)
            .unwrap();
        let second = CatalogStore::new(second_root.path())
            .begin_update()
            .unwrap()
            .publish(&catalog)
            .unwrap();

        assert_eq!(first.digest, second.digest);
        assert_eq!(
            CatalogStore::new(first_root.path()).load_active().unwrap(),
            Some(first)
        );
    }

    #[test]
    fn snapshot_round_trip_preserves_distinct_revisions_for_identical_bytes() {
        let fixture = synthetic_tachi_fixture("ALPHA");
        let bytes = serde_json::to_vec(&fixture).unwrap();
        let first =
            TachiFixtureAdapter::parse(&bytes, SourceRevision::git_commit(REVISION).unwrap())
                .unwrap();
        let second = TachiFixtureAdapter::parse(
            &bytes,
            SourceRevision::git_commit("1123456789abcdef0123456789abcdef01234567").unwrap(),
        )
        .unwrap();
        let catalog = Catalog::default()
            .federate(FederationInput {
                tachi: Some(first),
                ..FederationInput::default()
            })
            .catalog
            .federate(FederationInput {
                tachi: Some(second),
                ..FederationInput::default()
            })
            .catalog;
        let root = TempDir::new().unwrap();
        let active = CatalogStore::new(root.path())
            .begin_update()
            .unwrap()
            .publish(&catalog)
            .unwrap();
        let loaded = CatalogStore::new(root.path())
            .load_active()
            .unwrap()
            .unwrap();
        assert_eq!(loaded.digest, active.digest);
        assert_eq!(loaded.catalog, catalog);
        assert_eq!(loaded.catalog.source_evidence.len(), 2);
    }

    #[test]
    fn every_publish_failure_exposes_old_or_complete_new_snapshot() {
        let points = [
            PublishPoint::SnapshotFileSynced,
            PublishPoint::StagingDirectorySynced,
            PublishPoint::SnapshotRenamed,
            PublishPoint::ContentParentSynced,
            PublishPoint::ManifestFileSynced,
            PublishPoint::ManifestRenamed,
            PublishPoint::ManifestParentSynced,
        ];
        for point in points {
            let root = TempDir::new().unwrap();
            let store = CatalogStore::new(root.path());
            let old = synthetic_catalog("OLD");
            let new = synthetic_catalog("NEW");
            store.begin_update().unwrap().publish(&old).unwrap();

            let result = store
                .begin_update()
                .unwrap()
                .publish_with(&new, |observed| {
                    if observed == point {
                        Err(io::Error::other("injected publish failure"))
                    } else {
                        Ok(())
                    }
                });
            assert!(result.is_err());
            let active = store.load_active().unwrap().unwrap().catalog;
            if matches!(
                point,
                PublishPoint::ManifestRenamed | PublishPoint::ManifestParentSynced
            ) {
                assert_eq!(active, new, "failure at {point:?}");
            } else {
                assert_eq!(active, old, "failure at {point:?}");
            }
        }
    }

    #[test]
    fn publish_rejects_a_changed_base_manifest() {
        let root = TempDir::new().unwrap();
        let store = CatalogStore::new(root.path());
        store
            .begin_update()
            .unwrap()
            .publish(&synthetic_catalog("OLD"))
            .unwrap();
        let update = store.begin_update().unwrap();
        let replacement = json!({
            "schema": "scorepeek-active-catalog-v1",
            "catalog_digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "snapshot_path": "content/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/catalog.sqlite3"
        });
        let replacement = serde_json::to_vec(&replacement).unwrap();
        let error = update
            .publish_with(&synthetic_catalog("NEW"), |point| {
                if point == PublishPoint::ManifestFileSynced {
                    fs::write(root.path().join("manifests/active.json"), &replacement)?;
                }
                Ok(())
            })
            .unwrap_err();
        assert!(matches!(
            error,
            super::CatalogStoreError::BaseDigestChanged { .. }
        ));
    }

    #[test]
    fn self_consistent_digest_does_not_bypass_semantic_validation() {
        let root = TempDir::new().unwrap();
        let store = CatalogStore::new(root.path());
        let active = store
            .begin_update()
            .unwrap()
            .publish(&synthetic_catalog("ALPHA"))
            .unwrap();
        let old_directory = root.path().join("content").join(&active.digest);
        let snapshot = old_directory.join("catalog.sqlite3");
        let connection = Connection::open(&snapshot).unwrap();
        connection
            .execute(
                "INSERT INTO dqn_bindings VALUES ('ROGUE', 'ROGUE', ?1)",
                ["00000000-0000-0000-0000-000000000000"],
            )
            .unwrap();
        connection.close().unwrap();
        let digest = super::digest_file(&snapshot).unwrap();
        let new_directory = root.path().join("content").join(&digest);
        fs::rename(old_directory, new_directory).unwrap();
        let manifest = super::ActiveManifest {
            schema: super::MANIFEST_SCHEMA.to_owned(),
            catalog_digest: digest.clone(),
            snapshot_path: format!("content/{digest}/catalog.sqlite3"),
        };
        fs::write(
            root.path().join("manifests/active.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let error = store.load_active().unwrap_err();
        assert!(matches!(
            error,
            super::CatalogStoreError::InvalidSnapshot(_)
        ));
    }

    #[test]
    fn self_consistent_policy_tamper_is_rejected() {
        let root = TempDir::new().unwrap();
        let store = CatalogStore::new(root.path());
        let active = store
            .begin_update()
            .unwrap()
            .publish(&synthetic_catalog("ALPHA"))
            .unwrap();
        let old_directory = root.path().join("content").join(&active.digest);
        let snapshot = old_directory.join("catalog.sqlite3");
        let connection = Connection::open(&snapshot).unwrap();
        connection
            .execute(
                "DELETE FROM source_authority WHERE source_id = 'tachi' AND field_name = 'title_kind'",
                [],
            )
            .unwrap();
        connection.close().unwrap();
        let digest = super::digest_file(&snapshot).unwrap();
        let new_directory = root.path().join("content").join(&digest);
        fs::rename(old_directory, new_directory).unwrap();
        let manifest = super::ActiveManifest {
            schema: super::MANIFEST_SCHEMA.to_owned(),
            catalog_digest: digest.clone(),
            snapshot_path: format!("content/{digest}/catalog.sqlite3"),
        };
        fs::write(
            root.path().join("manifests/active.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let error = store.load_active().unwrap_err();
        assert!(matches!(
            error,
            super::CatalogStoreError::InvalidSnapshot(_)
        ));
    }

    #[test]
    fn self_consistent_availability_tamper_is_rejected() {
        let root = TempDir::new().unwrap();
        let store = CatalogStore::new(root.path());
        let active = store
            .begin_update()
            .unwrap()
            .publish(&synthetic_catalog("ALPHA"))
            .unwrap();
        let old_directory = root.path().join("content").join(&active.digest);
        let snapshot = old_directory.join("catalog.sqlite3");
        let connection = Connection::open(&snapshot).unwrap();
        connection
            .execute(
                "UPDATE songs SET tachi_primary_infinitas = 1,
                 infinitas_status = 'confirmed_present'",
                [],
            )
            .unwrap();
        connection.close().unwrap();
        let digest = super::digest_file(&snapshot).unwrap();
        let new_directory = root.path().join("content").join(&digest);
        fs::rename(old_directory, new_directory).unwrap();
        let manifest = super::ActiveManifest {
            schema: super::MANIFEST_SCHEMA.to_owned(),
            catalog_digest: digest.clone(),
            snapshot_path: format!("content/{digest}/catalog.sqlite3"),
        };
        fs::write(
            root.path().join("manifests/active.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let error = store.load_active().unwrap_err();
        assert!(matches!(
            error,
            super::CatalogStoreError::InvalidSnapshot(_)
        ));
    }

    #[test]
    fn self_consistent_conflicted_status_tamper_is_rejected() {
        let root = TempDir::new().unwrap();
        let store = CatalogStore::new(root.path());
        let active = store
            .begin_update()
            .unwrap()
            .publish(&synthetic_catalog("ALPHA"))
            .unwrap();
        let old_directory = root.path().join("content").join(&active.digest);
        let snapshot = old_directory.join("catalog.sqlite3");
        let connection = Connection::open(&snapshot).unwrap();
        connection
            .execute("UPDATE songs SET infinitas_status = 'conflicted'", [])
            .unwrap();
        connection.close().unwrap();
        let digest = super::digest_file(&snapshot).unwrap();
        let new_directory = root.path().join("content").join(&digest);
        fs::rename(old_directory, new_directory).unwrap();
        let manifest = super::ActiveManifest {
            schema: super::MANIFEST_SCHEMA.to_owned(),
            catalog_digest: digest.clone(),
            snapshot_path: format!("content/{digest}/catalog.sqlite3"),
        };
        fs::write(
            root.path().join("manifests/active.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let error = store.load_active().unwrap_err();
        assert!(matches!(
            error,
            super::CatalogStoreError::InvalidSnapshot(_)
        ));
    }

    fn synthetic_catalog(title: &str) -> Catalog {
        let fixture = synthetic_tachi_fixture(title);
        let snapshot = TachiFixtureAdapter::parse(
            &serde_json::to_vec(&fixture).unwrap(),
            SourceRevision::git_commit(REVISION).unwrap(),
        )
        .unwrap();
        Catalog::default()
            .federate(FederationInput {
                tachi: Some(snapshot),
                ..FederationInput::default()
            })
            .catalog
    }

    fn synthetic_tachi_fixture(title: &str) -> serde_json::Value {
        json!({
            "schema": "scorepeek-tachi-fixture-v1",
            "records": [{
                "source_song_id": "anchor-1",
                "title": title,
                "title_kind": "in_game_display",
                "artist": "SYNTHETIC ARTIST",
                "version": "SYNTHETIC VERSION",
                "charts": [
                    { "play_type": "single", "difficulty": "normal", "level": 4, "notes": 400,
                      "source_chart_id": "spn", "product_versions": ["synthetic-v1"], "primary": true },
                    { "play_type": "single", "difficulty": "hyper", "level": 8, "notes": 800,
                      "source_chart_id": "sph", "product_versions": ["synthetic-v1"], "primary": true }
                ],
                "primary_infinitas": false
            }]
        })
    }
}
