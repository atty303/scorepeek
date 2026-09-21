//! Service worker lifecycle.

pub(super) fn spawn(
    worker: impl FnOnce() + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("scorepeek-runtime-service".into())
        .spawn(worker)
}
