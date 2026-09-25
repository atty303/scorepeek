//! Error contract shared by score event decoding, fact integration, and storage.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Sql(rusqlite::Error),
    Json(serde_json::Error),
    UnsupportedContract,
    UnsupportedDatabase(i64),
    Migration(&'static str, Box<Error>),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "scores filesystem: {error}"),
            Self::Sql(error) => write!(f, "scores database: {error}"),
            Self::Json(error) => write!(f, "scores event: {error}"),
            Self::UnsupportedContract => f.write_str("unsupported scores event contract"),
            Self::UnsupportedDatabase(version) => {
                write!(f, "unsupported scores database version {version}")
            }
            Self::Migration(stage, cause) => write!(f, "scores migration {stage} failed: {cause}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sql(error)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
