use crate::recognition::ScreenClass;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawScreenState {
    Known(ScreenClass),
    Unknown,
}

impl From<ScreenClass> for RawScreenState {
    fn from(value: ScreenClass) -> Self {
        match value {
            ScreenClass::Unknown => Self::Unknown,
            known => Self::Known(known),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SemanticScreenEpisode {
    pub id: u64,
    pub screen: ScreenClass,
    pub started_sequence: u64,
    pub started_ms: u64,
    pub last_sequence: u64,
    pub last_ms: u64,
    pub suspended: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreenEpisodeTransition {
    None,
    Started(SemanticScreenEpisode),
    Suspended(SemanticScreenEpisode),
    Resumed(SemanticScreenEpisode),
    Continued(SemanticScreenEpisode),
    Replaced {
        closed: SemanticScreenEpisode,
        started: SemanticScreenEpisode,
    },
    ChronologyReset {
        closed: SemanticScreenEpisode,
        started: Option<SemanticScreenEpisode>,
    },
}

#[derive(Clone, Debug, Default)]
pub struct ScreenEpisodeResolver {
    next_id: u64,
    active: Option<SemanticScreenEpisode>,
}

impl ScreenEpisodeResolver {
    #[must_use]
    pub const fn active(&self) -> Option<SemanticScreenEpisode> {
        self.active
    }

    pub fn observe(
        &mut self,
        raw: RawScreenState,
        sequence: u64,
        monotonic_ms: u64,
    ) -> ScreenEpisodeTransition {
        if let Some(active) = self.active
            && (sequence < active.last_sequence || monotonic_ms < active.last_ms)
        {
            self.active = None;
            let started = match raw {
                RawScreenState::Known(screen) => Some(self.start(screen, sequence, monotonic_ms)),
                RawScreenState::Unknown => None,
            };
            return ScreenEpisodeTransition::ChronologyReset {
                closed: active,
                started,
            };
        }

        match (self.active, raw) {
            (None, RawScreenState::Unknown) => ScreenEpisodeTransition::None,
            (None, RawScreenState::Known(screen)) => {
                ScreenEpisodeTransition::Started(self.start(screen, sequence, monotonic_ms))
            }
            (Some(mut active), RawScreenState::Unknown) => {
                active.last_sequence = sequence;
                active.last_ms = monotonic_ms;
                if active.suspended {
                    self.active = Some(active);
                    ScreenEpisodeTransition::Continued(active)
                } else {
                    active.suspended = true;
                    self.active = Some(active);
                    ScreenEpisodeTransition::Suspended(active)
                }
            }
            (Some(mut active), RawScreenState::Known(screen)) if active.screen == screen => {
                let resumed = active.suspended;
                active.last_sequence = sequence;
                active.last_ms = monotonic_ms;
                active.suspended = false;
                self.active = Some(active);
                if resumed {
                    ScreenEpisodeTransition::Resumed(active)
                } else {
                    ScreenEpisodeTransition::Continued(active)
                }
            }
            (Some(active), RawScreenState::Known(screen)) => {
                let started = self.start(screen, sequence, monotonic_ms);
                ScreenEpisodeTransition::Replaced {
                    closed: active,
                    started,
                }
            }
        }
    }

    pub fn finish(&mut self) -> Option<SemanticScreenEpisode> {
        self.active.take()
    }

    fn start(
        &mut self,
        screen: ScreenClass,
        sequence: u64,
        monotonic_ms: u64,
    ) -> SemanticScreenEpisode {
        self.next_id = self.next_id.saturating_add(1);
        let episode = SemanticScreenEpisode {
            id: self.next_id,
            screen,
            started_sequence: sequence,
            started_ms: monotonic_ms,
            last_sequence: sequence,
            last_ms: monotonic_ms,
            suspended: false,
        };
        self.active = Some(episode);
        episode
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_suspends_and_same_known_screen_resumes_the_episode() {
        let mut resolver = ScreenEpisodeResolver::default();
        let ScreenEpisodeTransition::Started(started) =
            resolver.observe(ScreenClass::Result.into(), 1, 100)
        else {
            panic!("result starts an episode");
        };
        assert!(matches!(
            resolver.observe(ScreenClass::Unknown.into(), 2, 200),
            ScreenEpisodeTransition::Suspended(_)
        ));
        let ScreenEpisodeTransition::Resumed(resumed) =
            resolver.observe(ScreenClass::Result.into(), 3, 300)
        else {
            panic!("result resumes the episode");
        };
        assert_eq!(resumed.id, started.id);
    }

    #[test]
    fn next_known_screen_replaces_the_suspended_episode() {
        let mut resolver = ScreenEpisodeResolver::default();
        resolver.observe(ScreenClass::Result.into(), 1, 100);
        resolver.observe(ScreenClass::Unknown.into(), 2, 200);
        let ScreenEpisodeTransition::Replaced { closed, started } =
            resolver.observe(ScreenClass::MusicSelect.into(), 3, 300)
        else {
            panic!("different known screen replaces the episode");
        };
        assert_eq!(closed.screen, ScreenClass::Result);
        assert_eq!(started.screen, ScreenClass::MusicSelect);
        assert_ne!(closed.id, started.id);
    }

    #[test]
    fn time_reversal_closes_instead_of_merging_evidence() {
        let mut resolver = ScreenEpisodeResolver::default();
        resolver.observe(ScreenClass::Result.into(), 2, 200);
        assert!(matches!(
            resolver.observe(ScreenClass::Result.into(), 1, 100),
            ScreenEpisodeTransition::ChronologyReset { .. }
        ));
    }
}
