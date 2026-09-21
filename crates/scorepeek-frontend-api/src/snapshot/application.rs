use crate::Revision;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApplicationSnapshot {
    pub revision: Revision,
    pub running: bool,
    pub diagnostic_run_id: Option<String>,
    pub run: Option<RunSnapshot>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunSnapshot {
    pub watcher_state: String,
    pub session_count: u64,
    pub active_session_id: Option<String>,
    pub capture_generation: Option<u64>,
    pub raw_screen: Option<String>,
    pub semantic_screen: Option<String>,
    pub recording_status: String,
    pub recording_memory_used_bytes: u64,
    pub recording_memory_limit_bytes: u64,
    pub recording_dropped_frames: u64,
    pub event_stream_status: String,
    pub connected_clients: u64,
    pub dropped_events: u64,
    pub disconnected_clients: u64,
    pub result_count: u64,
    pub latest_result_label: Option<String>,
    pub scores_summary: Option<String>,
    pub overlay_summary: String,
    pub message: String,
}
