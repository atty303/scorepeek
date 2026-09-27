use crate::{RANKS, cell, group, letters, live, state_letters, text};
use scorepeek_skin_sdk::{BoundaryDirection, ScoreProgress, score_progress};
use scorepeek_skin_sdk::{Node, Widget};
use serde_json::Value;
pub fn render(w: &Widget, state: &Value, phase: f64) -> Node {
    let id = &w.id;
    let b = &state["best"];
    let d = &state["detail"];
    let rank = text(&b["dj_level"]);
    let clear = text(&b["clear"]);
    let progress = b["score"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .zip(state["chart"]["notes"].as_u64())
        .and_then(|(s, n)| score_progress(s, n));
    let distance = distance(progress, rank);
    let bar = bar(id, progress);
    let details = details(id, d);
    group(
        &format!("{id}-content"),
        "score-content",
        "",
        vec![
            group(
                &format!("{id}-summary"),
                "score-summary",
                "",
                vec![
                    group(
                        &format!("{id}-top"),
                        "score-top",
                        "",
                        vec![
                            cell(
                                &format!("{id}-score"),
                                "SCORE",
                                text(&b["score"]),
                                40.0,
                                "silver",
                            ),
                            group(
                                &format!("{id}-rank"),
                                "metric rank",
                                "",
                                vec![
                                    letters(&format!("{id}-rank-label"), "DJ LEVEL", 15.0, "label"),
                                    state_letters(
                                        &format!("{id}-rank-result"),
                                        rank,
                                        34.0,
                                        true,
                                        phase,
                                    ),
                                    letters(&format!("{id}-distance"), &distance, 16.0, "silver"),
                                ],
                            ),
                        ],
                    ),
                    group(
                        &format!("{id}-middle"),
                        "score-middle",
                        "",
                        vec![
                            cell(
                                &format!("{id}-miss"),
                                "MISS COUNT",
                                text(&b["miss"]),
                                34.0,
                                "silver",
                            ),
                            group(
                                &format!("{id}-clear"),
                                "metric clear",
                                "",
                                vec![
                                    letters(&format!("{id}-clear-label"), "CLEAR", 15.0, "label"),
                                    state_letters(
                                        &format!("{id}-clear-result"),
                                        clear,
                                        if clear == "FULL COMBO" { 19.0 } else { 21.0 },
                                        false,
                                        phase,
                                    ),
                                ],
                            ),
                        ],
                    ),
                    group(
                        &format!("{id}-rate"),
                        if progress.is_some() {
                            "rate"
                        } else {
                            "rate unknown"
                        },
                        "",
                        bar,
                    ),
                ],
            ),
            group(&format!("{id}-details"), "score-details", "", details),
        ],
    )
}

fn details(id: &str, d: &Value) -> Vec<Node> {
    let mut details = vec![];
    for (field, label, tone) in [
        ("pgreat", "PGREAT", "gold"),
        ("great", "GREAT", "silver"),
        ("good", "GOOD", "silver"),
        ("bad", "BAD", "red"),
        ("poor", "POOR", "red"),
        ("combo_break", "COMBO BREAK", "red"),
        ("fast", "FAST", "blue"),
        ("slow", "SLOW", "red"),
    ] {
        details.push(group(
            &format!("{id}-{field}"),
            &format!("judgment {field}"),
            "",
            vec![
                letters(&format!("{id}-{field}-label"), label, 14.0, tone),
                letters(&format!("{id}-{field}-value"), text(&d[field]), 19.0, tone),
            ],
        ));
    }
    details.push(group(
        &format!("{id}-options"),
        "options",
        "",
        vec![
            letters(
                &format!("{id}-options-label"),
                "PLAY OPTIONS",
                13.0,
                "label",
            ),
            live(
                &format!("{id}-options-value"),
                "options-value",
                text(&d["play_options"]),
            ),
        ],
    ));

    details
}

fn bar(id: &str, progress: Option<ScoreProgress>) -> Vec<Node> {
    let mut bar = vec![];
    if let Some(p) = progress {
        bar.push(group(
            &format!("{id}-fill"),
            "rate-fill",
            &format!(
                "width:{}%;",
                f64::from(u32::try_from(p.rate_hundredths).unwrap_or(0)) / 100.0
            ),
            vec![],
        ));
    } else {
        bar.push(live(&format!("{id}-unknown"), "rate-unknown", "?"));
    }
    for (i, rank) in RANKS[..8].iter().enumerate() {
        let percent = if i == 0 {
            0.0
        } else {
            f64::from(u32::try_from(i + 1).unwrap_or(0)) / 9.0 * 100.0
        };
        bar.push(group(
            &format!("{id}-tick-{i}"),
            "rate-tick",
            &format!("left:{percent}%;"),
            vec![letters(
                &format!("{id}-tick-label-{i}"),
                rank,
                12.0,
                "label",
            )],
        ));
    }

    bar
}

fn distance(progress: Option<ScoreProgress>, rank: &str) -> String {
    progress
        .and_then(|p| {
            RANKS
                .iter()
                .position(|r| *r == rank)
                .and_then(|r| p.nearest_boundary(r))
        })
        .map_or_else(
            || "?".into(),
            |d| match d.direction {
                BoundaryDirection::ExactMax => "MAX".into(),
                BoundaryDirection::Above => format!("{}+{}", RANKS[d.boundary_index], d.difference),
                BoundaryDirection::Below => format!("{}-{}", RANKS[d.boundary_index], d.difference),
            },
        )
}
