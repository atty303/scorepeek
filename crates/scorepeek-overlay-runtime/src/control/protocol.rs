use scorepeek_overlay::{Backend, CanvasPresentation};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Maximum wire size of one newline-delimited overlay control request or response.
pub const CONTROL_MESSAGE_MAX_BYTES: usize = 1024 * 1024;

/// Serializes one bounded newline-delimited control message.
/// # Errors
/// Returns an error when serialization fails or the wire frame exceeds the shared limit.
pub fn encode_message<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if bytes.len().saturating_add(1) > CONTROL_MESSAGE_MAX_BYTES {
        return Err("overlay control message exceeds maximum size".to_owned());
    }
    bytes.push(b'\n');
    Ok(bytes)
}

/// Decodes one complete bounded newline-delimited control message.
/// # Errors
/// Returns an error when the frame is oversized, incomplete, or malformed.
pub fn decode_message<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    if bytes.len() > CONTROL_MESSAGE_MAX_BYTES {
        return Err("overlay control message exceeds maximum size".to_owned());
    }
    let Some(payload) = bytes.strip_suffix(b"\n") else {
        return Err("overlay control message is incomplete".to_owned());
    };
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Request {
    AcquireBackend {
        backend: Backend,
        editor_id: String,
    },
    KeepAliveBackend {
        backend: Backend,
        editor_id: String,
    },
    ReleaseBackend {
        backend: Backend,
        editor_id: String,
    },
    GetBackend {
        backend: Backend,
    },
    UpdateBackendDraft {
        backend: Backend,
        editor_id: String,
        canvases: Vec<CanvasPresentation>,
    },
    CommitBackend {
        backend: Backend,
        editor_id: String,
        canvases: Vec<CanvasPresentation>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Response {
    pub ok: bool,
    pub readonly: bool,
    pub error: Option<String>,
    #[serde(default)]
    pub canvases: Vec<CanvasPresentation>,
    pub generation: Option<u64>,
    #[serde(default)]
    pub dirty: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_limit_includes_the_newline_at_both_boundaries() {
        let maximum = "x".repeat(CONTROL_MESSAGE_MAX_BYTES - 3);
        let encoded = encode_message(&maximum).unwrap();
        assert_eq!(encoded.len(), CONTROL_MESSAGE_MAX_BYTES);
        assert_eq!(decode_message::<String>(&encoded).unwrap(), maximum);

        let oversized = "x".repeat(CONTROL_MESSAGE_MAX_BYTES - 2);
        assert!(encode_message(&oversized).is_err());
        let mut oversized_wire = serde_json::to_vec(&oversized).unwrap();
        oversized_wire.push(b'\n');
        assert_eq!(oversized_wire.len(), CONTROL_MESSAGE_MAX_BYTES + 1);
        assert!(decode_message::<String>(&oversized_wire).is_err());
    }
}
