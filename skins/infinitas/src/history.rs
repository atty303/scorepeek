use crate::{group, letters, live, state_letters, text};
use scorepeek_skin_sdk::{Node, Widget};
use serde_json::Value;
pub fn render(w: &Widget, state: &Value, phase: f64) -> Node {
    let id = &w.id;
    let mut rows = vec![letters(&format!("{id}-heading"), "HISTORY", 18.0, "silver")];
    let labels = ["DATE", "SCORE", "DJ LEVEL", "MISS", "CLEAR"];
    rows.push(group(
        &format!("{id}-head"),
        "history-row history-head",
        "",
        labels
            .iter()
            .enumerate()
            .map(|(i, label)| letters(&format!("{id}-head-{i}"), label, 14.0, "label"))
            .collect(),
    ));
    let count = w.settings["history_count"].as_u64().unwrap_or(5);
    for (i, play) in state["history"]["plays"]
        .as_array()
        .into_iter()
        .flatten()
        .take(usize::try_from(count).unwrap_or(5))
        .enumerate()
    {
        rows.push(group(
            &format!("{id}-row-{i}"),
            "history-row",
            "",
            vec![
                live(
                    &format!("{id}-{i}-date"),
                    "history-date",
                    text(&play["notified_at"]),
                ),
                letters(
                    &format!("{id}-{i}-score"),
                    text(&play["score"]),
                    18.0,
                    "silver",
                ),
                state_letters(
                    &format!("{id}-{i}-rank"),
                    text(&play["dj_level"]),
                    18.0,
                    true,
                    phase,
                ),
                letters(
                    &format!("{id}-{i}-miss"),
                    text(&play["miss"]),
                    17.0,
                    "silver",
                ),
                state_letters(
                    &format!("{id}-{i}-clear"),
                    text(&play["clear"]),
                    15.0,
                    false,
                    phase,
                ),
            ],
        ));
    }
    group(&format!("{id}-content"), "history-content", "", rows)
}
