//! Projection of typed domain decisions into runtime diagnostic and public events.

use super::{RUN_EVENT_SCHEMA, RunEvent, RunEventKind};
use scorepeek_core::event::DomainTransitionKind;

#[allow(
    clippy::too_many_lines,
    reason = "the projection covers every domain transition variant exhaustively"
)]
pub(super) fn run_event(transition: &DomainTransitionKind) -> RunEvent {
    let kind = match transition.clone() {
        DomainTransitionKind::MusicSelectBestObserved {
            session_id,
            snapshot,
        } => RunEventKind::MusicSelectBestObserved {
            session_id,
            snapshot,
        },
        DomainTransitionKind::MusicSelectResolverChanged { session_id, state } => {
            RunEventKind::MusicSelectResolverChanged { session_id, state }
        }
        DomainTransitionKind::ResultChanged {
            session_id,
            source_sequence,
            state,
        } => RunEventKind::ResultChanged {
            session_id,
            source_sequence,
            state,
        },
        DomainTransitionKind::ResultPanelSideChanged {
            session_id,
            screen_episode_id,
            source_sequence,
            state,
            reason,
        } => RunEventKind::ResultPanelSideChanged {
            session_id,
            screen_episode_id,
            source_sequence,
            state,
            reason,
        },
        DomainTransitionKind::ResultSelectContextMismatch {
            session_id,
            screen_episode_id,
            source_sequence,
            select_play_side,
            result_play_side,
        } => RunEventKind::ResultSelectContextMismatch {
            session_id,
            screen_episode_id,
            source_sequence,
            select_play_side,
            result_play_side,
        },
        DomainTransitionKind::MusicSelectionChanged {
            session_id,
            screen_episode_id,
            source_sequence,
            revision,
            state,
        } => RunEventKind::MusicSelectionChanged {
            session_id,
            screen_episode_id,
            source_sequence,
            revision,
            state,
        },
        DomainTransitionKind::TemporalResultChanged {
            session_id,
            source_sequence,
            transitions,
            state,
            stable_song,
        } => RunEventKind::TemporalResultChanged {
            session_id,
            source_sequence,
            transitions,
            state,
            stable_song,
        },
        DomainTransitionKind::TemporalMusicSelectChanged {
            session_id,
            source_sequence,
            reasons,
            state,
            retained_song,
            candidate_song,
        } => RunEventKind::TemporalMusicSelectChanged {
            session_id,
            source_sequence,
            reasons,
            state,
            retained_song,
            candidate_song,
        },
        DomainTransitionKind::NumericResultChanged {
            session_id,
            source_sequence,
            state,
            reason,
            event_suppression_reason,
        } => RunEventKind::NumericResultChanged {
            session_id,
            source_sequence,
            state,
            reason,
            event_suppression_reason,
        },
        DomainTransitionKind::PlayAttemptChanged {
            session_id,
            source_sequence,
            state,
        } => RunEventKind::PlayAttemptChanged {
            session_id,
            source_sequence,
            state,
        },
        DomainTransitionKind::ResolverStateChanged {
            session_id,
            screen_episode_id,
            source_sequence,
            scope,
            state,
            select_play_type,
            result_play_type,
            play_type_mismatch,
            top,
            runner_up,
            runner_song,
            runner_chart,
            top_candidates,
            support,
            margin,
            song_margin,
            chart_margin,
            selected_family_support,
            runner_up_family_support,
            observation_count,
        } => RunEventKind::ResolverStateChanged {
            session_id,
            screen_episode_id,
            source_sequence,
            scope,
            state,
            select_play_type,
            result_play_type,
            play_type_mismatch,
            top,
            runner_up,
            runner_song,
            runner_chart,
            top_candidates,
            support,
            margin,
            song_margin,
            chart_margin,
            selected_family_support,
            runner_up_family_support,
            observation_count,
        },
        DomainTransitionKind::SelectionDifficultyChanged {
            session_id,
            screen_episode_id,
            source_sequence,
            target,
            reason,
            current,
        } => RunEventKind::SelectionDifficultyChanged {
            session_id,
            screen_episode_id,
            source_sequence,
            target,
            reason,
            current,
        },
    };
    RunEvent {
        schema: RUN_EVENT_SCHEMA.to_owned(),
        kind,
    }
}
