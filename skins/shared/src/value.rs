pub(crate) fn clear_role(value: &str) -> &'static str {
    match value {
        "FULL COMBO" | "FULL COMBO CLEAR" | "FC" => "full-combo",
        "EX HARD CLEAR" | "EX HARD" | "EXH" => "ex-hard",
        "HARD CLEAR" | "HARD" => "hard",
        "CLEAR" => "clear",
        "EASY CLEAR" | "EASY" => "easy",
        "ASSIST CLEAR" | "ASSIST" => "assist",
        "FAILED" => "failed",
        _ => "unknown",
    }
}
pub(crate) fn path_text<'a>(value: &'a Value, path: &str) -> &'a str {
    value.pointer(path).and_then(Value::as_str).unwrap_or("")
}
pub(crate) fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}
pub(crate) fn integer(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}
pub(crate) fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}
pub(crate) fn value_text(value: Option<&Value>) -> String {
    value.map_or_else(String::new, |v| {
        v.as_str().map_or_else(
            || {
                if v.is_null() {
                    String::new()
                } else {
                    v.to_string()
                }
            },
            str::to_owned,
        )
    })
}
pub(crate) fn shown(value: impl AsRef<str>) -> String {
    if value.as_ref().is_empty() {
        "—".into()
    } else {
        value.as_ref().into()
    }
}
use serde_json::Value;
