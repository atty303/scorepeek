//! Publisher-side `SQLite` snapshot schema and writer.

use rusqlite::{Connection, Transaction, params};
use scorepeek_core::catalog::{
    Catalog, CatalogSong, Completeness, Difficulty, DisplayVariantKind, EvidenceId,
    InfinitasStatus, LineageId, PlayType, RevisionStrategy, SourceId,
};
use std::error::Error;
use std::fmt;
use std::path::Path;

const SNAPSHOT_SCHEMA: &str = "scorepeek-catalog-snapshot-v1";

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

#[derive(Debug)]
pub enum SnapshotError {
    Invalid(String),
    Sqlite(rusqlite::Error),
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(detail) => write!(formatter, "invalid catalog snapshot: {detail}"),
            Self::Sqlite(error) => write!(formatter, "catalog SQLite operation failed: {error}"),
        }
    }
}

impl Error for SnapshotError {}

impl From<rusqlite::Error> for SnapshotError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

/// Writes one complete publisher snapshot using the versioned catalog schema.
/// # Errors
/// Returns domain validation, `SQLite`, or I/O errors.
pub fn write_snapshot(path: &Path, catalog: &Catalog) -> Result<(), SnapshotError> {
    catalog.validate().map_err(SnapshotError::Invalid)?;
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

fn evidence_key(evidence_id: &EvidenceId) -> String {
    format!("{}:{}", evidence_id.revision, evidence_id.content_sha256)
}

const fn source_id_label(value: SourceId) -> &'static str {
    match value {
        SourceId::Tachi => "tachi",
        SourceId::Textage => "textage",
        SourceId::DqnIidxapi => "dqn_iidxapi",
    }
}

const fn lineage_id_label(value: LineageId) -> &'static str {
    match value {
        LineageId::GameMdb => "game_mdb",
        LineageId::Textage => "textage",
        LineageId::OfficialInfinitasHtml => "official_infinitas_html",
    }
}

const fn revision_strategy_label(value: RevisionStrategy) -> &'static str {
    match value {
        RevisionStrategy::GitCommit => "git_commit",
        RevisionStrategy::ContentSha256 => "content_sha256",
    }
}

const fn completeness_label(value: Completeness) -> &'static str {
    match value {
        Completeness::NonExhaustive => "non_exhaustive",
    }
}

const fn play_type_label(value: PlayType) -> &'static str {
    match value {
        PlayType::Single => "single",
        PlayType::Double => "double",
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

const fn display_variant_kind_label(value: DisplayVariantKind) -> &'static str {
    match value {
        DisplayVariantKind::InGameDisplay => "in_game_display",
        DisplayVariantKind::OfficialDisplay => "official_display",
        DisplayVariantKind::EamusementCsv => "eamusement_csv",
        DisplayVariantKind::AlternateDisplay => "alternate_display",
        DisplayVariantKind::SearchTerm => "search_term",
    }
}

const fn infinitas_status_label(value: InfinitasStatus) -> &'static str {
    match value {
        InfinitasStatus::ConfirmedPresent => "confirmed_present",
        InfinitasStatus::Unknown => "unknown",
        InfinitasStatus::Conflicted => "conflicted",
    }
}
