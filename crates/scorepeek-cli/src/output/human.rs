use std::io::Write as _;

pub(super) fn write(text: &str) {
    let _ = std::io::stdout().lock().write_all(text.as_bytes());
}
