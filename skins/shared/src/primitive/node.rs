pub(crate) fn el(key: &str, tag: &str, attrs: &[(&str, String)], children: Vec<Node>) -> Node {
    Node::element(
        key,
        tag,
        attrs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect::<BTreeMap<_, _>>(),
        children,
    )
}
pub(crate) fn text(key: &str, value: impl Into<String>) -> Node {
    Node::text(key, value)
}
pub(crate) fn node_text(key: &str, tag: &str, attrs: &[(&str, String)], value: &str) -> Node {
    el(key, tag, attrs, vec![text(&format!("{key}:text"), value)])
}
pub(crate) fn error_tree(error: String) -> Output {
    Output {
        schedule: Schedule::Idle,
        tree: el(
            "canvas",
            "main",
            &[("class", "overlay-canvas skin-error".into())],
            vec![text("error", error)],
        ),
    }
}

use scorepeek_skin_sdk::{Node, Output, Schedule};
use std::collections::BTreeMap;
