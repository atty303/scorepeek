use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use scorepeek::capture::GamescopeProfileBinding;
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::{calibration_marker, capture_calibration};

const MAX_PROFILE_BYTES: u64 = 64 * 1024;
const PROFILE_STAGING_SUFFIX: &str = ".scorepeek-staging";
const MAX_JOINED_SESSION_GENERATIONS: usize = 8;
const MAX_JOINED_SESSION_AGGREGATE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_JOINED_SESSION_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub struct SelectedProfile {
    pub path: PathBuf,
    pub digest: String,
    pub binding: GamescopeProfileBinding,
}

#[derive(Serialize)]
struct SetupSummary {
    schema: &'static str,
    profile: String,
    path: PathBuf,
    profile_sha256: String,
    observed_width: u32,
    observed_height: u32,
    source_rectangle: scorepeek::capture::FractionalRectangle,
    verified_fiducial_count: u32,
}

#[derive(Serialize)]
struct ProfileSummary<'a> {
    name: &'a str,
    path: &'a Path,
    profile_sha256: &'a str,
    observed_width: u32,
    observed_height: u32,
    source_rectangle: scorepeek::capture::FractionalRectangle,
}

pub fn try_command(args: &[OsString], bundle: &Path) -> Option<Result<(), String>> {
    match args {
        [internal] if internal == "__calibration-marker" => Some(calibration_marker::run_x11()),
        [profile, list] if profile == "profile" && list == "list" => Some(list_profiles()),
        [
            setup,
            gamescope,
            profile_flag,
            name,
            delimiter,
            gamescope_args @ ..,
        ] if setup == "setup"
            && gamescope == "gamescope"
            && profile_flag == "--profile"
            && delimiter == "--" =>
        {
            Some(setup_gamescope(name, gamescope_args, bundle))
        }
        _ => None,
    }
}

pub fn select_for_run(name: Option<&OsStr>) -> Result<SelectedProfile, String> {
    let profiles = load_profiles()?;
    if let Some(name) = name {
        let name = profile_name(name)?;
        return profiles
            .into_iter()
            .find(|(candidate, _)| candidate == &name)
            .map(|(_, profile)| profile)
            .ok_or_else(|| format!("capture profile {name:?} does not exist"));
    }
    match profiles.len() {
        0 => Err("no capture profile exists; run `scorepeek setup gamescope --profile NAME -- GAMESCOPE_ARGS...` first".to_owned()),
        1 => Ok(profiles.into_iter().next().expect("profile count is one").1),
        _ => Err("multiple capture profiles exist; select one with `scorepeek run --profile NAME`".to_owned()),
    }
}

pub struct RoutineStatePaths {
    pub diagnostic_session_store: PathBuf,
    recording_staging_store: PathBuf,
    pub recording_enabled: bool,
    _run_lock: File,
}

pub struct RoutineSessionPaths {
    pub root: PathBuf,
    pub capture_root: PathBuf,
    pub capture_directory: PathBuf,
    pub recognition_directory: PathBuf,
}

impl RoutineSessionPaths {
    pub fn cleanup(&self) -> Result<(), String> {
        let parent = self
            .root
            .parent()
            .ok_or_else(|| "recording staging session has no parent".to_owned())?;
        fs::remove_dir_all(&self.root)
            .map_err(|error| format!("recording staging cleanup failed: {error}"))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("recording staging cleanup sync failed: {error}"))
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
    let diagnostic_session_store = scorepeek.join("diagnostic-sessions");
    let recording_staging_store = scorepeek.join("recording-staging");
    if recording_enabled {
        ensure_directory_tree(&diagnostic_session_store)?;
        ensure_directory_tree(&recording_staging_store)?;
        ensure_session_store_capacity(&diagnostic_session_store, "joined diagnostic")?;
        ensure_session_store_capacity(&recording_staging_store, "recording staging")?;
    }
    Ok(RoutineStatePaths {
        diagnostic_session_store,
        recording_staging_store,
        recording_enabled,
        _run_lock: run_lock,
    })
}

