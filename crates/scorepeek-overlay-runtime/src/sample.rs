//! Native editor preview data.
use scorepeek_overlay::{
    BestView, Chart, GraphPlay, GraphTick, History, HistoryPlay, LampState, OverlayState,
    ResultDetail, ScreenKind, ScreenView,
};

/// Stable editor-only data for arranging widgets while recognition is inactive.
#[must_use]
pub fn editor_sample_state() -> OverlayState {
    let day = 86_400_000_i64;
    let end = 1_788_134_400_000_i64;
    let scores = [
        0.57, 0.72, 0.67, 0.79, 0.69, 0.61, 0.65, 0.88, 0.80, 0.84, 0.86, 0.64, 0.68, 0.62, 0.83,
        0.70, 0.66, 0.74,
    ];
    let misses = [
        0.12, 0.15, 0.09, 0.22, 0.30, 0.14, 0.18, 0.24, 0.46, 0.34, 0.39, 0.58, 0.21, 0.18, 0.16,
        0.22, 0.24, 0.18,
    ];
    let graph = scores
        .into_iter()
        .zip(misses)
        .zip(0_i64..)
        .map(|((score_ratio, miss_ratio), index)| GraphPlay {
            received_unix_ms: end - day * (178 - index * 10),
            score_ratio,
            miss_ratio: Some(miss_ratio),
        })
        .collect();
    OverlayState {
        connected: false,
        chart: Some(Chart {
            song_id: "editor-sample".into(),
            play_type: "single".into(),
            difficulty: "hyper".into(),
            title: "NEON CIRCUIT".into(),
            artist: "SAMPLE ARTIST".into(),
            level: Some(12),
            notes: Some(1877),
        }),
        system: LampState::Inactive,
        result_signal: LampState::Active,
        best: BestView {
            score: "2846".into(),
            dj_level: "AA".into(),
            miss: "12".into(),
            clear: "HARD CLEAR".into(),
        },
        detail: ResultDetail {
            pgreat: "1324".into(),
            great: "198".into(),
            good: "21".into(),
            bad: "6".into(),
            poor: "12".into(),
            fast: "143".into(),
            slow: "137".into(),
            combo_break: "9".into(),
            play_options: "RANDOM".into(),
        },
        history: History {
            recorded: true,
            plays: (0..20)
                .map(|index| HistoryPlay {
                    notified_at: format!("2026.08.{:02} 21:{:02}", 28 - index, 10 + index),
                    score: (2846 - index * 13).to_string(),
                    dj_level: if index == 1 { "AAA" } else { "AA" }.into(),
                    miss: (12 + index).to_string(),
                    clear: if index % 5 == 4 {
                        "CLEAR"
                    } else {
                        "HARD CLEAR"
                    }
                    .into(),
                })
                .collect(),
            graph,
            graph_ticks: ["MAR", "APR", "MAY", "JUN", "JUL", "AUG"]
                .into_iter()
                .zip(0_i64..)
                .map(|(label, index)| GraphTick {
                    unix_ms: end - day * (178 - index * 30),
                    label: label.into(),
                })
                .collect(),
            graph_start_unix_ms: [
                end - day * 30,
                end - day * 90,
                end - day * 183,
                end - day * 365,
            ],
            graph_end_unix_ms: end,
        },
        screen: ScreenView {
            kind: Some(ScreenKind::MusicSelect),
            suspended_since_unix_ms: None,
            revision: 0,
        },
    }
}
