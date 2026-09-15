use scorepeek::capture::GamescopeSourceSnapshot;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatchDecision {
    WaitAbsent,
    WaitAmbiguous,
    WaitConsumed,
    Admit { node_id: u32, generation: u64 },
}

#[derive(Default)]
pub struct SourceLifetimes {
    consumed_node: Option<u32>,
    next_generation: u64,
}

impl SourceLifetimes {
    pub fn new() -> Self {
        Self {
            consumed_node: None,
            next_generation: 1,
        }
    }

    pub fn observe(&mut self, snapshot: GamescopeSourceSnapshot) -> WatchDecision {
        match snapshot {
            GamescopeSourceSnapshot::Absent => {
                self.consumed_node = None;
                WatchDecision::WaitAbsent
            }
            GamescopeSourceSnapshot::Ambiguous { .. } => WatchDecision::WaitAmbiguous,
            GamescopeSourceSnapshot::Unique { node_id } if self.consumed_node == Some(node_id) => {
                WatchDecision::WaitConsumed
            }
            GamescopeSourceSnapshot::Unique { node_id } => WatchDecision::Admit {
                node_id,
                generation: self.next_generation,
            },
        }
    }

    pub fn generation_ended(&mut self, node_id: u32, readmit_same_node: bool) {
        self.consumed_node = (!readmit_same_node).then_some(node_id);
        self.next_generation = self.next_generation.saturating_add(1);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatcherState {
    WaitingForSource,
    AmbiguousSources,
    RemoteUnavailable,
    CatalogUnavailable,
}

impl WatcherState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WaitingForSource => "waiting_for_source",
            Self::AmbiguousSources => "ambiguous_sources",
            Self::RemoteUnavailable => "remote_unavailable",
            Self::CatalogUnavailable => "catalog_unavailable",
        }
    }
}

#[cfg(test)]
mod tests {
    use scorepeek::capture::GamescopeSourceSnapshot;

    use super::{SourceLifetimes, WatchDecision};

    #[test]
    fn one_attempt_per_node_lifetime_and_new_generation_after_absence() {
        let mut lifetimes = SourceLifetimes::new();
        assert_eq!(
            lifetimes.observe(GamescopeSourceSnapshot::Unique { node_id: 41 }),
            WatchDecision::Admit {
                node_id: 41,
                generation: 1
            }
        );
        lifetimes.generation_ended(41, false);
        assert_eq!(
            lifetimes.observe(GamescopeSourceSnapshot::Unique { node_id: 41 }),
            WatchDecision::WaitConsumed
        );
        assert_eq!(
            lifetimes.observe(GamescopeSourceSnapshot::Absent),
            WatchDecision::WaitAbsent
        );
        assert_eq!(
            lifetimes.observe(GamescopeSourceSnapshot::Unique { node_id: 41 }),
            WatchDecision::Admit {
                node_id: 41,
                generation: 2
            }
        );
    }

    #[test]
    fn rejected_lifetime_does_not_consume_a_generation() {
        let mut lifetimes = SourceLifetimes::new();
        assert!(matches!(
            lifetimes.observe(GamescopeSourceSnapshot::Unique { node_id: 1 }),
            WatchDecision::Admit { generation: 1, .. }
        ));
        assert!(matches!(
            lifetimes.observe(GamescopeSourceSnapshot::Unique { node_id: 1 }),
            WatchDecision::Admit { generation: 1, .. }
        ));
        assert!(matches!(
            lifetimes.observe(GamescopeSourceSnapshot::Unique { node_id: 2 }),
            WatchDecision::Admit { generation: 1, .. }
        ));
    }

    #[test]
    fn ambiguous_candidates_are_never_admitted() {
        let mut lifetimes = SourceLifetimes::new();
        assert_eq!(
            lifetimes.observe(GamescopeSourceSnapshot::Ambiguous { candidate_count: 2 }),
            WatchDecision::WaitAmbiguous
        );
    }

    #[test]
    fn contract_change_readmits_the_same_node_as_a_new_generation() {
        let mut lifetimes = SourceLifetimes::new();
        let snapshot = GamescopeSourceSnapshot::Unique { node_id: 41 };
        assert_eq!(
            lifetimes.observe(snapshot),
            WatchDecision::Admit {
                node_id: 41,
                generation: 1,
            }
        );
        lifetimes.generation_ended(41, true);
        assert_eq!(
            lifetimes.observe(snapshot),
            WatchDecision::Admit {
                node_id: 41,
                generation: 2,
            }
        );
    }
}
