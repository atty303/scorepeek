use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OverlaySnapshot {
    pub wayland_running: bool,
    pub web_running: bool,
}
