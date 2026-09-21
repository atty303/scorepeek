//! Machine-readable publisher report envelope.

#[derive(serde::Serialize)]
pub struct Report<T> {
    pub schema: &'static str,
    pub data: T,
}
