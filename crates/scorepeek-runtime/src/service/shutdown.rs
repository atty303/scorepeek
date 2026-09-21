//! Service worker shutdown and join boundary.

pub(super) fn join(worker: std::thread::JoinHandle<()>) {
    let _ = worker.join();
}
