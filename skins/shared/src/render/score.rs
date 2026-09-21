use super::{chrome, label, metallic};
use crate::motion::clear_motion_style;
use crate::primitive::{el, heading, label_element, node_text};
use crate::theme::Skin;
use crate::value::{clear_role, path_text, shown};
use scorepeek_skin_sdk::{Node, Widget};
use serde_json::Value;
#[allow(clippy::too_many_lines)]
pub(crate) fn score_widget(key: &str, widget: &Widget, state: &Value, skin: Skin) -> Node {
    let score = shown(path_text(state, "/best/score"));
    let rank = shown(path_text(state, "/best/dj_level"));
    let miss = shown(path_text(state, "/best/miss"));
    let clear = shown(path_text(state, "/best/clear"));
    let fields = [
        ("PGREAT", "/detail/pgreat", "pgreat"),
        ("GREAT", "/detail/great", "great"),
        ("GOOD", "/detail/good", "good"),
        ("BAD", "/detail/bad", "bad"),
        ("POOR", "/detail/poor", "poor"),
        ("FAST", "/detail/fast", "fast"),
        ("SLOW", "/detail/slow", "slow"),
        ("COMBO BREAK", "/detail/combo_break", "combo"),
        ("PLAY OPTIONS", "/detail/play_options", "options"),
    ];
    let mut detail = vec![heading(
        &format!("{key}:detail:title"),
        "RESULT DETAIL",
        skin,
    )];
    for (i, (name, path, class)) in fields.iter().enumerate() {
        detail.push(el(
            &format!("{key}:detail:{i}"),
            "div",
            &[("class", format!("detail-row {class}"))],
            vec![
                el(
                    &format!("{key}:detail:{i}:name"),
                    "span",
                    &[],
                    vec![label(&format!("{key}:detail:{i}:label"), name, skin, 15)],
                ),
                node_text(
                    &format!("{key}:detail:{i}:value"),
                    "b",
                    &[],
                    &shown(path_text(state, path)),
                ),
            ],
        ));
    }
    el(
        &format!("{key}:score"),
        "section",
        &[("class", "widget score-widget".into())],
        vec![
            chrome(key, widget, skin),
            el(
                &format!("{key}:content"),
                "div",
                &[("class", "widget-content score-content".into())],
                vec![
                    el(
                        &format!("{key}:best"),
                        "div",
                        &[("class", "best-section".into())],
                        vec![
                            heading(&format!("{key}:best:title"), "BEST", skin),
                            el(
                                &format!("{key}:best:grid"),
                                "div",
                                &[("class", "best-grid".into())],
                                vec![
                                    el(
                                        &format!("{key}:score-main"),
                                        "div",
                                        &[("class", "score-main".into())],
                                        vec![
                                            label_element(
                                                &format!("{key}:score-label"),
                                                "label",
                                                "EX SCORE",
                                                skin,
                                                15,
                                            ),
                                            el(
                                                &format!("{key}:score-value"),
                                                "strong",
                                                &[],
                                                vec![metallic(
                                                    &format!("{key}:score-value:type"),
                                                    &score,
                                                    skin,
                                                    66,
                                                    false,
                                                )],
                                            ),
                                            el(
                                                &format!("{key}:clear"),
                                                "span",
                                                &[
                                                    ("class", "clear-value".into()),
                                                    ("data-clear", clear_role(&clear).into()),
                                                    ("style", clear_motion_style(&clear)),
                                                ],
                                                vec![label(
                                                    &format!("{key}:clear:label"),
                                                    &clear,
                                                    skin,
                                                    20,
                                                )],
                                            ),
                                        ],
                                    ),
                                    el(
                                        &format!("{key}:best-side"),
                                        "div",
                                        &[("class", "best-side".into())],
                                        vec![
                                            label_element(
                                                &format!("{key}:rank-label"),
                                                "label",
                                                "DJ LEVEL",
                                                skin,
                                                15,
                                            ),
                                            el(
                                                &format!("{key}:rank"),
                                                "strong",
                                                &[
                                                    ("class", "dj-level".into()),
                                                    ("data-rank", rank.clone()),
                                                ],
                                                vec![metallic(
                                                    &format!("{key}:rank:type"),
                                                    &rank,
                                                    skin,
                                                    48,
                                                    true,
                                                )],
                                            ),
                                            label_element(
                                                &format!("{key}:miss-label"),
                                                "label",
                                                "MISS COUNT",
                                                skin,
                                                15,
                                            ),
                                            node_text(&format!("{key}:miss"), "b", &[], &miss),
                                        ],
                                    ),
                                ],
                            ),
                        ],
                    ),
                    el(
                        &format!("{key}:detail"),
                        "div",
                        &[("class", "detail-section".into())],
                        detail,
                    ),
                ],
            ),
        ],
    )
}
