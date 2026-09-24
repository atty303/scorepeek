//! Core-owned hypothesis and selection reducer invariants.

use std::collections::BTreeMap;

use serde_json::json;

use super::*;
use crate::catalog::{Chart, ChartKey, Difficulty, PlayType};
use crate::event::{
    ResolverResolutionState, ResultPanelSideEpisodeState, ResultPanelSideTransitionReason,
    SelectionDifficultyTarget, SelectionDifficultyTransitionReason,
};
use crate::recognition::result::{
    PlayOption, PlayOptions, PlayOptionsObservation, PlayOptionsUnknownReason,
    ResultPerformanceResolution, ResultPerformanceUnknownReason,
};
use crate::recognition::screen::ResultPanelSide;
use crate::recognition::shared::{
    EvidenceFamily, JointEvidenceCandidate, JointEvidenceObservation,
};

#[test]
fn one_numeric_challenger_cannot_replace_an_accepted_song_or_chart() {
    let base = NumericResultView {
        song_id: serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap(),
        clear_type: "CLEAR".into(),
        chart: Chart {
            key: ChartKey {
                play_type: PlayType::Single,
                difficulty: Difficulty::Hyper,
            },
            level: 8,
            notes: 764,
        },
        current_score: 1_286,
        performance: ResultPerformanceResolution::Unknown {
            resolver_id: "synthetic".into(),
            reason: ResultPerformanceUnknownReason::IncompleteJudgments,
        },
        source_sequence: 2,
    };
    for change in ["song", "chart"] {
        let mut challenger = base.clone();
        challenger.source_sequence = 3;
        if change == "song" {
            challenger.song_id =
                serde_json::from_str("\"00000000-0000-0000-0000-000000000002\"").unwrap();
        } else {
            challenger.chart.key.difficulty = Difficulty::Another;
        }
        let mut reducer = DomainReducer {
            accepted_numeric_result: Some(base.clone()),
            ..DomainReducer::default()
        };
        assert!(
            reducer
                .stabilize_numeric_result(challenger.clone(), false)
                .is_none()
        );
        assert_eq!(reducer.accepted_numeric_result, Some(base.clone()));
        assert_eq!(reducer.pending_numeric_result.unwrap().view, challenger);
    }
}

#[test]
fn conflicting_result_panel_side_detaches_retained_select_context() {
    let mut reducer = DomainReducer::default();
    reducer
        .engine
        .retained_select
        .select_play_sides
        .insert(PlaySide::OnePlayer, 2);
    reducer
        .result_panel_side
        .observe(1, 1, ResultPanelSide::Right);
    reducer
        .result_panel_side
        .observe(1, 2, ResultPanelSide::Right);
    let session = "synthetic".to_owned();
    reducer
        .reduce_result_observation(
            Some(&session),
            3,
            300,
            None,
            None,
            None,
            &JointEvidenceObservation {
                catalog_song_count: 0,
                candidates: Vec::new(),
            },
        )
        .unwrap();
    assert!(reducer.result_select_context_detached);
    assert!(reducer.engine.retained_select.select_play_sides.is_empty());
    assert!(reducer.effects.iter().any(|effect| matches!(
        effect,
        DomainEffect::Event(DomainTransitionKind::ResultSelectContextMismatch {
            select_play_side: PlaySide::OnePlayer,
            result_play_side: PlaySide::TwoPlayer,
            ..
        })
    )));
}

#[test]
fn episode_evidence_accumulates_by_family_caps_and_unknown_does_not_erase() {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
    let candidate = JointEvidenceCandidate {
        song_id,
        chart: crate::catalog::Chart {
            key: crate::catalog::ChartKey {
                play_type: PlayType::Single,
                difficulty: Difficulty::Hyper,
            },
            level: 8,
            notes: 764,
        },
        display_titles: vec!["TEST SONG".to_owned()],
        artist: "TEST ARTIST".to_owned(),
        family_support: BTreeMap::from([
            (EvidenceFamily::ResultTitle, 70),
            (EvidenceFamily::ResultArtist, 35),
            (EvidenceFamily::ResultChart, 50),
        ]),
        support: 155,
    };
    let observation = JointEvidenceObservation {
        catalog_song_count: 0,
        candidates: vec![candidate],
    };
    let mut accumulator = HypothesisAccumulator::default();
    let chart_factor = ResultChartFactor {
        play_type: Some(PlayType::Single),
        difficulty: Some(Difficulty::Hyper),
        notes: Some(764),
        level: Some(8),
    };
    accumulator.observe(100, &observation, None, Some(chart_factor));
    assert_eq!(
        accumulator.summary().state,
        ResolverResolutionState::JointCandidate
    );
    accumulator.observe(
        200,
        &JointEvidenceObservation {
            catalog_song_count: 0,
            candidates: Vec::new(),
        },
        None,
        None,
    );
    accumulator.observe(300, &observation, None, Some(chart_factor));
    assert_eq!(
        accumulator.summary().state,
        ResolverResolutionState::AcceptedJoint
    );
    for tick in 0..20 {
        accumulator.observe(400 + tick, &observation, None, None);
    }
    let accepted = accumulator.summary().accepted().unwrap();
    let stored = &accumulator.candidates[&JointKey {
        song_id,
        chart_key: accepted.chart.key,
    }];
    assert!(
        stored
            .family_support
            .values()
            .any(|support| { *support > u64::from(EVIDENCE_FAMILY_CAP) })
    );
    assert!(
        accumulator
            .summary()
            .selected_family_support
            .values()
            .all(|support| support.normalized() <= EVIDENCE_FAMILY_CAP)
    );
}

