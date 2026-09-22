//! Private child-to-parent diagnostics, never the browser display protocol.
use serde::Serialize;
use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(crate) fn emit(operation: &str, data: &impl Serialize) {
    let timestamp_unix_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    let record = serde_json::json!({
        "schema": "scorepeek-overlay-diagnostic-v1",
        "sequence": NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        "timestamp_unix_us": timestamp_unix_us,
        "process_id": std::process::id(),
        "operation": operation,
        "data": data,
    });
    if let Ok(bytes) = serde_json::to_string(&record) {
        let _ = writeln!(std::io::stdout().lock(), "{bytes}");
    }
}

/// Runs the private child entrypoint without initializing recognition.
/// # Errors
/// Returns configuration or overlay errors after emitting the terminal diagnostic.
pub fn run() -> Result<(), String> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run_inner)).unwrap_or_else(
        |payload| {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("unknown overlay panic");
            Err(format!("overlay panic: {message}"))
        },
    );
    emit(
        "child_exit",
        &serde_json::json!({"success": result.is_ok(), "error": result.as_ref().err()}),
    );
    result
}

fn run_inner() -> Result<(), String> {
    let (config, input) = crate::bridge::data::read_config()?;
    emit(
        "canvases_loaded",
        &serde_json::json!({
            "backend": config.backend,
            "canvas_count": config.canvases.len(),
            "canvas_ids": config.canvases.iter().map(|canvas| canvas.id.as_str()).collect::<Vec<_>>(),
        }),
    );
    match config.backend {
        crate::bridge::data::Backend::Wayland => crate::host::run(config, input),
        crate::bridge::data::Backend::Obs => {
            Err("OBS config was sent to the Wayland process role".into())
        }
    }
}
