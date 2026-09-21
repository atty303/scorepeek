use crate::recognition::{MusicSelectLayout, ResultLayout};
use serde::Deserialize;

/// Versioned canonical recognition layout.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalLayout {
    pub(crate) schema: String,
    pub(crate) canonical_frame_contract_id: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub result: ResultLayout,
    pub music_select: MusicSelectLayout,
}
