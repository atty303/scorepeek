//! Read-only verification of a complete canonical recording directory.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};

use scorepeek_core::canonical_recording::{
    CanonicalRecordingManifest, CanonicalTick, ElisionReason, TickDisposition,
};
use sha2::{Digest, Sha256};

const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TICK_INDEX_BYTES: u64 = 512 * 1024 * 1024;
const MAX_TICKS: usize = 250_000;
const MAX_SEGMENT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const FRAME_BYTES: usize = 1920 * 1080 * 3;

#[derive(Debug)]
pub enum RecordingError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Invalid(&'static str),
}

impl From<std::io::Error> for RecordingError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for RecordingError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

pub struct ValidatedRecording {
    pub manifest: CanonicalRecordingManifest,
    pub ticks: Vec<CanonicalTick>,
}

fn regular_file(path: &Path, limit: u64) -> Result<u64, RecordingError> {
    let metadata = path.symlink_metadata()?;
    if !metadata.file_type().is_file() || metadata.len() > limit {
        return Err(RecordingError::Invalid(
            "canonical artifact is not a bounded regular file",
        ));
    }
    Ok(metadata.len())
}

fn sha256_file(path: &Path) -> Result<(String, u64), RecordingError> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut bytes = 0_u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        bytes = bytes.saturating_add(count as u64);
    }
    let digest = hash.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut hex, "{byte:02x}").expect("String write");
    }
    Ok((hex, bytes))
}

fn probe_segment(path: &Path) -> Result<(), RecordingError> {
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,profile,width,height,pix_fmt",
            "-of",
            "json",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()?;
    if !probe.status.success() || probe.stdout.len() > 4096 {
        return Err(RecordingError::Invalid("canonical segment probe failed"));
    }
    let streams: serde_json::Value = serde_json::from_slice(&probe.stdout)?;
    let stream = streams
        .pointer("/streams/0")
        .ok_or(RecordingError::Invalid("canonical video stream missing"))?;
    let supported_codec = stream["codec_name"] == "ffv1"
        || (stream["codec_name"] == "h264"
            && stream["profile"] == "High 4:4:4 Predictive"
            && stream["pix_fmt"] == "gbrp");
    if !supported_codec || stream["width"] != 1920 || stream["height"] != 1080 {
        return Err(RecordingError::Invalid(
            "canonical segment video contract differs",
        ));
    }
    Ok(())
}

fn decoded_frames(path: &Path, maximum: u64) -> Result<u64, RecordingError> {
    let mut child = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:v:0", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let result = (|| {
        let mut stdout = child
            .stdout
            .take()
            .ok_or(RecordingError::Invalid("decoder stdout missing"))?;
        let mut pixels = vec![0_u8; FRAME_BYTES];
        let mut count = 0_u64;
        loop {
            let first = stdout.read(&mut pixels[..1])?;
            if first == 0 {
                break;
            }
            stdout.read_exact(&mut pixels[1..])?;
            count += 1;
            if count > maximum {
                return Err(RecordingError::Invalid(
                    "decoded frame count exceeds manifest",
                ));
            }
        }
        Ok(count)
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    let count = result?;
    if !status.success() {
        return Err(RecordingError::Invalid("canonical segment decode failed"));
    }
    Ok(count)
}

/// Verifies one completed, self-contained recording before any corpus generation is published.
///
/// # Errors
/// Rejects absent, changed, malformed, incomplete, or undecodable canonical artifacts.
pub fn read_complete(root: &Path) -> Result<ValidatedRecording, RecordingError> {
    read_recording(root, true, None)
}

/// Replay decodes every retained frame itself, so it verifies the byte contract here and
/// checks decoded frame count while consuming the segment rather than decoding it twice.
pub(crate) fn read_for_replay(
    root: &Path,
    on_verified_segment: &dyn Fn(usize, usize),
) -> Result<ValidatedRecording, RecordingError> {
    read_recording(root, false, Some(on_verified_segment))
}