#[test]
fn family_normalization_preserves_candidate_ratios_above_the_cap() {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
    let make = |difficulty, support| JointEvidenceCandidate {
        song_id,
        chart: crate::catalog::Chart {
            key: crate::catalog::ChartKey {
                play_type: PlayType::Single,
                difficulty,
            },
            level: 8,
            notes: 764,
        },
        display_titles: vec!["TEST SONG".to_owned()],
        artist: "TEST ARTIST".to_owned(),
        family_support: BTreeMap::from([(EvidenceFamily::ResultArtist, support)]),
        support,
    };
    let observation = JointEvidenceObservation {
        catalog_song_count: 0,
        candidates: vec![make(Difficulty::Hyper, 170), make(Difficulty::Another, 70)],
    };
    let mut accumulator = HypothesisAccumulator::default();
    for tick in 0..3 {
        accumulator.observe(100 + tick, &observation, None, None);
    }
    let summary = accumulator.summary();
    assert_eq!(summary.support, 300);
    assert_eq!(summary.support.saturating_sub(summary.margin), 123);
    assert_eq!(summary.margin, 177);
    assert_eq!(
        summary.selected_family_support[&EvidenceFamily::ResultArtist],
        EvidenceContribution::new(510, 300)
    );
    assert_eq!(
        summary.runner_up_family_support[&EvidenceFamily::ResultArtist],
        EvidenceContribution::new(210, 123)
    );
}

#[test]
fn chart_factors_wait_for_song_evidence_and_apply_to_later_candidates() {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000011\"").unwrap();
    let mut accumulator = HypothesisAccumulator::default();
    accumulator.observe(
        100,
        &JointEvidenceObservation {
            catalog_song_count: 2,
            candidates: Vec::new(),
        },
        Some(Difficulty::Hyper),
        Some(ResultChartFactor {
            play_type: None,
            difficulty: Some(Difficulty::Hyper),
            notes: Some(1_136),
            level: Some(10),
        }),
    );
    assert_eq!(
        accumulator.summary().state,
        ResolverResolutionState::Unresolved
    );

    let candidate = |difficulty, notes| JointEvidenceCandidate {
        song_id,
        chart: crate::catalog::Chart {
            key: crate::catalog::ChartKey {
                play_type: PlayType::Single,
                difficulty,
            },
            level: 10,
            notes,
        },
        display_titles: vec!["∀".to_owned()],
        artist: "BEMANI Sound Team \"HuΣeR\" respect for D.J.Amuro".to_owned(),
        family_support: BTreeMap::from([(EvidenceFamily::ResultArtist, 220)]),
        support: 220,
    };
    accumulator.observe(
        200,
        &JointEvidenceObservation {
            catalog_song_count: 2,
            candidates: vec![
                candidate(Difficulty::Hyper, 1_136),
                candidate(Difficulty::Another, 1_500),
            ],
        },
        None,
        None,
    );
    let summary = accumulator.summary();
    assert_eq!(
        summary.selected.unwrap().chart.key.difficulty,
        Difficulty::Hyper
    );
    assert_eq!(summary.support, 370);
    assert_eq!(summary.chart_margin, 140);
}

#[test]
fn selection_epoch_retains_difficulty_until_song_evidence_arrives() {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000012\"").unwrap();
    let mut epochs = SelectionEpochTracker::default();
    let pending = epochs.observe(
        100,
        &JointEvidenceObservation {
            catalog_song_count: 2,
            candidates: Vec::new(),
        },
        Some(Difficulty::Hyper),
    );
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].target, SelectionDifficultyTarget::Pending);
    assert_eq!(
        pending[0].reason,
        SelectionDifficultyTransitionReason::Changed
    );
    let candidate = |difficulty| JointEvidenceCandidate {
        song_id,
        chart: crate::catalog::Chart {
            key: crate::catalog::ChartKey {
                play_type: PlayType::Single,
                difficulty,
            },
            level: 10,
            notes: if difficulty == Difficulty::Hyper {
                1_136
            } else {
                1_500
            },
        },
        display_titles: vec!["∀".to_owned()],
        artist: "ARTIST".to_owned(),
        family_support: BTreeMap::from([(EvidenceFamily::SelectTitle, 300)]),
        support: 300,
    };
    let applied = epochs.observe(
        200,
        &JointEvidenceObservation {
            catalog_song_count: 2,
            candidates: vec![candidate(Difficulty::Hyper), candidate(Difficulty::Another)],
        },
        None,
    );
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].target, SelectionDifficultyTarget::Incumbent);
    assert_eq!(
        applied[0].reason,
        SelectionDifficultyTransitionReason::PendingApplied
    );
    let summary = epochs.incumbent.summary();
    assert_eq!(
        summary.selected.unwrap().chart.key.difficulty,
        Difficulty::Hyper
    );
    assert_eq!(summary.chart_margin, 50);
    assert!(epochs.pending_difficulty.is_none());
}

