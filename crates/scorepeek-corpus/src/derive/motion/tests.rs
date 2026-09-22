use scorepeek_core::replay::{CanonicalLayout, Roi, ScreenClass};
use scorepeek_runtime::capture::{
    FractionalRectangle, GamescopeProfileBinding, MeasuredGamescopeProfileBindingAuthoringInput,
    RationalCoordinate,
};

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::os::unix::fs::MetadataExt as _;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;

use super::{
    CorrectSongExpectation, CorrectSongLabel, CorrectSongLabels, MAX_PROCESS_STDERR_BYTES,
    MotionEvidence, MotionReviewDecision, MotionReviewDecisions, MusicSelectDwellPolicy,
    MusicSelectTemporalCandidatePolicy, OBSERVATION_SCHEMA, ObservationRecord, OperatorReviewState,
    RegionMotion, ReviewCompleteness, ReviewState, ReviewedMotionPair, ReviewedMotionSet,
    ReviewedMotionSpan, VideoIdentity, apply_music_select_motion_review, canonical_line,
    digest_bytes, evaluate_correctness_runs, evaluate_dwell_policy, parse_showinfo_pts,
    plan_music_select_motion_review, read_bounded_stream, region_motion_packed,
    replay_temporal_states, review_windows, run_bounded_output, select_expression,
    selected_frame_targets, stationary_runs, stored_screen, supported_observation_schema,
    validate_correct_song_labels, validate_reviewed_motion_set, verify_video_unchanged,
};

#[test]
fn music_select_readers_accept_only_the_corpus_observation_schema() {
    assert!(supported_observation_schema(&serde_json::Value::String(
        OBSERVATION_SCHEMA.to_owned()
    )));
    assert!(!supported_observation_schema(&serde_json::Value::String(
        "scorepeek-recognition-observation-v23".to_owned()
    )));
}

#[test]
fn dwell_candidate_records_nonstationary_stability_and_resets_on_identity_change() {
    let reviewed = synthetic_reviewed_set();
    let observations = synthetic_dwell_observations(&reviewed);
    let result = evaluate_dwell_policy(
        &reviewed,
        &observations,
        3,
        MusicSelectDwellPolicy::new(200).unwrap(),
    )
    .unwrap();
    assert_eq!(result.stationary_runs, 3);
    assert_eq!(result.stabilized_runs, 3);
    assert_eq!(result.unresolved_stationary_runs, 0);
    assert_eq!(result.stabilization_latency_ms.samples, 2);
    assert_eq!(result.stabilization_latency_ms.minimum, Some(200));
    assert_eq!(result.resets.scrolling_pairs_with_prior_stability, 1);
    assert_eq!(result.resets.scrolling_resets, 0);
    assert_eq!(result.resets.selection_changes_with_prior_stability, 1);
    assert_eq!(result.resets.selection_change_resets, 1);
    assert_eq!(result.resets.missed_selection_change_resets, 0);
    assert_eq!(result.resets.predicate_context_resets, 1);
    assert_eq!(result.stable_nonstationary_pairs.total, 1);
    assert_eq!(result.stable_nonstationary_pairs.scrolling, 1);
    assert_eq!(result.stabilizations_on_nonstationary_pairs.total, 0);
    assert_eq!(result.candidate_replacements, 1);
}

#[test]
fn dwell_evaluation_requires_complete_canonical_review_truth() {
    let reviewed = synthetic_reviewed_set();
    let bytes = canonical_line(&reviewed).unwrap();
    let denominators = validate_reviewed_motion_set(&reviewed, &bytes).unwrap();
    assert_eq!(denominators.stationary_pairs, 5);
    assert_eq!(denominators.scrolling_pairs, 1);
    assert_eq!(denominators.selection_change_pairs, 1);
    assert_eq!(denominators.operator_context_pairs, 1);
    assert_eq!(denominators.predicate_context_pairs, 1);
    let mut incomplete = synthetic_reviewed_set();
    incomplete.completeness.complete = false;
    assert!(
        validate_reviewed_motion_set(&incomplete, &canonical_line(&incomplete).unwrap()).is_err()
    );
}

