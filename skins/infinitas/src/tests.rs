use super::*;
use serde_json::json;

fn input(kind: &str) -> Input {
    serde_json::from_value(json!({
        "schema":"scorepeek-skin-input-v2", "backend":"obs", "monotonic_ms":0,
        "canvas":{"id":"test","skin":"dev.atty303.infinitas","width":640,"height":640,"properties":{"background":"none"}},
        "widgets":[{"id":"subject","kind":kind,"x":0,"y":0,"width":600,"height":300}],
        "state":{"chart":{"notes":100},"best":{"score":"0","dj_level":"F","miss":"0","clear":"NO PLAY"},"detail":{},"history":{"plays":[]}}
    })).unwrap()
}
fn nodes<'a>(node: &'a Node, found: &mut Vec<&'a Node>) {
    found.push(node);
    if let Node::Element { children, .. } = node {
        for child in children {
            nodes(child, found);
        }
    }
}
fn texts(node: &Node) -> Vec<&str> {
    let mut all = vec![];
    nodes(node, &mut all);
    all.into_iter()
        .filter_map(|node| {
            if let Node::Text { text, .. } = node {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect()
}
fn has_class(node: &Node, class: &str) -> bool {
    let mut all = vec![];
    nodes(node, &mut all);
    all.into_iter().any(|node|matches!(node,Node::Element{attributes,..}if attributes.get("class").is_some_and(|value|value==class)))
}
#[test]
fn missing_and_invalid_scores_never_claim_zero_progress() {
    let mut input = input("score");
    let zero = render(&input);
    assert!(has_class(&zero.tree, "rate-fill"));
    assert!(texts(&zero.tree).contains(&"F+0"));
    for (score, notes) in [
        (json!(""), json!(100)),
        (json!("bad"), json!(100)),
        (json!("201"), json!(100)),
        (json!("0"), json!(null)),
        (json!("0"), json!(0)),
    ] {
        input.state["best"]["score"] = score;
        input.state["chart"]["notes"] = notes;
        let output = render(&input);
        assert!(!has_class(&output.tree, "rate-fill"));
        assert!(has_class(&output.tree, "rate unknown"));
        assert!(!texts(&output.tree).contains(&"F+0"));
    }
    input.state["best"]["score"] = json!("150");
    input.state["best"]["dj_level"] = json!("AAA");
    input.state["chart"]["notes"] = json!(100);
    let mismatch = render(&input);
    assert!(texts(&mismatch.tree).contains(&"AAA"));
    assert!(texts(&mismatch.tree).contains(&"?"));
    assert!(
        !texts(&mismatch.tree)
            .iter()
            .any(|text| text.starts_with("AAA+") || text.starts_with("AAA-"))
    );
}
#[test]
fn graph_preserves_missing_intervals_and_upward_miss_scale() {
    let mut input = input("history-graph");
    input.state["history"] = json!({"graph_start_unix_ms":[0,0,0,0],"graph_end_unix_ms":100,"graph_ticks":[],"graph":[
        {"received_unix_ms":10,"score_ratio":0.5,"miss_ratio":0.75},
        {"received_unix_ms":20,"score_ratio":0.6,"miss_ratio":null},
        {"received_unix_ms":30,"score_ratio":0.7,"miss_ratio":0.1}
    ]});
    let output = render(&input);
    let mut all = vec![];
    nodes(&output.tree, &mut all);
    assert!(
        !all.iter()
            .any(|node| node.key().starts_with("subject-miss_ratio-line-"))
    );
    let ys: Vec<f64> = all
        .iter()
        .filter_map(|node| match node {
            Node::Element {
                key, attributes, ..
            } if key.starts_with("subject-miss_ratio-point-") => {
                attributes.get("cy").and_then(|v| v.parse().ok())
            }
            _ => None,
        })
        .collect();
    assert_eq!(ys.len(), 2);
    assert!(ys[0] < ys[1]);
    assert_eq!(
        all.iter()
            .filter(|node| node.key().starts_with("subject-score_ratio-line-"))
            .count(),
        2
    );
}
#[test]
fn history_uses_requested_count_and_keeps_full_supplied_dates() {
    let mut input = input("history-list");
    input.state["history"]["plays"]=json!((0..50).map(|i|json!({"notified_at":format!("ROW {i:02} FULL DATE"),"score":"100","dj_level":"B","miss":"12","clear":"CLEAR"})).collect::<Vec<_>>());
    let default = render(&input);
    let default_text = texts(&default.tree);
    assert!(default_text.contains(&"ROW 04 FULL DATE"));
    assert!(!default_text.contains(&"ROW 05 FULL DATE"));
    input.widgets[0].settings = json!({"history_count":50});
    let full = render(&input);
    full.validate().unwrap();
    assert!(texts(&full.tree).contains(&"ROW 49 FULL DATE"));
}
