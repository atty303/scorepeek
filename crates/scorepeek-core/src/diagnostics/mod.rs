//! Portable diagnostic contracts; transports remain runtime-owned.

pub mod binding;
pub mod policy;
pub mod record;
pub mod schema;

pub use binding::DiagnosticBinding;
pub use policy::DiagnosticPolicy;
pub use record::DiagnosticRecord;
