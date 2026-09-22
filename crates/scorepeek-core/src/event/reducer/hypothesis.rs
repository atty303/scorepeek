#![allow(
    clippy::wildcard_imports,
    reason = "this file is an implementation partition of its parent reducer authority"
)]

use super::*;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct JointKey {
    pub song_id: ScorepeekSongId,
    pub chart_key: ChartKey,
}

#[derive(Clone, Debug)]
pub struct AccumulatedHypothesis {
    pub candidate: JointEvidenceCandidate,
    pub family_support: BTreeMap<EvidenceFamily, u64>,
}

#[derive(Clone, Debug)]
struct RankedHypothesis<'a> {
    accumulated: &'a AccumulatedHypothesis,
    family_support: BTreeMap<EvidenceFamily, EvidenceContribution>,
    support: u16,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ResultChartFactor {
    pub play_type: Option<PlayType>,
    pub difficulty: Option<Difficulty>,
    pub notes: Option<u32>,
    pub level: Option<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct HypothesisAccumulator {
    pub candidates: BTreeMap<JointKey, AccumulatedHypothesis>,
    pub select_difficulty: Option<CurrentSelectionDifficulty>,
    pub select_play_types: BTreeMap<PlayType, u64>,
    pub select_play_sides: BTreeMap<PlaySide, u64>,
    result_chart_factors: BTreeMap<ResultChartFactor, u64>,
    has_result_evidence: bool,
    pub first_observation_ms: Option<u64>,
    pub last_observation_ms: Option<u64>,
    pub observation_count: u32,
}

#[derive(Clone, Debug)]
pub struct HypothesisSummary {
    pub state: ResolverResolutionState,
    pub select_play_type: Option<PlayType>,
    pub result_play_type: Option<PlayType>,
    pub selected: Option<JointEvidenceCandidate>,
    pub runner_up: Option<JointEvidenceCandidate>,
    pub runner_song: Option<JointEvidenceCandidate>,
    pub runner_chart: Option<JointEvidenceCandidate>,
    pub top_candidates: Vec<JointEvidenceCandidate>,
    pub support: u16,
    pub margin: u16,
    pub song_margin: u16,
    pub chart_margin: u16,
    pub selected_family_support: BTreeMap<EvidenceFamily, EvidenceContribution>,
    pub runner_up_family_support: BTreeMap<EvidenceFamily, EvidenceContribution>,
}

impl HypothesisSummary {
    #[must_use]
    pub fn accepted(&self) -> Option<JointEvidenceCandidate> {
        (self.state == ResolverResolutionState::AcceptedJoint)
            .then(|| self.selected.clone())
            .flatten()
    }
}
impl HypothesisAccumulator {
    #[must_use]
    #[allow(
        clippy::too_many_lines,
        reason = "hierarchical projection keeps normalization and both runner dimensions together"
    )]
    pub fn summary(&self) -> HypothesisSummary {
        let mut expanded = self.candidates.clone();
        let select_play_type = self.resolved_select_play_type();
        let result_play_type = self.resolved_result_play_type();
        for accumulated in expanded.values_mut() {
            let select_chart = self.select_difficulty.map_or(0, |current| {
                u64::from(current.difficulty == accumulated.candidate.chart.key.difficulty)
                    * current.support()
            });
            if select_chart > 0 {
                accumulated
                    .family_support
                    .insert(EvidenceFamily::SelectChart, select_chart);
            }
            let result_chart = self
                .result_chart_factors
                .iter()
                .map(|(factor, observations)| {
                    let difficulty = u64::from(
                        factor.difficulty == Some(accumulated.candidate.chart.key.difficulty),
                    ) * 50;
                    let notes =
                        u64::from(factor.notes == Some(accumulated.candidate.chart.notes)) * 100;
                    let level =
                        u64::from(factor.level == Some(accumulated.candidate.chart.level)) * 10;
                    difficulty.max(notes).max(level) * observations
                })
                .sum();
            if result_chart > 0 {
                accumulated
                    .family_support
                    .insert(EvidenceFamily::ResultChart, result_chart);
            }
            if result_play_type == Some(accumulated.candidate.chart.key.play_type) {
                accumulated
                    .family_support
                    .insert(EvidenceFamily::ResultPlayType, 100);
            }
            if select_play_type == Some(accumulated.candidate.chart.key.play_type) {
                accumulated
                    .family_support
                    .insert(EvidenceFamily::SelectPlayType, 100);
            }
        }
        let mut family_maxima = BTreeMap::<EvidenceFamily, u64>::new();
        for candidate in expanded.values() {
            for (family, raw) in &candidate.family_support {
                let maximum = family_maxima.entry(*family).or_default();
                *maximum = (*maximum).max(*raw);
            }
        }
        let mut ranked = expanded
            .values()
            .map(|accumulated| {
                let family_support = accumulated
                    .family_support
                    .iter()
                    .map(|(family, raw)| {
                        let maximum = family_maxima[family];
                        let normalized = normalize_family_support(*raw, maximum);
                        (*family, EvidenceContribution::new(*raw, normalized))
                    })
                    .collect::<BTreeMap<_, _>>();
                let support = family_support.values().fold(0_u16, |total, value| {
                    total.saturating_add(value.normalized())
                });
                RankedHypothesis {
                    accumulated,
                    family_support,
                    support,
                }
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|left, right| {
            right
                .support
                .cmp(&left.support)
                .then_with(|| {
                    left.accumulated
                        .candidate
                        .song_id
                        .cmp(&right.accumulated.candidate.song_id)
                })
                .then_with(|| {
                    left.accumulated
                        .candidate
                        .chart
                        .key
                        .cmp(&right.accumulated.candidate.chart.key)
                })
        });
        let Some(selected) = ranked.first() else {
            return HypothesisSummary {
                state: ResolverResolutionState::Unresolved,
                select_play_type,
                result_play_type,
                selected: None,
                runner_up: None,
                runner_song: None,
                runner_chart: None,
                top_candidates: Vec::new(),
                support: 0,
                margin: 0,
                song_margin: 0,
                chart_margin: 0,
                selected_family_support: BTreeMap::new(),
                runner_up_family_support: BTreeMap::new(),
            };
        };
        let support = selected.support;
        let runner_up = ranked.get(1);
        let runner_support = runner_up.map_or(0, |candidate| candidate.support);
        let margin = support.saturating_sub(runner_support);
        let runner_song = ranked.iter().find(|candidate| {
            candidate.accumulated.candidate.song_id != selected.accumulated.candidate.song_id
        });
        let runner_chart = ranked.iter().find(|candidate| {
            candidate.accumulated.candidate.song_id == selected.accumulated.candidate.song_id
                && candidate.accumulated.candidate.chart.key
                    != selected.accumulated.candidate.chart.key
        });
        let song_margin = support.saturating_sub(runner_song.map_or(0, |value| value.support));
        let chart_margin = support.saturating_sub(runner_chart.map_or(0, |value| value.support));
        let song_is_resolved = runner_song.is_none() || song_margin >= JOINT_ACCEPT_MARGIN;
        let chart_is_resolved = select_play_type.is_some() || result_play_type.is_some();
        let chart_is_resolved =
            chart_is_resolved && (runner_chart.is_none() || chart_margin >= JOINT_ACCEPT_MARGIN);
        let state = if self.has_result_evidence
            && support >= JOINT_ACCEPT_SUPPORT
            && song_is_resolved
            && chart_is_resolved
        {
            ResolverResolutionState::AcceptedJoint
        } else if support >= JOINT_ACCEPT_SUPPORT && song_is_resolved {
            ResolverResolutionState::SongProjected
        } else if support >= JOINT_ACCEPT_SUPPORT {
            ResolverResolutionState::Conflict
        } else {
            ResolverResolutionState::JointCandidate
        };
        HypothesisSummary {
            state,
            select_play_type,
            result_play_type,
            selected: Some(selected.accumulated.candidate.clone()),
            runner_up: runner_up.map(|value| value.accumulated.candidate.clone()),
            runner_song: runner_song.map(|value| value.accumulated.candidate.clone()),
            runner_chart: runner_chart.map(|value| value.accumulated.candidate.clone()),
            top_candidates: ranked
                .iter()
                .take(3)
                .map(|value| value.accumulated.candidate.clone())
                .collect(),
            support,
            margin,
            song_margin,
            chart_margin,
            selected_family_support: selected.family_support.clone(),
            runner_up_family_support: runner_up
                .map_or_else(BTreeMap::new, |value| value.family_support.clone()),
        }
    }

    pub fn observe_at(
        &mut self,
        sequence: u64,
        monotonic_ms: u64,
        observation: &JointEvidenceObservation,
        select_difficulty: Option<Difficulty>,
        select_play_type: Option<PlayType>,
        result_chart_factor: Option<ResultChartFactor>,
    ) {
        self.first_observation_ms.get_or_insert(monotonic_ms);
        self.last_observation_ms = Some(monotonic_ms);
        self.observation_count = self.observation_count.saturating_add(1);
        for candidate in &observation.candidates {
            let key = JointKey {
                song_id: candidate.song_id,
                chart_key: candidate.chart.key,
            };
            let accumulated = self
                .candidates
                .entry(key)
                .or_insert_with(|| AccumulatedHypothesis {
                    candidate: candidate.clone(),
                    family_support: BTreeMap::new(),
                });
            for (family, delta) in &candidate.family_support {
                if matches!(
                    family,
                    EvidenceFamily::SelectChart | EvidenceFamily::ResultChart
                ) {
                    continue;
                }
                let value = accumulated.family_support.entry(*family).or_default();
                *value = value.saturating_add(u64::from(*delta));
            }
        }
        if let Some(difficulty) = select_difficulty {
            self.observe_select_difficulty(difficulty, sequence, monotonic_ms);
        }
        if let Some(play_type) = select_play_type {
            let count = self.select_play_types.entry(play_type).or_default();
            *count = count.saturating_add(1);
        }
        if let Some(factor) = result_chart_factor {
            let value = self.result_chart_factors.entry(factor).or_default();
            *value = value.saturating_add(1);
            self.has_result_evidence |= factor.play_type.is_some()
                || factor.difficulty.is_some()
                || factor.notes.is_some()
                || factor.level.is_some();
        }
        self.has_result_evidence |= observation.candidates.iter().any(|candidate| {
            candidate.family_support.iter().any(|(family, support)| {
                *support > 0
                    && matches!(
                        family,
                        EvidenceFamily::ResultTitle
                            | EvidenceFamily::ResultArtist
                            | EvidenceFamily::ResultChart
                            | EvidenceFamily::ResultPlayType
                    )
            })
        });
    }

    #[cfg(feature = "reducer-test-support")]
    pub fn observe(
        &mut self,
        monotonic_ms: u64,
        observation: &JointEvidenceObservation,
        select_difficulty: Option<Difficulty>,
        result_chart_factor: Option<ResultChartFactor>,
    ) {
        self.observe_at(
            monotonic_ms,
            monotonic_ms,
            observation,
            select_difficulty,
            None,
            result_chart_factor,
        );
    }

    pub fn add_from(&mut self, other: &Self) {
        for accumulated in other.candidates.values() {
            let key = JointKey {
                song_id: accumulated.candidate.song_id,
                chart_key: accumulated.candidate.chart.key,
            };
            let target = self
                .candidates
                .entry(key)
                .or_insert_with(|| AccumulatedHypothesis {
                    candidate: accumulated.candidate.clone(),
                    family_support: BTreeMap::new(),
                });
            for (family, support) in &accumulated.family_support {
                let value = target.family_support.entry(*family).or_default();
                *value = value.saturating_add(*support);
            }
        }
        self.adopt_newer_select_difficulty(other.select_difficulty);
        for (play_type, observations) in &other.select_play_types {
            let value = self.select_play_types.entry(*play_type).or_default();
            *value = value.saturating_add(*observations);
        }
        for (factor, observations) in &other.result_chart_factors {
            let value = self.result_chart_factors.entry(*factor).or_default();
            *value = value.saturating_add(*observations);
        }
        self.has_result_evidence |= other.has_result_evidence;
    }

    #[must_use]
    pub fn resolved_result_play_type(&self) -> Option<PlayType> {
        let mut observed = BTreeMap::<PlayType, u64>::new();
        for (factor, observations) in &self.result_chart_factors {
            if let Some(play_type) = factor.play_type {
                let count = observed.entry(play_type).or_default();
                *count = count.saturating_add(*observations);
            }
        }
        if observed.len() != 1 {
            return None;
        }
        observed
            .into_iter()
            .next()
            .and_then(|(play_type, observations)| (observations >= 2).then_some(play_type))
    }

    #[must_use]
    pub fn resolved_select_play_type(&self) -> Option<PlayType> {
        if self.select_play_types.len() != 1 {
            return None;
        }
        self.select_play_types
            .iter()
            .next()
            .and_then(|(play_type, observations)| (*observations >= 2).then_some(*play_type))
    }

    #[must_use]
    pub fn resolved_select_play_side(&self) -> Option<PlaySide> {
        if self.select_play_sides.len() != 1 {
            return None;
        }
        self.select_play_sides
            .iter()
            .next()
            .and_then(|(play_side, observations)| (*observations >= 2).then_some(*play_side))
    }

    pub fn observe_select_difficulty(
        &mut self,
        difficulty: Difficulty,
        sequence: u64,
        monotonic_ms: u64,
    ) -> bool {
        if let Some(current) = &mut self.select_difficulty {
            current.observe(difficulty, sequence, monotonic_ms)
        } else {
            self.select_difficulty = Some(CurrentSelectionDifficulty::observed(
                difficulty,
                sequence,
                monotonic_ms,
            ));
            true
        }
    }

    pub fn adopt_newer_select_difficulty(&mut self, incoming: Option<CurrentSelectionDifficulty>) {
        if incoming.is_some_and(|value| {
            self.select_difficulty
                .is_none_or(|current| value.last_sequence() > current.last_sequence())
        }) {
            self.select_difficulty = incoming;
        }
    }
}

