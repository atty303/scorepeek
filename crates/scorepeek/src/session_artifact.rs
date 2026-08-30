use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

const MAX_FILES: usize = 20_000;
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_NDJSON_RECORDS: usize = 250_000;
const MAX_NDJSON_RECORD_BYTES: u64 = 1024 * 1024;

#[derive(Serialize)]
struct SessionManifest<'a> {
    schema: &'static str,
    source_kind: &'static str,
    session_id: &'a str,
    capture_generation: u64,
    profile_sha256: &'a str,
    catalog_sha256: &'a str,
    recognition_interval_ms: u64,
    processed_ticks: u64,
    busy_skips: u64,
    maximum_consecutive_busy_skips: u64,
    completeness: &'a str,
    capture_manifest_sha256: &'a str,
    recognition_manifest_sha256: &'a str,
    event_manifest_sha256: &'a str,
    artifacts: Vec<Artifact>,
}

#[derive(Serialize)]
struct Artifact {
    kind: &'static str,
    path: String,
    sha256: String,
    bytes: u64,
}

pub struct PublishRequest<'a> {
    pub root: &'a Path,
    pub session_id: &'a str,
    pub capture_generation: u64,
    pub profile_sha256: &'a str,
    pub catalog_sha256: &'a str,
    pub processed_ticks: u64,
    pub busy_skips: u64,
    pub maximum_consecutive_busy_skips: u64,
    pub completeness: &'a str,
    pub capture_directory: &'a Path,
    pub capture_manifest_sha256: &'a str,
    pub recognition_directory: &'a Path,
    pub recognition_manifest_sha256: &'a str,
    pub event_directory: &'a Path,
    pub event_manifest_sha256: &'a str,
    pub profile_path: &'a Path,
}

pub fn publish(request: &PublishRequest<'_>) -> Result<PathBuf, String> {
    let destination = request.root.join(request.session_id);
    if destination.symlink_metadata().is_ok() {
        return Err("diagnostic session already exists".to_owned());
    }
    let staging = request
        .root
        .join(format!(".session-staging-{}", request.session_id));
    if staging.symlink_metadata().is_ok() {
        return Err("diagnostic session staging already exists".to_owned());
    }
    create_directory(&staging)?;
    write_file(&staging.join(".scorepeek-session-staging"), b"v1\n")?;
    let capture = staging.join("capture");
    let recognition = staging.join("recognition");
    create_directory(&capture)?;
    create_directory(&recognition)?;

    let mut artifacts = Vec::new();
    link_directory(
        request.capture_directory,
        &capture,
        "capture",
        &mut artifacts,
    )?;
    let profile_path = capture.join("profile.json");
    fs::hard_link(request.profile_path, &profile_path)
        .map_err(|error| format!("capture profile link failed: {error}"))?;
    let profile_metadata = profile_path
        .metadata()
        .map_err(|error| format!("capture profile metadata failed: {error}"))?;
    artifacts.push(Artifact {
        kind: "capture",
        path: "capture/profile.json".to_owned(),
        sha256: digest_file(&profile_path)?,
        bytes: profile_metadata.len(),
    });
    link_directory(
        request.recognition_directory,
        &recognition,
        "recognition",
        &mut artifacts,
    )?;
    let recognition_manifest_sha256 =
        rewrite_recognition_stream(request, &recognition, &mut artifacts)?;
    link_event_component(request, &staging, &mut artifacts)?;
    if artifacts.len() > MAX_FILES {
        return Err("diagnostic session file capacity exceeded".to_owned());
    }
    let manifest = SessionManifest {
        schema: "scorepeek-private-diagnostic-session-v3",
        source_kind: "live_run",
        session_id: request.session_id,
        capture_generation: request.capture_generation,
        profile_sha256: request.profile_sha256,
        catalog_sha256: request.catalog_sha256,
        recognition_interval_ms: scorepeek::recognition_cadence::RECOGNITION_INTERVAL_MS,
        processed_ticks: request.processed_ticks,
        busy_skips: request.busy_skips,
        maximum_consecutive_busy_skips: request.maximum_consecutive_busy_skips,
        completeness: request.completeness,
        capture_manifest_sha256: request.capture_manifest_sha256,
        recognition_manifest_sha256: &recognition_manifest_sha256,
        event_manifest_sha256: request.event_manifest_sha256,
        artifacts,
    };
    let mut bytes = serde_json::to_vec(&manifest)
        .map_err(|_| "diagnostic session manifest serialization failed".to_owned())?;
    bytes.push(b'\n');
    write_file(&staging.join("manifest.json"), &bytes)?;
    fs::remove_file(staging.join(".scorepeek-session-staging"))
        .map_err(|error| format!("diagnostic session marker removal failed: {error}"))?;
    File::open(&staging)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("diagnostic session staging sync failed: {error}"))?;
    fs::rename(&staging, &destination)
        .map_err(|error| format!("diagnostic session publication failed: {error}"))?;
    File::open(request.root)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("diagnostic session store sync failed: {error}"))?;
    Ok(destination)
}

