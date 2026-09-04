//! Owned subprocesses. Closing stdin revokes their lifetime lease.
use crate::runtime::Config;
use std::{
    io::{BufRead as _, BufReader, Write as _},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub const ENTRYPOINT: &str = "__scorepeek-overlay";

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
            .spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) {
                        let mut records = observations
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if records.len() < 128 {
                            records.push(serde_json::json!({"backend": backend, "record": record}));
                        }
                    }
                }
            });
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
        for (_, child, _) in &mut self.owned {
            child.stdin.take();
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.owned.is_empty() && Instant::now() < deadline {
            self.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        for (_, mut child, reader) in self.owned.drain(..) {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(reader) = reader {
                let _ = reader.join();
            }
        }
    }
}

impl Drop for Children {
    fn drop(&mut self) {
        self.shutdown();
    }
}