#[test]
fn selection_difficulty_tracks_every_known_change_without_changing_song_evidence() {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000013\"").unwrap();
    let candidate = |difficulty| JointEvidenceCandidate {
        song_id,
        chart: crate::catalog::Chart {
            key: crate::catalog::ChartKey {
                play_type: PlayType::Single,
                difficulty,
            },
            level: 10,
            notes: 1_000,
        },
        display_titles: vec!["X".to_owned()],
        artist: "ARTIST".to_owned(),
        family_support: BTreeMap::from([(EvidenceFamily::SelectTitle, 300)]),
        support: 300,
    };
    let song_evidence = JointEvidenceObservation {
        catalog_song_count: 2,
        candidates: vec![
            candidate(Difficulty::Normal),
            candidate(Difficulty::Hyper),
            candidate(Difficulty::Another),
        ],
    };
    let no_song_evidence = JointEvidenceObservation {
        catalog_song_count: 2,
        candidates: Vec::new(),
    };
    let mut epochs = SelectionEpochTracker::default();
    let first = epochs.observe_at(3_240, 324_000, &song_evidence, Some(Difficulty::Hyper));
    assert_eq!(first.len(), 1);
    for sequence in 3_241..=3_252 {
        assert!(
            epochs
                .observe_at(
                    sequence,
                    sequence * 100,
                    &song_evidence,
                    Some(Difficulty::Hyper)
                )
                .is_empty()
        );
    }
    let song_support = epochs
        .incumbent
        .candidates
        .values()
        .next()
        .unwrap()
        .family_support[&EvidenceFamily::SelectTitle];
    let observation_count = epochs.incumbent.observation_count;

    for (sequence, difficulty) in [
        (3_255, Difficulty::Another),
        (3_296, Difficulty::Normal),
        (3_302, Difficulty::Hyper),
        (3_307, Difficulty::Another),
    ] {
        let transitions = epochs.observe_at(
            sequence,
            sequence * 100,
            &no_song_evidence,
            Some(difficulty),
        );
        assert_eq!(transitions.len(), 1);
        assert_eq!(
            transitions[0].reason,
            SelectionDifficultyTransitionReason::Changed
        );
        assert_eq!(transitions[0].target, SelectionDifficultyTarget::Incumbent);
        assert_eq!(transitions[0].current.unwrap().difficulty, difficulty);
        assert_eq!(
            epochs
                .incumbent
                .summary()
                .selected
                .unwrap()
                .chart
                .key
                .difficulty,
            difficulty
        );
        assert_eq!(epochs.incumbent.observation_count, observation_count);
        assert_eq!(
            epochs
                .incumbent
                .candidates
                .values()
                .next()
                .unwrap()
                .family_support[&EvidenceFamily::SelectTitle],
            song_support
        );
    }
}

#[test]
fn unknown_difficulty_gap_retains_current_and_emits_no_transition() {
    let mut accumulator = HypothesisAccumulator::default();
    assert!(accumulator.observe_select_difficulty(Difficulty::Another, 10, 1_000));
    let retained = accumulator.select_difficulty.unwrap();
    let mut epochs = SelectionEpochTracker {
        incumbent: accumulator,
        ..SelectionEpochTracker::default()
    };
    epochs.incumbent.observation_count = 1;
    let transitions = epochs.observe_at(
        11,
        1_100,
        &JointEvidenceObservation {
            catalog_song_count: 2,
            candidates: Vec::new(),
        },
        None,
    );
    assert!(transitions.is_empty());
    assert_eq!(epochs.incumbent.select_difficulty, Some(retained));
}

#[test]
fn snapshot_merge_adopts_newer_difficulty_once_instead_of_adding_votes() {
    let mut retained = HypothesisAccumulator::default();
    retained.observe_select_difficulty(Difficulty::Hyper, 100, 1_000);
    for sequence in 101..110 {
        retained.observe_select_difficulty(Difficulty::Hyper, sequence, sequence * 10);
    }
    let mut incoming = HypothesisAccumulator::default();
    incoming.observe_select_difficulty(Difficulty::Another, 200, 2_000);
    retained.add_from(&incoming);
    retained.add_from(&incoming);
    let current = retained.select_difficulty.unwrap();
    assert_eq!(current.difficulty, Difficulty::Another);
    assert_eq!(current.consecutive_known, 1);
    assert_eq!(current.last_sequence(), 200);
}

#[test]
fn late_difficulty_observation_cannot_replace_newer_current_state() {
    let mut accumulator = HypothesisAccumulator::default();
    assert!(accumulator.observe_select_difficulty(Difficulty::Another, 200, 2_000));
    assert!(!accumulator.observe_select_difficulty(Difficulty::Normal, 199, 1_990));
    let current = accumulator.select_difficulty.unwrap();
    assert_eq!(current.difficulty, Difficulty::Another);
    assert_eq!(current.consecutive_known, 1);
    assert_eq!(current.last_sequence(), 200);
}

