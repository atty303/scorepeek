//! Portable score facts, event decoding, `SQLite` storage, migration, and queries.

pub mod event;
pub mod facts;
pub mod migration;
pub mod query;
pub mod store;

pub use facts::PlaySide;
pub use store::{Error, Store};
