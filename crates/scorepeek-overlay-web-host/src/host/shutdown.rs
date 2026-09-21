//! Parent-lifetime shutdown for the private Web-host process.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

/// Stops the host when the supervising parent's pipe closes or produces a byte.
///
/// # Errors
/// Returns an error when the watcher thread cannot be started.
pub(crate) fn watch_parent(
    mut input: impl std::io::Read + Send + 'static,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("overlay-parent".into())
        .spawn(move || {
            let mut byte = [0];
            let _ = input.read(&mut byte);
            stop.store(true, Ordering::Release);
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(crate) async fn requested(stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
