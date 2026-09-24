use std::{ffi::OsString, os::unix::ffi::OsStringExt as _, process::Command};

#[test]
fn config_outputs_keep_their_public_shapes_and_status() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.toml");
    std::fs::write(&path, "# sample\n").unwrap();
    let path = path.to_str().unwrap();

    for (args, expected) in [
        (vec!["path"], format!("{path}\n")),
        (vec!["show"], "# sample\n".to_owned()),
        (vec!["check"], format!("config is valid: {path}\n")),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_scorepeek"))
            .args(["--config", path, "config", args[0]])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
    }

    for (action, expected) in [
        ("path", serde_json::json!({"path": path})),
        (
            "show",
            serde_json::json!({"path": path, "present": true, "content": "# sample\n"}),
        ),
        (
            "check",
            serde_json::json!({"path": path, "present": true, "valid": true}),
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_scorepeek"))
            .args(["--config", path, "config", action, "--format", "json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
            expected
        );
    }
}

#[test]
fn non_utf8_config_path_is_human_readable_and_json_fails() {
    let root = tempfile::tempdir().unwrap();
    let config_home = root
        .path()
        .join(OsString::from_vec(b"config-\xff".to_vec()));
    let human = Command::new(env!("CARGO_BIN_EXE_scorepeek"))
        .args(["config", "path"])
        .env("XDG_CONFIG_HOME", &config_home)
        .env_remove("SCOREPEEK_CONFIG")
        .output()
        .unwrap();
    assert!(human.status.success());
    assert!(
        String::from_utf8(human.stdout)
            .unwrap()
            .contains("config-�/scorepeek/config.toml")
    );

    let json = Command::new(env!("CARGO_BIN_EXE_scorepeek"))
        .args(["config", "path", "--format", "json"])
        .env("XDG_CONFIG_HOME", &config_home)
        .env_remove("SCOREPEEK_CONFIG")
        .output()
        .unwrap();
    assert_eq!(json.status.code(), Some(1));
    assert!(json.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&json.stderr).contains("config path must be UTF-8 for JSON output")
    );
}
