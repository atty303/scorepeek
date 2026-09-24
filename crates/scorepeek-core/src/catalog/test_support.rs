//! Synthetic completed catalogs for tests without a publisher dependency.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    Catalog, CatalogSong, Chart, ChartAssertion, DisplayVariant, DisplayVariantKind, EvidenceId,
    InfinitasStatus, ScorepeekSongId, SourceEvidence, SourceId, SourcePolicy,
};

pub struct SyntheticTachiRecord<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub title_kind: DisplayVariantKind,
    pub artist: &'a str,
    pub version: &'a str,
    pub charts: Vec<Chart>,
    pub primary_infinitas: bool,
}

#[must_use]
pub fn catalog_from_tachi(records: &[SyntheticTachiRecord<'_>]) -> Catalog {
    federate_tachi(
        &Catalog::default(),
        records,
        "0123456789abcdef0123456789abcdef01234567",
    )
}

#[must_use]
pub fn federate_tachi(
    base: &Catalog,
    records: &[SyntheticTachiRecord<'_>],
    revision: &str,
) -> Catalog {
    let mut catalog = base.clone();
    let evidence = synthetic_evidence(records.len(), revision);
    let evidence_id = EvidenceId {
        source_id: SourceId::Tachi,
        revision: revision.to_owned(),
        content_sha256: evidence.content_sha256.clone(),
    };
    for record in records {
        let song_id = ScorepeekSongId::from_tachi_id(record.id);
        let title = DisplayVariant {
            value: record.title.to_owned(),
            source_id: SourceId::Tachi,
            kind: record.title_kind,
            evidence_id: evidence_id.clone(),
        };
        let assertions = record.charts.iter().enumerate().map(|(index, chart)| {
            (
                chart.key,
                ChartAssertion {
                    source_chart_id: format!("synthetic-{index}"),
                    product_versions: BTreeSet::from(["synthetic-v1".to_owned()]),
                    primary: true,
                    evidence_id: evidence_id.clone(),
                },
            )
        });
        if let Some(song) = catalog.songs.get_mut(&song_id) {
            if !song
                .title_variants
                .iter()
                .any(|variant| variant.value == title.value && variant.kind == title.kind)
            {
                song.title_variants.insert(title);
            }
            for (key, assertion) in assertions {
                song.chart_assertions
                    .entry(key)
                    .or_default()
                    .insert(assertion);
            }
            song.tachi_primary_infinitas |= record.primary_infinitas;
            if song.tachi_primary_infinitas {
                song.infinitas_status = InfinitasStatus::ConfirmedPresent;
            }
        } else {
            let key = record.id.to_owned();
            catalog.songs.insert(
                song_id,
                CatalogSong {
                    song_id,
                    tachi_source_id: key.clone(),
                    title_variants: BTreeSet::from([title]),
                    artist: record.artist.to_owned(),
                    version: record.version.to_owned(),
                    charts: record
                        .charts
                        .iter()
                        .map(|chart| (chart.key, chart.clone()))
                        .collect(),
                    chart_assertions: assertions
                        .map(|(key, assertion)| (key, BTreeSet::from([assertion])))
                        .collect(),
                    infinitas_status: if record.primary_infinitas {
                        InfinitasStatus::ConfirmedPresent
                    } else {
                        InfinitasStatus::Unknown
                    },
                    source_bindings: BTreeMap::from([(
                        SourceId::Tachi,
                        BTreeSet::from([key.clone()]),
                    )]),
                    binding_evidence: BTreeMap::from([(
                        (SourceId::Tachi, key.clone()),
                        BTreeSet::from([evidence_id.clone()]),
                    )]),
                    binding_attributes: BTreeMap::from([(
                        (SourceId::Tachi, key, evidence_id.clone()),
                        BTreeMap::from([(
                            "primary_infinitas".to_owned(),
                            record.primary_infinitas.to_string(),
                        )]),
                    )]),
                    tachi_primary_infinitas: record.primary_infinitas,
                },
            );
        }
    }
    catalog
        .source_evidence
        .insert(evidence_id.clone(), evidence);
    catalog.latest_evidence.insert(SourceId::Tachi, evidence_id);
    catalog
}

fn synthetic_evidence(record_count: usize, revision: &str) -> SourceEvidence {
    let policy = SourcePolicy::tachi();
    SourceEvidence {
        source_id: SourceId::Tachi,
        lineage_id: policy.lineage_id,
        revision_strategy: policy.revision_strategy,
        revision: revision.to_owned(),
        content_sha256: "a".repeat(64),
        byte_size: record_count,
        record_count,
        parser_version: policy.parser_version.to_owned(),
        declared_scope: policy.declared_scope.to_owned(),
        completeness: policy.completeness,
        field_authority: policy
            .field_authority
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        freshness: policy.freshness.to_owned(),
        rights_and_provenance: policy.rights_and_provenance.to_owned(),
    }
}