fn link_event_component(
    request: &PublishRequest<'_>,
    staging: &Path,
    artifacts: &mut Vec<Artifact>,
) -> Result<(), String> {
    if digest_file(&request.event_directory.join("manifest.json"))? != request.event_manifest_sha256
    {
        return Err("event component manifest changed before publication".to_owned());
    }
    for (source, destination, digest) in [
        (
            "manifest.json",
            "event-manifest.json",
            request.event_manifest_sha256.to_owned(),
        ),
        (
            "events.ndjson",
            "events.ndjson",
            digest_file(&request.event_directory.join("events.ndjson"))?,
        ),
    ] {
        let destination_path = staging.join(destination);
        fs::hard_link(request.event_directory.join(source), &destination_path)
            .map_err(|error| format!("event artifact link failed: {error}"))?;
        let bytes = destination_path
            .metadata()
            .map_err(|error| format!("event artifact metadata failed: {error}"))?
            .len();
        artifacts.push(Artifact {
            kind: "events",
            path: destination.to_owned(),
            sha256: digest,
            bytes,
        });
    }
    Ok(())
}

#[derive(Deserialize)]
struct PredicateFactDocument {
    fact: PredicateFact,
}

#[derive(Deserialize)]
struct PredicateFact {
    tick_sequence: u64,
    monotonic_start_ms: u64,
    detail: Value,
}

