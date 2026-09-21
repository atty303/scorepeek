//! Stable domain and run event authority.

pub mod domain;
pub mod projection;
pub mod run;
pub mod schema;

pub use run::RunEventEnvelope;
pub use schema::RUN_EVENT_SCHEMA;
