//! Runtime transport for the public event stream.

mod client;
mod debug_projection;
mod domain_projection;
mod input;
pub mod server;
pub mod snapshot;

mod projection;
pub(crate) mod run;
mod schema;
pub(crate) use projection::{diagnostic_run_event_value, run_event_from_field_observation};
pub(crate) use run::{RunEvent, RunEventKind, SongResolutionPresentation};
pub(crate) use schema::RUN_EVENT_SCHEMA;
