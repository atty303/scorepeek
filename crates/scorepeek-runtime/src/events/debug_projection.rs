//! Human-oriented diagnostic formatting of core state facts.

use std::collections::BTreeMap;

use scorepeek_core::catalog::{Difficulty, PlayType};
use scorepeek_core::event::{
    AttemptNodeSnapshot, DomainSnapshot, EvidenceContribution, GateDecision, GateKind,
    ResolverNodeSnapshot, ResolverScope,
};
use scorepeek_core::recognition::shared::{EvidenceFamily, JointEvidenceCandidate};
use scorepeek_core::session::attempt::PlayAttemptState;
use serde_json::{Value, json};

fn candidate_label(candidate: &JointEvidenceCandidate) -> String {
    let title = candidate.display_titles.first().map_or("?", String::as_str);
    let play_type = match candidate.chart.key.play_type {
        PlayType::Single => "SP",
        PlayType::Double => "DP",
    };
    let difficulty = match candidate.chart.key.difficulty {
        Difficulty::Beginner => "BEGINNER",
        Difficulty::Normal => "NORMAL",
        Difficulty::Hyper => "HYPER",
        Difficulty::Another => "ANOTHER",
        Difficulty::Leggendaria => "LEGGENDARIA",
    };
    format!(
        "{title} / {play_type:?} {difficulty:?} Lv{} notes={}",
        candidate.chart.level, candidate.chart.notes
    )
}

fn candidate(candidate: Option<&JointEvidenceCandidate>) -> Option<String> {
    candidate.map(candidate_label)
}

fn family_contributions(
    contributions: &BTreeMap<EvidenceFamily, EvidenceContribution>,
) -> Vec<String> {
    let mut values = contributions
        .iter()
        .map(|(family, contribution)| {
            format!(
                "{}={}",
                format!("{family:?}").to_ascii_lowercase(),
                contribution.normalized()
            )
        })
        .collect::<Vec<_>>();
    values.sort();
    values
}

fn resolver_node(node: &ResolverNodeSnapshot) -> Value {
    let label = match node.scope {
        ResolverScope::SelectionIncumbent => "MUSIC SELECT resolver",
        ResolverScope::SelectionSuccessor => "successor",
        ResolverScope::Result => "RESULT resolver",
        ResolverScope::AttemptJoint => "attempt joint resolver",
    };
    json!({
        "label": label,
        "started_ms": node.started_ms,
        "last_observation_ms": node.last_observation_ms,
        "observations": node.observations,
        "top": candidate(node.top.as_ref()),
        "runner_up": candidate(node.runner_up.as_ref()),
        "runner_song": candidate(node.runner_song.as_ref()),
        "runner_chart": candidate(node.runner_chart.as_ref()),
        "top_candidates": node.top_candidates.iter().map(candidate_label).collect::<Vec<_>>(),
        "support": node.support, "margin": node.margin,
        "song_margin": node.song_margin, "chart_margin": node.chart_margin,
        "select_play_type": node.select_play_type,
        "result_play_type": node.result_play_type,
        "play_type_mismatch": node.play_type_mismatch,
        "family_contributions": family_contributions(&node.family_contributions),
        "current_difficulty": node.current_difficulty,
        "state": node.state,
    })
}

fn attempt_node(node: &AttemptNodeSnapshot) -> Value {
    let (phase, path) = match &node.attempt_state {
        PlayAttemptState::Idle => ("idle".to_owned(), String::new()),
        PlayAttemptState::UnlinkedResult { .. } => ("unlinked_result".to_owned(), "R".to_owned()),
        PlayAttemptState::Attempt { attempt } => {
            let path = [
                (attempt.path.select_observed, 'S'),
                (attempt.path.decide_observed, 'D'),
                (attempt.path.play_observed, 'P'),
                (attempt.path.result_observed, 'R'),
            ]
            .into_iter()
            .filter_map(|(observed, label)| observed.then_some(label.to_string()))
            .collect::<Vec<_>>()
            .join("-");
            (format!("{:?}", attempt.phase).to_ascii_lowercase(), path)
        }
    };
    json!({
        "attempt_id": node.attempt_id,
        "started_ms": node.started_ms,
        "phase_started_ms": node.phase_started_ms,
        "phase": phase, "path": path,
        "select_top": candidate(node.select_top.as_ref()),
        "result_top": candidate(node.result_top.as_ref()),
        "joint_top": candidate(node.joint_top.as_ref()),
        "support": node.support, "margin": node.margin,
        "song_margin": node.song_margin, "chart_margin": node.chart_margin,
        "runner_song": candidate(node.runner_song.as_ref()),
        "runner_chart": candidate(node.runner_chart.as_ref()),
        "top_candidates": node.top_candidates.iter().map(candidate_label).collect::<Vec<_>>(),
        "family_contributions": family_contributions(&node.family_contributions),
        "state": node.state,
    })
}

fn gate_label(kind: GateKind) -> &'static str {
    match kind {
        GateKind::Link => "link",
        GateKind::Identity => "identity",
        GateKind::Clear => "clear",
        GateKind::Numeric => "numeric",
        GateKind::Drain => "drain",
        GateKind::Emit => "emit",
    }
}

fn gate_label_text(decision: GateDecision) -> &'static str {
    match decision {
        GateDecision::ResultConfirmed => "accepted: result confirmed",
        GateDecision::JointIdentityPending => "waiting: joint identity",
        GateDecision::NumericPending => "waiting: numeric performance",
        GateDecision::LinkedAttemptPending => "waiting: linked play attempt",
        GateDecision::Ready => "ready: domain promotion",
    }
}

pub(super) fn snapshot(snapshot: &DomainSnapshot) -> Value {
    json!({
        "now_ms": snapshot.now_ms,
        "raw_screen": snapshot.raw_screen,
        "screen": snapshot.screen,
        "suspended": snapshot.suspended,
        "finalizing": snapshot.finalizing,
        "screen_episode_id": snapshot.screen_episode_id,
        "screen_episode_started_ms": snapshot.screen_episode_started_ms,
        "source_sequence": snapshot.source_sequence,
        "latest_field_sequence": snapshot.latest_field_sequence,
        "latest_field_ms": snapshot.latest_field_ms,
        "selection_difficulty_target": snapshot.selection_difficulty_target,
        "selection_difficulty": snapshot.selection_difficulty,
        "local": snapshot.local.as_ref().map(resolver_node),
        "successor": snapshot.successor.as_ref().map(resolver_node),
        "attempt": snapshot.attempt.as_ref().map(attempt_node),
        "gate": gate_label_text(snapshot.gate),
        "gates": snapshot.gates.iter().map(|gate| json!({
            "label": gate_label(gate.kind), "state": gate.state,
        })).collect::<Vec<_>>(),
        "play_options": snapshot.play_options,
    })
}