#[test]
fn bounded_summary_does_not_truncate_resolver_authority() {
    let first = serde_json::from_str("\"00000000-0000-0000-0000-000000000051\"").unwrap();
    let second = serde_json::from_str("\"00000000-0000-0000-0000-000000000052\"").unwrap();
    let keys = [
        (PlayType::Single, Difficulty::Beginner),
        (PlayType::Single, Difficulty::Normal),
        (PlayType::Single, Difficulty::Hyper),
        (PlayType::Single, Difficulty::Another),
        (PlayType::Single, Difficulty::Leggendaria),
        (PlayType::Double, Difficulty::Normal),
        (PlayType::Double, Difficulty::Hyper),
        (PlayType::Double, Difficulty::Another),
    ];
    let candidate = |song_id, play_type, difficulty, support| JointEvidenceCandidate {
        song_id,
        chart: crate::catalog::Chart {
            key: crate::catalog::ChartKey {
                play_type,
                difficulty,
            },
            level: 10,
            notes: 1_000,
        },
        display_titles: vec![format!("SONG-{song_id:?}")],
        artist: "ARTIST".to_owned(),
        family_support: BTreeMap::from([(EvidenceFamily::ResultArtist, support)]),
        support,
    };
    let mut candidates = keys
        .into_iter()
        .map(|(play_type, difficulty)| candidate(first, play_type, difficulty, 300))
        .collect::<Vec<_>>();
    candidates.push(candidate(second, PlayType::Single, Difficulty::Hyper, 270));
    let joint_evidence = JointEvidenceObservation {
        catalog_song_count: 2,
        candidates,
    };
    let mut authority = HypothesisAccumulator::default();
    authority.observe(100, &joint_evidence, None, None);
    assert_eq!(authority.summary().state, ResolverResolutionState::Conflict);
    assert_eq!(joint_evidence.candidates.len(), 9);
    assert!(authority.summary().top_candidates.len() < joint_evidence.candidates.len());
}

#[test]
fn runner_song_and_runner_chart_are_independent_hierarchical_competitors() {
    let first = serde_json::from_str("\"00000000-0000-0000-0000-000000000021\"").unwrap();
    let second = serde_json::from_str("\"00000000-0000-0000-0000-000000000022\"").unwrap();
    let candidate = |song_id, play_type, support| JointEvidenceCandidate {
        song_id,
        chart: crate::catalog::Chart {
            key: crate::catalog::ChartKey {
                play_type,
                difficulty: Difficulty::Hyper,
            },
            level: 10,
            notes: 1_136,
        },
        display_titles: vec![format!("SONG-{song_id:?}")],
        artist: "ARTIST".to_owned(),
        family_support: BTreeMap::from([(EvidenceFamily::ResultArtist, support)]),
        support,
    };
    let mut accumulator = HypothesisAccumulator::default();
    accumulator.observe(
        100,
        &JointEvidenceObservation {
            catalog_song_count: 2,
            candidates: vec![
                candidate(first, PlayType::Single, 400),
                candidate(first, PlayType::Double, 380),
                candidate(second, PlayType::Single, 350),
            ],
        },
        None,
        None,
    );
    let summary = accumulator.summary();
    assert_eq!(summary.runner_up.as_ref().unwrap().song_id, first);
    assert_eq!(summary.runner_chart.as_ref().unwrap().song_id, first);
    assert_eq!(summary.runner_song.as_ref().unwrap().song_id, second);
    assert_eq!(summary.song_margin, 38);
    assert_eq!(summary.chart_margin, 15);
}

