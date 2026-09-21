//! Typed score fact identities.

use serde::{Deserialize, Serialize};

use super::store::Error;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaySide {
    OnePlayer,
    TwoPlayer,
}

impl PlaySide {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::OnePlayer => "one_player",
            Self::TwoPlayer => "two_player",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, Error> {
        match value {
            "one_player" => Ok(Self::OnePlayer),
            "two_player" => Ok(Self::TwoPlayer),
            _ => Err(Error::UnsupportedContract),
        }
    }
}
