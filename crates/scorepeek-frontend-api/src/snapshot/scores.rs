use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScoresSnapshot {
    pub enabled: bool,
    pub revision: u64,
}