#[test]
fn dwell_evaluation_rejects_a_mismatched_first_pair_endpoint() {
    let mut reviewed = synthetic_reviewed_set();
    reviewed.spans[0].pairs[0].previous_timestamp_ms = 1;
    reviewed.spans[0].pairs[0].motion.gap_ms = 99;
    let bytes = canonical_line(&reviewed).unwrap();
    assert!(validate_reviewed_motion_set(&reviewed, &bytes).is_ok());
    let observations = synthetic_dwell_observations(&synthetic_reviewed_set());
    assert!(
        evaluate_dwell_policy(
            &reviewed,
            &observations,
            3,
            MusicSelectDwellPolicy::new(200).unwrap(),
        )
        .is_err()
    );
}

#[test]
fn correctness_keeps_non_song_selection_out_of_coverage_and_counts_stability_as_wrong() {
    let reviewed = synthetic_reviewed_set();
    let observations = synthetic_dwell_observations(&reviewed);
    let runs = stationary_runs(&reviewed);
    let song_a = song_id(1);
    let song_b = song_id(2);
    let labels = [
        CorrectSongLabel {
            span_id: runs[0].span_id.clone(),
            first_sequence: runs[0].first_sequence,
            last_sequence: runs[0].last_sequence,
            expected: CorrectSongExpectation::Song {
                scorepeek_song_id: song_a,
            },
        },
        CorrectSongLabel {
            span_id: runs[1].span_id.clone(),
            first_sequence: runs[1].first_sequence,
            last_sequence: runs[1].last_sequence,
            expected: CorrectSongExpectation::Song {
                scorepeek_song_id: song_a,
            },
        },
        CorrectSongLabel {
            span_id: runs[2].span_id.clone(),
            first_sequence: runs[2].first_sequence,
            last_sequence: runs[2].last_sequence,
            expected: CorrectSongExpectation::NotSongSelection,
        },
    ];
    let policy = MusicSelectTemporalCandidatePolicy::new(200, 200).unwrap();
    let replay = replay_temporal_states(&reviewed, &observations, policy).unwrap();
    let stable = confirmed_temporal_states(&replay);
    let evaluation =
        evaluate_correctness_runs(&runs, &labels, &observations, &stable, policy, &replay).unwrap();

    assert_eq!(evaluation.denominators.stationary_runs, 3);
    assert_eq!(evaluation.denominators.expected_song_runs, 2);
    assert_eq!(evaluation.denominators.non_song_selection_runs, 1);
    assert_eq!(evaluation.raw.non_song_selection_runs_with_output, 1);
    assert_eq!(
        evaluation.candidate.expected_song_runs_stabilized_correct,
        2
    );
    assert_eq!(evaluation.candidate.non_song_selection_runs_stabilized, 1);
    assert_eq!(evaluation.candidate.aggregate.outcomes.incorrect, 1);
    assert_eq!(
        evaluation.candidate.wrong_stable_streak_duration_ms.maximum,
        Some(0)
    );
    assert_eq!(
        evaluation.candidate.runs[2].expected,
        CorrectSongExpectation::NotSongSelection
    );
    assert_eq!(evaluation.candidate.runs[2].candidate.outcomes.incorrect, 1);
    assert_eq!(song_b, observations[&8].accepted_song_id.unwrap());
}

#[test]
fn correctness_requires_one_ordered_catalog_bound_label_per_stationary_run() {
    let reviewed = synthetic_reviewed_set();
    let reviewed_sha256 = digest_bytes(&canonical_line(&reviewed).unwrap());
    let runs = stationary_runs(&reviewed);
    let song = song_id(1);
    let make_labels = |runs: &[super::StationaryRun]| CorrectSongLabels {
        schema: super::CORRECTNESS_LABEL_SCHEMA.to_owned(),
        source_reviewed_sha256: reviewed_sha256.clone(),
        labels: runs
            .iter()
            .map(|run| CorrectSongLabel {
                span_id: run.span_id.clone(),
                first_sequence: run.first_sequence,
                last_sequence: run.last_sequence,
                expected: CorrectSongExpectation::Song {
                    scorepeek_song_id: song,
                },
            })
            .collect(),
        authority: "operator_review".to_owned(),
    };
    let labels = make_labels(&runs);
    let bytes = canonical_line(&labels).unwrap();
    assert!(
        validate_correct_song_labels(
            &labels,
            &bytes,
            &reviewed_sha256,
            &runs,
            &BTreeSet::from([song]),
        )
        .is_ok()
    );
    let mut incomplete = make_labels(&runs);
    incomplete.labels.pop();
    assert!(
        validate_correct_song_labels(
            &incomplete,
            &canonical_line(&incomplete).unwrap(),
            &reviewed_sha256,
            &runs,
            &BTreeSet::from([song]),
        )
        .is_err()
    );
    assert!(
        validate_correct_song_labels(&labels, &bytes, &reviewed_sha256, &runs, &BTreeSet::new(),)
            .is_err()
    );
}

