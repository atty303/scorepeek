use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceId {
    Tachi,
    Textage,
    DqnIidxapi,
}

impl SourceId {
    pub(crate) const COUNT: usize = 3;
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LineageId {
    GameMdb,
    Textage,
    OfficialInfinitasHtml,
}

impl LineageId {
    pub(crate) const COUNT: usize = 3;
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SourcePolicy {
    pub source_id: SourceId,
    pub lineage_id: LineageId,
    pub revision_strategy: RevisionStrategy,
    pub parser_version: &'static str,
    pub declared_scope: &'static str,
    pub completeness: Completeness,
    pub field_authority: &'static [&'static str],
    pub freshness: &'static str,
    pub rights_and_provenance: &'static str,
}

impl SourcePolicy {
    pub(super) const fn for_id(source_id: SourceId) -> Self {
        match source_id {
            SourceId::Tachi => Self::tachi(),
            SourceId::Textage => Self::textage(),
            SourceId::DqnIidxapi => Self::dqn(),
        }
    }

    pub(crate) const fn tachi() -> Self {
        Self {
            source_id: SourceId::Tachi,
            lineage_id: LineageId::GameMdb,
            revision_strategy: RevisionStrategy::GitCommit,
            parser_version: "scorepeek-tachi-fixture-parser-v1",
            declared_scope: "general_iidx_identity_and_charts",
            completeness: Completeness::NonExhaustive,
            field_authority: &[
                "source_song_id",
                "title",
                "title_kind",
                "artist",
                "version",
                "charts",
                "source_chart_id",
                "product_versions",
                "chart_primary",
                "primary_infinitas",
            ],
            freshness: "pinned_git_commit_at_sync",
            rights_and_provenance: "tachi_iidx_seeds_local_snapshot",
        }
    }

    pub(crate) const fn textage() -> Self {
        Self {
            source_id: SourceId::Textage,
            lineage_id: LineageId::Textage,
            revision_strategy: RevisionStrategy::ContentSha256,
            parser_version: "scorepeek-textage-fixture-parser-v1",
            declared_scope: "metadata_display_and_chart_corroboration",
            completeness: Completeness::NonExhaustive,
            field_authority: &[
                "source_song_id",
                "title",
                "title_kind",
                "artist",
                "version",
                "charts",
                "source_chart_id",
                "product_versions",
                "chart_primary",
                "bpm_min",
                "bpm_max",
                "infinitas_flag_corroboration",
            ],
            freshness: "mutable_http_bytes_pinned_by_sha256",
            rights_and_provenance: "textage_local_snapshot_no_redistribution",
        }
    }

