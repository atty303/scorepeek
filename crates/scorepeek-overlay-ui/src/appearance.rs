use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    collections::HashSet,
    sync::{LazyLock, Mutex},
};

static SKIN_IDS: LazyLock<Mutex<HashSet<&'static str>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// An installed skin's reverse-domain identity. Values are interned for the
/// process lifetime so canvas presentations remain cheap and copyable.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Skin(&'static str);

impl Skin {
    pub const CYAN_SYSTEM_ID: &'static str = "dev.atty303.scorepeek.skin.cyan-system";
    pub const RESULT_AURORA_ID: &'static str = "dev.atty303.scorepeek.skin.result-aurora";
    pub const DJ_BLACKBOX_ID: &'static str = "dev.atty303.scorepeek.skin.dj-blackbox";
    #[allow(non_upper_case_globals)]
    pub const CyanSystem: Self = Self(Self::CYAN_SYSTEM_ID);
    #[allow(non_upper_case_globals)]
    pub const ResultAurora: Self = Self(Self::RESULT_AURORA_ID);
    #[allow(non_upper_case_globals)]
    pub const DjBlackbox: Self = Self(Self::DJ_BLACKBOX_ID);

    #[must_use]
    pub const fn name(self) -> &'static str {
        self.0
    }

    #[must_use]
    pub fn graph_colors(self) -> GraphColors {
        match self.0 {
            Self::RESULT_AURORA_ID => GraphColors {
                score: "#d368f2",
                miss: "#ffd334",
            },
            Self::DJ_BLACKBOX_ID => GraphColors {
                score: "#b9e92b",
                miss: "#ffad00",
            },
            _ => GraphColors {
                score: "#10dff4",
                miss: "#ffc719",
            },
        }
    }
}

impl Default for Skin {
    fn default() -> Self {
        Self::CyanSystem
    }
}

impl Serialize for Skin {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for Skin {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl std::str::FromStr for Skin {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            return Err("skin id must be non-empty".into());
        }
        Ok(match value {
            "cyan-system" | Self::CYAN_SYSTEM_ID => Self::CyanSystem,
            "result-aurora" | Self::RESULT_AURORA_ID => Self::ResultAurora,
            "dj-blackbox" | Self::DJ_BLACKBOX_ID => Self::DjBlackbox,
            value => {
                let mut ids = SKIN_IDS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(existing) = ids.get(value) {
                    Self(existing)
                } else {
                    let value = Box::leak(value.to_owned().into_boxed_str());
                    ids.insert(value);
                    Self(value)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Skin;

    #[test]
    fn repeated_dynamic_ids_reuse_the_process_interned_value() {
        let first: Skin = "dev.example.dynamic".parse().unwrap();
        let second: Skin = "dev.example.dynamic".parse().unwrap();
        assert!(std::ptr::eq(first.name(), second.name()));
    }
}

#[derive(Clone, Copy)]
pub struct GraphColors {
    pub score: &'static str,
    pub miss: &'static str,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Appearance {
    pub skin: Skin,
}
