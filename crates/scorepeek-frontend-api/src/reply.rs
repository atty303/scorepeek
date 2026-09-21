use crate::{ApplicationSnapshot, FrontendError, Revision};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FrontendReply {
    Accepted { revision: Revision },
    Snapshot { snapshot: Box<ApplicationSnapshot> },
    Completed { exit_code: u8 },
    Error { error: FrontendError },
}