fn ensure_session_store_capacity(root: &Path, label: &str) -> Result<(), String> {
    let mut entries = Vec::new();
    for entry in
        fs::read_dir(root).map_err(|error| format!("{label} store could not be read: {error}"))?
    {
        let entry = entry.map_err(|error| format!("{label} entry failed: {error}"))?;
        let metadata = entry
            .path()
            .symlink_metadata()
            .map_err(|error| format!("{label} entry inspection failed: {error}"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(format!("{label} store contains an unexpected entry"));
        }
        entries.push((
            metadata
                .modified()
                .map_err(|error| format!("{label} mtime failed: {error}"))?,
            entry.path(),
            directory_bytes(&entry.path(), label)?,
        ));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    let mut bytes = entries.iter().try_fold(0u64, |total, entry| {
        total
            .checked_add(entry.2)
            .ok_or_else(|| format!("{label} byte count overflowed"))
    })?;
    while entries.len() >= MAX_JOINED_SESSION_GENERATIONS
        || bytes > MAX_JOINED_SESSION_AGGREGATE_BYTES - MAX_JOINED_SESSION_BYTES
    {
        let (_, oldest, removed_bytes) = entries.remove(0);
        fs::remove_dir_all(&oldest)
            .map_err(|error| format!("old {label} removal failed: {error}"))?;
        File::open(root)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("{label} store sync failed: {error}"))?;
        bytes = bytes.saturating_sub(removed_bytes);
    }
    Ok(())
}

fn directory_bytes(root: &Path, label: &str) -> Result<u64, String> {
    let mut total = 0u64;
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| format!("{label} directory could not be read: {error}"))?
        {
            let entry = entry.map_err(|error| format!("{label} file failed: {error}"))?;
            let metadata = entry
                .path()
                .symlink_metadata()
                .map_err(|error| format!("{label} file could not be inspected: {error}"))?;
            if metadata.file_type().is_symlink() {
                return Err(format!("{label} session contains a symlink"));
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total = total
                    .checked_add(metadata.len())
                    .ok_or_else(|| format!("{label} byte count overflowed"))?;
            } else {
                return Err(format!("{label} session contains a special file"));
            }
        }
    }
    Ok(total)
}

impl RoutineStatePaths {
    pub fn recording_staging_store(&self) -> Option<PathBuf> {
        self.recording_enabled
            .then(|| self.recording_staging_store.clone())
    }

    pub fn start_recording_session(
        &self,
        session_id: &str,
    ) -> Result<Option<RoutineSessionPaths>, String> {
        if !self.recording_enabled {
            return Ok(None);
        }
        ensure_session_store_capacity(&self.recording_staging_store, "recording staging")?;
        create_recording_session(&self.recording_staging_store, session_id).map(Some)
    }
}

fn create_recording_session(root: &Path, session_id: &str) -> Result<RoutineSessionPaths, String> {
    create_recording_session_with_sync(root, session_id, sync_directory_component)
}

