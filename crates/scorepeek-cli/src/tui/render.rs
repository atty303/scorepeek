use scorepeek_frontend_api::RunSnapshot;

#[must_use]
pub fn lines(run: &RunSnapshot) -> Vec<String> {
    vec![
        format!(
            "scorepeek: state={} sessions={} session={} generation={} channel={} clients={} dropped={} disconnected={} message={} {}{}",
            run.watcher_state,
            run.session_count,
            run.active_session_id.as_deref().unwrap_or("-"),
            run.capture_generation
                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
            run.event_stream_status,
            run.connected_clients,
            run.dropped_events,
            run.disconnected_clients,
            run.message,
            run.scores_summary.as_deref().unwrap_or("scores=disabled"),
            run.overlay_summary,
        ),
        format!(
            "screen: raw={} semantic={} recording={} memory={}/{} dropped={}",
            run.raw_screen.as_deref().unwrap_or("-"),
            run.semantic_screen.as_deref().unwrap_or("-"),
            run.recording_status,
            run.recording_memory_used_bytes,
            run.recording_memory_limit_bytes,
            run.recording_dropped_frames,
        ),
        format!(
            "results: count={} latest={}",
            run.result_count,
            run.latest_result_label.as_deref().unwrap_or("-")
        ),
    ]
}