#[test]
fn correctness_reports_frame_identity_jitter_that_dwell_does_not_promote() {
    let reviewed = synthetic_reviewed_set();
    let mut observations = synthetic_dwell_observations(&reviewed);
    observations.get_mut(&2).unwrap().accepted_song_id = Some(song_id(2));
    let runs = stationary_runs(&reviewed);
    let labels = runs
        .iter()
        .enumerate()
        .map(|(index, run)| CorrectSongLabel {
            span_id: run.span_id.clone(),
            first_sequence: run.first_sequence,
            last_sequence: run.last_sequence,
            expected: CorrectSongExpectation::Song {
                scorepeek_song_id: if index == 2 { song_id(2) } else { song_id(1) },
            },
        })
        .collect::<Vec<_>>();
    let policy = MusicSelectTemporalCandidatePolicy::new(200, 200).unwrap();
    let replay = replay_temporal_states(&reviewed, &observations, policy).unwrap();
    let stable = confirmed_temporal_states(&replay);
    let evaluation =
        evaluate_correctness_runs(&runs, &labels, &observations, &stable, policy, &replay).unwrap();

    assert_eq!(
        evaluation.candidate.runs[0]
            .raw
            .accepted_identity_transitions,
        2
    );
    assert_eq!(
        evaluation.candidate.runs[0]
            .candidate
            .accepted_identity_transitions,
        0
    );
    assert_eq!(evaluation.candidate.runs[0].candidate.outcomes.incorrect, 0);
    assert_eq!(evaluation.raw.accepted_identity_transitions, 2);
    assert_eq!(
        evaluation.candidate.aggregate.accepted_identity_transitions,
        0
    );
}

#[test]
fn temporal_replay_resets_at_predicate_screen_context() {
    let reviewed = synthetic_reviewed_set();
    let observations = synthetic_dwell_observations(&reviewed);
    let replay = replay_temporal_states(
        &reviewed,
        &observations,
        MusicSelectTemporalCandidatePolicy::new(200, 200).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        replay
            .states
            .get(&("music-select-span-0001".to_owned(), 9))
            .unwrap(),
        scorepeek_core::replay::MusicSelectTemporalState::Empty
    ));
}

fn song_id(value: u8) -> scorepeek_core::replay::ScorepeekSongId {
    serde_json::from_str(&format!("\"00000000-0000-0000-0000-{value:012}\"")).unwrap()
}

fn confirmed_temporal_states(
    replay: &super::TemporalReplay,
) -> BTreeMap<(String, u64), Option<scorepeek_core::replay::ScorepeekSongId>> {
    replay
        .states
        .iter()
        .map(|(key, state)| (key.clone(), state.confirmed_value().copied()))
        .collect()
}

fn synthetic_dwell_observations(
    reviewed: &ReviewedMotionSet,
) -> BTreeMap<u64, super::DwellObservation> {
    let song_a = song_id(1);
    let song_b = song_id(2);
    let span = &reviewed.spans[0];
    let first = &span.pairs[0];
    let mut result = BTreeMap::from([(
        first.previous_sequence,
        super::DwellObservation {
            sequence: first.previous_sequence,
            timestamp_ms: first.previous_timestamp_ms,
            screen: first.previous_screen,
            accepted_song_id: Some(song_a),
        },
    )]);
    for pair in &span.pairs {
        let accepted_song_id = match pair.sequence {
            1..=5 => Some(song_a),
            6..=8 => Some(song_b),
            _ => None,
        };
        result.insert(
            pair.sequence,
            super::DwellObservation {
                sequence: pair.sequence,
                timestamp_ms: pair.source_timestamp_ms,
                screen: pair.screen,
                accepted_song_id,
            },
        );
    }
    result
}

