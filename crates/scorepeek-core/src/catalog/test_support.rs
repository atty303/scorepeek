use std::collections::BTreeSet;

use super::FederationInput;
use super::{
    Catalog, Chart, DisplayVariantKind, SourceChartObservation, SourceEvidence, SourceId,
    SourceObservation, SourcePolicy, SourceSnapshot, SourceTitleObservation, TachiObservation,
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
    let policy = SourcePolicy::tachi();
    let observations = records
        .iter()
        .map(|record| {
            SourceObservation::Tachi(TachiObservation {
                source_song_id: record.id.to_owned(),
                title_variants: BTreeSet::from([SourceTitleObservation {
                    value: record.title.to_owned(),
                    kind: record.title_kind,
                }]),
                artist: record.artist.to_owned(),
                version: record.version.to_owned(),
                charts: record
                    .charts
                    .iter()
                    .enumerate()
                    .map(|(index, chart)| SourceChartObservation {
                        chart: chart.clone(),
                        source_chart_id: format!("synthetic-{index}"),
                        product_versions: BTreeSet::from(["synthetic-v1".to_owned()]),
                        primary: true,
                    })
                    .collect(),
                primary_infinitas: record.primary_infinitas,
            })
        })
        .collect::<Vec<_>>();
    let mut field_authority = policy
        .field_authority
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    field_authority.sort();
    let evidence = SourceEvidence {
        source_id: SourceId::Tachi,
        lineage_id: policy.lineage_id,
        revision_strategy: policy.revision_strategy,
        revision: revision.to_owned(),
        content_sha256: "a".repeat(64),
        byte_size: records.len(),
        record_count: observations.len(),
        parser_version: policy.parser_version.to_owned(),
        declared_scope: policy.declared_scope.to_owned(),
        completeness: policy.completeness,
        field_authority,
        freshness: policy.freshness.to_owned(),
        rights_and_provenance: policy.rights_and_provenance.to_owned(),
    };
    base.federate(FederationInput {
        tachi: Some(SourceSnapshot {
            policy,
            evidence,
            observations,
        }),
        ..FederationInput::default()
    })
    .catalog
}