fn create_recording_session_with_sync(
    root: &Path,
    session_id: &str,
    sync: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<RoutineSessionPaths, String> {
    let session_root = root.join(session_id);
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder
        .create(&session_root)
        .map_err(|error| format!("recording staging session creation failed: {error}"))?;
    if let Err(error) = sync(&session_root) {
        let _ = fs::remove_dir(&session_root);
        let _ = File::open(root).and_then(|directory| directory.sync_all());
        return Err(error);
    }
    Ok(recording_session_paths(session_root))
}

fn recording_session_paths(root: PathBuf) -> RoutineSessionPaths {
    RoutineSessionPaths {
        capture_root: root.clone(),
        capture_directory: root.join("capture"),
        recognition_directory: root.join("recognition"),
        root,
    }
}

fn setup_gamescope(name: &OsStr, arguments: &[OsString], bundle: &Path) -> Result<(), String> {
    let name = profile_name(name)?;
    let arguments = gamescope_arguments(arguments)?;
    let directory = profile_directory()?;
    ensure_directory_tree(&directory)?;
    let output = directory.join(format!("{name}.json"));
    if output.symlink_metadata().is_ok() {
        return Err(format!("capture profile {name:?} already exists"));
    }
    let executable = env::current_exe()
        .map_err(|error| format!("current scorepeek executable could not be resolved: {error}"))?;
    let mut command = Command::new("gamescope");
    command.args(&arguments).arg("--").arg(&executable);
    if bundle.is_dir() {
        command.arg("--model-bundle").arg(bundle);
    }
    command
        .arg("__calibration-marker")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let mut child = command
        .spawn()
        .map_err(|error| format!("dedicated calibration Gamescope could not start: {error}"))?;
    thread::sleep(Duration::from_millis(750));
    if let Some(status) = child
        .try_wait()
        .map_err(|error| format!("calibration Gamescope status failed: {error}"))?
    {
        return Err(format!(
            "dedicated calibration Gamescope exited early: {status}"
        ));
    }
    let guided = capture_calibration::capture_guided_gamescope_profile(
        &capture_calibration::GuidedGamescopeProfileInput {
            expected_marker_rgb8: &calibration_marker::rgb8(),
        },
    );
    let _ = child.kill();
    let _ = child.wait();
    let guided = guided?;
    publish_create_only(&output, &guided.binding.bytes)?;
    let summary = SetupSummary {
        schema: "scorepeek-gamescope-profile-setup-v2",
        profile: name,
        path: output,
        profile_sha256: guided.binding.artifact_sha256,
        observed_width: guided.observed_width,
        observed_height: guided.observed_height,
        source_rectangle: guided.geometry,
        verified_fiducial_count: guided.verified_fiducial_count,
    };
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("profile setup result encoding failed: {error}"))?
    );
    Ok(())
}

fn list_profiles() -> Result<(), String> {
    let profiles = load_profiles()?;
    let summaries = profiles
        .iter()
        .map(|(name, profile)| ProfileSummary {
            name,
            path: &profile.path,
            profile_sha256: &profile.digest,
            observed_width: profile.binding.observed_width(),
            observed_height: profile.binding.observed_height(),
            source_rectangle: profile.binding.source_rectangle(),
        })
        .collect::<Vec<_>>();
    println!(
        "{}",
        serde_json::to_string(&summaries)
            .map_err(|error| format!("profile list encoding failed: {error}"))?
    );
    Ok(())
}

fn load_profiles() -> Result<Vec<(String, SelectedProfile)>, String> {
    let directory = profile_directory()?;
    load_profiles_from(&directory)
}

fn load_profiles_from(directory: &Path) -> Result<Vec<(String, SelectedProfile)>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "capture profile directory could not be read: {error}"
            ));
        }
    };
    let mut profiles = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("capture profile entry failed: {error}"))?;
        let path = entry.path();
        if path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.ends_with(PROFILE_STAGING_SUFFIX))
        {
            continue;
        }
        let Some(name) = path
            .file_stem()
            .and_then(OsStr::to_str)
            .filter(|_| path.extension() == Some(OsStr::new("json")))
        else {
            return Err(format!(
                "capture profile directory contains an unexpected entry: {}",
                path.display()
            ));
        };
        profile_name(OsStr::new(name))?;
        let bytes = read_profile(&path)?;
        let digest = sha256(&bytes);
        let binding = GamescopeProfileBinding::parse(&bytes, &digest)
            .map_err(|error| format!("capture profile {} is invalid: {error:?}", path.display()))?;
        if !binding.is_measured() {
            return Err(format!(
                "capture profile {} uses an obsolete schema; recreate it with `scorepeek setup gamescope`",
                path.display()
            ));
        }
        profiles.push((
            name.to_owned(),
            SelectedProfile {
                path,
                digest,
                binding,
            },
        ));
    }
    profiles.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(profiles)
}