#[test]
fn latest_failure_oracle_resolves_forall_from_cross_screen_factors() {
    let wrong = serde_json::from_str("\"00000000-0000-0000-0000-000000000031\"").unwrap();
    let expected = serde_json::from_str("\"00000000-0000-0000-0000-000000000032\"").unwrap();
    let chart = |song_id, play_type, notes, family, support| JointEvidenceCandidate {
        song_id,
        chart: crate::catalog::Chart {
            key: crate::catalog::ChartKey {
                play_type,
                difficulty: Difficulty::Hyper,
            },
            level: 10,
            notes,
        },
        display_titles: vec![if song_id == expected { "∀" } else { "A" }.to_owned()],
        artist: if song_id == expected {
            "BEMANI Sound Team \"HuΣeR\" respect for D.J.Amuro"
        } else {
            "OTHER"
        }
        .to_owned(),
        family_support: BTreeMap::from([(family, support)]),
        support,
    };
    let mut select = HypothesisAccumulator::default();
    select.observe(
        100,
        &JointEvidenceObservation {
            catalog_song_count: 2,
            candidates: vec![chart(
                wrong,
                PlayType::Single,
                1_000,
                EvidenceFamily::SelectTitle,
                300,
            )],
        },
        Some(Difficulty::Hyper),
        None,
    );
    let mut result = HypothesisAccumulator::default();
    result.observe(
        200,
        &JointEvidenceObservation {
            catalog_song_count: 2,
            candidates: vec![
                chart(
                    expected,
                    PlayType::Single,
                    1_136,
                    EvidenceFamily::ResultArtist,
                    300,
                ),
                chart(
                    expected,
                    PlayType::Double,
                    1_500,
                    EvidenceFamily::ResultArtist,
                    300,
                ),
            ],
        },
        None,
        Some(ResultChartFactor {
            play_type: Some(PlayType::Single),
            difficulty: Some(Difficulty::Hyper),
            notes: Some(1_136),
            level: None,
        }),
    );
    result.observe(
        201,
        &JointEvidenceObservation {
            catalog_song_count: 2,
            candidates: Vec::new(),
        },
        None,
        Some(ResultChartFactor {
            play_type: Some(PlayType::Single),
            difficulty: Some(Difficulty::Hyper),
            notes: Some(1_136),
            level: None,
        }),
    );
    select.add_from(&result);
    let summary = select.summary();
    let accepted = summary.accepted().expect("∀ SP HYPER should resolve");
    assert_eq!(accepted.song_id, expected);
    assert_eq!(accepted.chart.key.play_type, PlayType::Single);
    assert_eq!(accepted.chart.notes, 1_136);
    assert_eq!(summary.song_margin, 100);
    assert_eq!(summary.chart_margin, 200);
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the regression covers unresolved, accepted, conflicting, and mismatched SP/DP evidence"
)]
fn result_play_type_breaks_an_otherwise_identical_sibling_tie() {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000043\"").unwrap();
    let chart = |play_type| JointEvidenceCandidate {
        song_id,
        chart: crate::catalog::Chart {
            key: crate::catalog::ChartKey {
                play_type,
                difficulty: Difficulty::Hyper,
            },
            level: 8,
            notes: 829,
        },
        display_titles: vec!["Wizards!".to_owned()],
        artist: "ARTIST".to_owned(),
        family_support: BTreeMap::from([(EvidenceFamily::ResultTitle, 300)]),
        support: 300,
    };
    let observation = JointEvidenceObservation {
        catalog_song_count: 2,
        candidates: vec![chart(PlayType::Single), chart(PlayType::Double)],
    };
    let mut without_play_type = HypothesisAccumulator::default();
    without_play_type.observe(
        100,
        &observation,
        None,
        Some(ResultChartFactor {
            play_type: None,
            difficulty: Some(Difficulty::Hyper),
            notes: Some(829),
            level: Some(8),
        }),
    );
    assert_eq!(
        without_play_type.summary().state,
        ResolverResolutionState::SongProjected
    );

    let mut with_play_type = HypothesisAccumulator::default();
    with_play_type.observe(
        100,
        &observation,
        None,
        Some(ResultChartFactor {
            play_type: Some(PlayType::Single),
            difficulty: Some(Difficulty::Hyper),
            notes: Some(829),
            level: Some(8),
        }),
    );
    assert_eq!(
        with_play_type.summary().state,
        ResolverResolutionState::SongProjected
    );
    with_play_type.observe(
        101,
        &observation,
        None,
        Some(ResultChartFactor {
            play_type: Some(PlayType::Single),
            difficulty: Some(Difficulty::Hyper),
            notes: Some(829),
            level: Some(8),
        }),
    );
    let summary = with_play_type.summary();
    assert_eq!(summary.state, ResolverResolutionState::AcceptedJoint);
    assert_eq!(
        summary
            .selected
            .as_ref()
            .map(|candidate| candidate.chart.key.play_type),
        Some(PlayType::Single)
    );
    assert_eq!(summary.chart_margin, 100);
    assert_eq!(
        summary.selected_family_support[&EvidenceFamily::ResultPlayType].normalized(),
        100
    );

    with_play_type.observe(
        102,
        &observation,
        None,
        Some(ResultChartFactor {
            play_type: Some(PlayType::Double),
            difficulty: Some(Difficulty::Hyper),
            notes: Some(829),
            level: Some(8),
        }),
    );
    assert_eq!(
        with_play_type.summary().state,
        ResolverResolutionState::SongProjected
    );

    let mut mismatched_only = HypothesisAccumulator::default();
    let double_only = JointEvidenceObservation {
        catalog_song_count: 1,
        candidates: vec![chart(PlayType::Double)],
    };
    for monotonic_ms in [200, 201] {
        mismatched_only.observe(
            monotonic_ms,
            &double_only,
            None,
            Some(ResultChartFactor {
                play_type: Some(PlayType::Single),
                difficulty: Some(Difficulty::Hyper),
                notes: Some(829),
                level: Some(8),
            }),
        );
    }
    assert_eq!(
        mismatched_only.summary().state,
        ResolverResolutionState::AcceptedJoint
    );
    assert_eq!(
        mismatched_only
            .summary()
            .selected
            .as_ref()
            .map(|candidate| candidate.chart.key.play_type),
        Some(PlayType::Double)
    );
}

#[test]
fn select_play_type_resolves_ui_chart_without_joint_acceptance() {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000044\"").unwrap();
    let candidate = |play_type| JointEvidenceCandidate {
        song_id,
        chart: crate::catalog::Chart {
            key: crate::catalog::ChartKey {
                play_type,
                difficulty: Difficulty::Hyper,
            },
            level: 8,
            notes: 829,
        },
        display_titles: vec!["Wizards!".to_owned()],
        artist: "ARTIST".to_owned(),
        family_support: BTreeMap::from([(EvidenceFamily::SelectTitle, 300)]),
        support: 300,
    };
    let evidence = JointEvidenceObservation {
        catalog_song_count: 1,
        candidates: vec![candidate(PlayType::Single), candidate(PlayType::Double)],
    };
    let mut select = HypothesisAccumulator::default();
    for sequence in [1, 2] {
        select.observe_at(
            sequence,
            sequence * 100,
            &evidence,
            Some(Difficulty::Hyper),
            Some(PlayType::Double),
            None,
        );
    }
    let summary = select.summary();
    assert_ne!(summary.state, ResolverResolutionState::AcceptedJoint);
    assert_eq!(summary.select_play_type, Some(PlayType::Double));
    select.observe_at(
        3,
        300,
        &JointEvidenceObservation {
            catalog_song_count: 1,
            candidates: Vec::new(),
        },
        None,
        None,
        Some(ResultChartFactor {
            play_type: None,
            difficulty: None,
            notes: None,
            level: None,
        }),
    );
    assert_eq!(
        select.summary().state,
        ResolverResolutionState::SongProjected
    );
    assert_eq!(
        summary
            .selected
            .as_ref()
            .map(|candidate| candidate.chart.key.play_type),
        Some(PlayType::Double)
    );
}

