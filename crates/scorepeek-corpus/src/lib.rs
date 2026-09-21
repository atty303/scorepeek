//! Private corpus manifest, storage, ingest, replay, derivation, and evaluation.

#[cfg(feature = "runtime-replay")]
mod cli;
pub mod derive;
pub mod evaluation;
pub mod ingest;
pub mod manifest;
pub mod replay;
pub mod store;

#[cfg(feature = "runtime-replay")]
pub use cli::operation_main;
#[cfg(feature = "runtime-replay")]
pub use derive::motion::*;
pub use derive::regions::*;
#[cfg(feature = "runtime-replay")]
pub use evaluation::session::*;
pub use ingest::recording::*;
#[cfg(feature = "runtime-replay")]
pub use replay::oracle::*;
pub use store::*;
