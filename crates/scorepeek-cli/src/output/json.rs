//! JSON output is serialized by the frontend protocol and written verbatim.

pub fn write(text: &str) {
    super::human::write(text);
}