fn rewrite_recognition_stream(
    request: &PublishRequest<'_>,
    destination: &Path,
    artifacts: &mut Vec<Artifact>,
) -> Result<String, String> {
    if digest_file(&request.recognition_directory.join("manifest.json"))?
        != request.recognition_manifest_sha256
    {
        return Err("recognition component manifest changed before publication".to_owned());
    }
    let mut by_tick = BTreeMap::new();
    for_each_ndjson::<PredicateFactDocument, _>(
        &request.capture_directory.join("facts.ndjson"),
        "diagnostic fact",
        |document| {
            if document.fact.detail["kind"].as_str() != Some("screen_predicate_observation") {
                return Ok(());
            }
            let screen = match document.fact.detail["screen"].as_str() {
                Some("music_selection") => "music_select",
                Some("mode_selection") => "mode_select",
                Some("decide_transition") => "decide_transition",
                Some("gameplay") => "play",
                Some(screen @ ("result" | "unknown")) => screen,
                _ => return Err("screen predicate fact has an invalid screen".to_owned()),
            };
            by_tick.insert(
                document.fact.tick_sequence,
                serde_json::json!({
                    "schema":"scorepeek-recognition-observation-v5",
                    "tick_sequence":document.fact.tick_sequence,
                    "source_timestamp_ms":document.fact.monotonic_start_ms,
                    "screen":screen,
                    "fields":null,
                    "song_id":null,
                }),
            );
            Ok(())
        },
    )?;
    for_each_ndjson::<Value, _>(
        &request.recognition_directory.join("observations.ndjson"),
        "recognition observation",
        |mut observation| {
            let tick = observation["tick_sequence"]
                .as_u64()
                .ok_or_else(|| "recognition observation lacks a tick sequence".to_owned())?;
            let (source_timestamp_ms, screen) = match by_tick.get(&tick) {
                Some(base) => (base["source_timestamp_ms"].clone(), base["screen"].clone()),
                None if request.completeness == "partial" => {
                    recognition_observation_context(&observation)?
                }
                None => return Err("recognition observation has no predicate fact".to_owned()),
            };
            observation["source_timestamp_ms"] = source_timestamp_ms;
            observation["screen"] = screen;
            by_tick.insert(tick, observation);
            Ok(())
        },
    )?;
    if request.completeness == "complete"
        && u64::try_from(by_tick.len()).unwrap_or(u64::MAX) != request.processed_ticks
    {
        return Err("processed tick count differs from predicate observations".to_owned());
    }
    artifacts.retain(|artifact| {
        artifact.path != "recognition/observations.ndjson"
            && artifact.path != "recognition/manifest.json"
    });
    for filename in ["observations.ndjson", "manifest.json"] {
        fs::remove_file(destination.join(filename))
            .map_err(|error| format!("old recognition component removal failed: {error}"))?;
    }
    let (observations_path, observation_bytes) = write_merged_observations(destination, &by_tick)?;
    artifacts.push(Artifact {
        kind: "recognition",
        path: "recognition/observations.ndjson".to_owned(),
        sha256: digest_file(&observations_path)?,
        bytes: observation_bytes,
    });
    let manifest = serde_json::json!({
        "schema":"scorepeek-recognition-evidence-manifest-v3",
        "run_id":request.session_id,
        "profile_sha256":request.profile_sha256,
        "status":request.completeness,
        "catalog_sha256":request.catalog_sha256,
        "observations_sha256":digest_file(&observations_path)?,
        "observation_count":by_tick.len(),
        "observation_bytes":observation_bytes,
    });
    let mut manifest_bytes = serde_json::to_vec(&manifest)
        .map_err(|error| format!("recognition manifest encode failed: {error}"))?;
    manifest_bytes.push(b'\n');
    let manifest_path = destination.join("manifest.json");
    write_file(&manifest_path, &manifest_bytes)?;
    let manifest_sha256 = digest_file(&manifest_path)?;
    artifacts.push(Artifact {
        kind: "recognition",
        path: "recognition/manifest.json".to_owned(),
        sha256: manifest_sha256.clone(),
        bytes: manifest_bytes.len() as u64,
    });
    Ok(manifest_sha256)
}

fn recognition_observation_context(observation: &Value) -> Result<(Value, Value), String> {
    let timestamp = observation["timing"]["monotonic_start_ms"]
        .as_u64()
        .ok_or_else(|| "recognition observation has no predicate fact or timestamp".to_owned())?;
    let screen = observation["fields"]["screen"]
        .as_str()
        .ok_or_else(|| "recognition observation has no predicate fact or screen".to_owned())?;
    if !matches!(screen, "music_select" | "result" | "unknown") {
        return Err("recognition observation has an invalid screen".to_owned());
    }
    Ok((Value::from(timestamp), Value::from(screen)))
}

