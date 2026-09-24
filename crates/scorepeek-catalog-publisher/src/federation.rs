//! Publication-only source observations, federation, and quarantine.

use scorepeek_core::catalog::policy::{SourceId, SourcePolicy};
use scorepeek_core::catalog::{
    Catalog, CatalogSong, Chart, ChartAssertion, DisplayVariant, DisplayVariantKind, DqnBinding,
    EvidenceId, ExactTitleArtist, InfinitasStatus, ScorepeekSongId, SourceEvidence,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug)]
pub struct SourceSnapshot {
    pub policy: SourcePolicy,
    pub evidence: SourceEvidence,
    pub observations: Vec<SourceObservation>,
}

impl SourceSnapshot {
    #[must_use]
    pub const fn policy(&self) -> &SourcePolicy {
        &self.policy
    }

    #[must_use]
    pub const fn evidence(&self) -> &SourceEvidence {
        &self.evidence
    }
}

#[derive(Clone, Debug)]
pub enum SourceObservation {
    Tachi(TachiObservation),
    Textage(TextageObservation),
    Dqn(DqnObservation),
}

#[derive(Clone, Debug)]
pub struct TachiObservation {
    pub source_song_id: String,
    pub title_variants: BTreeSet<SourceTitleObservation>,
    pub artist: String,
    pub version: String,
    pub charts: Vec<SourceChartObservation>,
    pub primary_infinitas: bool,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SourceTitleObservation {
    pub value: String,
    pub kind: DisplayVariantKind,
}

#[derive(Clone, Debug)]
pub struct TextageObservation {
    pub source_song_id: String,
    pub title: String,
    pub artist: String,
    pub version: String,
    pub title_kind: DisplayVariantKind,
    pub charts: Vec<SourceChartObservation>,
    pub infinitas_flag: bool,
    pub bpm_min: u16,
    pub bpm_max: u16,
}

#[derive(Clone, Debug)]
pub struct DqnObservation {
    pub title: String,
    pub artist: String,
    pub pack: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SourceChartObservation {
    pub chart: Chart,
    pub source_chart_id: String,
    pub product_versions: BTreeSet<String>,
    pub primary: bool,
}

#[derive(Clone, Debug, Default)]
pub struct FederationInput {
    pub tachi: Option<SourceSnapshot>,
    pub textage: Option<SourceSnapshot>,
    pub dqn: Option<SourceSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FederationOutput {
    pub catalog: Catalog,
    pub quarantine: Vec<QuarantineEntry>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct QuarantineEntry {
    pub source_id: SourceId,
    pub source_key: String,
    pub reason: QuarantineReason,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuarantineReason {
    SourcePolicyMismatch,
    ProvisionalWithoutTachiAnchor,
    AmbiguousIdentity,
    ExistingIdentityBridge,
    ConflictingChart,
    CriticalConflict,
    DqnBindingRegression,
    SourceHealthRegression,
}

pub trait CatalogFederationExt {
    fn federate(&self, input: FederationInput) -> FederationOutput;
}

impl CatalogFederationExt for Catalog {
    fn federate(&self, input: FederationInput) -> FederationOutput {
        let mut catalog = self.clone();
        let mut quarantine = Vec::new();

        apply_tachi(&mut catalog, input.tachi, &mut quarantine);
        apply_textage(&mut catalog, input.textage, &mut quarantine);
        apply_dqn(self, &mut catalog, input.dqn, &mut quarantine);
        refresh_infinitas_status(&mut catalog);
        prune_unreferenced_evidence(&mut catalog);
        quarantine.sort();

        FederationOutput {
            catalog,
            quarantine,
        }
    }
}

fn evidence_id(evidence: &SourceEvidence) -> EvidenceId {
    EvidenceId {
        source_id: evidence.source_id,
        revision: evidence.revision.clone(),
        content_sha256: evidence.content_sha256.clone(),
    }
}

fn apply_tachi(
    catalog: &mut Catalog,
    snapshot: Option<SourceSnapshot>,
    quarantine: &mut Vec<QuarantineEntry>,
) {
    let Some(snapshot) = verified_snapshot(snapshot, &SourcePolicy::tachi(), quarantine) else {
        return;
    };
    if !source_is_healthy(catalog, &snapshot, quarantine) {
        return;
    }
    let SourceSnapshot {
        evidence,
        observations,
        ..
    } = snapshot;
    let evidence_id = evidence_id(&evidence);
    let mut records: Vec<_> = observations
        .into_iter()
        .filter_map(|observation| match observation {
            SourceObservation::Tachi(record) => Some(record),
            _ => None,
        })
        .collect();
    records.sort_by(|left, right| left.source_song_id.cmp(&right.source_song_id));

    for record in records {
        apply_tachi_record(catalog, record, &evidence_id, quarantine);
    }
    catalog
        .source_evidence
        .insert(evidence_id.clone(), evidence);
    catalog.latest_evidence.insert(SourceId::Tachi, evidence_id);
}

fn apply_tachi_record(
    catalog: &mut Catalog,
    record: TachiObservation,
    evidence_id: &EvidenceId,
    quarantine: &mut Vec<QuarantineEntry>,
) {
    let song_id = ScorepeekSongId::from_tachi_id(&record.source_song_id);
    if let Some(existing) = catalog.songs.get_mut(&song_id) {
        if existing.artist != record.artist || existing.version != record.version {
            quarantine.push(entry(
                SourceId::Tachi,
                record.source_song_id,
                QuarantineReason::CriticalConflict,
            ));
            return;
        }
        if has_chart_conflict(existing, &record.charts) {
            quarantine.push(entry(
                SourceId::Tachi,
                record.source_song_id,
                QuarantineReason::ConflictingChart,
            ));
            return;
        }
        add_tachi_title_variants(existing, record.title_variants, evidence_id);
        let binding_key = record.source_song_id.clone();
        add_binding_attributes(
            existing,
            SourceId::Tachi,
            &binding_key,
            BTreeMap::from([(
                "primary_infinitas".to_owned(),
                record.primary_infinitas.to_string(),
            )]),
            evidence_id,
        );
        add_charts(existing, record.charts, evidence_id);
        existing.tachi_primary_infinitas |= record.primary_infinitas;
        return;
    }

    let charts: BTreeMap<_, _> = record
        .charts
        .iter()
        .map(|observation| (observation.chart.key, observation.chart.clone()))
        .collect();
    let chart_assertions = record
        .charts
        .iter()
        .map(|observation| {
            (
                observation.chart.key,
                BTreeSet::from([ChartAssertion {
                    source_chart_id: observation.source_chart_id.clone(),
                    product_versions: observation.product_versions.clone(),
                    primary: observation.primary,
                    evidence_id: evidence_id.clone(),
                }]),
            )
        })
        .collect();
    let binding_key = record.source_song_id.clone();
    catalog.songs.insert(
        song_id,
        CatalogSong {
            song_id,
            tachi_source_id: binding_key.clone(),
            title_variants: record
                .title_variants
                .into_iter()
                .map(|variant| DisplayVariant {
                    value: variant.value,
                    source_id: SourceId::Tachi,
                    kind: variant.kind,
                    evidence_id: evidence_id.clone(),
                })
                .collect(),
            artist: record.artist,
            version: record.version,
            charts,
            chart_assertions,
            infinitas_status: InfinitasStatus::Unknown,
            source_bindings: BTreeMap::from([(
                SourceId::Tachi,
                BTreeSet::from([binding_key.clone()]),
            )]),
            binding_evidence: BTreeMap::from([(
                (SourceId::Tachi, binding_key.clone()),
                BTreeSet::from([evidence_id.clone()]),
            )]),
            binding_attributes: BTreeMap::from([(
                (SourceId::Tachi, binding_key, evidence_id.clone()),
                BTreeMap::from([(
                    "primary_infinitas".to_owned(),
                    record.primary_infinitas.to_string(),
                )]),
            )]),
            tachi_primary_infinitas: record.primary_infinitas,
        },
    );
}

fn add_tachi_title_variants(
    song: &mut CatalogSong,
    variants: BTreeSet<SourceTitleObservation>,
    evidence_id: &EvidenceId,
) {
    for variant in variants {
        add_title_variant(
            song,
            SourceId::Tachi,
            variant.value,
            variant.kind,
            evidence_id,
        );
    }
}

fn add_title_variant(
    song: &mut CatalogSong,
    source_id: SourceId,
    value: String,
    kind: DisplayVariantKind,
    evidence_id: &EvidenceId,
) {
    let already_asserted = song.title_variants.iter().any(|existing| {
        existing.source_id == source_id && existing.kind == kind && existing.value == value
    });
    if !already_asserted {
        song.title_variants.insert(DisplayVariant {
            value,
            source_id,
            kind,
            evidence_id: evidence_id.clone(),
        });
    }
}

fn add_binding_attributes(
    song: &mut CatalogSong,
    source_id: SourceId,
    source_key: &str,
    attributes: BTreeMap<String, String>,
    evidence_id: &EvidenceId,
) {
    let binding_key = (source_id, source_key.to_owned());
    let already_asserted = song
        .binding_evidence
        .get(&binding_key)
        .is_some_and(|evidence_ids| {
            evidence_ids.iter().any(|existing_evidence| {
                song.binding_attributes.get(&(
                    source_id,
                    source_key.to_owned(),
                    existing_evidence.clone(),
                )) == Some(&attributes)
            })
        });
    if already_asserted {
        return;
    }
    song.binding_evidence
        .entry(binding_key)
        .or_default()
        .insert(evidence_id.clone());
    song.binding_attributes.insert(
        (source_id, source_key.to_owned(), evidence_id.clone()),
        attributes,
    );
}

fn apply_textage(
    catalog: &mut Catalog,
    snapshot: Option<SourceSnapshot>,
    quarantine: &mut Vec<QuarantineEntry>,
) {
    let Some(snapshot) = verified_snapshot(snapshot, &SourcePolicy::textage(), quarantine) else {
        return;
    };
    if !source_is_healthy(catalog, &snapshot, quarantine) {
        return;
    }
    let SourceSnapshot {
        evidence,
        observations,
        ..
    } = snapshot;
    let evidence_id = evidence_id(&evidence);
    let mut records: Vec<_> = observations
        .into_iter()
        .filter_map(|observation| match observation {
            SourceObservation::Textage(record) => Some(record),
            _ => None,
        })
        .collect();
    records.sort_by(|left, right| left.source_song_id.cmp(&right.source_song_id));

    for record in records {
        let bound = find_binding(catalog, SourceId::Textage, &record.source_song_id);
        let matches = textage_matches(catalog, &record);
        let resolved = match (bound, matches.as_slice()) {
            (Some(bound), matches) if matches.iter().any(|candidate| *candidate != bound) => {
                quarantine.push(entry(
                    SourceId::Textage,
                    record.source_song_id,
                    QuarantineReason::ExistingIdentityBridge,
                ));
                continue;
            }
            (Some(bound), _) => Some(bound),
            (None, [song_id]) => Some(*song_id),
            (None, []) => {
                quarantine.push(entry(
                    SourceId::Textage,
                    record.source_song_id,
                    QuarantineReason::ProvisionalWithoutTachiAnchor,
                ));
                continue;
            }
            (None, _) => {
                quarantine.push(entry(
                    SourceId::Textage,
                    record.source_song_id,
                    QuarantineReason::AmbiguousIdentity,
                ));
                continue;
            }
        };

        let song = catalog
            .songs
            .get_mut(&resolved.expect("resolved Textage identity"))
            .expect("Textage identity references active song");
        if has_chart_conflict(song, &record.charts) {
            quarantine.push(entry(
                SourceId::Textage,
                record.source_song_id,
                QuarantineReason::ConflictingChart,
            ));
            continue;
        }
        add_title_variant(
            song,
            SourceId::Textage,
            record.title,
            record.title_kind,
            &evidence_id,
        );
        song.source_bindings
            .entry(SourceId::Textage)
            .or_default()
            .insert(record.source_song_id.clone());
        add_binding_attributes(
            song,
            SourceId::Textage,
            &record.source_song_id,
            BTreeMap::from([
                (
                    "infinitas_flag".to_owned(),
                    record.infinitas_flag.to_string(),
                ),
                ("bpm_min".to_owned(), record.bpm_min.to_string()),
                ("bpm_max".to_owned(), record.bpm_max.to_string()),
            ]),
            &evidence_id,
        );
        add_charts(song, record.charts, &evidence_id);
    }
    catalog
        .source_evidence
        .insert(evidence_id.clone(), evidence);
    catalog
        .latest_evidence
        .insert(SourceId::Textage, evidence_id);
}

fn apply_dqn(
    previous: &Catalog,
    catalog: &mut Catalog,
    snapshot: Option<SourceSnapshot>,
    quarantine: &mut Vec<QuarantineEntry>,
) {
    let Some(snapshot) = verified_snapshot(snapshot, &SourcePolicy::dqn(), quarantine) else {
        return;
    };
    if !source_is_healthy(catalog, &snapshot, quarantine) {
        return;
    }
    let SourceSnapshot {
        evidence,
        observations,
        ..
    } = snapshot;
    let evidence_id = evidence_id(&evidence);
    let records: Vec<_> = observations
        .into_iter()
        .filter_map(|observation| match observation {
            SourceObservation::Dqn(record) => Some(record),
            _ => None,
        })
        .collect();
    let current_tuples: BTreeSet<_> = records
        .iter()
        .map(|record| exact_title_artist(&record.title, &record.artist))
        .collect();
    let prior_regressed = previous.dqn_bindings.iter().any(|(tuple, binding)| {
        !current_tuples.contains(tuple)
            || unique_title_artist_match(catalog, tuple) != Some(binding.song_id)
    });
    if prior_regressed {
        for record in records {
            quarantine.push(entry(
                SourceId::DqnIidxapi,
                format!("{}\u{0}{}", record.title, record.artist),
                QuarantineReason::DqnBindingRegression,
            ));
        }
        return;
    }

    let mut candidates = Vec::new();
    for record in records {
        let tuple = exact_title_artist(&record.title, &record.artist);
        match title_artist_matches(catalog, &tuple).as_slice() {
            [song_id] => {
                candidates.push((tuple, *song_id, record.pack));
            }
            [] => quarantine.push(entry(
                SourceId::DqnIidxapi,
                format!("{}\u{0}{}", record.title, record.artist),
                QuarantineReason::ProvisionalWithoutTachiAnchor,
            )),
            _ => quarantine.push(entry(
                SourceId::DqnIidxapi,
                format!("{}\u{0}{}", record.title, record.artist),
                QuarantineReason::AmbiguousIdentity,
            )),
        }
    }
    for (tuple, song_id, pack) in candidates {
        let binding = catalog
            .dqn_bindings
            .entry(tuple)
            .or_insert_with(|| DqnBinding {
                song_id,
                evidence_packs: BTreeMap::new(),
            });
        binding
            .evidence_packs
            .entry(evidence_id.clone())
            .or_default()
            .insert(pack);
    }
    catalog
        .source_evidence
        .insert(evidence_id.clone(), evidence);
    catalog
        .latest_evidence
        .insert(SourceId::DqnIidxapi, evidence_id);
}

fn verified_snapshot(
    snapshot: Option<SourceSnapshot>,
    expected: &SourcePolicy,
    quarantine: &mut Vec<QuarantineEntry>,
) -> Option<SourceSnapshot> {
    let snapshot = snapshot?;
    if snapshot.policy != *expected {
        quarantine.push(entry(
            snapshot.policy.source_id,
            snapshot.evidence.revision.clone(),
            QuarantineReason::SourcePolicyMismatch,
        ));
        return None;
    }
    Some(snapshot)
}

fn source_is_healthy(
    catalog: &Catalog,
    snapshot: &SourceSnapshot,
    quarantine: &mut Vec<QuarantineEntry>,
) -> bool {
    let previous_count = catalog
        .latest_evidence
        .get(&snapshot.policy.source_id)
        .and_then(|evidence_id| catalog.source_evidence.get(evidence_id))
        .map(|evidence| evidence.record_count);
    if snapshot.evidence.record_count == 0
        || previous_count.is_some_and(|count| snapshot.evidence.record_count < count)
    {
        quarantine.push(entry(
            snapshot.policy.source_id,
            snapshot.evidence.revision.clone(),
            QuarantineReason::SourceHealthRegression,
        ));
        return false;
    }
    true
}

fn textage_matches(catalog: &Catalog, record: &TextageObservation) -> Vec<ScorepeekSongId> {
    let title = nfc(&record.title);
    let artist = nfc(&record.artist);
    let version = nfc(&record.version);
    catalog
        .songs
        .values()
        .filter(|song| {
            nfc(&song.artist) == artist
                && nfc(&song.version) == version
                && song.title_variants.iter().any(|variant| {
                    variant.source_id == SourceId::Tachi
                        && identity_variant(variant)
                        && nfc(&variant.value) == title
                })
                && matching_chart_count(song, &record.charts) >= 2
        })
        .map(|song| song.song_id)
        .collect()
}

fn matching_chart_count(song: &CatalogSong, charts: &[SourceChartObservation]) -> usize {
    charts
        .iter()
        .filter(|observation| {
            song.charts
                .get(&observation.chart.key)
                .is_some_and(|known| known.notes == observation.chart.notes)
        })
        .count()
}

fn find_binding(catalog: &Catalog, source: SourceId, key: &str) -> Option<ScorepeekSongId> {
    catalog.songs.values().find_map(|song| {
        song.source_bindings
            .get(&source)
            .is_some_and(|bindings| bindings.contains(key))
            .then_some(song.song_id)
    })
}

fn has_chart_conflict(song: &CatalogSong, charts: &[SourceChartObservation]) -> bool {
    charts.iter().any(|observation| {
        song.charts
            .get(&observation.chart.key)
            .is_some_and(|existing| existing != &observation.chart)
    })
}

fn add_charts(
    song: &mut CatalogSong,
    charts: Vec<SourceChartObservation>,
    evidence_id: &EvidenceId,
) {
    for observation in charts {
        let key = observation.chart.key;
        song.charts.entry(key).or_insert(observation.chart);
        let assertions = song.chart_assertions.entry(key).or_default();
        let already_asserted = assertions.iter().any(|existing| {
            existing.evidence_id.source_id == evidence_id.source_id
                && existing.source_chart_id == observation.source_chart_id
                && existing.product_versions == observation.product_versions
                && existing.primary == observation.primary
        });
        if !already_asserted {
            assertions.insert(ChartAssertion {
                source_chart_id: observation.source_chart_id,
                product_versions: observation.product_versions,
                primary: observation.primary,
                evidence_id: evidence_id.clone(),
            });
        }
    }
}

fn prune_unreferenced_evidence(catalog: &mut Catalog) {
    let mut referenced: BTreeSet<_> = catalog.latest_evidence.values().cloned().collect();
    for song in catalog.songs.values() {
        referenced.extend(
            song.title_variants
                .iter()
                .map(|variant| variant.evidence_id.clone()),
        );
        referenced.extend(
            song.chart_assertions
                .values()
                .flatten()
                .map(|assertion| assertion.evidence_id.clone()),
        );
        referenced.extend(song.binding_evidence.values().flatten().cloned());
    }
    for binding in catalog.dqn_bindings.values() {
        referenced.extend(binding.evidence_packs.keys().cloned());
    }
    catalog
        .source_evidence
        .retain(|evidence_id, _| referenced.contains(evidence_id));
}

fn exact_title_artist(title: &str, artist: &str) -> ExactTitleArtist {
    ExactTitleArtist {
        title: nfc(title),
        artist: nfc(artist),
    }
}

fn unique_title_artist_match(
    catalog: &Catalog,
    tuple: &ExactTitleArtist,
) -> Option<ScorepeekSongId> {
    match title_artist_matches(catalog, tuple).as_slice() {
        [song_id] => Some(*song_id),
        _ => None,
    }
}

fn title_artist_matches(catalog: &Catalog, tuple: &ExactTitleArtist) -> Vec<ScorepeekSongId> {
    catalog
        .songs
        .values()
        .filter(|song| {
            nfc(&song.artist) == tuple.artist
                && song
                    .title_variants
                    .iter()
                    .any(|variant| identity_variant(variant) && nfc(&variant.value) == tuple.title)
        })
        .map(|song| song.song_id)
        .collect()
}

const fn identity_variant(variant: &DisplayVariant) -> bool {
    !matches!(variant.kind, DisplayVariantKind::SearchTerm)
}

fn refresh_infinitas_status(catalog: &mut Catalog) {
    let dqn_song_ids: BTreeSet<_> = catalog
        .dqn_bindings
        .values()
        .map(|binding| binding.song_id)
        .collect();
    for song in catalog.songs.values_mut() {
        song.infinitas_status =
            if song.tachi_primary_infinitas || dqn_song_ids.contains(&song.song_id) {
                InfinitasStatus::ConfirmedPresent
            } else {
                InfinitasStatus::Unknown
            };
    }
}

fn nfc(value: &str) -> String {
    value.nfc().collect()
}

fn entry(source_id: SourceId, source_key: String, reason: QuarantineReason) -> QuarantineEntry {
    QuarantineEntry {
        source_id,
        source_key,
        reason,
    }
}
