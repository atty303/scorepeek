//! Portable numeric-model input contracts.

use serde::{Deserialize, Serialize};

pub const NUMERIC_DICTIONARY: &str = "0123456789-";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NumericField {
    Level,
    Notes,
    CurrentScore,
    PreviousScore,
    PreviousMissCount,
    MissCount,
    Pgreat,
    Great,
    Good,
    Bad,
    Poor,
    Fast,
    Slow,
    ComboBreak,
}

impl NumericField {
    pub const ALL: [Self; 14] = [
        Self::Level,
        Self::Notes,
        Self::CurrentScore,
        Self::PreviousScore,
        Self::PreviousMissCount,
        Self::MissCount,
        Self::Pgreat,
        Self::Great,
        Self::Good,
        Self::Bad,
        Self::Poor,
        Self::Fast,
        Self::Slow,
        Self::ComboBreak,
    ];

    #[must_use]
    pub const fn maximum_digits(self) -> usize {
        match self {
            Self::Level => 2,
            Self::ComboBreak => 3,
            _ => 4,
        }
    }

    #[must_use]
    pub const fn allows_dash(self) -> bool {
        matches!(
            self,
            Self::PreviousScore
                | Self::PreviousMissCount
                | Self::MissCount
                | Self::Fast
                | Self::Slow
                | Self::ComboBreak
        )
    }

    #[must_use]
    pub const fn allows_leading_zeroes(self) -> bool {
        matches!(self, Self::Notes)
    }
}
