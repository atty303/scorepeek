//! Owned subprocesses. Closing stdin revokes their lifetime lease.
use crate::runtime::Config;
use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    os::unix::process::ExitStatusExt as _,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub const ENTRYPOINT: &str = "__scorepeek-overlay";
const MAX_DIAGNOSTIC_LINE_BYTES: u64 = 1024 * 1024;

fn process_exit_observation(backend: &str, status: std::process::ExitStatus) -> serde_json::Value {
    serde_json::json!({
        "backend": backend,
        "operation":"process_exit",
        "status":if status.success() { "success" } else { "error" },
        "error_type":if status.success() { None } else if status.signal().is_some() { Some("signal") } else { Some("exit_status") },
        "success":status.success(),
        "code":status.code(),
        "signal":status.signal()
    })
}

#[derive(Default)]
pub struct Children {
    status: std::collections::BTreeMap<String, &'static str>,
    owned: Vec<(String, Child, Option<JoinHandle<()>>)>,
    observations: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl Children {
    /// Starts one independent overlay without acquiring capture or model resources.
    /// # Errors
    /// Returns spawn or configuration-pipe errors.
    pub fn start(&mut self, executable: &Path, config: &Config) -> Result<(), String> {
        let name = format!("{:?}", config.backend);
        self.status.insert(name.clone(), "failed");
        let mut bytes = serde_json::to_vec(config).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        let mut child = Command::new(executable)
            .arg(ENTRYPOINT)
            .env("WGPU_BACKEND", "vulkan")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("{name} overlay start: {error}"))?;
        let sent = child
            .stdin
            .as_mut()
            .ok_or_else(|| "overlay pipe missing".to_owned())
            .and_then(|pipe| pipe.write_all(&bytes).map_err(|error| error.to_string()));
        if let Err(error) = sent {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{name} overlay configuration: {error}"));
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "overlay diagnostic pipe missing".to_owned())?;
        let observations = Arc::clone(&self.observations);
        let backend = name.clone();
        let reader = std::thread::Builder::new()
            .name("overlay-diagnostics".into())
            .spawn(move || read_diagnostics(stdout, &observations, &backend));
        match reader {
            Ok(reader) => {
                self.status.insert(name.clone(), "running");
                self.owned.push((name, child, Some(reader)));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("overlay diagnostics: {error}"));
            }
        }
        Ok(())
    }

    /// Takes private observations for the existing run diagnostic recorder.
    pub fn take_observations(&self) -> Vec<serde_json::Value> {
        std::mem::take(
            &mut *self
                .observations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// Returns newly observed child exits, once per child.
    pub fn poll(&mut self) -> Vec<String> {
        let mut exits = Vec::new();
        let health = &mut self.status;
        self.owned
            .retain_mut(|(name, child, reader)| match child.try_wait() {
                Ok(Some(status)) => {
                    health.insert(
                        name.clone(),
                        if status.success() {
                            "stopped"
                        } else {
                            "failed"
                        },
                    );
                    if let Some(reader) = reader.take() {
                        let _ = reader.join();
                    }
                    push_observation(&self.observations, process_exit_observation(name, status));
                    exits.push(format!("{name} overlay exited: {status}"));
                    false
                }
                Ok(None) => true,
                Err(error) => {
                    health.insert(name.clone(), "failed");
                    exits.push(format!("{name} overlay wait failed: {error}"));
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Some(reader) = reader.take() {
                        let _ = reader.join();
                    }
                    self.observations.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(
                        serde_json::json!({"backend": name, "operation":"process_wait", "error_type":"wait_failed", "error":error.to_string()})
                    );
                    false
                }
            });
        exits
    }

    /// Stable run status, including children that have already exited.
    #[must_use]
    pub fn summary(&self) -> String {
        self.status
            .iter()
            .fold(String::new(), |mut summary, (backend, status)| {
                use std::fmt::Write as _;
                let _ = write!(summary, " {backend}={status}");
                summary
            })
    }

    /// Closes all leases together, then reaps only processes owned by this instance.
    pub fn shutdown(&mut self) {
        self.shutdown_with_timeout(Duration::from_secs(2));
    }

    fn shutdown_with_timeout(&mut self, timeout: Duration) {
        for (_, child, _) in &mut self.owned {
            child.stdin.take();
        }
        let deadline = Instant::now() + timeout;
        while !self.owned.is_empty() && Instant::now() < deadline {
            self.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        for (name, mut child, reader) in self.owned.drain(..) {
            let _ = child.kill();
            let waited = child.wait();
            if let Some(reader) = reader {
                let _ = reader.join();
            }
            match waited {
                Ok(status) => {
                    self.status.insert(name.clone(), "failed");
                    push_observation(&self.observations, process_exit_observation(&name, status));
                }
                Err(error) => {
                    self.status.insert(name.clone(), "failed");
                    push_observation(
                        &self.observations,
                        serde_json::json!({"backend":name,"operation":"process_wait","status":"error","error_type":"wait_failed","error":error.to_string()}),
                    );
                }
            }
        }
    }
}

fn read_diagnostics(
    stdout: impl std::io::Read,
    observations: &Mutex<Vec<serde_json::Value>>,
    backend: &str,
) {
    let mut expected_sequence = 1_u64;
    let mut terminal_seen = false;
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = match reader
            .by_ref()
            .take(MAX_DIAGNOSTIC_LINE_BYTES + 1)
            .read_until(b'\n', &mut line)
        {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => {
                push_observation(
                    observations,
                    serde_json::json!({"backend": backend, "transport":"stdout", "error_type":"read_failed", "error":error.to_string()}),
                );
                return;
            }
        };
        if read as u64 > MAX_DIAGNOSTIC_LINE_BYTES {
            push_observation(
                observations,
                serde_json::json!({"backend": backend, "transport":"stdout", "error_type":"record_too_large", "limit_bytes":MAX_DIAGNOSTIC_LINE_BYTES}),
            );
            return;
        }
        if line.last() != Some(&b'\n') {
            push_observation(
                observations,
                serde_json::json!({"backend": backend, "transport":"stdout", "error_type":"malformed_ndjson", "error":"incomplete terminal record"}),
            );
            break;
        }
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let observation = match serde_json::from_slice::<serde_json::Value>(&line) {
            Ok(record) => {
                let sequence = record["sequence"].as_u64();
                if sequence != Some(expected_sequence) {
                    push_observation(
                        observations,
                        serde_json::json!({"backend": backend, "transport":"stdout", "error_type":"sequence_gap", "expected_sequence":expected_sequence, "actual_sequence":sequence}),
                    );
                }
                expected_sequence = sequence.unwrap_or(expected_sequence).saturating_add(1);
                terminal_seen |= record["operation"] == "child_exit";
                serde_json::json!({"backend": backend, "record": record})
            }
            Err(error) => {
                serde_json::json!({"backend": backend, "transport":"stdout", "error_type":"malformed_ndjson", "error":error.to_string()})
            }
        };
        push_observation(observations, observation);
    }
    push_observation(
        observations,
        if terminal_seen {
            serde_json::json!({"backend": backend, "transport":"stdout", "operation":"eof"})
        } else {
            serde_json::json!({"backend": backend, "transport":"stdout", "operation":"eof", "error_type":"unexpected_eof"})
        },
    );
}

fn push_observation(observations: &Mutex<Vec<serde_json::Value>>, value: serde_json::Value) {
    observations
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(value);
}

impl Drop for Children {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forced_shutdown_records_the_child_exit() {
        let child = Command::new("sh")
            .args(["-c", "sleep 10"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let mut children = Children::default();
        children.status.insert("Wayland".into(), "running");
        children.owned.push(("Wayland".into(), child, None));

        children.shutdown_with_timeout(Duration::ZERO);

        assert_eq!(children.summary(), " Wayland=failed");
        let observations = children.take_observations();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0]["operation"], "process_exit");
        assert_eq!(observations[0]["status"], "error");
        assert_eq!(observations[0]["error_type"], "signal");
    }
}
