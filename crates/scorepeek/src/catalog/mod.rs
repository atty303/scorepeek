mod acquisition;
mod adapter;
mod federation;
mod store;
mod sync;

pub use acquisition::DqnAcquisitionError;
pub use adapter::{
    AdapterError, DqnLiveAdapter, SourceRevision, TachiFixtureAdapter, TextageFixtureAdapter,
};
pub use federation::{
    Catalog, CatalogSong, Chart, ChartAssertion, ChartKey, Difficulty, DisplayVariant,
    DisplayVariantKind, EvidenceId, FederationInput, FederationOutput, InfinitasStatus, LineageId,
    PlayType, QuarantineEntry, QuarantineReason, RevisionStrategy, ScorepeekSongId, SourceEvidence,
    SourceId, SourcePolicy, SourceSnapshot,
};
pub use store::{ActiveCatalog, CatalogStore, CatalogStoreError, CatalogUpdate};
pub use sync::{CatalogSync, CatalogSyncError, CatalogSyncResult, CatalogSyncSummary};

#[cfg(test)]
mod tests;