fn mode_conflict_fixture(
    select_mode: PlayType,
    result_mode: Option<PlayType>,
    notes: Option<u32>,
) -> HypothesisAccumulator {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000045\"").unwrap();
    let evidence = |family| JointEvidenceObservation {
        catalog_song_count: 1,
        candidates: [(PlayType::Single, 829), (PlayType::Double, 900)]
            .map(|(play_type, notes)| JointEvidenceCandidate {
                song_id,
                chart: crate::catalog::Chart {
                    key: crate::catalog::ChartKey {
                        play_type,
                        difficulty: Difficulty::Hyper,
                    },
                    level: 8,
                    notes,
                },
                display_titles: vec!["Wizards!".to_owned()],
                artist: "ARTIST".to_owned(),
                family_support: BTreeMap::from([(family, 300)]),
                support: 300,
            })
            .to_vec(),
    };
    let mut joint = HypothesisAccumulator::default();
    for sequence in [1, 2] {
        joint.observe_at(
            sequence,
            sequence * 100,
            &evidence(EvidenceFamily::SelectTitle),
            Some(Difficulty::Hyper),
            Some(select_mode),
            None,
        );
    }
    for sequence in [3, 4] {
        joint.observe_at(
            sequence,
            sequence * 100,
            &evidence(EvidenceFamily::ResultTitle),
            None,
            None,
            Some(ResultChartFactor {
                play_type: result_mode,
                difficulty: Some(Difficulty::Hyper),
                notes,
                level: Some(8),
            }),
        );
    }
    joint
}

#[test]
fn select_result_mode_conflicts_follow_chart_evidence_in_both_directions() {
    for (select_mode, result_mode) in [
        (PlayType::Single, PlayType::Double),
        (PlayType::Double, PlayType::Single),
    ] {
        for (notes, expected) in [(829, PlayType::Single), (900, PlayType::Double)] {
            let summary =
                mode_conflict_fixture(select_mode, Some(result_mode), Some(notes)).summary();
            assert_eq!(summary.state, ResolverResolutionState::AcceptedJoint);
            assert_eq!(summary.select_play_type, Some(select_mode));
            assert_eq!(summary.result_play_type, Some(result_mode));
            assert_eq!(summary.selected.unwrap().chart.key.play_type, expected);
        }
    }
}

#[test]
fn mode_conflicts_require_the_existing_chart_margin() {
    let mut joint = mode_conflict_fixture(PlayType::Single, Some(PlayType::Double), None);
    assert_eq!(
        joint.summary().state,
        ResolverResolutionState::SongProjected
    );
    assert_eq!(joint.summary().chart_margin, 0);
    for candidate in joint.candidates.values_mut() {
        if candidate.candidate.chart.key.play_type == PlayType::Double {
            candidate
                .family_support
                .insert(EvidenceFamily::ResultArtist, 20);
        }
    }
    assert_eq!(joint.summary().chart_margin, 20);
    assert_eq!(
        joint.summary().state,
        ResolverResolutionState::SongProjected
    );
}

#[test]
fn select_mode_supplements_unknown_result_mode() {
    let summary = mode_conflict_fixture(PlayType::Double, None, None).summary();
    assert_eq!(summary.state, ResolverResolutionState::AcceptedJoint);
    assert_eq!(summary.result_play_type, None);
    assert_eq!(
        summary.selected.unwrap().chart.key.play_type,
        PlayType::Double
    );
    assert_eq!(
        summary.selected_family_support[&EvidenceFamily::SelectPlayType].normalized(),
        100
    );
}
fn play_options_observation(values: Vec<PlayOption>) -> PlayOptionsObservation {
    PlayOptionsObservation {
        parsed: PlayOptions::Known { values },
        ..PlayOptionsObservation::default()
    }
}

#[test]
fn play_options_require_two_matching_episode_observations() {
    let expected = vec![PlayOption::Random, PlayOption::Legacy];
    let mut accumulator = PlayOptionsEpisodeAccumulator::default();
    accumulator.observe(1, play_options_observation(expected.clone()));
    accumulator.observe(1, play_options_observation(expected.clone()));
    assert_eq!(
        accumulator.resolved(),
        PlayOptions::Unknown {
            reason: PlayOptionsUnknownReason::InsufficientObservations
        }
    );
    accumulator.observe(2, play_options_observation(expected.clone()));
    assert_eq!(
        accumulator.resolved(),
        PlayOptions::Known { values: expected }
    );
}

#[test]
fn conflicting_play_options_remain_optional_unknown() {
    let mut accumulator = PlayOptionsEpisodeAccumulator::default();
    accumulator.observe(1, play_options_observation(vec![PlayOption::Random]));
    accumulator.observe(2, play_options_observation(vec![PlayOption::Mirror]));
    assert_eq!(
        accumulator.resolved(),
        PlayOptions::Unknown {
            reason: PlayOptionsUnknownReason::ConflictingObservations
        }
    );
}

