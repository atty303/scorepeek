//! Portable score facts, event decoding, `SQLite` storage, migration, and queries.

mod error;
pub mod event;
pub mod facts;
pub mod migration;
pub mod query;
pub mod store;

pub use error::Error;
pub use facts::PlaySide;
pub use store::Store;
