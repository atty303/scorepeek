//! Cross-source federation inputs, outputs, and quarantine results.

use serde::Serialize;

use super::model::{Catalog, SourceSnapshot};
use super::policy::SourceId;

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
