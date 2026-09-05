//! Private child-to-parent diagnostics, never the browser display protocol.
use serde::Serialize;
use std::io::Write as _;

pub(crate) fn emit(operation: &str, data: &impl Serialize) {
    let record = serde_json::json!({"operation": operation, "data": data});
    if let Ok(bytes) = serde_json::to_string(&record) {
        let _ = writeln!(std::io::stdout().lock(), "{bytes}");
    }
}

/// Runs the private child entrypoint without initializing recognition.
/// # Errors
/// Returns configuration or overlay errors after emitting the terminal diagnostic.
pub fn run() -> Result<(), String> {
    let (config, input) = crate::runtime::read_config()?;
    emit(
        "canvases_loaded",
        &serde_json::json!({
            "backend": config.backend,
            "canvas_count": config.canvases.len(),
            "canvas_ids": config.canvases.iter().map(|canvas| canvas.id.as_str()).collect::<Vec<_>>(),
        }),
    );
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match config.backend {
        crate::runtime::Backend::Wayland => crate::native::run(config, input),
        crate::runtime::Backend::Obs => crate::web::run(config, input),
    }))
    .unwrap_or_else(|payload| {
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("unknown overlay panic");
        Err(format!("overlay panic: {message}"))
    });
    emit(
        "child_exit",
        &serde_json::json!({"success": result.is_ok(), "error": result.as_ref().err()}),
    );
    result
}
