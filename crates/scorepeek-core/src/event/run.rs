//! Run-event envelope shared by live execution and replay.

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct RunEventEnvelope<T> {
    pub schema: String,
    #[serde(flatten)]
    pub kind: T,
}

impl<T> RunEventEnvelope<T>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    /// Serializes this envelope to its transport-neutral JSON representation.
    ///
    /// # Errors
    /// Returns an error when the event payload cannot be represented as JSON.
    pub fn to_value(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(self)
            .map_err(|error| format!("run event serialization failed: {error}"))
    }

    /// Validates and decodes a transport-neutral JSON run event.
    ///
    /// # Errors
    /// Returns an error for malformed payloads or an unsupported event schema.
    pub fn from_value(value: serde_json::Value) -> Result<Self, String> {
        let event: Self = serde_json::from_value(value)
            .map_err(|error| format!("run event contract validation failed: {error}"))?;
        if event.schema != super::schema::RUN_EVENT_SCHEMA {
            return Err("run event schema is unsupported".to_owned());
        }
        Ok(event)
    }
}
