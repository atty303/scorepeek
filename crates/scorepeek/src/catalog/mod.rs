mod acquisition;
mod adapter;
mod federation;
mod store;
mod sync;
mod tachi_acquisition;
mod textage_acquisition;
mod textage_adapter;

pub use acquisition::DqnAcquisitionError;
pub use adapter::{
    AdapterError, DqnLiveAdapter, SourceRevision, TachiFixtureAdapter, TachiLiveAdapter,
    TextageFixtureAdapter,
};
pub use federation::{
    Catalog, CatalogSong, Chart, ChartAssertion, ChartKey, Difficulty, DisplayVariant,
    DisplayVariantKind, EvidenceId, FederationInput, FederationOutput, InfinitasStatus, LineageId,
    PlayType, QuarantineEntry, QuarantineReason, RevisionStrategy, ScorepeekSongId, SourceEvidence,
    SourceId, SourcePolicy, SourceSnapshot,
};
pub use store::{ActiveCatalog, CatalogStore, CatalogStoreError, CatalogUpdate};
pub use sync::{
    CatalogSync, CatalogSyncError, CatalogSyncResult, CatalogSyncSource, CatalogSyncSummary,
};
pub use tachi_acquisition::{TachiAcquisitionError, TachiResource};
pub use textage_acquisition::{TextageAcquisitionError, TextageResource};

#[cfg(test)]
mod tests;
