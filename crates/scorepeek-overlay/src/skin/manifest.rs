use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    collections::HashSet,
    sync::{LazyLock, Mutex},
};

static SKIN_IDS: LazyLock<Mutex<HashSet<&'static str>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// An installed skin's reverse-domain identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Skin(&'static str);

impl Skin {
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.0
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
        crate::validate_skin_id(value)?;
        let mut ids = SKIN_IDS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = ids.get(value) {
            Ok(Self(existing))
        } else {
            let value = Box::leak(value.to_owned().into_boxed_str());
            ids.insert(value);
            Ok(Self(value))
        }
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
