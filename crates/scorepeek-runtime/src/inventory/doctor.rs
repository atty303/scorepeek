use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const SCHEMA: &str = "scorepeek-target-inventory-v1";
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_PROBE_OUTPUT: usize = 64 * 1024;

pub struct Inventory {
    os: BTreeMap<String, String>,
    observations: BTreeMap<&'static str, Observation>,
}

enum Observation {
    Detected(String),
    Unavailable,
    Failed(i32),
}

trait Runner {
    fn output(&self, program: &str, args: &[&str]) -> io::Result<Output>;
}

struct SystemRunner;

impl Runner for SystemRunner {
    fn output(&self, program: &str, args: &[&str]) -> io::Result<Output> {
        Self::output_with_timeout(program, args, PROBE_TIMEOUT)
    }
}

impl SystemRunner {
    fn output_with_timeout(program: &str, args: &[&str], timeout: Duration) -> io::Result<Output> {
        let mut child = Command::new(program)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("probe stdout was not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("probe stderr was not piped"))?;
        let mut stdout_reader = match BoundedReader::spawn(stdout) {
            Ok(reader) => reader,
            Err(error) => return cleanup_without_readers(&mut child, error),
        };
        let mut stderr_reader = match BoundedReader::spawn(stderr) {
            Ok(reader) => reader,
            Err(error) => {
                drop(stdout_reader);
                return cleanup_without_readers(&mut child, error);
            }
        };
        let deadline = Instant::now() + timeout;
        let mut status = None;

        loop {
            if status.is_none() {
                match child.try_wait() {
                    Ok(child_status) => status = child_status,
                    Err(error) => {
                        return cleanup_after_error(
                            &mut child,
                            stdout_reader,
                            stderr_reader,
                            error,
                        );
                    }
                }
            }
            if let Err(error) = stdout_reader.poll() {
                return cleanup_after_error(&mut child, stdout_reader, stderr_reader, error);
            }
            if let Err(error) = stderr_reader.poll() {
                return cleanup_after_error(&mut child, stdout_reader, stderr_reader, error);
            }
            if let Some(status) = status
                && stdout_reader.is_complete()
                && stderr_reader.is_complete()
            {
                return Ok(Output {
                    status,
                    stdout: stdout_reader.finish()?,
                    stderr: stderr_reader.finish()?,
                });
            }

            if Instant::now() >= deadline {
                return cleanup_after_error(
                    &mut child,
                    stdout_reader,
                    stderr_reader,
                    io::Error::new(io::ErrorKind::TimedOut, "probe timed out"),
                );
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
}

struct BoundedReader {
    receiver: Receiver<io::Result<Vec<u8>>>,
    handle: JoinHandle<()>,
    output: Option<Vec<u8>>,
}

impl BoundedReader {
    fn spawn(stream: impl Read + Send + 'static) -> io::Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("scorepeek-probe-reader".to_owned())
            .spawn(move || {
                let result = read_bounded(stream);
                let _ = sender.send(result);
            })?;
        Ok(Self {
            receiver,
            handle,
            output: None,
        })
    }

    fn poll(&mut self) -> io::Result<()> {
        if self.output.is_some() {
            return Ok(());
        }
        match self.receiver.try_recv() {
            Ok(Ok(output)) => {
                self.output = Some(output);
                Ok(())
            }
            Ok(Err(error)) => Err(error),
            Err(TryRecvError::Empty) => Ok(()),
            Err(TryRecvError::Disconnected) => Err(io::Error::other("probe output reader stopped")),
        }
    }

    const fn is_complete(&self) -> bool {
        self.output.is_some()
    }

    fn finish(self) -> io::Result<Vec<u8>> {
        self.handle
            .join()
            .map_err(|_| io::Error::other("probe output reader panicked"))?;
        self.output
            .ok_or_else(|| io::Error::other("probe output was not collected"))
    }
}

fn read_bounded(mut stdout: impl Read) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    stdout
        .by_ref()
        .take((MAX_PROBE_OUTPUT + 1) as u64)
        .read_to_end(&mut output)?;
    if output.len() > MAX_PROBE_OUTPUT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "probe output exceeded limit",
        ));
    }
    Ok(output)
}

fn cleanup_after_error(
    child: &mut Child,
    stdout_reader: BoundedReader,
    stderr_reader: BoundedReader,
    original_error: io::Error,
) -> io::Result<Output> {
    drop(stdout_reader);
    drop(stderr_reader);
    cleanup_without_readers(child, original_error)
}

fn cleanup_without_readers(child: &mut Child, original_error: io::Error) -> io::Result<Output> {
    if let Err(cleanup_error) = terminate(child) {
        return Err(io::Error::other(format!(
            "probe failed ({original_error}); cleanup failed ({cleanup_error})"
        )));
    }
    Err(original_error)
}

