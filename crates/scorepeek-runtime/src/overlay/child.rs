//! Owned overlay child-process handle and lifetime lease.

use std::process::{Child, ExitStatus};
use std::thread::JoinHandle;

pub(crate) struct OwnedChild {
    name: String,
    process: Child,
    diagnostics: Option<JoinHandle<()>>,
}

impl OwnedChild {
    pub(crate) fn new(name: String, process: Child, diagnostics: Option<JoinHandle<()>>) -> Self {
        Self {
            name,
            process,
            diagnostics,
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn revoke_lease(&mut self) {
        self.process.stdin.take();
    }

    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.process.try_wait()
    }

    pub(crate) fn kill_and_wait(&mut self) -> std::io::Result<ExitStatus> {
        let _ = self.process.kill();
        self.process.wait()
    }

    pub(crate) fn join_diagnostics(&mut self) {
        if let Some(reader) = self.diagnostics.take() {
            let _ = reader.join();
        }
    }
}

#[cfg(test)]
impl OwnedChild {
    pub(crate) fn without_diagnostics(name: &str, process: Child) -> Self {
        Self::new(name.to_owned(), process, None)
    }
}
