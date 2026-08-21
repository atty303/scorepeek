use std::collections::BTreeSet;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SongCandidates<S: Ord>(BTreeSet<S>);

impl<S: Ord> SongCandidates<S> {
    /// Constructs a non-empty set of screen-local song candidates.
    ///
    /// # Errors
    /// Returns [`SongCandidatesError::Empty`] when the screen-local resolver did not retain any
    /// candidate. Callers must preserve that resolver's typed unknown reason instead.
    pub fn new(candidates: impl IntoIterator<Item = S>) -> Result<Self, SongCandidatesError> {
        let candidates: BTreeSet<S> = candidates.into_iter().collect();
        if candidates.is_empty() {
            return Err(SongCandidatesError::Empty);
        }
        Ok(Self(candidates))
    }

    pub fn iter(&self) -> impl Iterator<Item = &S> {
        self.0.iter()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SongCandidatesError {
    Empty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SongContextClearReason {
    TitleObserved,
    SessionEnded,
    CoverageGap,
    RecognitionBindingChanged,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SongContextObservation<S: Ord> {
    StableSelection(SongCandidates<S>),
    Preserve,
    Clear(SongContextClearReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SongContextChange {
    Replaced,
    Preserved,
    Cleared,
    AlreadyEmpty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextualSongEvidence {
    ResultOnly,
    ResultAndStableSelection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextualSongUnknownReason {
    AmbiguousResult,
    SelectionConflict,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextualSongDecision<S> {
    Accepted {
        song_id: S,
        evidence: ContextualSongEvidence,
    },
    Unknown {
        reason: ContextualSongUnknownReason,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SongContext<S: Ord> {
    stable_selection: Option<SongCandidates<S>>,
}

impl<S: Ord> Default for SongContext<S> {
    fn default() -> Self {
        Self {
            stable_selection: None,
        }
    }
}

impl<S: Ord> SongContext<S> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&mut self, observation: SongContextObservation<S>) -> SongContextChange {
        match observation {
            SongContextObservation::StableSelection(candidates) => {
                self.stable_selection = Some(candidates);
                SongContextChange::Replaced
            }
            SongContextObservation::Preserve => SongContextChange::Preserved,
            SongContextObservation::Clear(_) => {
                if self.stable_selection.take().is_some() {
                    SongContextChange::Cleared
                } else {
                    SongContextChange::AlreadyEmpty
                }
            }
        }
    }

    #[must_use]
    pub fn stable_selection(&self) -> Option<&SongCandidates<S>> {
        self.stable_selection.as_ref()
    }

    #[must_use]
    pub fn resolve_result(&self, result: &SongCandidates<S>) -> ContextualSongDecision<S>
    where
        S: Clone,
    {
        let Some(selection) = &self.stable_selection else {
            return unique_result(result).unwrap_or(ContextualSongDecision::Unknown {
                reason: ContextualSongUnknownReason::AmbiguousResult,
            });
        };
        let intersection: Vec<_> = result.0.intersection(&selection.0).cloned().collect();
        match intersection.as_slice() {
            [song_id] => ContextualSongDecision::Accepted {
                song_id: song_id.clone(),
                evidence: ContextualSongEvidence::ResultAndStableSelection,
            },
            [] => ContextualSongDecision::Unknown {
                reason: ContextualSongUnknownReason::SelectionConflict,
            },
            _ => ContextualSongDecision::Unknown {
                reason: ContextualSongUnknownReason::AmbiguousResult,
            },
        }
    }
}

fn unique_result<S: Clone + Ord>(result: &SongCandidates<S>) -> Option<ContextualSongDecision<S>> {
    let mut candidates = result.iter();
    let song_id = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }
    Some(ContextualSongDecision::Accepted {
        song_id: song_id.clone(),
        evidence: ContextualSongEvidence::ResultOnly,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates(values: &[u8]) -> SongCandidates<u8> {
        SongCandidates::new(values.iter().copied()).unwrap()
    }

    #[test]
    fn ordinary_standard_session_uses_only_selection_context() {
        let mut context = SongContext::new();

        context.observe(SongContextObservation::StableSelection(candidates(&[1, 2])));
        for _scene in ["transition", "gameplay", "transition"] {
            context.observe(SongContextObservation::Preserve);
        }
        assert_eq!(
            context.resolve_result(&candidates(&[2, 3])),
            ContextualSongDecision::Accepted {
                song_id: 2,
                evidence: ContextualSongEvidence::ResultAndStableSelection,
            }
        );

        for _scene in ["result replay transition", "gameplay retry", "transition"] {
            context.observe(SongContextObservation::Preserve);
        }
        assert_eq!(
            context.resolve_result(&candidates(&[2, 4])),
            ContextualSongDecision::Accepted {
                song_id: 2,
                evidence: ContextualSongEvidence::ResultAndStableSelection,
            }
        );

        context.observe(SongContextObservation::StableSelection(candidates(&[5])));
        assert_eq!(
            context.resolve_result(&candidates(&[5, 6])),
            ContextualSongDecision::Accepted {
                song_id: 5,
                evidence: ContextualSongEvidence::ResultAndStableSelection,
            }
        );
    }

    #[test]
    fn gameplay_retry_does_not_create_or_count_play_attempts() {
        let mut context = SongContext::new();
        context.observe(SongContextObservation::StableSelection(candidates(&[7])));
        for _scene in 0..32 {
            context.observe(SongContextObservation::Preserve);
        }
        assert_eq!(context.stable_selection(), Some(&candidates(&[7])));
    }

    #[test]
    fn dan_flow_without_selection_context_keeps_result_resolution_screen_local() {
        let mut context = SongContext::new();
        for _scene in ["mode select", "course select", "transition", "gameplay"] {
            context.observe(SongContextObservation::Preserve);
        }
        assert_eq!(
            context.resolve_result(&candidates(&[1, 2])),
            ContextualSongDecision::Unknown {
                reason: ContextualSongUnknownReason::AmbiguousResult,
            }
        );
        assert_eq!(
            context.resolve_result(&candidates(&[2])),
            ContextualSongDecision::Accepted {
                song_id: 2,
                evidence: ContextualSongEvidence::ResultOnly,
            }
        );
    }

    #[test]
    fn explicit_resets_clear_context_but_unrecognized_scenes_do_not() {
        for reason in [
            SongContextClearReason::TitleObserved,
            SongContextClearReason::SessionEnded,
            SongContextClearReason::CoverageGap,
            SongContextClearReason::RecognitionBindingChanged,
        ] {
            let mut context = SongContext::new();
            context.observe(SongContextObservation::StableSelection(candidates(&[1])));
            context.observe(SongContextObservation::Preserve);
            assert_eq!(
                context.observe(SongContextObservation::Clear(reason)),
                SongContextChange::Cleared
            );
            assert_eq!(context.stable_selection(), None);
        }
    }

    #[test]
    fn readable_selection_result_conflict_remains_unknown() {
        let mut context = SongContext::new();
        context.observe(SongContextObservation::StableSelection(candidates(&[1])));
        assert_eq!(
            context.resolve_result(&candidates(&[2])),
            ContextualSongDecision::Unknown {
                reason: ContextualSongUnknownReason::SelectionConflict,
            }
        );
    }

    #[test]
    fn candidate_sets_cannot_be_empty() {
        assert_eq!(
            SongCandidates::<u8>::new([]),
            Err(SongCandidatesError::Empty)
        );
    }
}
