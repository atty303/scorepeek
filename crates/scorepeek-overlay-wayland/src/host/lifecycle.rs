//! Parent-process lease and shell lifecycle.

pub use crate::host::event_loop::Shell;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

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
