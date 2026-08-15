use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::{
    AdapterError, Catalog, CatalogStore, DqnLiveAdapter, FederationInput, InfinitasStatus,
    QuarantineReason, SourceRevision, TachiFixtureAdapter, TextageFixtureAdapter,
};

const GIT_REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

#[test]
fn fixture_adapter_rejects_unknown_fields_and_duplicate_ids() {
    let unknown = json!({
        "schema": "scorepeek-tachi-fixture-v1",
        "records": [],
        "unexpected": true
    });
    let error = TachiFixtureAdapter::parse(&serde_json::to_vec(&unknown).unwrap(), git_revision())
        .unwrap_err();
    assert!(error.to_string().contains("unknown field"));

    let record = tachi_record("anchor-1", "ALPHA", "ARTIST A", "V1", false);
    let duplicate = fixture("scorepeek-tachi-fixture-v1", &[record.clone(), record]);
    let error =
        TachiFixtureAdapter::parse(&serde_json::to_vec(&duplicate).unwrap(), git_revision())
            .unwrap_err();
    assert!(error.to_string().contains("duplicate source ID"));
}

#[test]
fn dqn_live_adapter_rejects_transport_truncation_and_schema_drift() {
    let bytes = serde_json::to_vec(&vec![dqn_record("ALPHA", "ARTIST A")]).unwrap();
    let truncated = &bytes[..bytes.len() - 1];
    let error = DqnLiveAdapter::parse(truncated, content_revision(&bytes)).unwrap_err();
    assert!(matches!(error, AdapterError::InvalidJson(_)));

    let drifted = serde_json::to_vec(&json!([{
        "title": "ALPHA",
        "artist": "ARTIST A",
        "packName": null,
        "introducedBySchemaDrift": true
    }]))
    .unwrap();
    let error = DqnLiveAdapter::parse(&drifted, content_revision(&drifted)).unwrap_err();
    assert!(error.to_string().contains("unknown field"));

    let missing = serde_json::to_vec(&json!([{
        "title": "ALPHA",
        "artist": "ARTIST A"
    }]))
    .unwrap();
    let error = DqnLiveAdapter::parse(&missing, content_revision(&missing)).unwrap_err();
    assert!(error.to_string().contains("missing field `packName`"));
}

