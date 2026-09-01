use crate::screen_episode::{
    RawScreenState, ScreenEpisodeResolver, ScreenEpisodeTransition, SemanticScreenEpisode,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticEpisodePhase {
    Started,
    Suspended,
    Resumed,
    Closing,
    Finalized,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimelineAction {
    Semantic {
        episode: SemanticScreenEpisode,
        phase: SemanticEpisodePhase,
    },
    DrainAdmitted {
        episode: SemanticScreenEpisode,
    },
}

#[derive(Debug)]
pub struct TimelineStep {
    pub active_episode_id: Option<u64>,
    pub actions: Vec<TimelineAction>,
}

#[derive(Default)]
pub struct TimelineDriver {
    resolver: ScreenEpisodeResolver,
}

impl TimelineDriver {
    #[must_use]
    pub fn observe(
        &mut self,
        raw: RawScreenState,
        sequence: u64,
        monotonic_ms: u64,
    ) -> TimelineStep {
        let transition = self.resolver.observe(raw, sequence, monotonic_ms);
        TimelineStep {
            active_episode_id: self.resolver.active().map(|episode| episode.id),
            actions: transition_actions(transition),
        }
    }

    #[must_use]
    pub fn finish(&mut self) -> Vec<TimelineAction> {
        self.resolver.finish().map_or_else(Vec::new, |episode| {
            vec![
                TimelineAction::Semantic {
                    episode,
                    phase: SemanticEpisodePhase::Closing,
                },
                TimelineAction::DrainAdmitted { episode },
                TimelineAction::Semantic {
                    episode,
                    phase: SemanticEpisodePhase::Finalized,
                },
            ]
        })
    }

    #[must_use]
    pub fn active_episode_id(&self) -> Option<u64> {
        self.resolver.active().map(|episode| episode.id)
    }
}

fn transition_actions(transition: ScreenEpisodeTransition) -> Vec<TimelineAction> {
    use SemanticEpisodePhase::{Closing, Finalized, Resumed, Started, Suspended};
    use TimelineAction::{DrainAdmitted, Semantic};
    match transition {
        ScreenEpisodeTransition::None | ScreenEpisodeTransition::Continued(_) => Vec::new(),
        ScreenEpisodeTransition::Started(episode) => vec![Semantic {
            episode,
            phase: Started,
        }],
        ScreenEpisodeTransition::Suspended(episode) => vec![Semantic {
            episode,
            phase: Suspended,
        }],
        ScreenEpisodeTransition::Resumed(episode) => vec![Semantic {
            episode,
            phase: Resumed,
        }],
        ScreenEpisodeTransition::Replaced { closed, started } => vec![
            Semantic {
                episode: closed,
                phase: Closing,
            },
            DrainAdmitted { episode: closed },
            Semantic {
                episode: closed,
                phase: Finalized,
            },
            Semantic {
                episode: started,
                phase: Started,
            },
        ],
        ScreenEpisodeTransition::ChronologyReset { closed, started } => {
            let mut actions = vec![
                Semantic {
                    episode: closed,
                    phase: Closing,
                },
                DrainAdmitted { episode: closed },
                Semantic {
                    episode: closed,
                    phase: Finalized,
                },
            ];
            if let Some(episode) = started {
                actions.push(Semantic {
                    episode,
                    phase: Started,
                });
            }
            actions
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognition::ScreenClass;

    #[test]
    fn replacement_has_one_close_drain_finalize_start_order() {
        let mut driver = TimelineDriver::default();
        let _ = driver.observe(RawScreenState::Known(ScreenClass::MusicSelect), 1, 100);
        let step = driver.observe(RawScreenState::Known(ScreenClass::Play), 2, 200);
        assert_eq!(step.actions.len(), 4);
        assert!(matches!(
            step.actions.as_slice(),
            [
                TimelineAction::Semantic {
                    phase: SemanticEpisodePhase::Closing,
                    ..
                },
                TimelineAction::DrainAdmitted { .. },
                TimelineAction::Semantic {
                    phase: SemanticEpisodePhase::Finalized,
                    ..
                },
                TimelineAction::Semantic {
                    phase: SemanticEpisodePhase::Started,
                    ..
                }
            ]
        ));
    }

    #[test]
    fn unknown_suspends_and_same_screen_resumes_the_same_episode() {
        let mut driver = TimelineDriver::default();
        let started = driver.observe(RawScreenState::Known(ScreenClass::Result), 1, 100);
        let episode_id = started.active_episode_id.unwrap();
        let suspended = driver.observe(RawScreenState::Unknown, 2, 200);
        assert_eq!(suspended.active_episode_id, Some(episode_id));
        assert!(matches!(
            suspended.actions.as_slice(),
            [TimelineAction::Semantic {
                phase: SemanticEpisodePhase::Suspended,
                ..
            }]
        ));
        let resumed = driver.observe(RawScreenState::Known(ScreenClass::Result), 3, 300);
        assert_eq!(resumed.active_episode_id, Some(episode_id));
        assert!(matches!(
            resumed.actions.as_slice(),
            [TimelineAction::Semantic {
                episode: SemanticScreenEpisode {
                    screen: ScreenClass::Result,
                    ..
                },
                phase: SemanticEpisodePhase::Resumed,
            }]
        ));
    }
}