#[test]
fn selection_epoch_hands_off_only_the_latest_unfinished_successor() {
    let song = |suffix: u8| {
        serde_json::from_str(&format!("\"00000000-0000-0000-0000-{suffix:012}\"")).unwrap()
    };
    let observation = |song_id, support| JointEvidenceObservation {
        catalog_song_count: 100,
        candidates: vec![JointEvidenceCandidate {
            song_id,
            chart: crate::catalog::Chart {
                key: crate::catalog::ChartKey {
                    play_type: PlayType::Single,
                    difficulty: Difficulty::Hyper,
                },
                level: 8,
                notes: 764,
            },
            display_titles: vec![format!("SONG {song_id:?}")],
            artist: "ARTIST".to_owned(),
            family_support: BTreeMap::from([(EvidenceFamily::SelectTitleLexical, support)]),
            support,
        }],
    };
    let incumbent = song(1);
    let successor = song(2);
    let mut epochs = SelectionEpochTracker::default();
    epochs.observe(100, &observation(incumbent, 300), None);
    epochs.observe(200, &observation(successor, 70), None);
    assert_eq!(
        epochs.handoff().summary().selected.unwrap().song_id,
        successor
    );

    epochs.observe(300, &observation(incumbent, 300), None);
    assert!(epochs.successor.candidates.is_empty());
    assert_eq!(
        epochs.handoff().summary().selected.unwrap().song_id,
        incumbent
    );
}

#[test]
fn markerless_successor_is_active_without_inheriting_incumbent_difficulty() {
    let song = |suffix: u8| {
        serde_json::from_str(&format!("\"00000000-0000-0000-0000-{suffix:012}\"")).unwrap()
    };
    let observation = |song_id, support| JointEvidenceObservation {
        catalog_song_count: 100,
        candidates: vec![JointEvidenceCandidate {
            song_id,
            chart: crate::catalog::Chart {
                key: crate::catalog::ChartKey {
                    play_type: PlayType::Single,
                    difficulty: Difficulty::Hyper,
                },
                level: 8,
                notes: 764,
            },
            display_titles: vec!["TEST".to_owned()],
            artist: "ARTIST".to_owned(),
            family_support: BTreeMap::from([(EvidenceFamily::SelectTitle, support)]),
            support,
        }],
    };
    let incumbent = song(1);
    let successor = song(2);
    let mut epochs = SelectionEpochTracker::default();
    epochs.observe_at(
        100,
        1_000,
        &observation(incumbent, 300),
        Some(Difficulty::Hyper),
    );
    let successor_started = epochs.observe_at(200, 2_000, &observation(successor, 70), None);
    assert_eq!(
        epochs.active_difficulty_state(),
        Some((SelectionDifficultyTarget::Successor, None))
    );
    assert!(successor_started.iter().any(|transition| {
        transition.target == SelectionDifficultyTarget::Successor
            && transition.reason == SelectionDifficultyTransitionReason::TargetSwitch
            && transition.current.is_none()
    }));

    let incumbent_resumed = epochs.observe_at(300, 3_000, &observation(incumbent, 300), None);
    assert_eq!(
        epochs
            .active_difficulty_state()
            .unwrap()
            .1
            .unwrap()
            .difficulty,
        Difficulty::Hyper
    );
    assert!(incumbent_resumed.iter().any(|transition| {
        transition.target == SelectionDifficultyTarget::Incumbent
            && transition.reason == SelectionDifficultyTransitionReason::TargetSwitch
            && transition.current.is_some()
    }));
}

#[test]
fn accepted_hypothesis_can_return_to_conflict_on_new_contradictory_evidence() {
    let first_song = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
    let second_song = serde_json::from_str("\"00000000-0000-0000-0000-000000000002\"").unwrap();
    let candidate = |song_id, family| JointEvidenceCandidate {
        song_id,
        chart: crate::catalog::Chart {
            key: crate::catalog::ChartKey {
                play_type: PlayType::Single,
                difficulty: Difficulty::Hyper,
            },
            level: 8,
            notes: 764,
        },
        display_titles: vec!["TEST".to_owned()],
        artist: "ARTIST".to_owned(),
        family_support: BTreeMap::from([(family, 300)]),
        support: 300,
    };
    let mut accumulator = HypothesisAccumulator::default();
    accumulator.observe(
        100,
        &JointEvidenceObservation {
            catalog_song_count: 0,
            candidates: vec![candidate(first_song, EvidenceFamily::ResultTitle)],
        },
        None,
        None,
    );
    for monotonic_ms in [101, 102] {
        accumulator.observe(
            monotonic_ms,
            &JointEvidenceObservation {
                catalog_song_count: 0,
                candidates: Vec::new(),
            },
            None,
            Some(ResultChartFactor {
                play_type: Some(PlayType::Single),
                difficulty: None,
                notes: None,
                level: None,
            }),
        );
    }
    assert_eq!(
        accumulator.summary().state,
        ResolverResolutionState::AcceptedJoint
    );
    accumulator.observe(
        200,
        &JointEvidenceObservation {
            catalog_song_count: 0,
            candidates: vec![candidate(second_song, EvidenceFamily::ResultArtist)],
        },
        None,
        None,
    );
    assert_eq!(
        accumulator.summary().state,
        ResolverResolutionState::Conflict
    );
}

fn music_selection_test_observation() -> JointEvidenceObservation {
    let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000046\"").unwrap();
    JointEvidenceObservation {
        catalog_song_count: 2,
        candidates: vec![JointEvidenceCandidate {
            song_id,
            chart: crate::catalog::Chart {
                key: crate::catalog::ChartKey {
                    play_type: PlayType::Double,
                    difficulty: Difficulty::Hyper,
                },
                level: 8,
                notes: 829,
            },
            display_titles: vec!["Wizards!".to_owned()],
            artist: "ARTIST".to_owned(),
            family_support: BTreeMap::from([(EvidenceFamily::SelectTitle, 300)]),
            support: 300,
        }],
    }
}