fn write_merged_observations(
    destination: &Path,
    by_tick: &BTreeMap<u64, Value>,
) -> Result<(PathBuf, u64), String> {
    let path = destination.join("observations.ndjson");
    let file = create_file(&path)?;
    let mut writer = std::io::BufWriter::new(file);
    let mut total = 0_u64;
    for observation in by_tick.values() {
        let mut bytes = serde_json::to_vec(observation)
            .map_err(|error| format!("recognition observation encode failed: {error}"))?;
        bytes.push(b'\n');
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_NDJSON_RECORD_BYTES {
            return Err("recognition observation exceeds its record byte bound".to_owned());
        }
        total = total
            .checked_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
            .ok_or_else(|| "recognition observation byte count overflow".to_owned())?;
        if total > MAX_FILE_BYTES {
            return Err("recognition observation stream exceeds its byte bound".to_owned());
        }
        writer
            .write_all(&bytes)
            .map_err(|error| format!("recognition observation write failed: {error}"))?;
    }
    writer
        .flush()
        .map_err(|error| format!("recognition observation flush failed: {error}"))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|error| format!("recognition observation sync failed: {error}"))?;
    Ok((path, total))
}

fn for_each_ndjson<T, F>(path: &Path, description: &str, mut visit: F) -> Result<(), String>
where
    T: for<'de> Deserialize<'de>,
    F: FnMut(T) -> Result<(), String>,
{
    let file =
        File::open(path).map_err(|error| format!("{description}s could not be read: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut record_count = 0_usize;
    loop {
        line.clear();
        let read = reader
            .by_ref()
            .take(MAX_NDJSON_RECORD_BYTES + 1)
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("{description} could not be read: {error}"))?;
        if read == 0 {
            break;
        }
        if u64::try_from(read).unwrap_or(u64::MAX) > MAX_NDJSON_RECORD_BYTES
            || line.last() != Some(&b'\n')
        {
            return Err(format!("{description} exceeds its record byte bound"));
        }
        record_count = record_count.saturating_add(1);
        if record_count > MAX_NDJSON_RECORDS {
            return Err(format!("{description} count exceeds its bound"));
        }
        let record = serde_json::from_slice(&line)
            .map_err(|error| format!("{description} is invalid: {error}"))?;
        visit(record)?;
    }
    Ok(())
}

fn link_directory(
    source: &Path,
    destination: &Path,
    kind: &'static str,
    artifacts: &mut Vec<Artifact>,
) -> Result<(), String> {
    for entry in fs::read_dir(source)
        .map_err(|error| format!("diagnostic component could not be read: {error}"))?
    {
        let entry = entry.map_err(|error| format!("diagnostic component entry failed: {error}"))?;
        let metadata = entry
            .path()
            .symlink_metadata()
            .map_err(|error| format!("diagnostic component metadata failed: {error}"))?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > MAX_FILE_BYTES
        {
            return Err("diagnostic component contains an invalid file".to_owned());
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "diagnostic component filename must be UTF-8".to_owned())?;
        let target = destination.join(&name);
        fs::hard_link(entry.path(), &target)
            .map_err(|error| format!("diagnostic component link failed: {error}"))?;
        artifacts.push(Artifact {
            kind,
            path: format!("{kind}/{name}"),
            sha256: digest_file(&target)?,
            bytes: metadata.len(),
        });
    }
    File::open(destination)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("diagnostic component directory sync failed: {error}"))
}

fn create_directory(path: &Path) -> Result<(), String> {
    DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|error| format!("diagnostic session directory creation failed: {error}"))
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = create_file(path)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("diagnostic session file write failed: {error}"))
}

fn create_file(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("diagnostic session file creation failed: {error}"))
}

