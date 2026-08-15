use std::collections::BTreeMap;
use std::fmt::Write as _;
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
    observations.insert(
        "gamescope",
        probe_stderr(
            runner,
            "/usr/bin/gamescope",
            &["--version"],
            gamescope_version,
        ),
    );
    observations.insert("gamescope_session_flags", Observation::Unavailable);
    observations.insert(
        "gstreamer",
        probe(
            runner,
            "/usr/bin/gst-inspect-1.0",
            &["--version"],
            gstreamer_version,
        ),
    );
    observations.insert(
        "pipewire",
        probe(
            runner,
            "/usr/bin/pipewire",
            &["--version"],
            pipewire_version,
        ),
    );
    observations.insert(
        "obs_studio_flatpak",
        probe(
            runner,
            "/usr/bin/flatpak",
            &["list", "--app", "--columns=application,version"],
            flatpak_obs_version,
        ),
    );
    observations.insert(
        "obs_vkcapture",
        probe(
            runner,
            "/usr/bin/rpm",
            &["-qa", "obs-vkcapture", "--qf", "%{NAME}\\t%{EVR}\\n"],
            rpm_obs_vkcapture_version,
        ),
    );
    observations.insert("obs_websocket", Observation::Unavailable);
    observations.insert(
        "gamescope_pipewire_caps",
        probe(
            runner,
            "/usr/bin/gst-device-monitor-1.0",
            &["Video/Source"],
            gamescope_caps_summary,
        ),
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

fn probe_stderr(
    runner: &impl Runner,
    program: &str,
    args: &[&str],
    parse: fn(&str) -> Option<String>,
) -> Observation {
    probe_stream(runner, program, args, parse, true)
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

fn gamescope_version(output: &str) -> Option<String> {
    let output = strip_ansi_sgr(output)?;
    output.lines().find_map(|line| {
        let version = line.split_once("gamescope version ")?.1;
        version.split_whitespace().next().and_then(version_token)
    })
}

fn gstreamer_version(output: &str) -> Option<String> {
    prefixed_version(output, "gst-inspect-1.0 version ")
}

fn pipewire_version(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("Compiled with libpipewire ")
            .and_then(version_token)
    })
}

fn prefixed_version(output: &str, prefix: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(prefix).and_then(version_token))
}

fn version_token(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'~' | b':' | b'-')
        }))
    .then(|| value.to_owned())
}

fn strip_ansi_sgr(value: &str) -> Option<String> {
    let mut output = String::new();
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\u{1b}' {
            output.push(character);
            continue;
        }
        if characters.next()? != '[' {
            return None;
        }
        loop {
            let parameter = characters.next()?;
            if parameter == 'm' {
                break;
            }
            if !parameter.is_ascii_digit() && parameter != ';' {
                return None;
            }
        }
    }
    Some(output)
}

fn flatpak_obs_version(output: &str) -> Option<String> {
    listed_version(output, "com.obsproject.Studio")
}

fn rpm_obs_vkcapture_version(output: &str) -> Option<String> {
    listed_version(output, "obs-vkcapture")
}

fn listed_version(output: &str, expected_id: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (id, version) = line.split_once('\t')?;
        (id == expected_id)
            .then(|| version_token(version))
            .flatten()
    })
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

fn gamescope_caps_summary(output: &str) -> Option<String> {
    let devices = output
        .split("Device found:")
        .filter(|device| is_gamescope_video_source(device))
        .collect::<Vec<_>>();
    let [device] = devices.as_slice() else {
        return None;
    };

    let mut summary = vec![
        "node.name=gamescope".to_owned(),
        "media.class=Video/Source".to_owned(),
    ];
    let caps = collect_caps(device)?;
    for structure in caps {
        summary.extend(parse_caps(structure)?);
    }

    let summary = summary.join(" | ");
    (!summary.is_empty()).then_some(summary)
}

