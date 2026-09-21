//! Portable diagnostic contracts; transports remain runtime-owned.

pub mod binding;
pub mod policy;
pub mod record;
pub mod schema;

pub use binding::{
    DiagnosticBinding, DiagnosticReplayBinding, DiagnosticResource, DiagnosticRunDescriptor,
};
pub use policy::DiagnosticPolicy;
pub use record::DiagnosticRecord;