fn digest_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path)
        .map_err(|error| format!("diagnostic session file could not be opened: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("diagnostic session file read failed: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let mut encoded = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::Value;

    use super::{PublishRequest, digest_file, publish};

    fn write_event_component(root: &Path) -> (PathBuf, String) {
        let events = root.join("event-input");
        fs::create_dir(&events).unwrap();
        fs::write(
            events.join("events.ndjson"),
            b"{\"schema\":\"scorepeek-run-event-v2\",\"channel_sequence\":1,\"event\":\"session_started\"}\n",
        )
        .unwrap();
        fs::write(events.join("manifest.json"), b"{}\n").unwrap();
        let manifest_sha256 = digest_file(&events.join("manifest.json")).unwrap();
        (events, manifest_sha256)
    }

    #[test]
    fn publication_expands_predicates_and_overlays_retained_recognition() {
        let root = tempfile::tempdir().unwrap();
        let capture = root.path().join("capture-input");
        let recognition = root.path().join("recognition-input");
        let sessions = root.path().join("sessions");
        fs::create_dir(&capture).unwrap();
        fs::create_dir(&recognition).unwrap();
        fs::create_dir(&sessions).unwrap();
        fs::write(
            capture.join("facts.ndjson"),
            concat!(
                "{\"fact\":{\"tick_sequence\":0,\"monotonic_start_ms\":100,\"detail\":{\"kind\":\"screen_predicate_observation\",\"screen\":\"unknown\"}}}\n",
                "{\"fact\":{\"tick_sequence\":1,\"monotonic_start_ms\":200,\"detail\":{\"kind\":\"screen_predicate_observation\",\"screen\":\"decide_transition\"}}}\n",
                "{\"fact\":{\"tick_sequence\":2,\"monotonic_start_ms\":300,\"detail\":{\"kind\":\"screen_predicate_observation\",\"screen\":\"gameplay\"}}}\n",
                "{\"fact\":{\"tick_sequence\":3,\"monotonic_start_ms\":400,\"detail\":{\"kind\":\"screen_predicate_observation\",\"screen\":\"result\"}}}\n"
            ),
        )
        .unwrap();
        fs::write(capture.join("manifest.json"), b"{}\n").unwrap();
        fs::write(
            recognition.join("observations.ndjson"),
            b"{\"schema\":\"scorepeek-recognition-observation-v5\",\"tick_sequence\":3,\"fields\":{\"title\":\"measured\"},\"song_id\":null}\n",
        )
        .unwrap();
        fs::write(recognition.join("manifest.json"), b"{}\n").unwrap();
        let profile = root.path().join("profile.json");
        fs::write(&profile, b"{}\n").unwrap();
        let recognition_manifest_sha256 = digest_file(&recognition.join("manifest.json")).unwrap();
        let (events, event_manifest_sha256) = write_event_component(root.path());
        let published = publish(&PublishRequest {
            root: &sessions,
            session_id: "session",
            capture_generation: 1,
            profile_sha256: &"1".repeat(64),
            catalog_sha256: &"2".repeat(64),
            processed_ticks: 4,
            busy_skips: 0,
            maximum_consecutive_busy_skips: 0,
            completeness: "complete",
            capture_directory: &capture,
            capture_manifest_sha256: &digest_file(&capture.join("manifest.json")).unwrap(),
            recognition_directory: &recognition,
            recognition_manifest_sha256: &recognition_manifest_sha256,
            event_directory: &events,
            event_manifest_sha256: &event_manifest_sha256,
            profile_path: &profile,
        })
        .unwrap();
        let records = fs::read_to_string(published.join("recognition/observations.ndjson"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 4);
        assert_eq!(records[0]["screen"], "unknown");
        assert_eq!(records[0]["source_timestamp_ms"], 100);
        assert_eq!(records[1]["screen"], "decide_transition");
        assert_eq!(records[2]["screen"], "play");
        assert_eq!(records[3]["screen"], "result");
        assert_eq!(records[3]["fields"]["title"], "measured");
        assert_eq!(
            fs::read(published.join("events.ndjson")).unwrap(),
            fs::read(events.join("events.ndjson")).unwrap()
        );
    }

    #[test]
    fn partial_publication_retains_recognition_when_its_predicate_fact_was_dropped() {
        let root = tempfile::tempdir().unwrap();
        let capture = root.path().join("capture-input");
        let recognition = root.path().join("recognition-input");
        let sessions = root.path().join("sessions");
        fs::create_dir(&capture).unwrap();
        fs::create_dir(&recognition).unwrap();
        fs::create_dir(&sessions).unwrap();
        fs::write(capture.join("facts.ndjson"), b"").unwrap();
        fs::write(capture.join("manifest.json"), b"{}\n").unwrap();
        fs::write(
            recognition.join("observations.ndjson"),
            b"{\"schema\":\"scorepeek-recognition-observation-v5\",\"tick_sequence\":7,\"timing\":{\"monotonic_start_ms\":800},\"fields\":{\"screen\":\"result\"}}\n",
        )
        .unwrap();
        fs::write(recognition.join("manifest.json"), b"{}\n").unwrap();
        let profile = root.path().join("profile.json");
        fs::write(&profile, b"{}\n").unwrap();
        let recognition_manifest_sha256 = digest_file(&recognition.join("manifest.json")).unwrap();
        let (events, event_manifest_sha256) = write_event_component(root.path());
        let published = publish(&PublishRequest {
            root: &sessions,
            session_id: "session",
            capture_generation: 1,
            profile_sha256: &"1".repeat(64),
            catalog_sha256: &"2".repeat(64),
            processed_ticks: 1,
            busy_skips: 7,
            maximum_consecutive_busy_skips: 7,
            completeness: "partial",
            capture_directory: &capture,
            capture_manifest_sha256: &digest_file(&capture.join("manifest.json")).unwrap(),
            recognition_directory: &recognition,
            recognition_manifest_sha256: &recognition_manifest_sha256,
            event_directory: &events,
            event_manifest_sha256: &event_manifest_sha256,
            profile_path: &profile,
        })
        .unwrap();
        let observation: Value = serde_json::from_str(
            fs::read_to_string(published.join("recognition/observations.ndjson"))
                .unwrap()
                .trim(),
        )
        .unwrap();
        assert_eq!(observation["source_timestamp_ms"], 800);
        assert_eq!(observation["screen"], "result");
    }

    #[test]
    fn complete_publication_rejects_recognition_without_its_predicate_fact() {
        let root = tempfile::tempdir().unwrap();
        let capture = root.path().join("capture-input");
        let recognition = root.path().join("recognition-input");
        let sessions = root.path().join("sessions");
        fs::create_dir(&capture).unwrap();
        fs::create_dir(&recognition).unwrap();
        fs::create_dir(&sessions).unwrap();
        fs::write(capture.join("facts.ndjson"), b"").unwrap();
        fs::write(capture.join("manifest.json"), b"{}\n").unwrap();
        fs::write(
            recognition.join("observations.ndjson"),
            b"{\"tick_sequence\":0,\"timing\":{\"monotonic_start_ms\":100},\"fields\":{\"screen\":\"result\"}}\n",
        )
        .unwrap();
        fs::write(recognition.join("manifest.json"), b"{}\n").unwrap();
        let profile = root.path().join("profile.json");
        fs::write(&profile, b"{}\n").unwrap();
        let recognition_manifest_sha256 = digest_file(&recognition.join("manifest.json")).unwrap();
        let (events, event_manifest_sha256) = write_event_component(root.path());
        let error = publish(&PublishRequest {
            root: &sessions,
            session_id: "session",
            capture_generation: 1,
            profile_sha256: &"1".repeat(64),
            catalog_sha256: &"2".repeat(64),
            processed_ticks: 1,
            busy_skips: 0,
            maximum_consecutive_busy_skips: 0,
            completeness: "complete",
            capture_directory: &capture,
            capture_manifest_sha256: &digest_file(&capture.join("manifest.json")).unwrap(),
            recognition_directory: &recognition,
            recognition_manifest_sha256: &recognition_manifest_sha256,
            event_directory: &events,
            event_manifest_sha256: &event_manifest_sha256,
            profile_path: &profile,
        })
        .unwrap_err();
        assert_eq!(error, "recognition observation has no predicate fact");
    }
}