    pub(crate) const fn dqn() -> Self {
        Self {
            source_id: SourceId::DqnIidxapi,
            lineage_id: LineageId::OfficialInfinitasHtml,
            revision_strategy: RevisionStrategy::ContentSha256,
            parser_version: "scorepeek-dqn-live-json-parser-v1",
            declared_scope: "positive_infinitas_roster_signal",
            completeness: Completeness::NonExhaustive,
            field_authority: &["title", "artist", "pack"],
            freshness: "mutable_http_bytes_pinned_by_sha256",
            rights_and_provenance: "dqn_iidxapi_official_page_derived_local_snapshot",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionStrategy {
    GitCommit,
    ContentSha256,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Completeness {
    NonExhaustive,
}

#[derive(Clone, Debug)]
pub struct SourceSnapshot {
    pub(super) policy: SourcePolicy,
    pub(super) evidence: SourceEvidence,
    pub(crate) observations: Vec<SourceObservation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SourceEvidence {
    pub(super) source_id: SourceId,
    pub(super) lineage_id: LineageId,
    pub(super) revision_strategy: RevisionStrategy,
    pub(super) revision: String,
    pub(super) content_sha256: String,
    pub(super) byte_size: usize,
    pub(super) record_count: usize,
    pub(super) parser_version: String,
    pub(super) declared_scope: String,
    pub(super) completeness: Completeness,
    pub(super) field_authority: Vec<String>,
    pub(super) freshness: String,
    pub(super) rights_and_provenance: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct EvidenceId {
    pub(super) source_id: SourceId,
    pub(super) revision: String,
    pub(super) content_sha256: String,
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

impl SourceEvidence {
    fn id(&self) -> EvidenceId {
        EvidenceId {
            source_id: self.source_id,
            revision: self.revision.clone(),
            content_sha256: self.content_sha256.clone(),
        }
    }
    #[must_use]
    pub const fn source_id(&self) -> SourceId {
        self.source_id
    }

    #[must_use]
    pub fn revision(&self) -> &str {
        &self.revision
    }

    #[must_use]
    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }

    #[must_use]
    pub const fn record_count(&self) -> usize {
        self.record_count
    }
}

#[derive(Clone, Debug)]
pub(crate) enum SourceObservation {
    Tachi(TachiObservation),
    Textage(TextageObservation),
    Dqn(DqnObservation),
}

#[derive(Clone, Debug)]
pub(crate) struct TachiObservation {
    pub source_song_id: String,
    pub title: String,
    pub artist: String,
    pub version: String,
    pub title_kind: DisplayVariantKind,
    pub charts: Vec<SourceChartObservation>,
    pub primary_infinitas: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct TextageObservation {
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
pub(crate) struct DqnObservation {
    pub title: String,
    pub artist: String,
    pub pack: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayType {
    Single,
    Double,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    Beginner,
    Normal,
    Hyper,
    Another,
    Leggendaria,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ChartKey {
    pub play_type: PlayType,
    pub difficulty: Difficulty,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Chart {
    pub key: ChartKey,
    pub level: u8,
    pub notes: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct SourceChartObservation {
    pub chart: Chart,
    pub source_chart_id: String,
    pub product_versions: BTreeSet<String>,
    pub primary: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ScorepeekSongId(Uuid);

impl ScorepeekSongId {
    fn from_tachi_id(tachi_id: &str) -> Self {
        let namespace = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            b"https://github.com/atty303/scorepeek/song",
        );
        Self(Uuid::new_v5(&namespace, tachi_id.as_bytes()))
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }

    pub(super) const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct DisplayVariant {
    pub value: String,
    pub source_id: SourceId,
    pub kind: DisplayVariantKind,
    pub evidence_id: EvidenceId,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayVariantKind {
    InGameDisplay,
    OfficialDisplay,
    EamusementCsv,
    AlternateDisplay,
    SearchTerm,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ChartAssertion {
    pub source_chart_id: String,
    pub product_versions: BTreeSet<String>,
    pub primary: bool,
    pub evidence_id: EvidenceId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InfinitasStatus {
    ConfirmedPresent,
    Unknown,
    Conflicted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CatalogSong {
    pub(super) song_id: ScorepeekSongId,
    pub(super) tachi_source_id: String,
    pub(super) title_variants: BTreeSet<DisplayVariant>,
    pub(super) artist: String,
    pub(super) version: String,
    pub(super) charts: BTreeMap<ChartKey, Chart>,
    pub(super) chart_assertions: BTreeMap<ChartKey, BTreeSet<ChartAssertion>>,
    pub(super) infinitas_status: InfinitasStatus,
    pub(super) source_bindings: BTreeMap<SourceId, BTreeSet<String>>,
    pub(super) binding_evidence: BTreeMap<(SourceId, String), BTreeSet<EvidenceId>>,
    pub(super) binding_attributes:
        BTreeMap<(SourceId, String, EvidenceId), BTreeMap<String, String>>,
    pub(super) tachi_primary_infinitas: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct Catalog {
    pub(super) songs: BTreeMap<ScorepeekSongId, CatalogSong>,
    pub(super) source_evidence: BTreeMap<EvidenceId, SourceEvidence>,
    pub(super) latest_evidence: BTreeMap<SourceId, EvidenceId>,
    pub(super) dqn_bindings: BTreeMap<ExactTitleArtist, DqnBinding>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(super) struct DqnBinding {
    pub(super) song_id: ScorepeekSongId,
    pub(super) evidence_packs: BTreeMap<EvidenceId, BTreeSet<Option<String>>>,
}

impl CatalogSong {
    #[must_use]
    pub const fn song_id(&self) -> ScorepeekSongId {
        self.song_id
    }

    #[must_use]
    pub fn title_variants(&self) -> &BTreeSet<DisplayVariant> {
        &self.title_variants
    }

    #[must_use]
    pub const fn infinitas_status(&self) -> InfinitasStatus {
        self.infinitas_status
    }
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(super) struct ExactTitleArtist {
    pub(super) title: String,
    pub(super) artist: String,
}

impl Catalog {
    #[must_use]
    pub fn songs(&self) -> &BTreeMap<ScorepeekSongId, CatalogSong> {
        &self.songs
    }

    #[must_use]
    pub fn source_evidence(&self) -> &BTreeMap<EvidenceId, SourceEvidence> {
        &self.source_evidence
    }

    #[must_use]
    pub fn federate(&self, input: FederationInput) -> FederationOutput {
        let mut catalog = self.clone();
        let mut quarantine = Vec::new();

        apply_tachi(&mut catalog, input.tachi, &mut quarantine);
        apply_textage(&mut catalog, input.textage, &mut quarantine);
        apply_dqn(self, &mut catalog, input.dqn, &mut quarantine);
        refresh_infinitas_status(&mut catalog);
        quarantine.sort();

        FederationOutput {
            catalog,
            quarantine,
        }
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        validate_source_evidence(&self.source_evidence, &self.latest_evidence)?;
        validate_songs(&self.songs, &self.source_evidence)?;
        validate_dqn_bindings(&self.songs, &self.dqn_bindings, &self.source_evidence)
    }
}

fn validate_source_evidence(
    all_evidence: &BTreeMap<EvidenceId, SourceEvidence>,
    latest_evidence: &BTreeMap<SourceId, EvidenceId>,
) -> Result<(), String> {
    for (evidence_id, evidence) in all_evidence {
        let source_id = evidence.source_id;
        if evidence.id() != *evidence_id || evidence.record_count == 0 {
            return Err(format!("invalid source evidence for {source_id:?}"));
        }
        validate_evidence_policy(evidence)?;
        if !is_lower_hex(&evidence.content_sha256, 64) {
            return Err(format!("invalid content digest for {source_id:?}"));
        }
        let revision_length = match evidence.revision_strategy {
            RevisionStrategy::GitCommit => 40,
            RevisionStrategy::ContentSha256 => 64,
        };
        if !is_lower_hex(&evidence.revision, revision_length) {
            return Err(format!("invalid revision for {source_id:?}"));
        }
        if evidence.revision_strategy == RevisionStrategy::ContentSha256
            && evidence.revision != evidence.content_sha256
        {
            return Err(format!("content revision mismatch for {source_id:?}"));
        }
    }
    for (source_id, evidence_id) in latest_evidence {
        if evidence_id.source_id != *source_id || !all_evidence.contains_key(evidence_id) {
            return Err(format!("invalid latest evidence for {source_id:?}"));
        }
    }
    Ok(())
}

fn validate_evidence_policy(evidence: &SourceEvidence) -> Result<(), String> {
    let expected = SourcePolicy::for_id(evidence.source_id);
    if evidence.lineage_id != expected.lineage_id
        || evidence.revision_strategy != expected.revision_strategy
        || evidence.parser_version != expected.parser_version
        || evidence.declared_scope != expected.declared_scope
        || evidence.completeness != expected.completeness
        || evidence.field_authority
            != expected
                .field_authority
                .iter()
                .map(|field| (*field).to_owned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        || evidence.freshness != expected.freshness
        || evidence.rights_and_provenance != expected.rights_and_provenance
    {
        return Err(format!(
            "source policy mismatch for {:?}",
            evidence.source_id
        ));
    }
    Ok(())
}

fn validate_songs(
    songs: &BTreeMap<ScorepeekSongId, CatalogSong>,
    all_evidence: &BTreeMap<EvidenceId, SourceEvidence>,
) -> Result<(), String> {
    let mut global_bindings = BTreeSet::new();
    for (song_id, song) in songs {
        validate_song_identity(*song_id, song)?;
        validate_song_variants(*song_id, song, all_evidence)?;
        validate_song_charts(*song_id, song, all_evidence)?;
        for (source_id, bindings) in &song.source_bindings {
            if bindings.is_empty() {
                return Err(format!("song {song_id:?} has invalid source binding"));
            }
            for binding in bindings {
                validate_catalog_text("source_binding", binding)?;
                if !global_bindings.insert((*source_id, binding.clone())) {
                    return Err(format!("duplicate global source binding {binding:?}"));
                }
                let key = (*source_id, binding.clone());
                let Some(evidence_ids) = song.binding_evidence.get(&key) else {
                    return Err(format!("source binding {binding:?} lacks provenance"));
                };
                if evidence_ids.is_empty()
                    || evidence_ids
                        .iter()
                        .any(|id| id.source_id != *source_id || !all_evidence.contains_key(id))
                {
                    return Err(format!("source binding {binding:?} has invalid provenance"));
                }
            }
        }
        if song.binding_evidence.keys().any(|(source_id, source_key)| {
            !song
                .source_bindings
                .get(source_id)
                .is_some_and(|bindings| bindings.contains(source_key))
        }) {
            return Err(format!("song {song_id:?} has orphan binding provenance"));
        }
        for ((source_id, source_key, evidence_id), attributes) in &song.binding_attributes {
            if evidence_id.source_id != *source_id
                || attributes.is_empty()
                || !song
                    .binding_evidence
                    .get(&(*source_id, source_key.clone()))
                    .is_some_and(|ids| ids.contains(evidence_id))
            {
                return Err(format!("song {song_id:?} has invalid binding attributes"));
            }
            for (key, value) in attributes {
                validate_catalog_text("binding_attribute_key", key)?;
                validate_catalog_text("binding_attribute_value", value)?;
            }
        }
        validate_required_binding_attributes(*song_id, song)?;
    }
    Ok(())
}

fn validate_song_identity(song_id: ScorepeekSongId, song: &CatalogSong) -> Result<(), String> {
    if song.song_id != song_id || ScorepeekSongId::from_tachi_id(&song.tachi_source_id) != song_id {
        return Err(format!("invalid Tachi-derived song ID {song_id:?}"));
    }
    validate_catalog_text("tachi_source_id", &song.tachi_source_id)?;
    validate_catalog_text("artist", &song.artist)?;
    validate_catalog_text("version", &song.version)?;
    let tachi_bindings = song.source_bindings.get(&SourceId::Tachi);
    if !tachi_bindings
        .is_some_and(|bindings| bindings.len() == 1 && bindings.contains(&song.tachi_source_id))
    {
        return Err(format!("song {song_id:?} has invalid Tachi binding"));
    }
    Ok(())
}

fn validate_song_variants(
    song_id: ScorepeekSongId,
    song: &CatalogSong,
    all_evidence: &BTreeMap<EvidenceId, SourceEvidence>,
) -> Result<(), String> {
    if song.title_variants.is_empty() {
        return Err(format!("song {song_id:?} has no title variants"));
    }
    for variant in &song.title_variants {
        validate_catalog_text("title_variant", &variant.value)?;
        if variant.evidence_id.source_id != variant.source_id
            || !all_evidence.contains_key(&variant.evidence_id)
            || song
                .source_bindings
                .get(&variant.source_id)
                .is_none_or(BTreeSet::is_empty)
        {
            return Err(format!(
                "song {song_id:?} variant lacks source evidence {:?}",
                variant.source_id
            ));
        }
    }
    Ok(())
}

fn validate_song_charts(
    song_id: ScorepeekSongId,
    song: &CatalogSong,
    all_evidence: &BTreeMap<EvidenceId, SourceEvidence>,
) -> Result<(), String> {
    for (key, chart) in &song.charts {
        if chart.key != *key || !(1..=12).contains(&chart.level) || chart.notes == 0 {
            return Err(format!("song {song_id:?} has invalid chart {key:?}"));
        }
        let Some(assertions) = song.chart_assertions.get(key) else {
            return Err(format!("song {song_id:?} chart {key:?} lacks provenance"));
        };
        if assertions.is_empty()
            || assertions.iter().any(|assertion| {
                assertion.source_chart_id.is_empty()
                    || assertion.product_versions.is_empty()
                    || !all_evidence.contains_key(&assertion.evidence_id)
                    || song
                        .source_bindings
                        .get(&assertion.evidence_id.source_id)
                        .is_none_or(BTreeSet::is_empty)
            })
        {
            return Err(format!(
                "song {song_id:?} chart {key:?} has invalid provenance"
            ));
        }
    }
    if song
        .chart_assertions
        .keys()
        .any(|key| !song.charts.contains_key(key))
    {
        return Err(format!(
            "song {song_id:?} has provenance for an absent chart"
        ));
    }
    Ok(())
}

fn validate_dqn_bindings(
    songs: &BTreeMap<ScorepeekSongId, CatalogSong>,
    bindings: &BTreeMap<ExactTitleArtist, DqnBinding>,
    all_evidence: &BTreeMap<EvidenceId, SourceEvidence>,
) -> Result<(), String> {
    for (tuple, binding) in bindings {
        let song_id = binding.song_id;
        let Some(song) = songs.get(&song_id) else {
            return Err(format!("dqn binding references absent song {song_id:?}"));
        };
        if tuple.title != nfc(&tuple.title)
            || tuple.artist != nfc(&tuple.artist)
            || nfc(&song.artist) != tuple.artist
            || !song
                .title_variants
                .iter()
                .any(|variant| identity_variant(variant) && nfc(&variant.value) == tuple.title)
        {
            return Err(format!("invalid dqn binding for song {song_id:?}"));
        }
        if binding.evidence_packs.is_empty()
            || binding.evidence_packs.iter().any(|(id, packs)| {
                id.source_id != SourceId::DqnIidxapi
                    || !all_evidence.contains_key(id)
                    || packs.is_empty()
                    || packs.iter().any(|pack| {
                        pack.as_ref()
                            .is_some_and(|pack| validate_catalog_text("dqn_pack", pack).is_err())
                    })
            })
        {
            return Err(format!("dqn binding lacks evidence for song {song_id:?}"));
        }
    }
    let dqn_song_ids: BTreeSet<_> = bindings.values().map(|binding| binding.song_id).collect();
    for song in songs.values() {
        let tachi_primary = tachi_primary_from_attributes(song)?;
        if song.tachi_primary_infinitas != tachi_primary {
            return Err(format!(
                "song {:?} has unproven Tachi availability",
                song.song_id
            ));
        }
        let expected = if tachi_primary || dqn_song_ids.contains(&song.song_id) {
            InfinitasStatus::ConfirmedPresent
        } else {
            InfinitasStatus::Unknown
        };
        if song.infinitas_status != expected {
            return Err(format!(
                "song {:?} has inconsistent availability",
                song.song_id
            ));
        }
    }
    Ok(())
}

fn validate_required_binding_attributes(
    song_id: ScorepeekSongId,
    song: &CatalogSong,
) -> Result<(), String> {
    for ((source_id, source_key), evidence_ids) in &song.binding_evidence {
        for evidence_id in evidence_ids {
            let attributes = song
                .binding_attributes
                .get(&(*source_id, source_key.clone(), evidence_id.clone()))
                .ok_or_else(|| format!("song {song_id:?} binding lacks typed attributes"))?;
            match source_id {
                SourceId::Tachi => {
                    if attributes.len() != 1
                        || attributes
                            .get("primary_infinitas")
                            .and_then(|value| value.parse::<bool>().ok())
                            .is_none()
                    {
                        return Err(format!("song {song_id:?} has invalid Tachi attributes"));
                    }
                }
                SourceId::Textage => validate_textage_attributes(song_id, attributes)?,
                SourceId::DqnIidxapi => {
                    return Err(format!("song {song_id:?} has invalid dqn source binding"));
                }
            }
        }
    }
    Ok(())
}

fn validate_textage_attributes(
    song_id: ScorepeekSongId,
    attributes: &BTreeMap<String, String>,
) -> Result<(), String> {
    let flag = attributes
        .get("infinitas_flag")
        .and_then(|value| value.parse::<bool>().ok());
    let minimum = attributes
        .get("bpm_min")
        .and_then(|value| value.parse::<u16>().ok());
    let maximum = attributes
        .get("bpm_max")
        .and_then(|value| value.parse::<u16>().ok());
    if attributes.len() != 3
        || flag.is_none()
        || minimum.is_none_or(|value| value == 0)
        || maximum.is_none()
        || minimum > maximum
    {
        return Err(format!("song {song_id:?} has invalid Textage attributes"));
    }
    Ok(())
}

fn tachi_primary_from_attributes(song: &CatalogSong) -> Result<bool, String> {
    let evidence_ids = song
        .binding_evidence
        .get(&(SourceId::Tachi, song.tachi_source_id.clone()))
        .ok_or_else(|| format!("song {:?} lacks Tachi evidence", song.song_id))?;
    evidence_ids.iter().try_fold(false, |primary, evidence_id| {
        let value = song
            .binding_attributes
            .get(&(
                SourceId::Tachi,
                song.tachi_source_id.clone(),
                evidence_id.clone(),
            ))
            .and_then(|attributes| attributes.get("primary_infinitas"))
            .and_then(|value| value.parse::<bool>().ok())
            .ok_or_else(|| format!("song {:?} lacks Tachi availability evidence", song.song_id))?;
        Ok(primary || value)
    })
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn validate_catalog_text(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(format!("invalid {field}"));
    }
    Ok(())
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
    let evidence_id = evidence.id();
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
        existing.title_variants.insert(DisplayVariant {
            value: record.title,
            source_id: SourceId::Tachi,
            kind: record.title_kind,
            evidence_id: evidence_id.clone(),
        });
        let binding_key = record.source_song_id.clone();
        existing
            .binding_evidence
            .entry((SourceId::Tachi, binding_key.clone()))
            .or_default()
            .insert(evidence_id.clone());
        existing.binding_attributes.insert(
            (SourceId::Tachi, binding_key, evidence_id.clone()),
            BTreeMap::from([(
                "primary_infinitas".to_owned(),
                record.primary_infinitas.to_string(),
            )]),
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
            title_variants: BTreeSet::from([DisplayVariant {
                value: record.title,
                source_id: SourceId::Tachi,
                kind: record.title_kind,
                evidence_id: evidence_id.clone(),
            }]),
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
    let evidence_id = evidence.id();
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
        song.title_variants.insert(DisplayVariant {
            value: record.title,
            source_id: SourceId::Textage,
            kind: record.title_kind,
            evidence_id: evidence_id.clone(),
        });
        song.source_bindings
            .entry(SourceId::Textage)
            .or_default()
            .insert(record.source_song_id.clone());
        song.binding_evidence
            .entry((SourceId::Textage, record.source_song_id.clone()))
            .or_default()
            .insert(evidence_id.clone());
        song.binding_attributes.insert(
            (
                SourceId::Textage,
                record.source_song_id.clone(),
                evidence_id.clone(),
            ),
            BTreeMap::from([
                (
                    "infinitas_flag".to_owned(),
                    record.infinitas_flag.to_string(),
                ),
                ("bpm_min".to_owned(), record.bpm_min.to_string()),
                ("bpm_max".to_owned(), record.bpm_max.to_string()),
            ]),
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
    let evidence_id = evidence.id();
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
        song.chart_assertions
            .entry(key)
            .or_default()
            .insert(ChartAssertion {
                source_chart_id: observation.source_chart_id,
                product_versions: observation.product_versions,
                primary: observation.primary,
                evidence_id: evidence_id.clone(),
            });
    }
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
