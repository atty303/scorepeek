use crate::{group, letters, live, number, text};
use scorepeek_skin_sdk::{Node, Widget};
use serde_json::Value;
pub fn render(w: &Widget, state: &Value) -> Node {
    let c = &state["chart"];
    let title = text(&c["title"]);
    let weight: usize = title
        .chars()
        .map(|c| if c.is_ascii() { 1 } else { 2 })
        .sum();
    let long = weight > 32;
    let artist = text(&c["artist"]);
    let difficulty = text(&c["difficulty"]).to_ascii_uppercase();
    let tone = match difficulty.as_str() {
        "BEGINNER" => "green",
        "NORMAL" => "blue",
        "HYPER" => "yellow",
        "ANOTHER" => "red",
        "LEGGENDARIA" => "purple",
        _ => "neutral",
    };
    let mode = match text(&c["play_type"]) {
        "single" => "SP",
        "double" => "DP",
        _ => "?",
    };
    let id = &w.id;
    group(
        &format!("{id}-content"),
        "selection-content",
        "",
        vec![
            group(
                &format!("{id}-song"),
                if long { "song long" } else { "song" },
                "",
                vec![
                    live(&format!("{id}-title"), "song-title", title),
                    live(&format!("{id}-artist"), "song-artist", artist),
                ],
            ),
            group(
                &format!("{id}-rail"),
                "selection-rail",
                "",
                vec![
                    letters(&format!("{id}-play"), mode, 19.0, "silver"),
                    letters(&format!("{id}-difficulty"), &difficulty, 16.0, tone),
                    letters(
                        &format!("{id}-level"),
                        &format!("LV {}", number(&c["level"])),
                        17.0,
                        tone,
                    ),
                    letters(
                        &format!("{id}-notes"),
                        &format!("NOTES {}", number(&c["notes"])),
                        16.0,
                        "silver",
                    ),
                ],
            ),
        ],
    )
}