fn synthetic_reviewed_set() -> ReviewedMotionSet {
    let states = [
        ("stationary", "operator_reviewed"),
        ("stationary", "operator_reviewed"),
        ("scrolling", "operator_reviewed"),
        ("stationary", "operator_reviewed"),
        ("selection_change", "operator_reviewed"),
        ("stationary", "operator_reviewed"),
        ("stationary", "operator_reviewed"),
        ("unknown", "predicate_screen_context"),
        ("unknown", "operator_screen_context"),
    ];
    let pairs = states
        .into_iter()
        .enumerate()
        .map(|(index, (state, reason))| {
            let previous_sequence = u64::try_from(index).unwrap() + 1;
            let context = reason == "predicate_screen_context";
            ReviewedMotionPair {
                previous_sequence,
                sequence: previous_sequence + 1,
                previous_timestamp_ms: u64::try_from(index).unwrap() * 100,
                source_timestamp_ms: (u64::try_from(index).unwrap() + 1) * 100,
                previous_screen: ScreenClass::MusicSelect,
                screen: if context {
                    ScreenClass::Unknown
                } else {
                    ScreenClass::MusicSelect
                },
                source_frame_index: index + 1,
                motion: MotionEvidence {
                    gap_ms: 100,
                    list_titles: empty_region_motion(),
                    active_list_title: empty_region_motion(),
                    central_title: empty_region_motion(),
                },
                review_state: ReviewState {
                    state: state.to_owned(),
                    reason: reason.to_owned(),
                },
            }
        })
        .collect();
    ReviewedMotionSet {
        schema: super::REVIEWED_SCHEMA.to_owned(),
        source_draft_sha256: "a".repeat(64),
        active_suite_sha256: "b".repeat(64),
        session_sha256: "c".repeat(64),
        source_session_id: "synthetic-session".to_owned(),
        video_sha256: "d".repeat(64),
        capture_profile_sha256: "e".repeat(64),
        normalizer_artifact_sha256: "f".repeat(64),
        canonical_layout_sha256: "1".repeat(64),
        sampling_interval_ms: 100,
        review_padding_ms: super::REVIEW_PADDING_MS,
        regions: CanonicalLayout::music_select_motion_regions().unwrap(),
        spans: vec![ReviewedMotionSpan {
            span_id: "music-select-span-0001".to_owned(),
            observed_first_sequence: 1,
            observed_last_sequence: 10,
            observed_first_timestamp_ms: 0,
            observed_last_timestamp_ms: 900,
            review_first_timestamp_ms: 0,
            review_last_timestamp_ms: 900,
            pairs,
        }],
        completeness: ReviewCompleteness {
            decision_interval_count: 5,
            reviewed_motion_pair_count: 7,
            operator_context_pair_count: 1,
            remaining_review_pair_count: 0,
            predicate_context_pair_count: 1,
            complete: true,
        },
        authority: "operator_review".to_owned(),
    }
}

const fn empty_region_motion() -> RegionMotion {
    RegionMotion {
        rgb_l1: 0,
        changed_pixels: 0,
        compared_pixels: 1,
        normalized_l1_ppm: 0,
    }
}

#[test]
fn review_windows_merge_overlapping_padding_around_fast_screen_flicker() {
    let records = [
        observation(1, 100, ScreenClass::Unknown),
        observation(2, 200, ScreenClass::MusicSelect),
        observation(3, 300, ScreenClass::MusicSelect),
        observation(4, 400, ScreenClass::Unknown),
        observation(8, 800, ScreenClass::MusicSelect),
    ];
    let windows = review_windows(&records);
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].observed_first_sequence, 2);
    assert_eq!(windows[0].observed_last_sequence, 8);
    assert_eq!(windows[0].review_first_timestamp_ms, 0);
    assert_eq!(windows[0].review_last_timestamp_ms, 1_300);
}

#[test]
fn stored_screen_accepts_non_music_path_contexts() {
    let mode = serde_json::json!({"screen": "mode_select"});
    let decide = serde_json::json!({"screen": "decide_transition"});
    let play = serde_json::json!({"screen": "play"});
    assert_eq!(
        stored_screen(&mode, "invalid").unwrap(),
        ScreenClass::ModeSelect
    );
    assert_eq!(
        stored_screen(&decide, "invalid").unwrap(),
        ScreenClass::DecideTransition
    );
    assert_eq!(stored_screen(&play, "invalid").unwrap(), ScreenClass::Play);
}

