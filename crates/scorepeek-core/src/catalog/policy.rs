//! Source admission and revision policy.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceId {
    Tachi,
    Textage,
    DqnIidxapi,
}

impl SourceId {
    pub const COUNT: usize = 3;
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LineageId {
    GameMdb,
    Textage,
    OfficialInfinitasHtml,
}

impl LineageId {
    pub const COUNT: usize = 3;
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
    #[must_use]
    pub const fn for_id(source_id: SourceId) -> Self {
        match source_id {
            SourceId::Tachi => Self::tachi(),
            SourceId::Textage => Self::textage(),
            SourceId::DqnIidxapi => Self::dqn(),
        }
    }

    #[must_use]
    pub const fn tachi() -> Self {
        Self {
            source_id: SourceId::Tachi,
            lineage_id: LineageId::GameMdb,
            revision_strategy: RevisionStrategy::GitCommit,
            parser_version: "scorepeek-tachi-live-json-parser-v1",
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

    #[must_use]
    pub const fn textage() -> Self {
        Self {
            source_id: SourceId::Textage,
            lineage_id: LineageId::Textage,
            revision_strategy: RevisionStrategy::ContentSha256,
            parser_version: "scorepeek-textage-live-js-parser-v1",
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

    #[must_use]
    pub const fn dqn() -> Self {
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