fn collect_caps(device: &str) -> Option<Vec<&str>> {
    let mut caps = Vec::new();
    let mut reading_caps = false;
    for line in device.lines().map(str::trim) {
        if let Some((key, value)) = line.split_once(':')
            && key.trim() == "caps"
        {
            let value = value.trim();
            if value.is_empty() {
                return None;
            }
            caps.push(value);
            reading_caps = true;
            continue;
        }
        if reading_caps {
            if line == "properties:" {
                break;
            }
            if line.is_empty() {
                continue;
            }
            if !line.starts_with("video/") {
                return None;
            }
            caps.push(line);
        }
    }
    (!caps.is_empty()).then_some(caps)
}

fn parse_caps(caps: &str) -> Option<Vec<String>> {
    let mut parsed = Vec::new();
    for structure in split_top_level(caps, ';') {
        let fields = split_top_level(structure, ',');
        let (media_type, memory) = parse_media_type(fields.first()?.trim())?;
        let mut values = BTreeMap::new();
        for field in fields.iter().skip(1) {
            let Some((key, value)) = field.split_once('=') else {
                continue;
            };
            let key = key.trim();
            if matches!(key, "format" | "width" | "height" | "framerate") {
                values.insert(key, caps_value(strip_caps_type(value.trim()))?);
            }
        }
        if !["format", "width", "height", "framerate"]
            .into_iter()
            .all(|key| values.contains_key(key))
        {
            return None;
        }
        parsed.push(format!("media_type={media_type}"));
        parsed.push(format!("memory={memory}"));
        for key in ["format", "width", "height", "framerate"] {
            parsed.push(format!("{key}={}", values[key]));
        }
    }
    (!parsed.is_empty()).then_some(parsed)
}

fn split_top_level(value: &str, separator: char) -> Vec<&str> {
    let mut depth = 0_u32;
    let mut start = 0;
    let mut fields = Vec::new();
    for (index, character) in value.char_indices() {
        match character {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            character if character == separator && depth == 0 => {
                fields.push(&value[start..index]);
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    fields.push(&value[start..]);
    fields
}

fn parse_media_type(value: &str) -> Option<(&'static str, String)> {
    if value == "video/x-raw" {
        return Some(("video/x-raw", "SystemMemory".to_owned()));
    }
    let feature = value
        .strip_prefix("video/x-raw(memory:")?
        .strip_suffix(')')?;
    Some(("video/x-raw", caps_value(feature)?))
}

fn strip_caps_type(value: &str) -> &str {
    value
        .strip_prefix("(string)")
        .or_else(|| value.strip_prefix("(int)"))
        .or_else(|| value.strip_prefix("(fraction)"))
        .unwrap_or(value)
        .trim()
}

fn is_gamescope_video_source(device: &str) -> bool {
    let mut name = false;
    let mut media_class = false;
    for line in device.lines().map(str::trim) {
        if let Some((key, value)) = line.split_once(':') {
            name |= key.trim() == "name" && unquote(value.trim()) == "gamescope";
        }
        if let Some((key, value)) = line.split_once('=') {
            name |= key.trim() == "node.name" && unquote(value.trim()) == "gamescope";
            media_class |= key.trim() == "media.class" && unquote(value.trim()) == "Video/Source";
        }
    }
    name && media_class
}

fn unquote(value: &str) -> &str {
    value.trim_matches('"')
}

fn caps_value(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'/' | b'['
                        | b']'
                        | b'{'
                        | b'}'
                        | b'('
                        | b')'
                        | b','
                        | b'.'
                        | b'_'
                        | b'-'
                        | b' '
                )
        }))
    .then(|| value.to_owned())
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
    pub fn to_json(&self) -> String {
        let mut json = String::from("{\"schema\":");
        write_json_string(&mut json, SCHEMA);
        json.push_str(",\"os\":{");
        write_string_map(&mut json, &self.os);
        json.push_str("},\"observations\":{");

        for (index, (name, observation)) in self.observations.iter().enumerate() {
            if index > 0 {
                json.push(',');
            }
            write_json_string(&mut json, name);
            json.push(':');
            observation.write_json(&mut json);
        }

        json.push_str("}}");
        json
    }
}