fn read_profile(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = path
        .metadata()
        .map_err(|error| format!("capture profile inspection failed: {error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_PROFILE_BYTES {
        return Err(format!(
            "capture profile is not a bounded regular file: {}",
            path.display()
        ));
    }
    let bytes =
        fs::read(path).map_err(|error| format!("capture profile could not be read: {error}"))?;
    if bytes.len() as u64 != metadata.len() {
        return Err(format!(
            "capture profile changed while reading: {}",
            path.display()
        ));
    }
    Ok(bytes)
}

fn profile_directory() -> Result<PathBuf, String> {
    Ok(xdg_base(
        env::var_os("XDG_CONFIG_HOME"),
        env::var_os("HOME"),
        ".config",
    )?
    .join("scorepeek/profiles"))
}

fn xdg_base(
    configured: Option<OsString>,
    home: Option<OsString>,
    fallback: &str,
) -> Result<PathBuf, String> {
    if let Some(configured) = configured {
        let path = PathBuf::from(configured);
        if path.is_absolute() && !path.as_os_str().is_empty() {
            return Ok(path);
        }
        return Err("XDG base directory must be absolute and non-empty".to_owned());
    }
    let home = PathBuf::from(
        home.ok_or_else(|| "HOME is required when an XDG base directory is unset".to_owned())?,
    );
    if !home.is_absolute() || home.as_os_str().is_empty() {
        return Err("HOME must be absolute and non-empty".to_owned());
    }
    Ok(home.join(fallback))
}

fn profile_name(name: &OsStr) -> Result<String, String> {
    let name = name
        .to_str()
        .ok_or_else(|| "profile name must be UTF-8".to_owned())?;
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err("profile name must be 1-64 ASCII letters, digits, '.', '_', or '-'".to_owned());
    }
    Ok(name.to_owned())
}

fn gamescope_arguments(arguments: &[OsString]) -> Result<Vec<String>, String> {
    if arguments.len() > 128 {
        return Err("Gamescope argument count exceeds 128".to_owned());
    }
    let mut total = 0usize;
    let mut result = Vec::with_capacity(arguments.len());
    for argument in arguments {
        let argument = argument
            .to_str()
            .ok_or_else(|| "Gamescope arguments must be UTF-8".to_owned())?;
        total = total
            .checked_add(argument.len())
            .filter(|total| *total <= 16 * 1024)
            .ok_or_else(|| "Gamescope arguments exceed 16 KiB".to_owned())?;
        if matches!(
            argument,
            "--" | "--help" | "--version" | "-R" | "--ready-fd" | "--keep-alive"
        ) || argument.starts_with("--ready-fd=")
            || argument.starts_with("--keep-alive=")
            || (argument.starts_with("-R") && argument.len() > 2)
        {
            return Err(format!(
                "Gamescope argument {argument:?} conflicts with calibration ownership"
            ));
        }
        result.push(argument.to_owned());
    }
    Ok(result)
}

fn ensure_directory_tree(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("capture profile directory must be absolute".to_owned());
    }
    let mut current = PathBuf::from("/");
    for component in path.components().skip(1) {
        current.push(component.as_os_str());
        match current.metadata() {
            Ok(metadata) if metadata.is_dir() => {
                sync_directory_component(&current)?;
                continue;
            }
            Ok(_) => {
                return Err(format!(
                    "capture profile directory component is not a directory: {}",
                    current.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "capture profile directory component could not be inspected: {error}"
                ));
            }
        }
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(&current) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = current.metadata().map_err(|error| {
                    format!("capture profile directory race could not be inspected: {error}")
                })?;
                if metadata.is_dir() {
                    sync_directory_component(&current)?;
                    continue;
                }
                return Err(format!(
                    "capture profile directory component is not a directory: {}",
                    current.display()
                ));
            }
            Err(error) => {
                return Err(format!(
                    "capture profile directory could not be created: {error}"
                ));
            }
        }
        sync_directory_component(&current)?;
    }
    Ok(())
}

fn sync_directory_component(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "capture profile directory has no parent".to_owned())?;
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .and_then(|()| File::open(parent)?.sync_all())
        .map_err(|error| format!("capture profile directory could not be synced: {error}"))
}

