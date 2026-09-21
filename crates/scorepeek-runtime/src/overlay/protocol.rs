//! Structured diagnostic protocol emitted by private overlay process roles.

use std::io::{BufRead as _, BufReader, Read as _};
use std::sync::{Mutex, mpsc::SyncSender};

const MAX_DIAGNOSTIC_LINE_BYTES: u64 = 1024 * 1024;

pub(crate) fn read_diagnostics(
    stdout: impl std::io::Read,
    observations: &Mutex<Vec<serde_json::Value>>,
    backend: &str,
    mut startup: Option<SyncSender<Result<(), String>>>,
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
                if record["operation"] == "child_ready" {
                    if let Some(sender) = startup.take() {
                        let _ = sender.send(Ok(()));
                    }
                } else if record["operation"] == "child_exit"
                    && let Some(sender) = startup.take()
                {
                    let error = record["data"]["error"]
                        .as_str()
                        .unwrap_or("overlay exited before initialization completed")
                        .to_owned();
                    let _ = sender.send(Err(error));
                }
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
    if let Some(sender) = startup {
        let _ = sender.send(Err(
            "overlay diagnostic stream ended before initialization completed".into(),
        ));
    }
}

pub(crate) fn push_observation(
    observations: &Mutex<Vec<serde_json::Value>>,
    value: serde_json::Value,
) {
    observations
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_boundary_ignores_later_child_failure_for_startup() {
        let input = concat!(
            "{\"sequence\":1,\"operation\":\"child_ready\",\"data\":{}}\n",
            "{\"sequence\":2,\"operation\":\"child_exit\",\"data\":{\"success\":false,\"error\":\"later failure\"}}\n"
        );
        let observations = Mutex::new(Vec::new());
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);

        read_diagnostics(input.as_bytes(), &observations, "Wayland", Some(sender));

        assert_eq!(receiver.recv().unwrap(), Ok(()));
        assert_eq!(observations.into_inner().unwrap().len(), 3);
    }

    #[test]
    fn child_failure_before_ready_fails_startup() {
        let input = "{\"sequence\":1,\"operation\":\"child_exit\",\"data\":{\"success\":false,\"error\":\"initialization failed\"}}\n";
        let observations = Mutex::new(Vec::new());
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);

        read_diagnostics(input.as_bytes(), &observations, "Wayland", Some(sender));

        assert_eq!(
            receiver.recv().unwrap(),
            Err("initialization failed".into())
        );
    }
}
