//! Portable diagnostic contracts; transports remain runtime-owned.

pub mod binding;
pub mod policy;
pub mod record;
pub mod schema;

pub use binding::{
    DiagnosticBinding, DiagnosticReplayBinding, DiagnosticResource, DiagnosticRunDescriptor,
};
pub use policy::{
    DEFAULT_AGGREGATE_BYTES, DEFAULT_SAMPLE_INTERVAL_MS, DiagnosticPolicy, DiagnosticRetention,
    NORMAL_RETENTION_HOURS, PRIORITY_RETENTION_HOURS,
};
pub use record::{
    DiagnosticCompleteness, DiagnosticContextChange, DiagnosticDecisionDomain,
    DiagnosticDecisionOutcome, DiagnosticDetail, DiagnosticErrorType, DiagnosticEventKind,
    DiagnosticEventOutcome, DiagnosticFact, DiagnosticFactErrorType, DiagnosticOperation,
    DiagnosticOperationStatus, DiagnosticRunStatus, DiagnosticScreen, DiagnosticTextField,
    FrameFieldStatus, RecognitionSamplingSummary,
};
pub use schema::{
    ARTIFACT_SCHEMA, BINDING_IDENTITY_SCHEMA, CAPTURE_MANIFEST_SCHEMA, CAPTURE_START_SCHEMA,
    FACT_SCHEMA,
};