impl Observation {
    fn write_json(&self, json: &mut String) {
        match self {
            Self::Detected(value) => {
                json.push_str("{\"status\":\"detected\",\"value\":");
                write_json_string(json, value);
                json.push('}');
            }
            Self::Unavailable => json.push_str("{\"status\":\"unavailable\"}"),
            Self::Failed(exit_code) => {
                write!(json, "{{\"status\":\"failed\",\"exit_code\":{exit_code}}}")
                    .expect("writing to a String cannot fail");
            }
        }
    }
}

fn write_string_map(json: &mut String, values: &BTreeMap<String, String>) {
    for (index, (key, value)) in values.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        write_json_string(json, key);
        json.push(':');
        write_json_string(json, value);
    }
}

fn write_json_string(json: &mut String, value: &str) {
    json.push('"');
    for character in value.chars() {
        match character {
            '"' => json.push_str("\\\""),
            '\\' => json.push_str("\\\\"),
            '\n' => json.push_str("\\n"),
            '\r' => json.push_str("\\r"),
            '\t' => json.push_str("\\t"),
            character if character.is_control() => {
                write!(json, "\\u{:04x}", u32::from(character))
                    .expect("writing to a String cannot fail");
            }
            character => json.push(character),
        }
    }
    json.push('"');
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
                success_streams(
                    "/usr/bin/gamescope --version",
                    "diagnostic path /run/user/1000/private\n",
                    "[gamescope] [\u{1b}[0;34mInfo\u{1b}[0m] console: gamescope version 3.16.19-128-g7282613+ (gcc 16.1.1)\n",
                ),
                success(
                    "/usr/bin/gst-inspect-1.0 --version",
                    "gst-inspect-1.0 version 1.26.0\n",
                ),
                success(
                    "/usr/bin/pipewire --version",
                    "pipewire\nCompiled with libpipewire 1.4.0\n",
                ),
                success(
                    "/usr/bin/flatpak list --app --columns=application,version",
                    "org.example.Other\t1.0\ncom.obsproject.Studio\t31.0.0\n",
                ),
                success(
                    "/usr/bin/rpm -qa obs-vkcapture --qf %{NAME}\\t%{EVR}\\n",
                    "obs-vkcapture\t1.5.0-1.fc42\n",
                ),
                success(
                    "/usr/bin/gst-device-monitor-1.0 Video/Source",
                    "Device found:\nname: private camera\nmedia.class = Video/Source\ncaps : video/x-raw, format=(string)YUY2, width=(int)1280, height=(int)720, framerate=(fraction)30/1\nDevice found:\nname: gamescope\ncaps : video/x-raw, format=(string)BGRx, width=(int)3840, height=(int)2160, framerate=(fraction)60/1\n       video/x-raw(memory:DMABuf), format=(string)NV12, width=(int)3840, height=(int)2160, framerate=(fraction)60/1\nproperties:\n  media.class = Video/Source\n",
                ),
            ]),
        };

        let inventory = collect_with(&runner, "/path/that/does/not/exist").to_json();

        assert!(inventory.contains("\"schema\":\"scorepeek-target-inventory-v1\""));
        assert!(inventory.contains("3.16.19-128-g7282613+"));
        assert!(inventory.contains("Example GPU [1234:5678] | Kernel driver in use: amdgpu"));
        assert!(inventory.contains("node.name=gamescope | media.class=Video/Source"));
        assert!(inventory.contains("memory=SystemMemory | format=BGRx | width=3840"));
        assert!(inventory.contains("memory=DMABuf | format=NV12 | width=3840"));
        assert!(!inventory.contains("YUY2"));
        assert!(!inventory.contains("/run/user/1000/private"));
        assert!(!inventory.contains("Subsystem: private"));
        assert!(!inventory.contains("secret_driver"));
        assert!(!inventory.contains("secret from stderr"));
        assert!(inventory.contains("\"obs_websocket\":{\"status\":\"unavailable\"}"));
        assert!(inventory.contains("\"gamescope_session_flags\":{\"status\":\"unavailable\"}"));
    }

    #[test]
    fn command_failures_expose_only_status_and_exit_code() {
        let runner = FakeRunner {
            outputs: HashMap::from([
                failed("/usr/bin/uname -r", 7, "secret from stderr"),
                missing("/usr/bin/lspci -Dnnk"),
                missing("/usr/bin/gamescope --version"),
                missing("/usr/bin/gst-inspect-1.0 --version"),
                missing("/usr/bin/pipewire --version"),
                missing("/usr/bin/flatpak list --app --columns=application,version"),
                missing("/usr/bin/rpm -qa obs-vkcapture --qf %{NAME}\\t%{EVR}\\n"),
                missing("/usr/bin/gst-device-monitor-1.0 Video/Source"),
            ]),
        };

        let inventory = collect_with(&runner, "/path/that/does/not/exist").to_json();

        assert!(inventory.contains("\"kernel\":{\"status\":\"failed\",\"exit_code\":7}"));
        assert!(inventory.contains("\"gpu\":{\"status\":\"unavailable\"}"));
        assert!(!inventory.contains("secret from stderr"));
    }

    #[test]
    fn absent_app_and_package_are_unavailable() {
        let runner = FakeRunner {
            outputs: HashMap::from([
                success(
                    "/usr/bin/flatpak list --app --columns=application,version",
                    "org.example.Other\t1.0\n",
                ),
                success(
                    "/usr/bin/rpm -qa obs-vkcapture --qf %{NAME}\\t%{EVR}\\n",
                    "",
                ),
            ]),
        };

        assert!(matches!(
            probe(
                &runner,
                "/usr/bin/flatpak",
                &["list", "--app", "--columns=application,version"],
                flatpak_obs_version,
            ),
            Observation::Unavailable
        ));
        assert!(matches!(
            probe(
                &runner,
                "/usr/bin/rpm",
                &["-qa", "obs-vkcapture", "--qf", "%{NAME}\\t%{EVR}\\n"],
                rpm_obs_vkcapture_version,
            ),
            Observation::Unavailable
        ));
    }

    #[test]
    fn json_strings_are_escaped() {
        let mut json = String::new();
        write_json_string(&mut json, "a\"b\\c\n");
        assert_eq!(json, "\"a\\\"b\\\\c\\n\"");
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
    fn multiple_gamescope_sources_are_rejected() {
        let device = "Device found:\nname: gamescope\nmedia.class = Video/Source\ncaps : video/x-raw, format=(string)BGRx, width=(int)3840, height=(int)2160, framerate=(fraction)60/1\n";
        assert!(gamescope_caps_summary(&format!("{device}{device}")).is_none());
    }

    #[test]
    fn caps_require_all_allowlisted_fields_and_record_memory() {
        let dmabuf = parse_caps(
            "video/x-raw(memory:DMABuf), format=(string)NV12, width=(int)3840, height=(int)2160, framerate=(fraction)60/1",
        )
        .expect("complete caps must parse");

        assert!(dmabuf.contains(&"memory=DMABuf".to_owned()));
        assert!(parse_caps("video/x-raw, width=(int)3840, height=(int)2160").is_none());
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

    fn success_streams(
        key: &str,
        stdout: &str,
        stderr: &str,
    ) -> (String, Result<(i32, String, String), io::ErrorKind>) {
        (
            key.to_owned(),
            Ok((0, stdout.to_owned(), stderr.to_owned())),
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
