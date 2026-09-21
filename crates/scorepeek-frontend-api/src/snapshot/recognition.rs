use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecognitionSnapshot {
    pub screen: Option<String>,
    pub revision: u64,
}
