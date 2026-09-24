//! Owned subprocesses. Closing stdin revokes their lifetime lease.
use scorepeek_overlay::Backend;
use scorepeek_overlay_runtime::data::Config;
use std::{
    io::Write as _,
    os::unix::process::ExitStatusExt as _,
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use super::child::OwnedChild;
use super::protocol::{push_observation, read_diagnostics};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

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
    owned: Vec<OwnedChild>,
    observations: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl Children {
    /// Starts one independent overlay without acquiring capture or model resources.
    /// # Errors
    /// Returns spawn or configuration-pipe errors.
    pub fn start(&mut self, executable: &Path, config: &Config) -> Result<(), String> {
        let name = format!("{:?}", config.backend());
        self.status.insert(name.clone(), "failed");
        let mut bytes = match config {
            Config::Wayland(value) => serde_json::to_vec(value),
            Config::Obs(value) => serde_json::to_vec(value),
        }
        .map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        let role = match config.backend() {
            Backend::Wayland => crate::process_role::WAYLAND_ENTRYPOINT,
            Backend::Obs => crate::process_role::WEB_ENTRYPOINT,
        };
        let mut child = Command::new(executable)
            .arg(role)
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
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
        let reader = std::thread::Builder::new()
            .name("overlay-diagnostics".into())
            .spawn(move || read_diagnostics(stdout, &observations, &backend, Some(startup_tx)));
        match reader {
            Ok(reader) => {
                let startup =
                    startup_rx
                        .recv_timeout(STARTUP_TIMEOUT)
                        .map_err(|error| match error {
                            std::sync::mpsc::RecvTimeoutError::Timeout => {
                                format!("{name} overlay initialization timed out")
                            }
                            std::sync::mpsc::RecvTimeoutError::Disconnected => {
                                format!("{name} overlay initialization channel closed")
                            }
                        });
                match startup.and_then(|result| result) {
                    Ok(()) => {
                        self.status.insert(name.clone(), "running");
                        self.owned.push(OwnedChild::new(name, child, Some(reader)));
                    }
                    Err(error) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = reader.join();
                        return Err(format!("{name} overlay initialization: {error}"));
                    }
                }
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
        self.owned.retain_mut(|child| match child.try_wait() {
                Ok(Some(status)) => {
                    let name = child.name().to_owned();
                    health.insert(
                        name.clone(),
                        if status.success() {
                            "stopped"
                        } else {
                            "failed"
                        },
                    );
                    child.join_diagnostics();
                    push_observation(&self.observations, process_exit_observation(&name, status));
                    exits.push(format!("{name} overlay exited: {status}"));
                    false
                }
                Ok(None) => true,
                Err(error) => {
                    let name = child.name().to_owned();
                    health.insert(name.clone(), "failed");
                    exits.push(format!("{name} overlay wait failed: {error}"));
                    let _ = child.kill_and_wait();
                    child.join_diagnostics();
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
        for child in &mut self.owned {
            child.revoke_lease();
        }
        let deadline = Instant::now() + timeout;
        while !self.owned.is_empty() && Instant::now() < deadline {
            self.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        for mut child in self.owned.drain(..) {
            let name = child.name().to_owned();
            let waited = child.kill_and_wait();
            child.join_diagnostics();
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
        children
            .owned
            .push(OwnedChild::without_diagnostics("Wayland", child));

        children.shutdown_with_timeout(Duration::ZERO);

        assert_eq!(children.summary(), " Wayland=failed");
        let observations = children.take_observations();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0]["operation"], "process_exit");
        assert_eq!(observations[0]["status"], "error");
        assert_eq!(observations[0]["error_type"], "signal");
    }
}