fn publish_create_only(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "profile path has no parent".to_owned())?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before the Unix epoch".to_owned())?
        .as_nanos();
    let staging = parent.join(format!(
        ".{}.{}.{}{}",
        path.file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("profile"),
        std::process::id(),
        nonce,
        PROFILE_STAGING_SUFFIX,
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staging)
        .map_err(|error| format!("capture profile staging could not be created: {error}"))?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&staging);
        return Err(format!("capture profile publication failed: {error}"));
    }
    if let Err(error) = fs::hard_link(&staging, path) {
        let _ = fs::remove_file(&staging);
        return Err(format!("capture profile publication failed: {error}"));
    }
    if let Err(error) = File::open(parent).and_then(|directory| directory.sync_all()) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(&staging);
        let _ = File::open(parent).and_then(|directory| directory.sync_all());
        return Err(format!("capture profile publication failed: {error}"));
    }
    if fs::remove_file(&staging).is_ok() {
        let _ = File::open(parent).and_then(|directory| directory.sync_all());
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_JOINED_SESSION_GENERATIONS, create_recording_session,
        create_recording_session_with_sync, ensure_directory_tree, ensure_session_store_capacity,
        gamescope_arguments, load_profiles_from, profile_name, publish_create_only, read_profile,
        recording_session_paths,
    };
    use std::ffi::{OsStr, OsString};
    use std::os::unix::fs::symlink;

    use scorepeek::capture::{
        FractionalRectangle, GamescopeProfileBinding, GamescopeProfileBindingAuthoringInput,
        RationalCoordinate, UncalibratedMemoryType, UncalibratedVideoContract,
    };

    #[test]
    fn profile_names_are_path_safe() {
        assert_eq!(
            profile_name(OsStr::new("bazzite-4k.120")).unwrap(),
            "bazzite-4k.120"
        );
        assert!(profile_name(OsStr::new("../profile")).is_err());
    }

    #[test]
    fn gamescope_arguments_are_passed_to_the_calibration_process() {
        let raw = [
            "--backend",
            "wayland",
            "-w1920",
            "--nested-height=1080",
            "-r",
            "120",
            "-S",
            "fit",
            "-Flinear",
            "--hdr-enabled",
        ]
        .map(OsString::from);
        let arguments = gamescope_arguments(&raw).unwrap();
        assert_eq!(arguments.last().unwrap(), "--hdr-enabled");
        assert_eq!(
            arguments,
            raw.iter()
                .map(|value| value.to_string_lossy())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn lifecycle_conflicts_are_rejected() {
        assert!(gamescope_arguments(&[OsString::from("--keep-alive")]).is_err());
        assert!(gamescope_arguments(&[OsString::from("--ready-fd=4")]).is_err());
        assert!(gamescope_arguments(&[OsString::from("-R4")]).is_err());
    }

    #[test]
    fn nested_directories_and_profiles_are_create_only() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("config/scorepeek/profiles");
        ensure_directory_tree(&directory).unwrap();
        assert!(directory.is_dir());

        let profile = directory.join("target.json");
        publish_create_only(&profile, b"first").unwrap();
        assert_eq!(std::fs::read(&profile).unwrap(), b"first");
        assert!(publish_create_only(&profile, b"second").is_err());
        assert_eq!(std::fs::read(profile).unwrap(), b"first");
    }

    #[test]
    fn directory_creation_follows_operator_symlinked_ancestors() {
        let temporary = tempfile::tempdir().unwrap();
        let actual_home = temporary.path().join("actual-home");
        std::fs::create_dir(&actual_home).unwrap();
        let home_alias = temporary.path().join("home");
        symlink(&actual_home, &home_alias).unwrap();

        let directory = home_alias.join("user/.config/scorepeek/profiles");
        ensure_directory_tree(&directory).unwrap();

        assert!(actual_home.join("user/.config/scorepeek/profiles").is_dir());
    }

    #[test]
    fn profile_files_may_be_operator_symlinks() {
        let temporary = tempfile::tempdir().unwrap();
        let actual = temporary.path().join("actual.json");
        std::fs::write(&actual, b"profile").unwrap();
        let alias = temporary.path().join("linked.json");
        symlink(&actual, &alias).unwrap();

        assert_eq!(read_profile(&alias).unwrap(), b"profile");
    }

    #[test]
    fn obsolete_local_profile_requires_setup_recreation() {
        let temporary = tempfile::tempdir().unwrap();
        let video = UncalibratedVideoContract {
            width: 4,
            height: 2,
            framerate_num: 0,
            framerate_denom: 1,
            maximum_framerate_num: 0,
            maximum_framerate_denom: 0,
            pixel_aspect_num: 0,
            pixel_aspect_denom: 0,
            chroma_site: 0,
            color_range: 0,
            color_matrix: 0,
            transfer_function: 0,
            color_primaries: 0,
        };
        let authored = GamescopeProfileBinding::author_local(
            GamescopeProfileBindingAuthoringInput {
                calibration_evidence_sha256: "1".repeat(64),
                environment_id: "local".to_owned(),
                gamescope_version: "3.16.19".to_owned(),
                backend_id: "wayland".to_owned(),
                output_width: 4,
                output_height: 2,
                nested_width: 4,
                nested_height: 2,
                nested_refresh_hz: 60,
                scaler: "auto".to_owned(),
                filter: "linear".to_owned(),
                observed_video_contract: video,
                memory_type: UncalibratedMemoryType::MemoryPointer,
                stride: 16,
                geometry: FractionalRectangle::new(
                    RationalCoordinate::new(0, 1).unwrap(),
                    RationalCoordinate::new(0, 1).unwrap(),
                    RationalCoordinate::new(4, 1).unwrap(),
                    RationalCoordinate::new(2, 1).unwrap(),
                ),
            },
            vec!["--backend".to_owned(), "wayland".to_owned()],
        )
        .unwrap();
        std::fs::write(temporary.path().join("old.json"), authored.bytes).unwrap();
        let Err(error) = load_profiles_from(temporary.path()) else {
            panic!("obsolete profile was accepted");
        };
        assert!(error.contains("obsolete schema"));
        assert!(error.contains("setup gamescope"));
    }

    #[test]
    fn session_store_reclaims_the_oldest_generation_at_capacity() {
        let temporary = tempfile::tempdir().unwrap();
        for index in 0..MAX_JOINED_SESSION_GENERATIONS {
            let generation = temporary.path().join(format!("run-{index}"));
            std::fs::create_dir(&generation).unwrap();
            std::fs::write(generation.join("manifest.json"), b"complete").unwrap();
        }
        ensure_session_store_capacity(temporary.path(), "test session").unwrap();
        assert_eq!(
            std::fs::read_dir(temporary.path()).unwrap().count(),
            MAX_JOINED_SESSION_GENERATIONS - 1
        );
        assert!(!temporary.path().join("run-0").exists());
    }

    #[test]
    fn recording_staging_is_grouped_by_session_then_purpose() {
        let paths = recording_session_paths(std::path::PathBuf::from(
            "/state/scorepeek/recording-staging/session-1",
        ));
        assert_eq!(paths.capture_root, paths.root);
        assert_eq!(paths.capture_directory, paths.root.join("capture"));
        assert_eq!(paths.recognition_directory, paths.root.join("recognition"));
    }

    #[test]
    fn recording_staging_cleanup_removes_the_complete_session_tree() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = create_recording_session(temporary.path(), "session-1").unwrap();
        let root = paths.root.clone();
        for purpose in ["capture", "recognition", "events", "canonical"] {
            std::fs::create_dir(root.join(purpose)).unwrap();
            std::fs::write(root.join(purpose).join("artifact"), b"evidence").unwrap();
        }

        paths.cleanup().unwrap();

        assert!(!root.exists());
        assert!(temporary.path().is_dir());

        let retried = create_recording_session(temporary.path(), "session-1").unwrap();
        assert_eq!(retried.root, root);
    }

    #[test]
    fn recording_staging_creation_rolls_back_after_sync_failure() {
        let temporary = tempfile::tempdir().unwrap();
        let Err(error) = create_recording_session_with_sync(temporary.path(), "session-1", |_| {
            Err("injected session sync failure".to_owned())
        }) else {
            panic!("injected sync failure was accepted");
        };

        assert_eq!(error, "injected session sync failure");
        assert!(!temporary.path().join("session-1").exists());
        assert!(create_recording_session(temporary.path(), "session-1").is_ok());
    }
}
