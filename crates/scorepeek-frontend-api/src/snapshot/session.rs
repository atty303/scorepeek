use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionSnapshot {
    pub session_id: Option<String>,
    pub active: bool,
}
