use blitz_dom::{BaseDocument, NodeId};

/// Bind the package's shared scope class to one host canvas without changing specificity.
pub(super) fn bind(document: &mut BaseDocument, style: NodeId, marker: &str) -> Result<(), String> {
    let scope = format!("[data-scorepeek-style-scope='{marker}']");
    bind_rules(document, style, &mut Vec::new(), &scope)
}

fn bind_rules(
    document: &mut BaseDocument,
    style: NodeId,
    path: &mut Vec<usize>,
    scope: &str,
) -> Result<(), String> {
    let count = document
        .stylesheet_rule_count(style, path)
        .ok_or("native skin stylesheet is missing")?;
    for index in 0..count {
        path.push(index);
        let rule = document
            .stylesheet_rule_info(style, path)
            .ok_or("native skin stylesheet rule is missing")?;
        if let Some((_, selector)) = rule
            .attributes
            .iter()
            .find(|(name, _)| *name == "selectorText")
        {
            let bound = bind_selector(selector, scope);
            if bound != *selector {
                document
                    .stylesheet_rule_set_selector_text(style, path, &bound)
                    .map_err(|error| format!("bind native skin stylesheet: {error:?}"))?;
            }
        }
        if rule.has_child_rules {
            bind_rules(document, style, path, scope)?;
        }
        path.pop();
    }
    Ok(())
}

fn bind_selector(selector: &str, scope: &str) -> String {
    const CLASS: &str = ".scorepeek-skin-scope";
    let mut result = String::with_capacity(selector.len());
    let mut rest = selector;
    let mut quote = None;
    let mut escaped = false;
    while let Some(ch) = rest.chars().next() {
        if !escaped && quote.is_none() && rest.starts_with(CLASS) {
            let after = &rest[CLASS.len()..];
            if !after.chars().next().is_some_and(|next| {
                next.is_alphanumeric() || matches!(next, '_' | '-' | '\\') || !next.is_ascii()
            }) {
                result.push_str(scope);
                rest = after;
                continue;
            }
        }
        result.push(ch);
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if quote == Some(ch) {
            quote = None;
        } else if quote.is_none() && matches!(ch, '\'' | '"') {
            quote = Some(ch);
        }
        rest = &rest[ch.len_utf8()..];
    }
    result
}
