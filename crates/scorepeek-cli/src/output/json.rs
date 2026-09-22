//! JSON output is serialized by the frontend protocol and written verbatim.

pub fn render(result: &scorepeek_frontend_api::CommandResult) -> Result<(), String> {
    let mut text = result.to_json().map_err(|error| error.to_string())?;
    text.push('\n');
    super::human::write(&text)
}

pub fn write(text: &str) {
    let _ = super::human::write(text);
}