#[test]
fn dqn_live_adapter_enforces_revision_strategy_and_content_pin() {
    let bytes = serde_json::to_vec(&vec![dqn_record("ALPHA", "ARTIST A")]).unwrap();
    let error = DqnLiveAdapter::parse(&bytes, git_revision()).unwrap_err();
    assert!(error.to_string().contains("revision strategy"));

    let error = DqnLiveAdapter::parse(
        &bytes,
        SourceRevision::content_sha256(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("expected pinned digest"));
}

#[test]
fn dqn_live_adapter_accepts_nullable_pack_name_and_rejects_duplicate_rows() {
    let records = vec![
        dqn_base_record("ALPHA", "ARTIST A"),
        dqn_record("BETA", "ARTIST B"),
    ];
    let bytes = serde_json::to_vec(&records).unwrap();
    let snapshot = DqnLiveAdapter::parse(&bytes, content_revision(&bytes)).unwrap();
    assert_eq!(snapshot.evidence().record_count(), 2);
    assert_eq!(
        snapshot.policy().parser_version,
        "scorepeek-dqn-live-json-parser-v1"
    );

    let duplicate = serde_json::to_vec(&vec![records[0].clone(), records[0].clone()]).unwrap();
    let error = DqnLiveAdapter::parse(&duplicate, content_revision(&duplicate)).unwrap_err();
    assert!(matches!(error, AdapterError::DuplicateRecord(_)));
}

#[test]
fn federation_is_deterministic_and_idempotent_for_record_order() {
    let first = tachi_record("anchor-1", "ALPHA", "ARTIST A", "V1", false);
    let second = tachi_record("anchor-2", "BETA", "ARTIST B", "V2", true);
    let forward = tachi_snapshot(&[first.clone(), second.clone()]);
    let reverse = tachi_snapshot(&[second, first]);

    let forward_output = Catalog::default().federate(FederationInput {
        tachi: Some(forward),
        ..FederationInput::default()
    });
    let reverse_output = Catalog::default().federate(FederationInput {
        tachi: Some(reverse),
        ..FederationInput::default()
    });
    assert_eq!(
        forward_output.catalog.songs.keys().collect::<Vec<_>>(),
        reverse_output.catalog.songs.keys().collect::<Vec<_>>()
    );
    for (song_id, forward_song) in &forward_output.catalog.songs {
        let reverse_song = &reverse_output.catalog.songs[song_id];
        assert_eq!(forward_song.tachi_source_id, reverse_song.tachi_source_id);
        assert_eq!(forward_song.artist, reverse_song.artist);
        assert_eq!(forward_song.version, reverse_song.version);
        assert_eq!(forward_song.charts, reverse_song.charts);
        assert_eq!(forward_song.infinitas_status, reverse_song.infinitas_status);
        assert_eq!(
            forward_song
                .title_variants
                .iter()
                .map(|variant| &variant.value)
                .collect::<BTreeSet<_>>(),
            reverse_song
                .title_variants
                .iter()
                .map(|variant| &variant.value)
                .collect::<BTreeSet<_>>()
        );
    }
    assert_eq!(forward_output.quarantine, reverse_output.quarantine);

    let rebuilt = forward_output.catalog.federate(FederationInput {
        tachi: Some(tachi_snapshot(&[
            tachi_record("anchor-1", "ALPHA", "ARTIST A", "V1", false),
            tachi_record("anchor-2", "BETA", "ARTIST B", "V2", true),
        ])),
        ..FederationInput::default()
    });
    assert_eq!(rebuilt, forward_output);
}

#[test]
fn textage_requires_exact_identity_and_multiple_matching_charts() {
    let base = catalog_with_tachi(&[tachi_record("anchor-1", "ALPHA", "ARTIST A", "V1", false)]);
    let fuzzy = textage_record("textage-1", "ALPHA!", "ARTIST A", "V1");
    let output = base.federate(FederationInput {
        textage: Some(textage_snapshot(&[fuzzy])),
        ..FederationInput::default()
    });
    assert_eq!(
        output.quarantine[0].reason,
        QuarantineReason::ProvisionalWithoutTachiAnchor
    );
    assert!(
        output
            .catalog
            .songs
            .values()
            .all(|song| !song.source_bindings.contains_key(&super::SourceId::Textage))
    );

    let exact = textage_record("textage-1", "ALPHA", "ARTIST A", "V1");
    let output = base.federate(FederationInput {
        textage: Some(textage_snapshot(&[exact])),
        ..FederationInput::default()
    });
    assert!(output.quarantine.is_empty());
    assert!(
        output
            .catalog
            .songs
            .values()
            .next()
            .unwrap()
            .source_bindings[&super::SourceId::Textage]
            .contains("textage-1")
    );
}

#[test]
fn textage_cannot_use_its_own_lineage_to_create_a_new_identity_edge() {
    let base = catalog_with_tachi(&[tachi_record(
        "anchor-1", "ORIGINAL", "ARTIST A", "V1", false,
    )]);
    let bound = base.federate(FederationInput {
        textage: Some(textage_snapshot(&[textage_record(
            "textage-1",
            "ORIGINAL",
            "ARTIST A",
            "V1",
        )])),
        ..FederationInput::default()
    });
    let renamed = bound.catalog.federate(FederationInput {
        textage: Some(textage_snapshot(&[textage_record(
            "textage-1",
            "TEXTAGE RENAME",
            "ARTIST A",
            "V1",
        )])),
        ..FederationInput::default()
    });
    let output = renamed.catalog.federate(FederationInput {
        textage: Some(textage_snapshot(&[
            textage_record("textage-1", "TEXTAGE RENAME", "ARTIST A", "V1"),
            textage_record("textage-2", "TEXTAGE RENAME", "ARTIST A", "V1"),
        ])),
        ..FederationInput::default()
    });
    assert!(output.quarantine.iter().any(|entry| {
        entry.source_key == "textage-2"
            && entry.reason == QuarantineReason::ProvisionalWithoutTachiAnchor
    }));
    let song = output.catalog.songs.values().next().unwrap();
    assert!(!song.source_bindings[&super::SourceId::Textage].contains("textage-2"));
}

#[test]
fn same_source_id_rename_preserves_song_id_and_old_variant() {
    let base = catalog_with_tachi(&[tachi_record("anchor-1", "ALPHA", "ARTIST A", "V1", false)]);
    let original_id = base.songs.keys().next().copied().unwrap();
    let output = base.federate(FederationInput {
        tachi: Some(tachi_snapshot(&[tachi_record(
            "anchor-1",
            "ALPHA REVISED",
            "ARTIST A",
            "V1",
            false,
        )])),
        ..FederationInput::default()
    });
    let song = output.catalog.songs.get(&original_id).unwrap();
    let variants: Vec<_> = song
        .title_variants
        .iter()
        .map(|variant| variant.value.as_str())
        .collect();
    assert_eq!(variants, ["ALPHA", "ALPHA REVISED"]);
    assert_eq!(output.catalog.source_evidence.len(), 2);
    assert_eq!(
        song.title_variants
            .iter()
            .map(|variant| &variant.evidence_id)
            .collect::<BTreeSet<_>>()
            .len(),
        2
    );
}

#[test]
fn search_term_does_not_create_identity_or_availability_edges() {
    let mut anchor = tachi_record("anchor-1", "SEARCH ALIAS", "ARTIST A", "V1", false);
    anchor["title_kind"] = json!("search_term");
    let base = catalog_with_tachi(&[anchor]);
    let textage = base.clone().federate(FederationInput {
        textage: Some(textage_snapshot(&[textage_record(
            "textage-1",
            "SEARCH ALIAS",
            "ARTIST A",
            "V1",
        )])),
        ..FederationInput::default()
    });
    assert_eq!(
        textage.quarantine[0].reason,
        QuarantineReason::ProvisionalWithoutTachiAnchor
    );

    let dqn = base.federate(FederationInput {
        dqn: Some(dqn_snapshot(&[dqn_record("SEARCH ALIAS", "ARTIST A")])),
        ..FederationInput::default()
    });
    assert_eq!(
        dqn.quarantine[0].reason,
        QuarantineReason::ProvisionalWithoutTachiAnchor
    );
    assert_eq!(
        dqn.catalog.songs.values().next().unwrap().infinitas_status,
        InfinitasStatus::Unknown
    );
}

#[test]
fn identical_tachi_bytes_at_distinct_revisions_preserve_both_evidence_records() {
    let fixture = fixture(
        "scorepeek-tachi-fixture-v1",
        &[tachi_record("anchor-1", "ALPHA", "ARTIST A", "V1", false)],
    );
    let bytes = serde_json::to_vec(&fixture).unwrap();
    let first = TachiFixtureAdapter::parse(&bytes, git_revision()).unwrap();
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
    assert_eq!(catalog.source_evidence.len(), 2);
    assert_eq!(
        catalog
            .source_evidence
            .values()
            .map(|evidence| evidence.revision.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        2
    );
}

#[test]
fn tachi_artist_or_version_change_is_a_critical_conflict() {
    let base = catalog_with_tachi(&[tachi_record("anchor-1", "ALPHA", "ARTIST A", "V1", false)]);
    let output = base.federate(FederationInput {
        tachi: Some(tachi_snapshot(&[tachi_record(
            "anchor-1",
            "ALPHA",
            "ARTIST CHANGED",
            "V1",
            false,
        )])),
        ..FederationInput::default()
    });
    assert_eq!(
        output.quarantine[0].reason,
        QuarantineReason::CriticalConflict
    );
    assert_eq!(output.catalog.songs, base.songs);
}

#[test]
fn existing_binding_that_bridges_two_ids_is_quarantined() {
    let base = catalog_with_tachi(&[
        tachi_record("anchor-1", "ALPHA", "ARTIST A", "V1", false),
        tachi_record("anchor-2", "BETA", "ARTIST B", "V2", false),
    ]);
    let bound = base.federate(FederationInput {
        textage: Some(textage_snapshot(&[textage_record(
            "textage-1",
            "ALPHA",
            "ARTIST A",
            "V1",
        )])),
        ..FederationInput::default()
    });
    let bridge = bound.catalog.federate(FederationInput {
        textage: Some(textage_snapshot(&[textage_record(
            "textage-1",
            "BETA",
            "ARTIST B",
            "V2",
        )])),
        ..FederationInput::default()
    });
    assert_eq!(
        bridge.quarantine[0].reason,
        QuarantineReason::ExistingIdentityBridge
    );
}

#[test]
fn dqn_adds_only_unique_exact_availability_bindings() {
    let base = catalog_with_tachi(&[
        tachi_record("anchor-1", "ALPHA", "ARTIST A", "V1", false),
        tachi_record("anchor-2", "DUPLICATE", "ARTIST D", "V1", false),
        tachi_record("anchor-3", "DUPLICATE", "ARTIST D", "V2", false),
    ]);
    let output = base.federate(FederationInput {
        dqn: Some(dqn_snapshot(&[
            dqn_record("ALPHA", "ARTIST A"),
            dqn_record("MISSING", "ARTIST M"),
            dqn_record("DUPLICATE", "ARTIST D"),
        ])),
        ..FederationInput::default()
    });

    let alpha = output
        .catalog
        .songs
        .values()
        .find(|song| song.tachi_source_id == "anchor-1")
        .unwrap();
    assert_eq!(alpha.infinitas_status, InfinitasStatus::ConfirmedPresent);
    assert_eq!(output.quarantine.len(), 2);
    assert!(
        output
            .quarantine
            .iter()
            .any(|entry| entry.reason == QuarantineReason::AmbiguousIdentity)
    );
    assert!(
        output
            .quarantine
            .iter()
            .any(|entry| entry.reason == QuarantineReason::ProvisionalWithoutTachiAnchor)
    );
}

#[test]
fn dqn_regression_keeps_last_known_good_binding_set() {
    let base = catalog_with_tachi(&[
        tachi_record("anchor-1", "ALPHA", "ARTIST A", "V1", false),
        tachi_record("anchor-2", "BETA", "ARTIST B", "V2", false),
    ]);
    let accepted = base.federate(FederationInput {
        dqn: Some(dqn_snapshot(&[dqn_record("ALPHA", "ARTIST A")])),
        ..FederationInput::default()
    });
    let regressed = accepted.catalog.federate(FederationInput {
        dqn: Some(dqn_snapshot(&[dqn_record("BETA", "ARTIST B")])),
        ..FederationInput::default()
    });

    let alpha = regressed
        .catalog
        .songs
        .values()
        .find(|song| song.tachi_source_id == "anchor-1")
        .unwrap();
    let beta = regressed
        .catalog
        .songs
        .values()
        .find(|song| song.tachi_source_id == "anchor-2")
        .unwrap();
    assert_eq!(alpha.infinitas_status, InfinitasStatus::ConfirmedPresent);
    assert_eq!(beta.infinitas_status, InfinitasStatus::Unknown);
    assert_eq!(
        regressed.quarantine[0].reason,
        QuarantineReason::DqnBindingRegression
    );
}

#[test]
fn dqn_nullable_pack_evidence_survives_catalog_snapshot_round_trip() {
    let catalog = catalog_with_tachi(&[tachi_record("anchor-1", "ALPHA", "ARTIST A", "V1", false)]);
    let catalog = catalog
        .federate(FederationInput {
            dqn: Some(dqn_snapshot(&[dqn_base_record("ALPHA", "ARTIST A")])),
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
}

#[test]
fn source_record_count_regression_is_quarantined() {
    let base = catalog_with_tachi(&[
        tachi_record("anchor-1", "ALPHA", "ARTIST A", "V1", false),
        tachi_record("anchor-2", "BETA", "ARTIST B", "V2", false),
    ]);
    let output = base.federate(FederationInput {
        tachi: Some(tachi_snapshot(&[tachi_record(
            "anchor-1",
            "ALPHA REVISED",
            "ARTIST A",
            "V1",
            false,
        )])),
        ..FederationInput::default()
    });
    assert_eq!(output.catalog, base);
    assert_eq!(
        output.quarantine[0].reason,
        QuarantineReason::SourceHealthRegression
    );
}

fn catalog_with_tachi(records: &[serde_json::Value]) -> Catalog {
    Catalog::default()
        .federate(FederationInput {
            tachi: Some(tachi_snapshot(records)),
            ..FederationInput::default()
        })
        .catalog
}

fn tachi_snapshot(records: &[serde_json::Value]) -> super::SourceSnapshot {
    let fixture = fixture("scorepeek-tachi-fixture-v1", records);
    TachiFixtureAdapter::parse(&serde_json::to_vec(&fixture).unwrap(), git_revision()).unwrap()
}

fn textage_snapshot(records: &[serde_json::Value]) -> super::SourceSnapshot {
    let fixture = fixture("scorepeek-textage-fixture-v1", records);
    let bytes = serde_json::to_vec(&fixture).unwrap();
    TextageFixtureAdapter::parse(&bytes, content_revision(&bytes)).unwrap()
}

fn dqn_snapshot(records: &[serde_json::Value]) -> super::SourceSnapshot {
    let bytes = serde_json::to_vec(records).unwrap();
    DqnLiveAdapter::parse(&bytes, content_revision(&bytes)).unwrap()
}

fn fixture(schema: &str, records: &[serde_json::Value]) -> serde_json::Value {
    json!({ "schema": schema, "records": records })
}

fn tachi_record(
    source_song_id: &str,
    title: &str,
    artist: &str,
    version: &str,
    primary_infinitas: bool,
) -> serde_json::Value {
    json!({
        "source_song_id": source_song_id,
        "title": title,
        "title_kind": "in_game_display",
        "artist": artist,
        "version": version,
        "charts": charts(),
        "primary_infinitas": primary_infinitas
    })
}

fn textage_record(
    source_song_id: &str,
    title: &str,
    artist: &str,
    version: &str,
) -> serde_json::Value {
    json!({
        "source_song_id": source_song_id,
        "title": title,
        "title_kind": "alternate_display",
        "artist": artist,
        "version": version,
        "charts": charts(),
        "bpm_min": 100,
        "bpm_max": 200,
        "infinitas_flag": true
    })
}

fn dqn_record(title: &str, artist: &str) -> serde_json::Value {
    json!({ "title": title, "artist": artist, "packName": "SYNTHETIC PACK" })
}

fn dqn_base_record(title: &str, artist: &str) -> serde_json::Value {
    json!({ "title": title, "artist": artist, "packName": null })
}

fn charts() -> serde_json::Value {
    json!([
        { "play_type": "single", "difficulty": "normal", "level": 4, "notes": 400,
          "source_chart_id": "spn", "product_versions": ["synthetic-v1"], "primary": true },
        { "play_type": "single", "difficulty": "hyper", "level": 8, "notes": 800,
          "source_chart_id": "sph", "product_versions": ["synthetic-v1"], "primary": true }
    ])
}

fn git_revision() -> SourceRevision {
    SourceRevision::git_commit(GIT_REVISION).unwrap()
}

fn content_revision(bytes: &[u8]) -> SourceRevision {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(encoded, "{byte:02x}").unwrap();
    }
    SourceRevision::content_sha256(encoded).unwrap()
}
use std::collections::BTreeSet;
use std::fmt::Write as _;
