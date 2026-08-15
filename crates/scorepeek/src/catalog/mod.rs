mod adapter;
mod federation;
mod store;

pub use adapter::{
    AdapterError, DqnFixtureAdapter, SourceRevision, TachiFixtureAdapter, TextageFixtureAdapter,
};
pub use federation::{
    Catalog, CatalogSong, Chart, ChartAssertion, ChartKey, Difficulty, DisplayVariant,
    DisplayVariantKind, EvidenceId, FederationInput, FederationOutput, InfinitasStatus, LineageId,
    PlayType, QuarantineEntry, QuarantineReason, RevisionStrategy, ScorepeekSongId, SourceEvidence,
    SourceId, SourcePolicy, SourceSnapshot,
};
pub use store::{ActiveCatalog, CatalogStore, CatalogStoreError, CatalogUpdate};

#[cfg(test)]
mod tests;
