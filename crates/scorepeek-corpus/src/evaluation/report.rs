//! Evaluation report serialization contracts.

#[derive(Clone, Debug, serde::Serialize)]
pub struct ReportSummary {
    pub schema: &'static str,
    pub failures: usize,
}