#[test]
fn packed_region_motion_counts_only_supplied_pixels() {
    let mut previous = vec![0_u8; 2 * 3];
    let mut current = previous.clone();
    let roi = Roi {
        x: 0,
        y: 0,
        width: 2,
        height: 1,
    };
    previous[0] = 10;
    current[0] = 20;
    let motion = region_motion_packed(&previous, &current, roi);
    assert_eq!(motion.changed_pixels, 1);
    assert_eq!(motion.rgb_l1, 10);
    assert_eq!(motion.compared_pixels, 2);
}

#[test]
fn selected_frames_reproduce_latest_frame_sampling_at_each_tick() {
    let target = observation(1, 99, ScreenClass::MusicSelect);
    let targets = BTreeMap::from([(1, (0, target))]);
    let selected = selected_frame_targets(&[0, 16, 99, 101], &targets).unwrap();
    assert_eq!(selected.keys().copied().collect::<Vec<_>>(), vec![2]);
    assert_eq!(selected[&2][0].1.timestamp_ms, 99);
}

#[test]
fn select_expression_compacts_regular_frame_runs_without_crossing_gaps() {
    assert_eq!(
        select_expression([6, 12, 18, 30, 36, 50]),
        "between(n\\,6\\,18)*not(mod(n-6\\,6))+eq(n\\,30)+eq(n\\,36)+eq(n\\,50)"
    );
}

#[test]
fn showinfo_pts_is_independent_bounded_decode_evidence() {
    let stderr = b"[Parsed_showinfo_1] n: 0 pts: 89800 pts_time:89.8 duration:16\n\
            [Parsed_showinfo_1] n: 1 pts: 89900 pts_time:89.9 duration:16\n";
    assert_eq!(parse_showinfo_pts(stderr).unwrap(), vec![89_800, 89_900]);
}

#[test]
fn process_stream_is_drained_but_retained_bytes_are_bounded() {
    let output = read_bounded_stream(&b"abcdef"[..], 4).unwrap();
    assert_eq!(output.bytes, b"abcd");
    assert!(output.truncated);
}

#[test]
fn bounded_process_timeout_reaps_the_child() {
    let mut command = Command::new("sleep");
    command.arg("10").stdin(Stdio::null());
    let error = run_bounded_output(
        &mut command,
        16,
        MAX_PROCESS_STDERR_BYTES,
        Duration::from_millis(10),
    )
    .unwrap_err();
    assert!(error.to_string().contains("execution_timeout"));
}

#[test]
fn final_video_check_rejects_path_replacement_even_with_equal_bytes() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("video.mkv");
    fs::write(&path, b"same bytes").unwrap();
    let original = File::open(&path).unwrap();
    let metadata = original.metadata().unwrap();
    let identity = VideoIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    let digest = digest_bytes(b"same bytes");
    fs::rename(&path, temporary.path().join("old.mkv")).unwrap();
    fs::write(&path, b"same bytes").unwrap();
    assert!(verify_video_unchanged(&path, identity, &digest).is_err());
}