#[test]
fn music_selection_requires_two_equal_play_sides_and_rejects_a_conflict() {
    let mut resolver = MusicSelectResolver::default();
    let mut evidence = music_selection_test_observation();
    evidence.candidates[0].chart.key.play_type = PlayType::Single;
    resolver.observe(
        1,
        100,
        &evidence,
        Some(Difficulty::Hyper),
        Some(evidence.candidates[0].chart.key.play_type),
        Some(PlaySide::TwoPlayer),
    );
    assert!(resolver.selected().is_none());
    resolver.observe(
        2,
        200,
        &evidence,
        Some(Difficulty::Hyper),
        Some(evidence.candidates[0].chart.key.play_type),
        Some(PlaySide::TwoPlayer),
    );
    assert!(matches!(
        resolver.selected(),
        Some(MusicSelectionState::Selected {
            play_side: PlaySide::TwoPlayer,
            ..
        })
    ));

    resolver.observe(
        3,
        300,
        &evidence,
        Some(Difficulty::Hyper),
        Some(evidence.candidates[0].chart.key.play_type),
        Some(PlaySide::OnePlayer),
    );
    assert!(resolver.selected().is_none());
}

#[test]
fn double_play_selection_requires_a_footer_play_side() {
    let mut resolver = MusicSelectResolver::default();
    let evidence = music_selection_test_observation();
    for (sequence, monotonic_ms) in [(1, 100), (2, 200)] {
        resolver.observe(
            sequence,
            monotonic_ms,
            &evidence,
            Some(Difficulty::Hyper),
            Some(evidence.candidates[0].chart.key.play_type),
            if sequence == 1 {
                None
            } else {
                Some(PlaySide::TwoPlayer)
            },
        );
    }
    assert!(resolver.selected().is_none());
    resolver.observe(
        3,
        300,
        &evidence,
        Some(Difficulty::Hyper),
        Some(evidence.candidates[0].chart.key.play_type),
        Some(PlaySide::TwoPlayer),
    );
    assert!(matches!(
        resolver.selected(),
        Some(MusicSelectionState::Selected {
            play_side: PlaySide::TwoPlayer,
            play_type: PlayType::Double,
            ..
        })
    ));
}

#[test]
fn result_panel_side_requires_two_fresh_matches_and_two_opposites_conflict() {
    let mut side = ResultPanelSideAccumulator::default();
    assert_eq!(
        side.observe(7, 10, ResultPanelSide::Right),
        Some((
            ResultPanelSideEpisodeState::Pending,
            ResultPanelSideTransitionReason::CandidateStarted,
        ))
    );
    assert_eq!(side.stable(), None);
    assert_eq!(side.observe(7, 10, ResultPanelSide::Right), None);
    assert_eq!(
        side.observe(7, 11, ResultPanelSide::Right),
        Some((
            ResultPanelSideEpisodeState::Stable {
                side: ResultPanelSide::Right,
            },
            ResultPanelSideTransitionReason::Accepted,
        ))
    );
    assert_eq!(side.stable(), Some(ResultPanelSide::Right));
    assert_eq!(
        side.observe(7, 12, ResultPanelSide::Left),
        Some((
            ResultPanelSideEpisodeState::Stable {
                side: ResultPanelSide::Right,
            },
            ResultPanelSideTransitionReason::OppositeObserved,
        ))
    );
    assert_eq!(side.stable(), Some(ResultPanelSide::Right));
    assert_eq!(side.observe(7, 13, ResultPanelSide::Right), None);
    assert_eq!(
        side.observe(7, 14, ResultPanelSide::Left),
        Some((
            ResultPanelSideEpisodeState::Conflicted {
                stable_side: ResultPanelSide::Right,
            },
            ResultPanelSideTransitionReason::Conflict,
        ))
    );
    assert_eq!(side.stable(), None);
    side.start_episode(8);
    assert_eq!(side.stable(), None);
}

#[test]
fn pre_stable_candidates_do_not_count_as_opposite_evidence() {
    let mut side = ResultPanelSideAccumulator::default();
    assert!(side.observe(7, 10, ResultPanelSide::Left).is_some());
    assert!(side.observe(7, 11, ResultPanelSide::Right).is_some());
    assert_eq!(
        side.observe(7, 12, ResultPanelSide::Right),
        Some((
            ResultPanelSideEpisodeState::Stable {
                side: ResultPanelSide::Right,
            },
            ResultPanelSideTransitionReason::Accepted,
        ))
    );
    assert_eq!(
        side.observe(7, 13, ResultPanelSide::Left),
        Some((
            ResultPanelSideEpisodeState::Stable {
                side: ResultPanelSide::Right,
            },
            ResultPanelSideTransitionReason::OppositeObserved,
        ))
    );
    assert_eq!(side.stable(), Some(ResultPanelSide::Right));
    assert!(matches!(
        side.observe(7, 14, ResultPanelSide::Left),
        Some((
            ResultPanelSideEpisodeState::Conflicted { .. },
            ResultPanelSideTransitionReason::Conflict
        ))
    ));
}

#[test]
fn result_play_side_serializes_plain_side_for_sp_and_dp() {
    assert_eq!(
        serde_json::to_value(result_play_side(Some(ResultPanelSide::Left)).unwrap()).unwrap(),
        json!("one_player")
    );
    assert_eq!(
        serde_json::to_value(result_play_side(Some(ResultPanelSide::Right)).unwrap()).unwrap(),
        json!("two_player")
    );
}
