use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

const REQUIRED_OBSERVATIONS: u8 = 3;
const VERSION_LENGTH: usize = 20;
const COLON_OFFSETS: [usize; 4] = [3, 5, 7, 9];

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "version", rename_all = "snake_case")]
pub enum GameVersionState {
    Identified(String),
    NotObserved,
    Ambiguous,
    ObserverFailed,
}

#[derive(Deserialize)]
#[serde(tag = "status", content = "version", rename_all = "snake_case")]
enum UncheckedGameVersionState {
    Identified(String),
    NotObserved,
    Ambiguous,
    ObserverFailed,
}

impl<'de> Deserialize<'de> for GameVersionState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match UncheckedGameVersionState::deserialize(deserializer)? {
            UncheckedGameVersionState::Identified(version) if valid_game_version(&version) => {
                Ok(Self::Identified(version))
            }
            UncheckedGameVersionState::Identified(_) => Err(D::Error::custom(
                "identified game version is structurally invalid",
            )),
            UncheckedGameVersionState::NotObserved => Ok(Self::NotObserved),
            UncheckedGameVersionState::Ambiguous => Ok(Self::Ambiguous),
            UncheckedGameVersionState::ObserverFailed => Ok(Self::ObserverFailed),
        }
    }
}

#[derive(Debug, Default)]
pub struct GameVersionResolver {
    last_sequence: Option<u64>,
    candidate: Option<String>,
    consecutive: u8,
    saw_valid: bool,
    observer_failed: bool,
    identified: Option<String>,
}

impl GameVersionResolver {
    #[must_use]
    pub fn identified(&self) -> Option<&str> {
        self.identified.as_deref()
    }

    pub fn observe_candidate(&mut self, sequence: u64, value: &str) -> Option<&str> {
        if self.identified.is_some() || self.last_sequence.is_some_and(|last| sequence <= last) {
            return None;
        }
        self.last_sequence = Some(sequence);
        if !valid_game_version(value) {
            self.candidate = None;
            self.consecutive = 0;
            return None;
        }
        self.saw_valid = true;
        if self.candidate.as_deref() == Some(value) {
            self.consecutive = self.consecutive.saturating_add(1);
        } else {
            self.candidate = Some(value.to_owned());
            self.consecutive = 1;
        }
        if self.consecutive == REQUIRED_OBSERVATIONS {
            self.identified = self.candidate.clone();
            return self.identified.as_deref();
        }
        None
    }

    pub fn observe_failure(&mut self, sequence: u64) {
        if self.identified.is_some() || self.last_sequence.is_some_and(|last| sequence <= last) {
            return;
        }
        self.last_sequence = Some(sequence);
        self.candidate = None;
        self.consecutive = 0;
        self.observer_failed = true;
    }

    #[must_use]
    pub fn state(&self) -> GameVersionState {
        if let Some(version) = &self.identified {
            GameVersionState::Identified(version.clone())
        } else if self.observer_failed {
            GameVersionState::ObserverFailed
        } else if self.saw_valid {
            GameVersionState::Ambiguous
        } else {
            GameVersionState::NotObserved
        }
    }
}

#[must_use]
pub fn valid_game_version(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == VERSION_LENGTH
        && bytes.iter().enumerate().all(|(index, byte)| {
            if COLON_OFFSETS.contains(&index) {
                *byte == b':'
            } else {
                byte.is_ascii_alphanumeric()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_only_the_complete_structural_contract() {
        assert!(valid_game_version("P2D:J:B:A:2026080500"));
        assert!(valid_game_version("ABC:1:2:Z:ABCDEFGHIJ"));
        assert!(!valid_game_version("P2D:J:B:A:202608050"));
        assert!(!valid_game_version("P2D-J:B:A:2026080500"));
        assert!(!valid_game_version("P2D:J:B:A:2026_80500"));
        assert!(
            serde_json::from_str::<GameVersionState>(r#"{"status":"identified","version":"bad"}"#)
                .is_err()
        );
    }

    #[test]
    fn identifies_three_equal_values_on_distinct_sequences() {
        let mut resolver = GameVersionResolver::default();
        assert_eq!(resolver.observe_candidate(10, "P2D:J:B:A:2026080500"), None);
        assert_eq!(resolver.observe_candidate(10, "P2D:J:B:A:2026080500"), None);
        assert_eq!(resolver.observe_candidate(11, "P2D:J:B:A:2026080500"), None);
        assert_eq!(
            resolver.observe_candidate(12, "P2D:J:B:A:2026080500"),
            Some("P2D:J:B:A:2026080500")
        );
        assert_eq!(
            resolver.state(),
            GameVersionState::Identified("P2D:J:B:A:2026080500".to_owned())
        );
    }

    #[test]
    fn another_valid_value_resets_the_run() {
        let mut resolver = GameVersionResolver::default();
        let first = "P2D:J:B:A:2026080500";
        let second = "P2D:J:B:A:2026080600";
        assert_eq!(resolver.observe_candidate(1, first), None);
        assert_eq!(resolver.observe_candidate(2, first), None);
        assert_eq!(resolver.observe_candidate(3, second), None);
        assert_eq!(resolver.observe_candidate(4, second), None);
        assert_eq!(resolver.observe_candidate(5, second), Some(second));
    }
}
