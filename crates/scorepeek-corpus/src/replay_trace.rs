use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use scorepeek::routine_output::{RunEvent, RunEventKind};
use serde::Serialize;

fn retained(event: &RunEvent) -> bool {
    !matches!(event.kind, RunEventKind::FieldObservation { .. })
}

const MAX_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub(crate) struct TraceStatus {
    path: PathBuf,
    written_events: usize,
    total_events: usize,
    bytes: u64,
    error: Option<String>,
}

pub(crate) struct ReplayTrace {
    root: PathBuf,
    generation: String,
    remaining: u64,
    initialization_error: Option<String>,
    executable_sha256: Option<String>,
}

impl ReplayTrace {
    pub(crate) fn new(root: PathBuf, generation: &str) -> Self {
        let setup = fs::create_dir(&root)
            .map_err(|error| error.to_string())
            .and_then(|()| {
                #[cfg(target_os = "linux")]
                let executable = Ok(PathBuf::from("/proc/self/exe"));
                #[cfg(not(target_os = "linux"))]
                let executable = std::env::current_exe().map_err(|error| error.to_string());
                executable.and_then(|path| {
                    crate::frame_corpus::digest_file(&path).map_err(|error| error.to_string())
                })
            });
        let (executable_sha256, initialization_error) = match setup {
            Ok(digest) => (Some(digest), None),
            Err(error) => (None, Some(error)),
        };
        Self {
            root,
            generation: generation.to_owned(),
            remaining: MAX_BYTES,
            initialization_error,
            executable_sha256,
        }
    }

    pub(crate) fn write_session(
        &mut self,
        index: usize,
        session: &str,
        events: &[RunEvent],
    ) -> TraceStatus {
        let mut status = TraceStatus {
            path: self.root.join(format!("session-{index}.ndjson")),
            written_events: 0,
            total_events: events.iter().filter(|event| retained(event)).count(),
            bytes: 0,
            error: self.initialization_error.clone(),
        };
        if status.error.is_some() {
            return status;
        }
        if let Err(error) = self.write_records(session, events, &mut status) {
            status.error = Some(error.to_string());
        }
        status
    }

    fn write_records(
        &mut self,
        session: &str,
        events: &[RunEvent],
        status: &mut TraceStatus,
    ) -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&status.path)?;
        let metadata = serde_json::json!({
            "schema": "scorepeek-private-replay-trace-v1",
            "generation_sha256": self.generation,
            "session_id": session,
            "executable_sha256": self.executable_sha256,
            "selected_sources_sha256": crate::frame_corpus::digest(concat!(
                include_str!("../../scorepeek/src/routine_output.rs"),
                include_str!("../../scorepeek/src/routine_output/music_select_best.rs"),
                include_str!("../../scorepeek/src/recognition.rs")
            ).as_bytes()),
            "integrated_layout_sha256": crate::frame_corpus::digest(include_bytes!("../../scorepeek/src/integrated-context-layout-v6.json")),
            "best_layout_sha256": crate::frame_corpus::digest(include_bytes!("../../scorepeek/src/music-select-best-layout-v1.json")),
            "numeric_manifest_sha256": scorepeek::recognition::NUMERIC_MODEL_MANIFEST_SHA256,
            "text_manifest_sha256": scorepeek::recognition::LIVE_MODEL_BUNDLE_MANIFEST_SHA256,
            "run_event_schema": scorepeek::routine_output::RUN_EVENT_SCHEMA,
        });
        self.write_line(&mut file, &metadata, status)?;
        for event in events.iter().filter(|event| retained(event)) {
            self.write_line(&mut file, event, status)?;
            status.written_events += 1;
        }
        file.sync_all()
    }

    fn write_line(
        &mut self,
        file: &mut impl Write,
        value: &impl Serialize,
        status: &mut TraceStatus,
    ) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(value)?;
        bytes.push(b'\n');
        let size = bytes.len() as u64;
        if size > self.remaining {
            return Err(io::Error::other("replay trace capacity exceeded"));
        }
        // Reserve before writing: a partial write still consumes the shared run budget.
        self.remaining -= size;
        file.write_all(&bytes)?;
        status.bytes += size;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_never_overwrites_and_capacity_is_shared() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("trace");
        let mut trace = ReplayTrace::new(root.clone(), "generation");
        let first = trace.write_session(0, "session", &[]);
        assert!(first.error.is_none());
        let original = fs::read(&first.path).unwrap();
        let header: serde_json::Value = serde_json::from_slice(&original).unwrap();
        assert_eq!(header["executable_sha256"].as_str().unwrap().len(), 64);
        assert_eq!(header["best_layout_sha256"].as_str().unwrap().len(), 64);
        assert!(trace.write_session(0, "session", &[]).error.is_some());
        assert_eq!(original, fs::read(&first.path).unwrap());
        trace.remaining = 0;
        assert!(
            trace
                .write_session(1, "session", &[])
                .error
                .unwrap()
                .contains("capacity")
        );
        assert!(
            ReplayTrace::new(root, "generation")
                .write_session(2, "session", &[])
                .error
                .is_some()
        );
    }
}