fn normalize_family_support(raw: u64, maximum: u64) -> u16 {
    if maximum <= u64::from(EVIDENCE_FAMILY_CAP) {
        return u16::try_from(raw).unwrap_or(EVIDENCE_FAMILY_CAP);
    }
    let scaled = u128::from(raw) * u128::from(EVIDENCE_FAMILY_CAP) / u128::from(maximum);
    u16::try_from(scaled).unwrap_or(EVIDENCE_FAMILY_CAP)
}

#[must_use]
pub fn credible_song_set(observation: &JointEvidenceObservation) -> BTreeSet<ScorepeekSongId> {
    let mut support_by_song = BTreeMap::<ScorepeekSongId, u16>::new();
    for candidate in &observation.candidates {
        let support = candidate
            .family_support
            .iter()
            .filter(|(family, _)| {
                matches!(
                    family,
                    EvidenceFamily::SelectTitle
                        | EvidenceFamily::SelectTitleLexical
                        | EvidenceFamily::SelectTitleStructural
                        | EvidenceFamily::SelectArtist
                )
            })
            .fold(0_u16, |total, (_, value)| total.saturating_add(*value));
        let current = support_by_song.entry(candidate.song_id).or_default();
        *current = (*current).max(support);
    }
    let maximum = support_by_song.values().copied().max().unwrap_or(0);
    if maximum == 0 {
        return BTreeSet::new();
    }
    let credible = support_by_song
        .iter()
        .filter_map(|(song, support)| (*support == maximum).then_some(*song))
        .collect::<BTreeSet<_>>();
    if observation.catalog_song_count > 0 && credible.len() == observation.catalog_song_count {
        BTreeSet::new()
    } else {
        credible
    }
}
