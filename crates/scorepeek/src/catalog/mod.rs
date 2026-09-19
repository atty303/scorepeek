pub mod artifact;
mod store;
#[cfg(test)]
pub(crate) mod test_support;
pub mod update;

mod federation {
    pub use scorepeek_catalog::*;
}

pub use federation::{
    Catalog, CatalogSong, Chart, ChartAssertion, ChartKey, Difficulty, DisplayVariant,
    DisplayVariantKind, DqnObservation, EvidenceId, InfinitasStatus, LineageId, PlayType,
    RevisionStrategy, ScorepeekSongId, SourceChartObservation, SourceEvidence, SourceId,
    SourceObservation, SourcePolicy, SourceSnapshot, SourceTitleObservation, TachiObservation,
    TextageObservation,
};
pub use store::{ActiveCatalog, CatalogOrigin, CatalogStore, CatalogStoreError, CatalogUpdate};
