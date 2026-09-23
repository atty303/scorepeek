use std::{os::unix::fs::PermissionsExt as _, process::Command};

#[test]
fn doctor_reports_inventory_from_an_isolated_home() {
    let root = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_scorepeek"));
    command.arg("doctor").arg("--format").arg("json");
    command.env_clear().env("PATH", "/usr/bin:/bin");
    for (key, directory) in [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_RUNTIME_DIR", "runtime"),
    ] {
        let path = root.path().join(directory);
        std::fs::create_dir(&path).unwrap();
        if key == "XDG_RUNTIME_DIR" {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        command.env(key, path);
    }

    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema"], "scorepeek-doctor-v5");
    assert_eq!(
        report["target_inventory"]["schema"],
        "scorepeek-target-inventory-v1"
    );
    for key in ["numeric_model", "catalog", "vulkan_layer"] {
        assert!(
            report[key]["status"].is_string(),
            "missing {key} status: {report}"
        );
    }
}