fn read_recording(
    root: &Path,
    verify_decoded_frames: bool,
    on_verified_segment: Option<&dyn Fn(usize, usize)>,
) -> Result<ValidatedRecording, RecordingError> {
    if !root.symlink_metadata()?.file_type().is_dir() {
        return Err(RecordingError::Invalid("recording root is not a directory"));
    }
    let manifest_path = root.join("canonical-manifest.json");
    regular_file(&manifest_path, MAX_MANIFEST_BYTES)?;
    let manifest: CanonicalRecordingManifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    manifest
        .validate_structure()
        .map_err(RecordingError::Invalid)?;

    let tick_path = root.join(&manifest.tick_index.path);
    let tick_bytes = regular_file(&tick_path, MAX_TICK_INDEX_BYTES)?;
    if tick_bytes != manifest.tick_index.bytes
        || sha256_file(&tick_path)?.0 != manifest.tick_index.sha256
    {
        return Err(RecordingError::Invalid(
            "canonical tick index integrity differs",
        ));
    }
    let mut ticks = Vec::new();
    let mut last_sequence = None;
    let mut last_source_sequence = None;
    let mut retained = Vec::new();
    for line in BufReader::new(File::open(&tick_path)?).lines() {
        if ticks.len() >= MAX_TICKS {
            return Err(RecordingError::Invalid(
                "canonical tick count exceeds bound",
            ));
        }
        let line = line?;
        let tick: CanonicalTick = serde_json::from_str(&line)?;
        tick.validate().map_err(RecordingError::Invalid)?;
        if tick.disposition == TickDisposition::Elided(ElisionReason::RecordingFailure) {
            return Err(RecordingError::Invalid(
                "complete canonical recording contains a failed tick",
            ));
        }
        if last_sequence.is_some_and(|last| tick.sequence <= last)
            || last_source_sequence.is_some_and(|last| tick.source_sequence < last)
        {
            return Err(RecordingError::Invalid("canonical tick chronology differs"));
        }
        if tick.disposition == TickDisposition::Retained {
            retained.push(tick.sequence);
        }
        last_sequence = Some(tick.sequence);
        last_source_sequence = Some(tick.source_sequence);
        ticks.push(tick);
    }
    if ticks.len() as u64 != manifest.tick_count {
        return Err(RecordingError::Invalid("canonical tick count differs"));
    }
    let mut cursor = 0_usize;
    for (index, segment) in manifest.segments.iter().enumerate() {
        let count = usize::try_from(segment.frames)
            .map_err(|_| RecordingError::Invalid("segment frame count exceeds platform"))?;
        let end = cursor
            .checked_add(count)
            .ok_or(RecordingError::Invalid("segment frame count overflow"))?;
        let slice = retained
            .get(cursor..end)
            .ok_or(RecordingError::Invalid("retained frame coverage differs"))?;
        if slice.first() != Some(&segment.first_sequence)
            || slice.last() != Some(&segment.last_sequence)
        {
            return Err(RecordingError::Invalid("segment sequence coverage differs"));
        }
        let path = root.join(&segment.path);
        if regular_file(&path, MAX_SEGMENT_BYTES)? != segment.bytes
            || sha256_file(&path)?.0 != segment.sha256
        {
            return Err(RecordingError::Invalid(
                "canonical segment integrity differs",
            ));
        }
        probe_segment(&path)?;
        if verify_decoded_frames {
            if decoded_frames(&path, segment.frames)? != segment.frames {
                return Err(RecordingError::Invalid("decoded frame count differs"));
            }
            if sha256_file(&path)?.0 != segment.sha256 {
                return Err(RecordingError::Invalid(
                    "canonical segment changed during decode",
                ));
            }
        }
        if let Some(notify) = on_verified_segment {
            notify(index + 1, manifest.segments.len());
        }
        cursor = end;
    }
    if cursor != retained.len() || sha256_file(&tick_path)?.0 != manifest.tick_index.sha256 {
        return Err(RecordingError::Invalid(
            "canonical recording changed during verification",
        ));
    }
    Ok(ValidatedRecording { manifest, ticks })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validates_synthetic_complete_recording_and_detects_changed_segment() {
        let root = tempfile::tempdir().unwrap();
        let segment = root.path().join("segment-0000.mkv");
        let status = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=1920x1080:r=1",
                "-frames:v",
                "1",
                "-c:v",
                "libx264rgb",
                "-crf",
                "0",
                "-pix_fmt",
                "bgr0",
            ])
            .arg(&segment)
            .status()
            .unwrap();
        assert!(status.success());
        let tick = json!({"sequence":1,"source_sequence":7,"source_timestamp_ms":90,
            "screen":"result","semantic_episode_id":null,"disposition":{"kind":"retained"}});
        let tick_path = root.path().join("canonical-ticks.ndjson");
        fs::write(&tick_path, format!("{tick}\n")).unwrap();
        let (tick_digest, tick_bytes) = sha256_file(&tick_path).unwrap();
        let (segment_digest, segment_bytes) = sha256_file(&segment).unwrap();
        let manifest = json!({
            "schema":"scorepeek-canonical-session-recording-v5",
            "frame_contract":"scorepeek-canonical-rgb8-1920x1080-v1",
            "session_id":"synthetic-1",
            "shape":{"width":1920,"height":1080,"pixel_format":"rgb8"},
            "tick_index":{"path":"canonical-ticks.ndjson","sha256":tick_digest,"bytes":tick_bytes,"count":1},
            "tick_count":1,
            "segments":[{"path":"segment-0000.mkv","first_sequence":1,"last_sequence":1,
                "frames":1,"bytes":segment_bytes,"sha256":segment_digest}],
            "completeness":"complete","completeness_reasons":[],
            "game_version":{"status":"not_observed"}
        });
        fs::write(
            root.path().join("canonical-manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let validated = read_complete(root.path()).unwrap();
        assert_eq!(validated.ticks.len(), 1);
        assert_eq!(validated.manifest.segments.len(), 1);
        fs::write(&segment, b"changed").unwrap();
        assert!(read_complete(root.path()).is_err());
    }
}