#[test]
#[allow(clippy::too_many_lines)]
fn public_review_plan_verifies_bindings_decodes_and_publishes_create_only() {
    let temporary = TempDir::new().unwrap();
    let store = temporary.path().join("store");
    fs::create_dir_all(store.join("objects")).unwrap();
    fs::create_dir_all(store.join("sessions")).unwrap();
    fs::create_dir_all(store.join("suites")).unwrap();
    let video = temporary.path().join("source.mkv");
    let status = Command::new(crate::ingest::recording::find_executable("ffmpeg").unwrap())
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=1920x1080:r=10:d=0.4",
            "-c:v",
            "ffv1",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&video)
        .status()
        .unwrap();
    assert!(status.success());
    let video_sha256 = digest_bytes(&fs::read(&video).unwrap());
    let source_session_id = format!("video-{}", &video_sha256[..24]);
    let coordinate = |value| RationalCoordinate::new(value, 1).unwrap();
    let authored =
        GamescopeProfileBinding::author_measured(MeasuredGamescopeProfileBindingAuthoringInput {
            observed_width: 1_920,
            observed_height: 1_080,
            geometry: FractionalRectangle::new(
                coordinate(0),
                coordinate(0),
                coordinate(1_920),
                coordinate(1_080),
            ),
        })
        .unwrap();
    let profile =
        GamescopeProfileBinding::parse(&authored.bytes, &authored.artifact_sha256).unwrap();
    let profile_ref = write_object(&store, &authored.bytes);
    let run = serde_json::to_vec(&json!({
        "schema": "scorepeek-private-diagnostic-capture-start-v4",
        "run_id": source_session_id,
        "binding": {
            "capture_profile_sha256": authored.capture_profile_sha256,
            "normalizer_sha256": profile.normalizer_artifact_sha256(),
            "canonical_layout_sha256": CanonicalLayout::sha256(),
        },
        "source": {"kind": "video_replay", "video_sha256": video_sha256},
    }))
    .unwrap();
    let run_ref = write_object(&store, &run);
    let observations = [
            json!({"schema": super::OBSERVATION_SCHEMA, "tick_sequence": 1, "source_timestamp_ms": 100, "screen": "music_select"}),
            json!({"schema": super::OBSERVATION_SCHEMA, "tick_sequence": 2, "source_timestamp_ms": 200, "screen": "music_select"}),
            json!({"schema": super::OBSERVATION_SCHEMA, "tick_sequence": 3, "source_timestamp_ms": 300, "screen": "unknown"}),
        ]
        .into_iter()
        .flat_map(|value| {
            let mut bytes = serde_json::to_vec(&value).unwrap();
            bytes.push(b'\n');
            bytes
        })
        .collect::<Vec<_>>();
    let observations_ref = write_object(&store, &observations);
    let session = serde_json::to_vec(&json!({
        "schema": super::SESSION_SCHEMA,
        "source_kind": "video_replay",
        "source_session_id": source_session_id,
        "profile_sha256": profile.capture_profile_sha256(),
        "recognition_interval_ms": 100,
        "artifacts": [
            artifact("capture/profile.json", &profile_ref),
            artifact("capture/run.json", &run_ref),
            artifact("analysis/observations.ndjson", &observations_ref),
        ],
    }))
    .unwrap();
    let session_sha256 = digest_bytes(&session);
    fs::write(
        store
            .join("sessions")
            .join(format!("{session_sha256}.json")),
        &session,
    )
    .unwrap();
    let suite = serde_json::to_vec(&json!({
        "schema": super::SUITE_SCHEMA,
        "entries": [{"session_sha256": session_sha256}],
    }))
    .unwrap();
    let suite_sha256 = digest_bytes(&suite);
    fs::write(
        store.join("suites").join(format!("{suite_sha256}.json")),
        suite,
    )
    .unwrap();
    fs::write(
        store.join("active-suite.json"),
        serde_json::to_vec(&json!({
            "schema": super::ACTIVE_SCHEMA,
            "generation_sha256": suite_sha256,
        }))
        .unwrap(),
    )
    .unwrap();
    let output = temporary.path().join("review.json");
    let summary =
        plan_music_select_motion_review(&store, &session_sha256, &video, &output).unwrap();
    assert_eq!(summary.span_count, 1);
    assert_eq!(summary.sample_count, 3);
    assert_eq!(summary.motion_pair_count, 2);
    assert!(output.is_file());
    let decisions = MotionReviewDecisions {
        schema: super::DECISIONS_SCHEMA.to_owned(),
        source_draft_sha256: digest_bytes(&fs::read(&output).unwrap()),
        decisions: vec![MotionReviewDecision {
            span_id: "music-select-span-0001".to_owned(),
            first_sequence: 2,
            last_sequence: 2,
            state: OperatorReviewState::Stationary,
        }],
    };
    let decisions_path = temporary.path().join("decisions.json");
    fs::write(&decisions_path, canonical_line(&decisions).unwrap()).unwrap();
    let reviewed_path = temporary.path().join("reviewed.json");
    let applied =
        apply_music_select_motion_review(&output, &decisions_path, &reviewed_path).unwrap();
    assert_eq!(applied.decision_interval_count, 1);
    assert_eq!(applied.reviewed_motion_pair_count, 1);
    assert_eq!(applied.operator_context_pair_count, 0);
    assert_eq!(applied.remaining_review_pair_count, 0);
    assert_eq!(applied.predicate_context_pair_count, 1);
    assert!(applied.complete);
    let reviewed: serde_json::Value =
        serde_json::from_slice(&fs::read(&reviewed_path).unwrap()).unwrap();
    assert_eq!(reviewed["schema"], super::REVIEWED_SCHEMA);
    assert_eq!(
        reviewed["spans"][0]["pairs"][0]["review_state"]["state"],
        "stationary"
    );
    assert_eq!(
        reviewed["spans"][0]["pairs"][1]["review_state"]["reason"],
        "predicate_screen_context"
    );
    assert!(
        apply_music_select_motion_review(&output, &decisions_path, &reviewed_path).is_err(),
        "review application must not replace an existing set"
    );
    let operator_context = MotionReviewDecisions {
        schema: super::DECISIONS_SCHEMA.to_owned(),
        source_draft_sha256: digest_bytes(&fs::read(&output).unwrap()),
        decisions: vec![MotionReviewDecision {
            span_id: "music-select-span-0001".to_owned(),
            first_sequence: 2,
            last_sequence: 2,
            state: OperatorReviewState::ScreenContext,
        }],
    };
    let operator_context_path = temporary.path().join("operator-context.json");
    fs::write(
        &operator_context_path,
        canonical_line(&operator_context).unwrap(),
    )
    .unwrap();
    let operator_context_reviewed = temporary.path().join("operator-context-reviewed.json");
    let applied = apply_music_select_motion_review(
        &output,
        &operator_context_path,
        &operator_context_reviewed,
    )
    .unwrap();
    assert_eq!(applied.reviewed_motion_pair_count, 0);
    assert_eq!(applied.operator_context_pair_count, 1);
    assert_eq!(applied.remaining_review_pair_count, 0);
    assert_eq!(applied.predicate_context_pair_count, 1);
    assert!(applied.complete);
    let reviewed: serde_json::Value =
        serde_json::from_slice(&fs::read(operator_context_reviewed).unwrap()).unwrap();
    assert_eq!(
        reviewed["spans"][0]["pairs"][0]["review_state"]["reason"],
        "operator_screen_context"
    );
    let invalid_context = MotionReviewDecisions {
        schema: super::DECISIONS_SCHEMA.to_owned(),
        source_draft_sha256: digest_bytes(&fs::read(&output).unwrap()),
        decisions: vec![MotionReviewDecision {
            span_id: "music-select-span-0001".to_owned(),
            first_sequence: 3,
            last_sequence: 3,
            state: OperatorReviewState::SelectionChange,
        }],
    };
    let invalid_path = temporary.path().join("invalid-context.json");
    fs::write(&invalid_path, canonical_line(&invalid_context).unwrap()).unwrap();
    assert!(
        apply_music_select_motion_review(
            &output,
            &invalid_path,
            &temporary.path().join("invalid-reviewed.json")
        )
        .is_err(),
        "context pairs cannot receive an operator decision"
    );
    let overlapping = MotionReviewDecisions {
        schema: super::DECISIONS_SCHEMA.to_owned(),
        source_draft_sha256: digest_bytes(&fs::read(&output).unwrap()),
        decisions: vec![
            MotionReviewDecision {
                span_id: "music-select-span-0001".to_owned(),
                first_sequence: 2,
                last_sequence: 2,
                state: OperatorReviewState::Stationary,
            },
            MotionReviewDecision {
                span_id: "music-select-span-0001".to_owned(),
                first_sequence: 2,
                last_sequence: 2,
                state: OperatorReviewState::Scrolling,
            },
        ],
    };
    let overlapping_path = temporary.path().join("overlapping.json");
    fs::write(&overlapping_path, canonical_line(&overlapping).unwrap()).unwrap();
    assert!(
        apply_music_select_motion_review(
            &output,
            &overlapping_path,
            &temporary.path().join("overlapping-reviewed.json")
        )
        .is_err(),
        "overlapping decision intervals must fail closed"
    );
    assert!(plan_music_select_motion_review(&store, &session_sha256, &video, &output).is_err());
}

fn write_object(store: &std::path::Path, bytes: &[u8]) -> (String, usize) {
    let sha256 = digest_bytes(bytes);
    fs::write(store.join("objects").join(&sha256), bytes).unwrap();
    (sha256, bytes.len())
}

fn artifact(path: &str, object: &(String, usize)) -> serde_json::Value {
    json!({"source_path": path, "sha256": object.0, "bytes": object.1})
}

const fn observation(sequence: u64, timestamp_ms: u64, screen: ScreenClass) -> ObservationRecord {
    ObservationRecord {
        sequence,
        timestamp_ms,
        screen,
    }
}