fn terminate(child: &mut Child) -> io::Result<()> {
    let already_exited = child.try_wait()?.is_some();
    let process_group = format!("-{}", child.id());
    let group_status = Command::new("/usr/bin/kill")
        .args(["-KILL", "--", &process_group])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;

    if !group_status.success() && !already_exited {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other("could not stop probe process group"));
    }
    if !already_exited {
        match child.kill() {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::InvalidInput => {}
            Err(error) => return Err(error),
        }
        child.wait()?;
    }
    Ok(())
}

pub fn collect() -> Inventory {
    collect_with(&SystemRunner, "/etc/os-release")
}

fn collect_with(runner: &impl Runner, os_release_path: &str) -> Inventory {
    let mut observations = BTreeMap::new();
    observations.insert(
        "kernel",
        probe(runner, "/usr/bin/uname", &["-r"], single_version),
    );
    observations.insert(
        "gpu",
        probe(runner, "/usr/bin/lspci", &["-Dnnk"], gpu_summary),
    );
    Inventory {
        os: read_os_release(os_release_path),
        observations,
    }
}

fn probe(
    runner: &impl Runner,
    program: &str,
    args: &[&str],
    parse: fn(&str) -> Option<String>,
) -> Observation {
    probe_stream(runner, program, args, parse, false)
}

fn probe_stream(
    runner: &impl Runner,
    program: &str,
    args: &[&str],
    parse: fn(&str) -> Option<String>,
    use_stderr: bool,
) -> Observation {
    match runner.output(program, args) {
        Ok(output) if output.status.success() => {
            let bytes = if use_stderr {
                output.stderr
            } else {
                output.stdout
            };
            String::from_utf8(bytes)
                .ok()
                .and_then(|stream| parse(&stream))
                .map_or(Observation::Unavailable, Observation::Detected)
        }
        Ok(output) => Observation::Failed(output.status.code().unwrap_or(-1)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Observation::Unavailable,
        Err(_) => Observation::Failed(-1),
    }
}

fn read_os_release(path: &str) -> BTreeMap<String, String> {
    let Ok(contents) = fs::read_to_string(path) else {
        return BTreeMap::new();
    };

    parse_os_release(&contents)
}

fn parse_os_release(contents: &str) -> BTreeMap<String, String> {
    contents
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(key, _)| matches!(*key, "ID" | "IMAGE_ID" | "VERSION_ID" | "VARIANT_ID"))
        .filter_map(|(key, value)| normalize_value(value).map(|value| (key.to_lowercase(), value)))
        .collect()
}

fn normalize_value(value: &str) -> Option<String> {
    let value = value.trim().trim_matches('"');
    safe_line(value)
}

fn single_version(output: &str) -> Option<String> {
    let mut lines = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let version = version_token(lines.next()?)?;
    lines.next().is_none().then_some(version)
}

fn version_token(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'~' | b':' | b'-')
        }))
    .then(|| value.to_owned())
}

fn gpu_summary(output: &str) -> Option<String> {
    let mut display_device = false;
    let mut summary = Vec::new();

    for line in output.lines() {
        let indented = line.chars().next().is_some_and(char::is_whitespace);
        if !indented {
            let line = line.trim();
            display_device =
                line.contains("VGA compatible controller") || line.contains("3D controller");
            if display_device && let Some(line) = safe_line(line) {
                summary.push(line);
            }
        } else if display_device {
            let line = line.trim();
            if line.starts_with("Kernel driver in use:")
                && let Some(line) = safe_line(line)
            {
                summary.push(line);
            }
        }
    }

    let summary = summary.join(" | ");
    (!summary.is_empty()).then_some(summary)
}

fn safe_line(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 512
        && value
            .chars()
            .all(|character| !character.is_control() || character == '\t'))
    .then(|| value.replace('\t', " "))
}

