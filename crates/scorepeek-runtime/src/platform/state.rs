#![allow(
    clippy::missing_errors_doc,
    reason = "internal application-service path helpers retain their existing call-site errors"
)]

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

pub struct RoutineStatePaths {
    pub recording_enabled: bool,
    _run_lock: File,
}

pub struct RoutineSessionPaths {
    pub root: PathBuf,
}

impl RoutineSessionPaths {
    pub fn cleanup(&self) -> Result<(), String> {
        let parent = self
            .root
            .parent()
            .ok_or_else(|| "recording session has no parent".to_owned())?;
        fs::remove_dir_all(&self.root)
            .map_err(|error| format!("recording session cleanup failed: {error}"))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("recording session cleanup sync failed: {error}"))
    }
}

pub fn state_paths(recording_enabled: bool) -> Result<RoutineStatePaths, String> {
    let state = xdg_base(
        env::var_os("XDG_STATE_HOME"),
        env::var_os("HOME"),
        ".local/state",
    )?;
    let scorepeek = state.join("scorepeek");
    ensure_directory_tree(&scorepeek)?;
    let run_lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(scorepeek.join("run.lock"))
        .map_err(|error| format!("ordinary run lock could not be opened: {error}"))?;
    run_lock
        .try_lock()
        .map_err(|error| format!("ordinary run lock could not be acquired: {error}"))?;
    Ok(RoutineStatePaths {
        recording_enabled,
        _run_lock: run_lock,
    })
}

impl RoutineStatePaths {
    pub fn start_recording_session(
        &self,
        run_root: Option<&Path>,
        session_id: &str,
    ) -> Result<Option<RoutineSessionPaths>, String> {
        if !self.recording_enabled {
            return Ok(None);
        }
        let run_root =
            run_root.ok_or_else(|| "diagnostic run directory is unavailable".to_owned())?;
        let sessions = run_root.join("sessions");
        ensure_directory_tree(&sessions)?;
        let root = sessions.join(session_id);
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(&root)
            .map_err(|error| format!("recording session creation failed: {error}"))?;
        if let Err(error) = sync_directory_and_parent(&root) {
            let _ = fs::remove_dir(&root);
            let _ = File::open(&sessions).and_then(|directory| directory.sync_all());
            return Err(format!("recording session sync failed: {error}"));
        }
        Ok(Some(RoutineSessionPaths { root }))
    }
}

pub(crate) fn ensure_directory_tree(path: &Path) -> Result<(), String> {
    let mut missing = Vec::new();
    let mut candidate = path;
    loop {
        match candidate.metadata() {
            Ok(metadata) if metadata.is_dir() => break,
            Ok(_) => return Err("state directory ancestor is not a directory".to_owned()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(candidate.to_owned());
                candidate = candidate
                    .parent()
                    .ok_or_else(|| "state directory has no existing ancestor".to_owned())?;
            }
            Err(error) => return Err(format!("state directory inspection failed: {error}")),
        }
    }
    let mut created = Vec::new();
    for directory in missing.into_iter().rev() {
        if let Err(error) = fs::DirBuilder::new().mode(0o700).create(&directory) {
            cleanup_created_directories(&created);
            return Err(format!("state directory creation failed: {error}"));
        }
        created.push(directory.clone());
        if let Err(error) = sync_directory_and_parent(&directory) {
            cleanup_created_directories(&created);
            return Err(format!("state directory sync failed: {error}"));
        }
    }
    if let Err(error) = sync_directory_and_parent(path) {
        cleanup_created_directories(&created);
        return Err(format!("state directory sync failed: {error}"));
    }
    Ok(())
}

fn sync_directory_and_parent(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn cleanup_created_directories(created: &[PathBuf]) {
    for directory in created.iter().rev() {
        let parent = directory.parent().map(Path::to_owned);
        let _ = fs::remove_dir(directory);
        if let Some(parent) = parent {
            let _ = File::open(parent).and_then(|directory| directory.sync_all());
        }
    }
}

fn xdg_base(
    explicit: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    fallback: &str,
) -> Result<PathBuf, String> {
    explicit
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|value| PathBuf::from(value).join(fallback)))
        .ok_or_else(|| "state path requires XDG_STATE_HOME or HOME".to_owned())
}
