use std::{
    io::Write as _,
    os::unix::net::UnixListener,
    process::{Command, Stdio},
};

fn store(finished: bool) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let run = root.path().join("scorepeek/diagnostics/run-1-0-1");
    std::fs::create_dir_all(&run).unwrap();
    let mut stream = std::fs::File::create(run.join("diagnostics.ndjson")).unwrap();
    let operations = if finished {
        vec!["first", "diagnostic_run_finished"]
    } else {
        vec!["first", "second"]
    };
    for (index, operation) in operations.iter().enumerate() {
        writeln!(
            stream,
            "{}",
            serde_json::json!({
                "schema":"scorepeek-diagnostic-event-v1",
                "run_id":"run-1-0-1",
                "sequence":index + 1,
                "observed_unix_us":0,
                "operation":operation,
                "data":{"status":"success"}
            })
        )
        .unwrap();
    }
    root
}

fn inspect(root: &tempfile::TempDir, format: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_scorepeek"));
    command.args([
        "diagnostic",
        "inspect",
        "--run-id",
        "run-1-0-1",
        "--format",
        format,
    ]);
    command.env("XDG_STATE_HOME", root.path());
    command
}

#[test]
fn inspection_formats_and_partial_status() {
    let complete = store(true);
    let human = inspect(&complete, "human").output().unwrap();
    assert!(human.status.success());
    assert_eq!(
        String::from_utf8(human.stdout).unwrap(),
        "scorepeek diagnostic inspection\n  run: run-1-0-1\n  active: false\n  partial: false\n  records: 2\nevents:\n  1: first status=success\n  2: diagnostic_run_finished status=success\n"
    );
    let json = inspect(&complete, "json").output().unwrap();
    assert!(json.status.success());
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value["header"]["partial"], false);
    assert_eq!(value["records"].as_array().unwrap().len(), 2);

    let partial = store(false);
    let output = inspect(&partial, "human").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("  2: second status=success"));
}

#[test]
fn closed_stdout_fails_inspection() {
    let root = store(true);
    let mut child = inspect(&root, "human")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("frontend output failed"));
}

#[test]
fn observe_replay_truncation_keeps_ndjson_and_warns_on_stderr() {
    let root = tempfile::tempdir().unwrap();
    let socket_dir = root.path().join("scorepeek");
    std::fs::create_dir(&socket_dir).unwrap();
    let listener = UnixListener::bind(socket_dir.join("diagnostics.sock")).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        std::io::Read::read_to_string(&mut stream, &mut request).unwrap();
        assert!(request.contains("\"seconds\":30"));
        writeln!(
            stream,
            "{}",
            serde_json::json!({
                "schema":"scorepeek-diagnostic-stream-header-v1",
                "oldest_sequence":5,
                "stream_start_sequence":5,
                "gap":false,
                "replay_truncated":true,
                "replay_seconds":30,
                "replay_available_us":12_500_000
            })
        )
        .unwrap();
        writeln!(
            stream,
            "{}",
            serde_json::json!({
                "sequence":5,
                "operation":"diagnostic_run_finished"
            })
        )
        .unwrap();
    });
    let output = Command::new(env!("CARGO_BIN_EXE_scorepeek"))
        .args(["diagnostic", "observe", "--replay", "30"])
        .env("XDG_RUNTIME_DIR", root.path())
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<serde_json::Value> = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["replay_truncated"], true);
    assert!(String::from_utf8_lossy(&output.stderr).contains("requested 30s diagnostic replay"));
}

#[test]
fn failed_run_warning_output_stops_the_service() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state-file");
    std::fs::write(&state, b"not a directory").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_scorepeek"))
        .args(["run", "--capture", "vulkan-layer"])
        .env("HOME", root.path())
        .env("XDG_STATE_HOME", &state)
        .env("XDG_CONFIG_HOME", root.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stderr.take());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert_eq!(status.code(), Some(1));
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("run did not stop after frontend warning output failed");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
