//! Domain-event envelope independent of runtime transport.

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct DomainEvent {
    pub event: String,
    #[serde(flatten)]
    pub data: serde_json::Value,
}