impl Inventory {
    pub fn into_report(self) -> scorepeek_frontend_api::TargetInventory {
        scorepeek_frontend_api::TargetInventory {
            schema: SCHEMA.to_owned(),
            os: self.os,
            observations: self
                .observations
                .into_iter()
                .map(|(name, observation)| {
                    (
                        name.to_owned(),
                        match observation {
                            Observation::Detected(value) => {
                                scorepeek_frontend_api::ProbeObservation::Detected { value }
                            }
                            Observation::Unavailable => {
                                scorepeek_frontend_api::ProbeObservation::Unavailable
                            }
                            Observation::Failed(exit_code) => {
                                scorepeek_frontend_api::ProbeObservation::Failed { exit_code }
                            }
                        },
                    )
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    struct FakeRunner {
        outputs: HashMap<String, Result<(i32, String, String), io::ErrorKind>>,
    }

    impl Runner for FakeRunner {
        fn output(&self, program: &str, args: &[&str]) -> io::Result<Output> {
            let key = format!("{program} {}", args.join(" "));
            match self.outputs.get(&key).expect("unexpected command") {
                Ok((code, stdout, stderr)) => Ok(Output {
                    status: ExitStatus::from_raw(*code << 8),
                    stdout: stdout.as_bytes().to_vec(),
                    stderr: stderr.as_bytes().to_vec(),
                }),
                Err(kind) => Err(io::Error::from(*kind)),
            }
        }
    }

    #[test]
    fn inventory_serializes_only_allowlisted_parsed_values() {
        let runner = FakeRunner {
            outputs: HashMap::from([
                success("/usr/bin/uname -r", "6.14.1-bazzite\n"),
                success(
                    "/usr/bin/lspci -Dnnk",
                    "0000:01:00.0 Ethernet controller: Private NIC\n\tKernel driver in use: secret_driver\n0000:03:00.0 VGA compatible controller: Example GPU [1234:5678]\n\tSubsystem: private\n\tKernel driver in use: amdgpu\n",
                ),
            ]),
        };

        let inventory = collect_with(&runner, "/path/that/does/not/exist").into_report();
        assert!(inventory.os.is_empty());
        assert_eq!(inventory.observations.len(), 2);
        assert!(
            matches!(inventory.observations.get("gpu"), Some(scorepeek_frontend_api::ProbeObservation::Detected { value }) if value == "0000:03:00.0 VGA compatible controller: Example GPU [1234:5678] | Kernel driver in use: amdgpu")
        );
        let json = serde_json::to_string(&inventory).unwrap();

        assert!(json.contains("\"schema\":\"scorepeek-target-inventory-v1\""));
        assert!(json.contains("Example GPU [1234:5678] | Kernel driver in use: amdgpu"));
        assert!(!json.contains("Subsystem: private"));
        assert!(!json.contains("secret_driver"));
        assert!(!json.contains("secret from stderr"));
        assert!(!json.contains("gamescope"));
        assert!(!json.contains("pipewire"));
        assert!(!json.contains("obs_"));
    }

    #[test]
    fn command_failures_expose_only_status_and_exit_code() {
        let runner = FakeRunner {
            outputs: HashMap::from([
                failed("/usr/bin/uname -r", 7, "secret from stderr"),
                missing("/usr/bin/lspci -Dnnk"),
            ]),
        };

        let inventory = collect_with(&runner, "/path/that/does/not/exist").into_report();
        assert!(matches!(
            inventory.observations.get("kernel"),
            Some(scorepeek_frontend_api::ProbeObservation::Failed { exit_code: 7 })
        ));
        assert!(matches!(
            inventory.observations.get("gpu"),
            Some(scorepeek_frontend_api::ProbeObservation::Unavailable)
        ));
        let json = serde_json::to_string(&inventory).unwrap();

        assert!(json.contains("\"kernel\":{\"status\":\"failed\",\"exit_code\":7}"));
        assert!(json.contains("\"gpu\":{\"status\":\"unavailable\"}"));
        assert!(!json.contains("secret from stderr"));
    }

    #[test]
    fn os_release_ignores_non_allowlisted_fields() {
        let os = parse_os_release(
            "ID=bazzite\nIMAGE_ID=repository/image\nVERSION_ID=42\nVARIANT_ID=desktop\nPRETTY_NAME=private-host-label\n",
        );

        assert_eq!(os.len(), 4);
        assert_eq!(os["id"], "bazzite");
        assert!(!os.values().any(|value| value == "private-host-label"));
    }

    #[test]
    fn stdout_reader_enforces_hard_limit() {
        let oversized = vec![b'x'; MAX_PROBE_OUTPUT + 1];
        assert_eq!(
            read_bounded(oversized.as_slice())
                .expect_err("oversized output must fail")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn system_runner_bounds_process_group_lifetime() {
        let started = Instant::now();
        let result = SystemRunner::output_with_timeout(
            "/bin/sh",
            &["-c", "/usr/bin/sleep 10 & wait"],
            Duration::from_millis(100),
        );

        assert_eq!(
            result.expect_err("probe must time out").kind(),
            io::ErrorKind::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    fn success(key: &str, stdout: &str) -> (String, Result<(i32, String, String), io::ErrorKind>) {
        (
            key.to_owned(),
            Ok((0, stdout.to_owned(), "secret from stderr".to_owned())),
        )
    }

    fn failed(
        key: &str,
        code: i32,
        stderr: &str,
    ) -> (String, Result<(i32, String, String), io::ErrorKind>) {
        (key.to_owned(), Ok((code, String::new(), stderr.to_owned())))
    }

    fn missing(key: &str) -> (String, Result<(i32, String, String), io::ErrorKind>) {
        (key.to_owned(), Err(io::ErrorKind::NotFound))
    }
}
