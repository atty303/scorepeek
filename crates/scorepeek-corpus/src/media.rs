use std::ffi::OsString;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use std::{os::unix::ffi::OsStrExt as _, os::unix::ffi::OsStringExt as _};

use super::*;

const PROBE_SCHEMA: &str = "scorepeek-private-media-probe-v4";
const PROBE_SUMMARY_SCHEMA: &str = "scorepeek-private-media-probe-summary-v4";
const EXTRACT_REQUEST_SCHEMA: &str = "scorepeek-private-observed-frame-extraction-v2";
const EXTRACT_SCHEMA: &str = "scorepeek-private-observed-frame-extraction-manifest-v2";
const EXTRACT_SUMMARY_SCHEMA: &str = "scorepeek-private-observed-frame-extraction-summary-v2";
const NORMALIZER_SCHEMA: &str = "scorepeek-domain-normalizer-artifact-v1";
const CANONICAL_EXTRACT_SCHEMA: &str = "scorepeek-private-canonical-frame-extraction-v1";
const CANONICAL_EXTRACT_SUMMARY_SCHEMA: &str =
    "scorepeek-private-canonical-frame-extraction-summary-v1";
const CANONICAL_NORMALIZER_IMPLEMENTATION: &str = "ffmpeg-swscale-bt709-limited-to-rgb24-v1";
const CANONICAL_NORMALIZER_FILTER: &str = "scale=1920:1080:flags=bitexact:in_color_matrix=bt709:out_color_matrix=bt709:in_range=tv:out_range=pc,format=rgb24";
const CALIBRATED_CAPTURE_PROFILE_SHA256: &str =
    "d5809dc9b2acc19837260053f4df59a454c9178ae2ac6a0602982effc9da4704";
const CALIBRATED_GAMESCOPE_VKCAPTURE_PROFILE_SHA256: &str =
    "f5f0c5a86b5edba6a8fd014ad85b3873be8f745c0b531d2b5b77f203770b046a";
const CALIBRATED_FFMPEG_SHA256: &str =
    "9eac5b2b5076db5ff853a6fa0dcd6b8de7d0cac8481eadda6c47cd935825f1ee";
const FFMPEG_VERSION: &str = "8.1.2";
const MAX_MEDIA_JSON: usize = 64 * 1024 * 1024;
const MAX_STDOUT: usize = 128 * 1024 * 1024;
const MAX_STDERR: usize = 1024 * 1024;
const MAX_TOOL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_EXTRACTED_FRAMES: usize = 512;
const MAX_EXTRACTED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_DIMENSION: u32 = 8_192;
const TOOL_TIMEOUT: Duration = Duration::from_mins(10);
const EXTRACT_STAGING_PREFIX: &str = ".scorepeek-frame-staging-";
const OUTPUT_STAGING_PREFIX: &str = ".scorepeek-private-output-";
const FILE_CLAIM_PREFIX: &str = ".scorepeek-file-claim-";
const FILE_CLAIM_MARKER_BYTES: &[u8] = b"scorepeek-private-file-claim-v2\n";
const OUTPUT_LOCK_FILE: &str = ".scorepeek-private-output.lock";
const INCOMPLETE_MARKER: &str = ".scorepeek-incomplete-v2";
const INCOMPLETE_MARKER_BYTES: &[u8] = b"scorepeek-private-observed-frame-extraction-v2\n";
const STAGING_MARKER: &str = ".scorepeek-staging-owner-v2";
const STAGING_MARKER_BYTES: &[u8] = b"scorepeek-private-observed-frame-staging-v2\n";
const INPUT_FORMAT: &str = "matroska";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolchainIdentity {
    ffmpeg_version: String,
    ffmpeg_sha256: String,
    ffprobe_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MediaProbeManifest {
    schema: String,
    fixture_id: String,
    source_manifest_sha256: String,
    source: ContentRef,
    capture_profile_id: String,
    toolchain: ToolchainIdentity,
    observed: ObservedMediaContract,
    video_stream_index: u32,
    index_basis: FrameIndexBasis,
    frames: Vec<ProbedFrame>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FrameIndexBasis {
    Ffv1PacketOrder,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObservedMediaContract {
    pub input_format: String,
    pub codec_name: String,
    pub pixel_format: String,
    pub width: u32,
    pub height: u32,
    pub source_time_base: TimeBase,
    pub color_range: Option<String>,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub color_primaries: Option<String>,
}

pub(crate) struct RawMediaObservation {
    toolchain: ToolchainIdentity,
    pub observed: ObservedMediaContract,
    video_stream_index: u32,
    index_basis: FrameIndexBasis,
    frames: Vec<ProbedFrame>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbedFrame {
    decode_index: u64,
    source_pts: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MediaProbeSummary {
    pub schema: String,
    pub fixture_id: String,
    pub media_probe_sha256: String,
    pub frame_count: u64,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FrameExtractionRequest {
    schema: String,
    fixture_id: String,
    source_manifest_sha256: String,
    media_probe_sha256: String,
    frames: Vec<RequestedFrame>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RequestedFrame {
    frame_id: String,
    decode_index: u64,
    source_pts: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrameExtractionManifest {
    schema: String,
    fixture_id: String,
    source_manifest_sha256: String,
    media_probe_sha256: String,
    capture_profile_id: String,
    extractor: ExtractorIdentity,
    input_format: String,
    source_time_base: TimeBase,
    width: u32,
    height: u32,
    video_stream_index: u32,
    frames: Vec<ExtractedFrame>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtractedFrame {
    frame_id: String,
    source_pts: i64,
    decode_index: u64,
    filename: String,
    frame_sha256: String,
    file_sha256: String,
    bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FrameExtractionSummary {
    pub schema: String,
    pub fixture_id: String,
    pub frame_extraction_sha256: String,
    pub frame_count: u64,
    pub extracted_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CanonicalFrameExtractionSummary {
    pub schema: String,
    pub fixture_id: String,
    pub normalizer_artifact_sha256: String,
    pub frame_extraction_sha256: String,
    pub frame_count: u64,
    pub extracted_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DomainNormalizerArtifact {
    schema: String,
    capture_profile_id: String,
    observed: ObservedMediaContract,
    canonical_frame_contract_id: String,
    implementation: String,
    ffmpeg_sha256: String,
    filter: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CanonicalFrameExtractionManifest {
    schema: String,
    fixture_id: String,
    source_manifest_sha256: String,
    media_probe_sha256: String,
    capture_profile_id: String,
    normalizer_artifact_sha256: String,
    canonical_frame_contract_id: String,
    extractor: ExtractorIdentity,
    source_time_base: TimeBase,
    video_stream_index: u32,
    frames: Vec<ExtractedFrame>,
}

#[derive(Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    streams: Vec<FfprobeStream>,
    #[serde(default)]
    packets: Vec<FfprobePacket>,
}

#[derive(Deserialize)]
struct FfprobeStream {
    index: Option<u32>,
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    time_base: Option<String>,
    codec_name: Option<String>,
    pix_fmt: Option<String>,
    color_range: Option<String>,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
}

#[derive(Deserialize)]
struct FfprobePacket {
    stream_index: Option<u32>,
    pts: Option<i64>,
}

pub(crate) fn inspect_recording(path: &Path) -> Result<RawMediaObservation, CorpusError> {
    let mut source = File::open(path)?;
    inspect_recording_file(&mut source)
}

pub(crate) fn inspect_recording_file(
    source: &mut File,
) -> Result<RawMediaObservation, CorpusError> {
    let toolchain = identify_toolchain()?;
    let probed = probe_recording_packets(source)?;
    let stream = unique_video_stream(&probed.streams)?;
    let (observed, video_stream_index) = observed_stream_contract(stream)?;
    validate_packet_index_contract(&observed)?;
    let mut frames = Vec::new();
    for packet in probed
        .packets
        .into_iter()
        .filter(|packet| packet.stream_index == Some(video_stream_index))
    {
        if frames.len() >= MAX_REPLAY_FRAMES {
            return Err(CorpusError::CapacityExceeded);
        }
        frames.push(ProbedFrame {
            decode_index: frames.len() as u64,
            source_pts: packet
                .pts
                .ok_or_else(|| invalid_media("FFV1 video packet has no integer PTS"))?,
        });
    }
    if frames.is_empty() {
        return Err(invalid_media("FFV1 video stream contains no packets"));
    }
    Ok(RawMediaObservation {
        toolchain,
        observed,
        video_stream_index,
        index_basis: FrameIndexBasis::Ffv1PacketOrder,
        frames,
    })
}

pub(crate) fn inspect_recording_contract(
    path: &Path,
) -> Result<ObservedMediaContract, CorpusError> {
    let mut source = File::open(path)?;
    inspect_recording_contract_file(&mut source)
}

fn inspect_recording_contract_file(
    source: &mut File,
) -> Result<ObservedMediaContract, CorpusError> {
    let probed = probe_recording(
        source,
        "stream=index,codec_type,codec_name,pix_fmt,width,height,time_base,color_range,color_space,color_transfer,color_primaries",
    )?;
    let stream = unique_video_stream(&probed.streams)?;
    let (observed, _) = observed_stream_contract(stream)?;
    validate_packet_index_contract(&observed)?;
    Ok(observed)
}

fn probe_recording_packets(source: &mut File) -> Result<FfprobeOutput, CorpusError> {
    validate_input_file(source)?;
    source.seek(SeekFrom::Start(0))?;
    let output = run_command(
        &find_executable("ffprobe")?,
        &os_args(
            &[
                "-v",
                "error",
                "-protocol_whitelist",
                "pipe",
                "-f",
                INPUT_FORMAT,
                "-i",
                "pipe:0",
                "-select_streams",
                "v",
                "-show_packets",
                "-show_entries",
                "stream=index,codec_type,codec_name,pix_fmt,width,height,time_base,color_range,color_space,color_transfer,color_primaries:packet=stream_index,pts",
                "-of",
                "json",
            ],
            None,
        ),
        Some(source.try_clone()?),
        MAX_STDOUT,
        TOOL_TIMEOUT,
    )?;
    serde_json::from_slice(&output)
        .map_err(|_| invalid_media("ffprobe returned invalid bounded packet JSON"))
}

fn probe_recording(source: &mut File, show_entries: &str) -> Result<FfprobeOutput, CorpusError> {
    validate_input_file(source)?;
    source.seek(SeekFrom::Start(0))?;
    let output = run_command(
        &find_executable("ffprobe")?,
        &os_args(
            &[
                "-v",
                "error",
                "-protocol_whitelist",
                "pipe",
                "-f",
                INPUT_FORMAT,
                "-i",
                "pipe:0",
                "-show_entries",
                show_entries,
                "-of",
                "json",
            ],
            None,
        ),
        Some(source.try_clone()?),
        MAX_STDOUT,
        TOOL_TIMEOUT,
    )?;
    serde_json::from_slice(&output)
        .map_err(|_| invalid_media("ffprobe returned invalid bounded JSON"))
}

fn observed_stream_contract(
    stream: &FfprobeStream,
) -> Result<(ObservedMediaContract, u32), CorpusError> {
    let width = stream
        .width
        .ok_or_else(|| invalid_media("video stream has no width"))?;
    let height = stream
        .height
        .ok_or_else(|| invalid_media("video stream has no height"))?;
    validate_dimensions(width, height)?;
    let video_stream_index = stream
        .index
        .ok_or_else(|| invalid_media("video stream has no index"))?;
    let source_time_base = parse_time_base(
        stream
            .time_base
            .as_deref()
            .ok_or_else(|| invalid_media("video stream has no time base"))?,
    )?;
    let codec_name = stream
        .codec_name
        .clone()
        .ok_or_else(|| invalid_media("video stream has no codec name"))?;
    let pixel_format = stream
        .pix_fmt
        .clone()
        .ok_or_else(|| invalid_media("video stream has no pixel format"))?;
    Ok((
        ObservedMediaContract {
            input_format: INPUT_FORMAT.to_owned(),
            codec_name,
            pixel_format,
            width,
            height,
            source_time_base,
            color_range: stream.color_range.clone(),
            color_space: stream.color_space.clone(),
            color_transfer: stream.color_transfer.clone(),
            color_primaries: stream.color_primaries.clone(),
        },
        video_stream_index,
    ))
}

fn validate_packet_index_contract(observed: &ObservedMediaContract) -> Result<(), CorpusError> {
    if observed.codec_name != "ffv1" {
        return Err(invalid_media(
            "fast media probing requires an FFV1 video stream",
        ));
    }
    Ok(())
}

impl CorpusStore {
    /// Probes one immutable stored source into a canonical PTS/decode-order manifest.
    ///
    /// # Errors
    /// Returns an error when the source, pinned tools, media, or private output is invalid.
    pub fn probe_media(
        &self,
        fixture_id: &str,
        output_path: impl AsRef<Path>,
    ) -> Result<MediaProbeSummary, CorpusError> {
        self.validate_root()?;
        validate_opaque_id(fixture_id, "fixture_id", ErrorContext::Request)?;
        validate_directory(&self.root, ErrorContext::Request)?;
        let (source_manifest, source_manifest_sha256) = load_bound_source(self, fixture_id)?;
        let mut source = self.open_verified_source(&source_manifest.source)?;
        let observation = inspect_recording_file(&mut source)?;
        write_media_probe(
            fixture_id,
            source_manifest,
            source_manifest_sha256,
            observation,
            output_path.as_ref(),
        )
    }

    pub(crate) fn probe_media_from_observation(
        &self,
        fixture_id: &str,
        observation: RawMediaObservation,
        output_path: impl AsRef<Path>,
    ) -> Result<MediaProbeSummary, CorpusError> {
        self.validate_root()?;
        validate_opaque_id(fixture_id, "fixture_id", ErrorContext::Request)?;
        validate_directory(&self.root, ErrorContext::Request)?;
        let (source_manifest, source_manifest_sha256) = load_bound_source(self, fixture_id)?;
        write_media_probe(
            fixture_id,
            source_manifest,
            source_manifest_sha256,
            observation,
            output_path.as_ref(),
        )
    }

    /// Extracts explicitly selected frames as private RGB8 P6 PPM files.
    ///
    /// # Errors
    /// Returns an error when any binding, selection, tool identity, or bounded output is invalid.
    pub fn extract_frames(
        &self,
        probe_path: impl AsRef<Path>,
        request_path: impl AsRef<Path>,
        output_directory: impl AsRef<Path>,
    ) -> Result<FrameExtractionSummary, CorpusError> {
        self.validate_root()?;
        let (probe, probe_digest) = read_probe(probe_path.as_ref())?;
        let request = read_extraction_request(request_path.as_ref())?;
        request.validate_against(&probe, &probe_digest)?;
        let (current_source, current_source_digest) = load_bound_source(self, &probe.fixture_id)?;
        validate_probe_source_binding(&probe, &current_source, &current_source_digest)?;
        let output = output_directory.as_ref();
        let (parent, output_lock) = lock_output_parent(output)?;
        if identify_toolchain()? != probe.toolchain {
            return Err(invalid_media(
                "pinned media tool identity changed after probing",
            ));
        }

        let (staging, extracted, extracted_bytes) = run_extraction(
            self,
            &probe,
            &request,
            &parent,
            None,
            probe.observed.width,
            probe.observed.height,
        )?;
        let parameters_sha256 = digest_bytes(&canonical_json(&request)?);
        let manifest = FrameExtractionManifest {
            schema: EXTRACT_SCHEMA.to_owned(),
            fixture_id: request.fixture_id,
            source_manifest_sha256: request.source_manifest_sha256,
            media_probe_sha256: probe_digest.clone(),
            capture_profile_id: probe.capture_profile_id,
            extractor: ExtractorIdentity {
                tool_id: "ffmpeg".to_owned(),
                tool_version: FFMPEG_VERSION.to_owned(),
                extractor_manifest_sha256: probe_digest,
                parameters_sha256,
            },
            input_format: probe.observed.input_format,
            source_time_base: probe.observed.source_time_base,
            width: probe.observed.width,
            height: probe.observed.height,
            video_stream_index: probe.video_stream_index,
            frames: extracted,
        };
        manifest.validate()?;
        let manifest_bytes = canonical_json(&manifest)?;
        let manifest_digest = digest_bytes(&manifest_bytes);
        write_atomic_file(
            staging.path(),
            &staging.path().join("manifest.json"),
            &manifest_bytes,
            OUTPUT_STAGING_PREFIX,
        )?;
        File::open(staging.path())?.sync_all()?;
        publish_extraction(staging.path(), output, &parent)?;
        drop(output_lock);
        Ok(FrameExtractionSummary {
            schema: EXTRACT_SUMMARY_SCHEMA.to_owned(),
            fixture_id: manifest.fixture_id,
            frame_extraction_sha256: manifest_digest,
            frame_count: manifest.frames.len() as u64,
            extracted_bytes,
        })
    }

    /// Normalizes explicitly selected recording frames into the fixed canonical RGB8 contract.
    ///
    /// # Errors
    /// Returns an error when the probe has no exact versioned normalizer, any source binding has
    /// changed, or canonical frame publication cannot complete privately and durably.
    pub fn extract_canonical_frames(
        &self,
        probe_path: impl AsRef<Path>,
        request_path: impl AsRef<Path>,
        output_directory: impl AsRef<Path>,
    ) -> Result<CanonicalFrameExtractionSummary, CorpusError> {
        self.validate_root()?;
        let (probe, probe_digest) = read_probe(probe_path.as_ref())?;
        let request = read_extraction_request(request_path.as_ref())?;
        request.validate_against(&probe, &probe_digest)?;
        let (current_source, current_source_digest) = load_bound_source(self, &probe.fixture_id)?;
        validate_probe_source_binding(&probe, &current_source, &current_source_digest)?;
        if identify_toolchain()? != probe.toolchain {
            return Err(invalid_media(
                "pinned media tool identity changed after probing",
            ));
        }
        let normalizer = DomainNormalizerArtifact::for_probe(&probe)?;
        let normalizer_bytes = canonical_json(&normalizer)?;
        let normalizer_artifact_sha256 = digest_bytes(&normalizer_bytes);
        let output = output_directory.as_ref();
        let (parent, output_lock) = lock_output_parent(output)?;
        let (staging, extracted, extracted_bytes) = run_extraction(
            self,
            &probe,
            &request,
            &parent,
            Some(&normalizer.filter),
            1_920,
            1_080,
        )?;
        let parameters_sha256 = digest_bytes(&canonical_json(&request)?);
        let manifest = CanonicalFrameExtractionManifest {
            schema: CANONICAL_EXTRACT_SCHEMA.to_owned(),
            fixture_id: request.fixture_id,
            source_manifest_sha256: request.source_manifest_sha256,
            media_probe_sha256: probe_digest.clone(),
            capture_profile_id: probe.capture_profile_id,
            normalizer_artifact_sha256: normalizer_artifact_sha256.clone(),
            canonical_frame_contract_id: CANONICAL_FRAME_CONTRACT_ID.to_owned(),
            extractor: ExtractorIdentity {
                tool_id: "ffmpeg".to_owned(),
                tool_version: FFMPEG_VERSION.to_owned(),
                extractor_manifest_sha256: probe_digest,
                parameters_sha256,
            },
            source_time_base: probe.observed.source_time_base,
            video_stream_index: probe.video_stream_index,
            frames: extracted,
        };
        manifest.validate()?;
        let manifest_bytes = canonical_json(&manifest)?;
        let manifest_digest = digest_bytes(&manifest_bytes);
        write_atomic_file(
            staging.path(),
            &staging.path().join("normalizer.json"),
            &normalizer_bytes,
            OUTPUT_STAGING_PREFIX,
        )?;
        write_atomic_file(
            staging.path(),
            &staging.path().join("manifest.json"),
            &manifest_bytes,
            OUTPUT_STAGING_PREFIX,
        )?;
        File::open(staging.path())?.sync_all()?;
        publish_extraction(staging.path(), output, &parent)?;
        drop(output_lock);
        Ok(CanonicalFrameExtractionSummary {
            schema: CANONICAL_EXTRACT_SUMMARY_SCHEMA.to_owned(),
            fixture_id: manifest.fixture_id,
            normalizer_artifact_sha256,
            frame_extraction_sha256: manifest_digest,
            frame_count: manifest.frames.len() as u64,
            extracted_bytes,
        })
    }
}

fn write_media_probe(
    fixture_id: &str,
    source_manifest: SourceManifest,
    source_manifest_sha256: String,
    observation: RawMediaObservation,
    output_path: &Path,
) -> Result<MediaProbeSummary, CorpusError> {
    let width = observation.observed.width;
    let height = observation.observed.height;
    let manifest = MediaProbeManifest {
        schema: PROBE_SCHEMA.to_owned(),
        fixture_id: fixture_id.to_owned(),
        source_manifest_sha256,
        source: source_manifest.source,
        capture_profile_id: source_manifest.capture_profile_id,
        toolchain: observation.toolchain,
        observed: observation.observed,
        video_stream_index: observation.video_stream_index,
        index_basis: observation.index_basis,
        frames: observation.frames,
    };
    manifest.validate()?;
    let bytes = canonical_json(&manifest)?;
    let digest = digest_bytes(&bytes);
    write_private_output(output_path, &bytes)?;
    Ok(MediaProbeSummary {
        schema: PROBE_SUMMARY_SCHEMA.to_owned(),
        fixture_id: manifest.fixture_id,
        media_probe_sha256: digest,
        frame_count: manifest.frames.len() as u64,
        width,
        height,
    })
}

impl MediaProbeManifest {
    fn validate(&self) -> Result<(), CorpusError> {
        if self.schema != PROBE_SCHEMA {
            return Err(invalid_media("unsupported media probe schema"));
        }
        validate_opaque_id(&self.fixture_id, "fixture_id", ErrorContext::Replay)?;
        validate_sha256(
            &self.source_manifest_sha256,
            "source_manifest_sha256",
            ErrorContext::Replay,
        )?;
        self.source.validate(ErrorContext::Replay)?;
        validate_token(
            &self.capture_profile_id,
            "capture_profile_id",
            ErrorContext::Replay,
        )?;
        self.toolchain.validate()?;
        self.observed.validate()?;
        if self.video_stream_index > 255 {
            return Err(invalid_media("video stream index is outside bounds"));
        }
        if self.index_basis != FrameIndexBasis::Ffv1PacketOrder
            || self.observed.codec_name != "ffv1"
        {
            return Err(invalid_media(
                "media probe index basis is incompatible with its codec",
            ));
        }
        if self.frames.is_empty() || self.frames.len() > MAX_REPLAY_FRAMES {
            return Err(invalid_media("probe frame count is outside bounds"));
        }
        if self
            .frames
            .iter()
            .enumerate()
            .any(|(index, frame)| frame.decode_index != index as u64)
        {
            return Err(invalid_media("probe decode indexes are not contiguous"));
        }
        Ok(())
    }
}

impl ObservedMediaContract {
    pub(crate) fn validate(&self) -> Result<(), CorpusError> {
        if self.input_format != INPUT_FORMAT {
            return Err(invalid_media("unsupported stored media container"));
        }
        validate_token(&self.codec_name, "codec_name", ErrorContext::Replay)?;
        validate_token(&self.pixel_format, "pixel_format", ErrorContext::Replay)?;
        validate_dimensions(self.width, self.height)?;
        self.source_time_base.validate()?;
        for (name, value) in [
            ("color_range", &self.color_range),
            ("color_space", &self.color_space),
            ("color_transfer", &self.color_transfer),
            ("color_primaries", &self.color_primaries),
        ] {
            if let Some(value) = value {
                validate_token(value, name, ErrorContext::Replay)?;
            }
        }
        Ok(())
    }
}

pub(crate) fn validate_recording_probe_bytes(
    bytes: &[u8],
    recording_sha256: &str,
    source_manifest_sha256: &str,
    source: &ContentRef,
    capture_profile_sha256: &str,
    observed: &ObservedMediaContract,
) -> Result<(), CorpusError> {
    let probe: MediaProbeManifest = serde_json::from_slice(bytes)?;
    probe.validate()?;
    if canonical_json(&probe)? != bytes
        || probe.fixture_id != recording_sha256
        || probe.source_manifest_sha256 != source_manifest_sha256
        || &probe.source != source
        || probe.capture_profile_id != capture_profile_sha256
        || &probe.observed != observed
    {
        return Err(invalid_media(
            "recording probe is not canonical or dataset-bound",
        ));
    }
    Ok(())
}

impl ToolchainIdentity {
    fn validate(&self) -> Result<(), CorpusError> {
        if self.ffmpeg_version != FFMPEG_VERSION {
            return Err(invalid_media("unsupported FFmpeg version"));
        }
        validate_sha256(&self.ffmpeg_sha256, "ffmpeg_sha256", ErrorContext::Replay)?;
        validate_sha256(&self.ffprobe_sha256, "ffprobe_sha256", ErrorContext::Replay)
    }
}

impl FrameExtractionRequest {
    fn validate_against(
        &self,
        probe: &MediaProbeManifest,
        probe_digest: &str,
    ) -> Result<(), CorpusError> {
        if self.schema != EXTRACT_REQUEST_SCHEMA
            || self.fixture_id != probe.fixture_id
            || self.source_manifest_sha256 != probe.source_manifest_sha256
            || self.media_probe_sha256 != probe_digest
        {
            return Err(invalid_media(
                "frame extraction request is not bound to its probe",
            ));
        }
        if self.frames.is_empty() || self.frames.len() > MAX_EXTRACTED_FRAMES {
            return Err(invalid_media("selected frame count is outside bounds"));
        }
        let mut previous = None;
        let mut ids = BTreeSet::new();
        for frame in &self.frames {
            validate_opaque_id(&frame.frame_id, "frame_id", ErrorContext::Replay)?;
            if !ids.insert(&frame.frame_id) {
                return Err(invalid_media("selected frame IDs must be unique"));
            }
            if previous.is_some_and(|value| value >= frame.decode_index) {
                return Err(invalid_media(
                    "selected decode indexes must be strictly increasing",
                ));
            }
            let decode_index = usize::try_from(frame.decode_index)
                .map_err(|_| invalid_media("selected decode index is outside the probe"))?;
            let probed = probe
                .frames
                .get(decode_index)
                .ok_or_else(|| invalid_media("selected decode index is outside the probe"))?;
            if probed.source_pts != frame.source_pts {
                return Err(invalid_media("selected frame PTS does not match the probe"));
            }
            previous = Some(frame.decode_index);
        }
        Ok(())
    }
}

impl FrameExtractionManifest {
    fn validate(&self) -> Result<(), CorpusError> {
        if self.schema != EXTRACT_SCHEMA {
            return Err(invalid_media("unsupported extraction manifest schema"));
        }
        validate_opaque_id(&self.fixture_id, "fixture_id", ErrorContext::Replay)?;
        validate_sha256(
            &self.source_manifest_sha256,
            "source_manifest_sha256",
            ErrorContext::Replay,
        )?;
        validate_sha256(
            &self.media_probe_sha256,
            "media_probe_sha256",
            ErrorContext::Replay,
        )?;
        validate_token(
            &self.capture_profile_id,
            "capture_profile_id",
            ErrorContext::Replay,
        )?;
        self.extractor.validate()?;
        if self.input_format != INPUT_FORMAT {
            return Err(invalid_media("unsupported extraction input container"));
        }
        self.source_time_base.validate()?;
        validate_dimensions(self.width, self.height)?;
        if self.video_stream_index > 255 {
            return Err(invalid_media("video stream index is outside bounds"));
        }
        if self.frames.is_empty() || self.frames.len() > MAX_EXTRACTED_FRAMES {
            return Err(invalid_media("extracted frame count is outside bounds"));
        }
        let mut previous = None;
        let mut ids = BTreeSet::new();
        for (index, frame) in self.frames.iter().enumerate() {
            validate_opaque_id(&frame.frame_id, "frame_id", ErrorContext::Replay)?;
            validate_sha256(&frame.frame_sha256, "frame_sha256", ErrorContext::Replay)?;
            validate_sha256(&frame.file_sha256, "file_sha256", ErrorContext::Replay)?;
            if frame.filename != format!("frame-{index:06}.ppm") || !ids.insert(&frame.frame_id) {
                return Err(invalid_media(
                    "extracted frame identity or filename is invalid",
                ));
            }
            if previous.is_some_and(|value| value >= frame.decode_index) {
                return Err(invalid_media(
                    "extracted decode indexes are not strictly increasing",
                ));
            }
            previous = Some(frame.decode_index);
        }
        Ok(())
    }
}

impl DomainNormalizerArtifact {
    fn for_probe(probe: &MediaProbeManifest) -> Result<Self, CorpusError> {
        let artifact = Self {
            schema: NORMALIZER_SCHEMA.to_owned(),
            capture_profile_id: probe.capture_profile_id.clone(),
            observed: probe.observed.clone(),
            canonical_frame_contract_id: CANONICAL_FRAME_CONTRACT_ID.to_owned(),
            implementation: CANONICAL_NORMALIZER_IMPLEMENTATION.to_owned(),
            ffmpeg_sha256: probe.toolchain.ffmpeg_sha256.clone(),
            filter: CANONICAL_NORMALIZER_FILTER.to_owned(),
        };
        artifact.validate()?;
        Ok(artifact)
    }

    fn validate(&self) -> Result<(), CorpusError> {
        if self.schema != NORMALIZER_SCHEMA
            || self.canonical_frame_contract_id != CANONICAL_FRAME_CONTRACT_ID
            || self.implementation != CANONICAL_NORMALIZER_IMPLEMENTATION
            || self.filter != CANONICAL_NORMALIZER_FILTER
        {
            return Err(invalid_media("unsupported canonical normalizer artifact"));
        }
        validate_sha256(
            &self.capture_profile_id,
            "capture_profile_id",
            ErrorContext::Replay,
        )?;
        validate_sha256(&self.ffmpeg_sha256, "ffmpeg_sha256", ErrorContext::Replay)?;
        self.observed.validate()?;
        if !matches!(
            self.capture_profile_id.as_str(),
            CALIBRATED_CAPTURE_PROFILE_SHA256 | CALIBRATED_GAMESCOPE_VKCAPTURE_PROFILE_SHA256
        ) || self.ffmpeg_sha256 != CALIBRATED_FFMPEG_SHA256
            || self.observed.input_format != INPUT_FORMAT
            || self.observed.codec_name != "ffv1"
            || self.observed.pixel_format != "yuv420p"
            || self.observed.width != 1_920
            || self.observed.height != 1_080
            || self.observed.source_time_base.numerator != 1
            || self.observed.source_time_base.denominator != 1_000
            || self.observed.color_range.as_deref() != Some("tv")
            || self.observed.color_space.as_deref() != Some("bt709")
            || self.observed.color_transfer.as_deref() != Some("bt709")
            || self.observed.color_primaries.as_deref() != Some("bt709")
        {
            return Err(invalid_media(
                "observed profile has no calibrated canonical normalizer",
            ));
        }
        Ok(())
    }
}

impl CanonicalFrameExtractionManifest {
    fn validate(&self) -> Result<(), CorpusError> {
        if self.schema != CANONICAL_EXTRACT_SCHEMA
            || self.canonical_frame_contract_id != CANONICAL_FRAME_CONTRACT_ID
        {
            return Err(invalid_media(
                "unsupported canonical frame extraction manifest",
            ));
        }
        validate_opaque_id(&self.fixture_id, "fixture_id", ErrorContext::Replay)?;
        for (name, value) in [
            ("source_manifest_sha256", &self.source_manifest_sha256),
            ("media_probe_sha256", &self.media_probe_sha256),
            (
                "normalizer_artifact_sha256",
                &self.normalizer_artifact_sha256,
            ),
        ] {
            validate_sha256(value, name, ErrorContext::Replay)?;
        }
        validate_token(
            &self.capture_profile_id,
            "capture_profile_id",
            ErrorContext::Replay,
        )?;
        self.extractor.validate()?;
        self.source_time_base.validate()?;
        if self.video_stream_index > 255
            || self.frames.is_empty()
            || self.frames.len() > MAX_EXTRACTED_FRAMES
        {
            return Err(invalid_media(
                "canonical extraction stream or frame count is invalid",
            ));
        }
        let mut previous = None;
        let mut ids = BTreeSet::new();
        for (index, frame) in self.frames.iter().enumerate() {
            validate_opaque_id(&frame.frame_id, "frame_id", ErrorContext::Replay)?;
            validate_sha256(&frame.frame_sha256, "frame_sha256", ErrorContext::Replay)?;
            validate_sha256(&frame.file_sha256, "file_sha256", ErrorContext::Replay)?;
            if frame.filename != format!("frame-{index:06}.ppm")
                || !ids.insert(&frame.frame_id)
                || previous.is_some_and(|value| value >= frame.decode_index)
            {
                return Err(invalid_media(
                    "canonical extracted frame identity or order is invalid",
                ));
            }
            previous = Some(frame.decode_index);
        }
        Ok(())
    }
}

fn run_extraction(
    store: &CorpusStore,
    probe: &MediaProbeManifest,
    request: &FrameExtractionRequest,
    parent: &Path,
    normalization_filter: Option<&str>,
    output_width: u32,
    output_height: u32,
) -> Result<(tempfile::TempDir, Vec<ExtractedFrame>, u64), CorpusError> {
    let pixel_bytes = u64::from(output_width)
        .checked_mul(u64::from(output_height))
        .and_then(|value| value.checked_mul(3))
        .ok_or(CorpusError::CapacityExceeded)?;
    let extracted_bytes = pixel_bytes
        .checked_mul(request.frames.len() as u64)
        .ok_or(CorpusError::CapacityExceeded)?;
    if extracted_bytes > MAX_EXTRACTED_BYTES {
        return Err(CorpusError::CapacityExceeded);
    }
    let staging = Builder::new()
        .prefix(EXTRACT_STAGING_PREFIX)
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(parent)?;
    write_owned_marker(staging.path(), STAGING_MARKER, STAGING_MARKER_BYTES)?;
    let select = frame_select_expression(&request.frames);
    let filter = normalization_filter.map_or_else(
        || format!("select={select},showinfo"),
        |normalizer| format!("select={select},{normalizer},showinfo"),
    );
    let mut source = store.open_verified_source(&probe.source)?;
    validate_input_file(&mut source)?;
    let mut args = os_args(
        &[
            "-v",
            "info",
            "-nostdin",
            "-protocol_whitelist",
            "pipe",
            "-f",
            INPUT_FORMAT,
            "-i",
            "pipe:0",
        ],
        None,
    );
    args.extend(os_args(
        &[
            "-map",
            &format!("0:{}", probe.video_stream_index),
            "-vf",
            &filter,
            "-fps_mode",
            "passthrough",
            "-pix_fmt",
            "rgb24",
            "-frames:v",
            &request.frames.len().to_string(),
            "-start_number",
            "0",
        ],
        Some(staging.path().join("frame-%06d.ppm").into_os_string()),
    ));
    let execution = run_command_capture(
        &find_executable("ffmpeg")?,
        &args,
        Some(source.try_clone()?),
        1024,
        TOOL_TIMEOUT,
    )?;
    validate_selected_frame_pts(&execution.stderr, &request.frames)?;

    let mut extracted = Vec::with_capacity(request.frames.len());
    for (index, selected) in request.frames.iter().enumerate() {
        let filename = format!("frame-{index:06}.ppm");
        let path = staging.path().join(&filename);
        if !path.symlink_metadata()?.is_file() {
            return Err(invalid_media(
                "extractor output is not a regular frame file",
            ));
        }
        let (pixels, bytes) = validate_ppm(&path, output_width, output_height, pixel_bytes)?;
        extracted.push(ExtractedFrame {
            frame_id: selected.frame_id.clone(),
            source_pts: selected.source_pts,
            decode_index: selected.decode_index,
            filename,
            frame_sha256: digest_bytes(&pixels),
            file_sha256: digest_regular_file(&path, pixel_bytes + 128)?,
            bytes,
        });
        File::open(path)?.sync_all()?;
    }
    if fs::read_dir(staging.path())?.count() != request.frames.len() + 1 {
        return Err(invalid_media("extractor produced an unexpected file set"));
    }
    Ok((staging, extracted, extracted_bytes))
}

fn frame_select_expression(frames: &[RequestedFrame]) -> String {
    let mut runs = Vec::new();
    let mut start = frames[0].decode_index;
    let mut end = start;
    for frame in &frames[1..] {
        if frame.decode_index == end + 1 {
            end = frame.decode_index;
            continue;
        }
        runs.push((start, end));
        start = frame.decode_index;
        end = start;
    }
    runs.push((start, end));
    if let Some(expression) = regular_run_select_expression(&runs) {
        return expression;
    }
    runs.into_iter()
        .map(|(start, end)| select_run(start, end))
        .collect::<Vec<_>>()
        .join("+")
}

fn regular_run_select_expression(runs: &[(u64, u64)]) -> Option<String> {
    let [(first_start, first_end), (second_start, second_end), ..] = runs else {
        return None;
    };
    let width = first_end.checked_sub(*first_start)?;
    let stride = second_start.checked_sub(*first_start)?;
    if stride == 0
        || second_end.checked_sub(*second_start)? != width
        || !runs.windows(2).all(|pair| {
            pair[1].0.checked_sub(pair[0].0) == Some(stride)
                && pair[1].1.checked_sub(pair[1].0) == Some(width)
        })
    {
        return None;
    }
    let last_end = runs.last()?.1;
    Some(format!(
        "between(n\\,{first_start}\\,{last_end})*lte(mod(n-{first_start}\\,{stride})\\,{width})"
    ))
}

fn select_run(start: u64, end: u64) -> String {
    if start == end {
        format!("eq(n\\,{start})")
    } else {
        format!("between(n\\,{start}\\,{end})")
    }
}

fn validate_selected_frame_pts(
    stderr: &[u8],
    requested: &[RequestedFrame],
) -> Result<(), CorpusError> {
    let log = std::str::from_utf8(stderr)
        .map_err(|_| invalid_media("FFmpeg extraction log is not UTF-8"))?;
    let mut observed = Vec::new();
    for line in log.lines().filter(|line| line.contains("Parsed_showinfo_")) {
        let tokens = line.split_ascii_whitespace().collect::<Vec<_>>();
        let output_index = tokens
            .windows(2)
            .find(|pair| pair[0] == "n:")
            .and_then(|pair| pair[1].parse::<u64>().ok());
        let source_pts = tokens
            .windows(2)
            .find(|pair| pair[0] == "pts:")
            .and_then(|pair| pair[1].parse::<i64>().ok());
        if let (Some(output_index), Some(source_pts)) = (output_index, source_pts) {
            observed.push((output_index, source_pts));
        }
    }
    if observed.len() != requested.len()
        || observed.iter().zip(requested).enumerate().any(
            |(index, ((output_index, source_pts), requested))| {
                *output_index != index as u64 || *source_pts != requested.source_pts
            },
        )
    {
        return Err(invalid_media(
            "decoded selection PTS does not match the packet-order probe",
        ));
    }
    Ok(())
}

fn load_bound_source(
    store: &CorpusStore,
    fixture_id: &str,
) -> Result<(SourceManifest, String), CorpusError> {
    let path = store
        .root
        .join("manifests")
        .join(format!("{fixture_id}.json"));
    validate_regular_file(&path, ErrorContext::Request)?;
    let bytes = read_bounded_regular(&path, MAX_REQUEST_BYTES, ErrorContext::Request)?;
    let manifest: SourceManifest = serde_json::from_slice(&bytes)?;
    manifest.validate()?;
    if manifest.fixture_id != fixture_id || canonical_json(&manifest)? != bytes {
        return Err(invalid_media(
            "stored source manifest is not canonical or fixture-bound",
        ));
    }
    resolve_stored_source_path_unverified(
        &store.root.join("content").join(&manifest.source.sha256),
        &manifest.source,
    )?;
    Ok((manifest, digest_bytes(&bytes)))
}

fn validate_probe_source_binding(
    probe: &MediaProbeManifest,
    source: &SourceManifest,
    source_digest: &str,
) -> Result<(), CorpusError> {
    if source_digest != probe.source_manifest_sha256
        || source.fixture_id != probe.fixture_id
        || source.source != probe.source
        || source.capture_profile_id != probe.capture_profile_id
    {
        return Err(invalid_media(
            "probe no longer matches its stored source manifest",
        ));
    }
    Ok(())
}

fn read_probe(path: &Path) -> Result<(MediaProbeManifest, String), CorpusError> {
    let bytes = read_bounded_regular(path, MAX_MEDIA_JSON, ErrorContext::Replay)?;
    let probe: MediaProbeManifest = serde_json::from_slice(&bytes)?;
    probe.validate()?;
    if canonical_json(&probe)? != bytes {
        return Err(invalid_media("media probe manifest is not canonical"));
    }
    Ok((probe, digest_bytes(&bytes)))
}

fn read_extraction_request(path: &Path) -> Result<FrameExtractionRequest, CorpusError> {
    let bytes = read_bounded_regular(path, MAX_MEDIA_JSON, ErrorContext::Replay)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn unique_video_stream(streams: &[FfprobeStream]) -> Result<&FfprobeStream, CorpusError> {
    let video = streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("video"))
        .collect::<Vec<_>>();
    if video.len() != 1 {
        return Err(invalid_media(
            "probe must resolve exactly one selected video stream",
        ));
    }
    Ok(video[0])
}

fn parse_time_base(value: &str) -> Result<TimeBase, CorpusError> {
    let (numerator, denominator) = value
        .split_once('/')
        .ok_or_else(|| invalid_media("video time base is not a rational"))?;
    let time_base = TimeBase {
        numerator: numerator
            .parse()
            .map_err(|_| invalid_media("video time-base numerator is invalid"))?,
        denominator: denominator
            .parse()
            .map_err(|_| invalid_media("video time-base denominator is invalid"))?,
    };
    time_base.validate()?;
    Ok(time_base)
}

#[cfg(test)]
fn validate_input_format(path: &Path) -> Result<(), CorpusError> {
    let mut source = File::open(path)?;
    validate_input_file(&mut source)
}

fn validate_input_file(source: &mut File) -> Result<(), CorpusError> {
    source.seek(SeekFrom::Start(0))?;
    let mut magic = [0_u8; 4];
    source
        .read_exact(&mut magic)
        .map_err(|_| invalid_media("stored media has no complete container signature"))?;
    if magic != [0x1a, 0x45, 0xdf, 0xa3] {
        return Err(invalid_media(
            "stored media is not an approved self-contained Matroska container",
        ));
    }
    source.seek(SeekFrom::Start(0))?;
    Ok(())
}

fn write_owned_marker(directory: &Path, name: &str, bytes: &[u8]) -> Result<(), CorpusError> {
    let path = directory.join(name);
    let mut marker = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    marker.write_all(bytes)?;
    marker.sync_all()?;
    File::open(directory)?.sync_all()?;
    Ok(())
}

fn validate_dimensions(width: u32, height: u32) -> Result<(), CorpusError> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(invalid_media("video dimensions are outside bounds"));
    }
    Ok(())
}

fn identify_toolchain() -> Result<ToolchainIdentity, CorpusError> {
    let ffmpeg = find_executable("ffmpeg")?;
    let ffprobe = find_executable("ffprobe")?;
    verify_tool_version(&ffmpeg, "ffmpeg")?;
    verify_tool_version(&ffprobe, "ffprobe")?;
    Ok(ToolchainIdentity {
        ffmpeg_version: FFMPEG_VERSION.to_owned(),
        ffmpeg_sha256: digest_regular_file(&ffmpeg, MAX_TOOL_BYTES)?,
        ffprobe_sha256: digest_regular_file(&ffprobe, MAX_TOOL_BYTES)?,
    })
}

fn verify_tool_version(path: &Path, tool: &str) -> Result<(), CorpusError> {
    let output = run_command(
        path,
        &[OsString::from("-version")],
        None,
        128 * 1024,
        Duration::from_secs(10),
    )?;
    let stdout = std::str::from_utf8(&output)
        .map_err(|_| invalid_media("media tool version output is not UTF-8"))?;
    let version = stdout
        .lines()
        .next()
        .and_then(|line| line.strip_prefix(&format!("{tool} version ")))
        .and_then(|remainder| remainder.split_ascii_whitespace().next())
        .map(|token| token.strip_prefix('n').unwrap_or(token))
        .and_then(|token| token.split('-').next());
    if version != Some(FFMPEG_VERSION) {
        return Err(invalid_media(
            "media tool version does not match the pinned release",
        ));
    }
    Ok(())
}

pub(crate) fn find_executable(name: &str) -> Result<PathBuf, CorpusError> {
    let path = std::env::var_os("PATH").ok_or_else(|| invalid_media("PATH is unavailable"))?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| {
            candidate
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.is_file())
        })
        .ok_or_else(|| invalid_media("pinned media tool is not available on PATH"))
}

fn os_args(prefix: &[&str], last: Option<OsString>) -> Vec<OsString> {
    let mut arguments = prefix.iter().map(OsString::from).collect::<Vec<_>>();
    arguments.extend(last);
    arguments
}

struct ToolOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_command(
    program: &Path,
    arguments: &[OsString],
    stdin: Option<File>,
    stdout_limit: usize,
    timeout: Duration,
) -> Result<Vec<u8>, CorpusError> {
    Ok(run_command_capture(program, arguments, stdin, stdout_limit, timeout)?.stdout)
}

fn run_command_capture(
    program: &Path,
    arguments: &[OsString],
    stdin: Option<File>,
    stdout_limit: usize,
    timeout: Duration,
) -> Result<ToolOutput, CorpusError> {
    let stdin = stdin.map_or_else(Stdio::null, Stdio::from);
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| invalid_media("media tool could not be started"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| invalid_media("media tool stdout was unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| invalid_media("media tool stderr was unavailable"))?;
    let stdout_reader = thread::spawn(move || read_stream(stdout, stdout_limit));
    let stderr_reader = thread::spawn(move || read_stream(stderr, MAX_STDERR));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            child.kill()?;
            child.wait()?;
            return Err(invalid_media("media tool exceeded its execution timeout"));
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| invalid_media("media tool stdout reader failed"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| invalid_media("media tool stderr reader failed"))??;
    if !status.success() {
        return Err(invalid_media(format!(
            "media tool failed with status {:?} and stderr_sha256 {}",
            status.code(),
            digest_bytes(&stderr)
        )));
    }
    Ok(ToolOutput { stdout, stderr })
}

fn read_stream(mut stream: impl Read, limit: usize) -> Result<Vec<u8>, CorpusError> {
    let mut kept = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut overflow = false;
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if kept.len().saturating_add(read) <= limit {
            kept.extend_from_slice(&buffer[..read]);
        } else {
            overflow = true;
        }
    }
    if overflow {
        return Err(CorpusError::CapacityExceeded);
    }
    Ok(kept)
}

fn private_new_path_parent(path: &Path) -> Result<&Path, CorpusError> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(invalid_media(
            "private output path must be absolute and named",
        ));
    }
    let basename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid_media("private output filename must be valid UTF-8"))?;
    if is_reserved_output_name(basename) {
        return Err(invalid_media(
            "private output uses a reserved internal name",
        ));
    }
    match path.symlink_metadata() {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => return Err(invalid_media("private output path already exists")),
        Err(error) => return Err(error.into()),
    }
    let parent = path
        .parent()
        .ok_or_else(|| invalid_media("private output path has no parent"))?;
    validate_directory(parent, ErrorContext::Replay)?;
    Ok(parent)
}

fn is_reserved_output_name(name: &str) -> bool {
    name == OUTPUT_LOCK_FILE
        || name == INCOMPLETE_MARKER
        || name == STAGING_MARKER
        || name.starts_with(EXTRACT_STAGING_PREFIX)
        || name.starts_with(OUTPUT_STAGING_PREFIX)
        || name.starts_with(FILE_CLAIM_PREFIX)
        || name.starts_with(".scorepeek-output-claim-")
}

fn validate_claim_basename(bytes: &[u8]) -> Result<(), CorpusError> {
    let name = std::str::from_utf8(bytes)
        .map_err(|_| invalid_media("private output claim basename is not UTF-8"))?;
    if name.is_empty()
        || name.len() > 255
        || name == "."
        || name == ".."
        || name.contains('/')
        || is_reserved_output_name(name)
    {
        return Err(invalid_media("private output claim basename is invalid"));
    }
    Ok(())
}

fn write_private_output(path: &Path, bytes: &[u8]) -> Result<(), CorpusError> {
    if bytes.len() > MAX_MEDIA_JSON {
        return Err(CorpusError::CapacityExceeded);
    }
    let (parent, lock) = lock_output_parent(path)?;
    publish_private_file(&parent, path, bytes)?;
    drop(lock);
    Ok(())
}

fn publish_private_file(parent: &Path, output: &Path, bytes: &[u8]) -> Result<(), CorpusError> {
    let basename = output
        .file_name()
        .ok_or_else(|| invalid_media("private output path has no filename"))?
        .as_bytes();
    if basename.len() > 255 {
        return Err(invalid_media("private output filename is too long"));
    }
    let mut claim = Builder::new()
        .prefix(FILE_CLAIM_PREFIX)
        .tempfile_in(parent)?;
    claim.write_all(FILE_CLAIM_MARKER_BYTES)?;
    claim.write_all(basename)?;
    claim.flush()?;
    claim.as_file().sync_all()?;
    File::open(parent)?.sync_all()?;
    let claim_name = claim
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid_media("private file claim name is invalid"))?;
    let staging = parent.join(format!("{claim_name}.staging"));
    let mut staged = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staging)?;
    let publication = (|| -> Result<(), CorpusError> {
        staged.write_all(bytes)?;
        staged.sync_all()?;
        fs::hard_link(&staging, output)?;
        File::open(output)?.sync_all()?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    drop(staged);
    if let Err(error) = publication {
        fs::remove_file(&staging)?;
        claim.close()?;
        File::open(parent)?.sync_all()?;
        return Err(error);
    }
    fs::remove_file(staging)?;
    claim.close()?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn lock_output_parent(path: &Path) -> Result<(PathBuf, File), CorpusError> {
    let parent = private_new_path_parent(path)?.to_owned();
    let lock = open_output_lock(&parent)?;
    lock.lock()?;
    recover_output_staging(&parent)?;
    private_new_path_parent(path)?;
    Ok((parent, lock))
}

fn open_output_lock(parent: &Path) -> Result<File, CorpusError> {
    let path = parent.join(OUTPUT_LOCK_FILE);
    let exists = match path.symlink_metadata() {
        Ok(metadata) if metadata.is_file() => true,
        Ok(_) => return Err(invalid_media("private output lock is not a regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    let lock = if exists {
        OpenOptions::new().read(true).write(true).open(&path)?
    } else {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?
    };
    if !path.symlink_metadata()?.is_file() || !lock.metadata()?.is_file() {
        return Err(invalid_media("private output lock changed while opening"));
    }
    lock.sync_all()?;
    File::open(parent)?.sync_all()?;
    Ok(lock)
}

fn recover_output_staging(parent: &Path) -> Result<(), CorpusError> {
    let mut changed = false;
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let metadata = entry.path().symlink_metadata()?;
        if name.starts_with(EXTRACT_STAGING_PREFIX) {
            if !metadata.is_dir() {
                return Err(invalid_media("frame staging entry is not a directory"));
            }
            validate_owned_marker(&entry.path(), STAGING_MARKER, STAGING_MARKER_BYTES)?;
            fs::remove_dir_all(entry.path())?;
            changed = true;
        } else if name.starts_with(FILE_CLAIM_PREFIX) && !name.ends_with(".staging") {
            recover_file_claim(parent, &entry.path(), &name, &metadata)?;
            changed = true;
        } else if let Some(digest) = name.strip_prefix(".scorepeek-output-claim-") {
            recover_output_claim(parent, &entry.path(), digest, &metadata)?;
            changed = true;
        }
    }
    if changed {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn validate_owned_marker(directory: &Path, name: &str, expected: &[u8]) -> Result<(), CorpusError> {
    let marker = directory.join(name);
    validate_regular_file(&marker, ErrorContext::Replay)?;
    if read_bounded_regular(&marker, 128, ErrorContext::Replay)? != expected {
        return Err(invalid_media("owned output marker is invalid"));
    }
    Ok(())
}

fn recover_file_claim(
    parent: &Path,
    claim: &Path,
    claim_name: &str,
    metadata: &fs::Metadata,
) -> Result<(), CorpusError> {
    if !metadata.is_file() {
        return Err(invalid_media("private file claim is not a regular file"));
    }
    validate_regular_file(claim, ErrorContext::Replay)?;
    let claim_bytes = read_bounded_regular(claim, 512, ErrorContext::Replay)?;
    let Some(basename) = claim_bytes.strip_prefix(FILE_CLAIM_MARKER_BYTES) else {
        return Err(invalid_media("private file claim marker is invalid"));
    };
    validate_claim_basename(basename)?;
    let staging = parent.join(format!("{claim_name}.staging"));
    match staging.symlink_metadata() {
        Ok(staged) if staged.is_file() => {
            validate_regular_file(&staging, ErrorContext::Replay)?;
            fs::remove_file(staging)?;
        }
        Ok(_) => return Err(invalid_media("private file staging is not a regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs::remove_file(claim)?;
    Ok(())
}

fn recover_output_claim(
    parent: &Path,
    claim: &Path,
    expected_digest: &str,
    metadata: &fs::Metadata,
) -> Result<(), CorpusError> {
    if !metadata.is_file() || !is_sha256(expected_digest) {
        return Err(invalid_media("output claim is invalid"));
    }
    validate_regular_file(claim, ErrorContext::Replay)?;
    let basename = read_bounded_regular(claim, 255, ErrorContext::Replay)?;
    if basename.is_empty() || digest_bytes(&basename) != expected_digest {
        fs::remove_file(claim)?;
        return Ok(());
    }
    validate_claim_basename(&basename)?;
    let destination = parent.join(OsString::from_vec(basename));
    match destination.symlink_metadata() {
        Ok(target) if target.is_dir() => {
            if validate_owned_marker(&destination, INCOMPLETE_MARKER, INCOMPLETE_MARKER_BYTES)
                .is_ok()
            {
                fs::remove_dir_all(&destination)?;
            }
        }
        Ok(_) => return Err(invalid_media("claimed output is not a directory")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs::remove_file(claim)?;
    Ok(())
}

fn publish_extraction(staging: &Path, output: &Path, parent: &Path) -> Result<(), CorpusError> {
    let basename = output
        .file_name()
        .ok_or_else(|| invalid_media("private output path has no filename"))?
        .as_bytes();
    if basename.len() > 255 {
        return Err(invalid_media("private output filename is too long"));
    }
    let claim = parent.join(format!(
        ".scorepeek-output-claim-{}",
        digest_bytes(basename)
    ));
    let mut claim_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&claim)?;
    claim_file.write_all(basename)?;
    claim_file.sync_all()?;
    File::open(parent)?.sync_all()?;

    if let Err(error) = fs::DirBuilder::new().mode(0o700).create(output) {
        fs::remove_file(&claim)?;
        File::open(parent)?.sync_all()?;
        return Err(error.into());
    }
    publish_claimed_extraction(staging, output)?;
    fs::remove_file(&claim)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn publish_claimed_extraction(staging: &Path, output: &Path) -> Result<(), CorpusError> {
    write_owned_marker(output, INCOMPLETE_MARKER, INCOMPLETE_MARKER_BYTES)?;
    validate_owned_marker(staging, STAGING_MARKER, STAGING_MARKER_BYTES)?;
    for entry in fs::read_dir(staging)? {
        let entry = entry?;
        if entry.file_name() == STAGING_MARKER {
            continue;
        }
        fs::rename(entry.path(), output.join(entry.file_name()))?;
    }
    File::open(output)?.sync_all()?;
    fs::remove_file(output.join(INCOMPLETE_MARKER))?;
    File::open(output)?.sync_all()?;
    Ok(())
}

fn validate_ppm(
    path: &Path,
    width: u32,
    height: u32,
    pixel_bytes: u64,
) -> Result<(Vec<u8>, u64), CorpusError> {
    let limit = usize::try_from(pixel_bytes + 128).map_err(|_| CorpusError::CapacityExceeded)?;
    let bytes = read_bounded_regular(path, limit, ErrorContext::Replay)?;
    let (tokens, offset) = ppm_header(&bytes)?;
    let expected = [
        "P6".to_owned(),
        width.to_string(),
        height.to_string(),
        "255".to_owned(),
    ];
    if tokens != expected {
        return Err(invalid_media(
            "extracted PPM header does not match RGB8 dimensions",
        ));
    }
    let pixels = bytes[offset..].to_vec();
    if pixels.len() as u64 != pixel_bytes {
        return Err(invalid_media("extracted PPM pixel byte count is invalid"));
    }
    Ok((pixels, bytes.len() as u64))
}

fn ppm_header(bytes: &[u8]) -> Result<(Vec<String>, usize), CorpusError> {
    let mut tokens = Vec::with_capacity(4);
    let mut index = 0;
    while tokens.len() < 4 {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index < bytes.len() && bytes[index] == b'#' {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        let start = index;
        while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if start == index {
            return Err(invalid_media("extracted PPM header is truncated"));
        }
        tokens.push(
            std::str::from_utf8(&bytes[start..index])
                .map_err(|_| invalid_media("extracted PPM header is not ASCII"))?
                .to_owned(),
        );
    }
    if index >= bytes.len() || !bytes[index].is_ascii_whitespace() {
        return Err(invalid_media("extracted PPM has no pixel separator"));
    }
    Ok((tokens, index + 1))
}

fn invalid_media(detail: impl Into<String>) -> CorpusError {
    CorpusError::InvalidMedia(detail.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn selection_binds_decode_index_and_pts() {
        let probe = sample_probe();
        let digest = digest_bytes(&canonical_json(&probe).unwrap());
        let mut request = sample_request(&probe, &digest);
        assert!(request.validate_against(&probe, &digest).is_ok());
        request.frames[1].source_pts = 666;
        assert!(request.validate_against(&probe, &digest).is_err());
        request.frames[1].source_pts = 667;
        request.frames.swap(0, 1);
        assert!(request.validate_against(&probe, &digest).is_err());
    }

    #[test]
    fn adjacent_requested_frames_compact_into_select_ranges() {
        let frames = [10_u64, 11, 20, 21, 22, 30]
            .into_iter()
            .map(|decode_index| RequestedFrame {
                frame_id: format!("frame-{decode_index}"),
                decode_index,
                source_pts: i64::try_from(decode_index).unwrap(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            frame_select_expression(&frames),
            "between(n\\,10\\,11)+between(n\\,20\\,22)+eq(n\\,30)"
        );
    }

    #[test]
    fn regular_adjacent_pairs_compact_into_one_select_expression() {
        let frames = [100_u64, 101, 235, 236, 370, 371]
            .into_iter()
            .map(|decode_index| RequestedFrame {
                frame_id: format!("frame-{decode_index}"),
                decode_index,
                source_pts: i64::try_from(decode_index).unwrap(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            frame_select_expression(&frames),
            "between(n\\,100\\,371)*lte(mod(n-100\\,135)\\,1)"
        );
    }

    #[test]
    fn extraction_rejects_cross_fixture_source_or_profile_substitution() {
        let mut probe = sample_probe();
        let source = SourceManifest {
            schema: SOURCE_MANIFEST_SCHEMA.to_owned(),
            fixture_id: probe.fixture_id.clone(),
            session_id: "session-1".to_owned(),
            capture_profile_id: probe.capture_profile_id.clone(),
            source: probe.source.clone(),
        };
        let digest = digest_bytes(&canonical_json(&source).unwrap());
        probe.source_manifest_sha256.clone_from(&digest);
        assert!(validate_probe_source_binding(&probe, &source, &digest).is_ok());

        let mut substituted = probe.clone();
        substituted.source.sha256 = "f".repeat(64);
        assert!(validate_probe_source_binding(&substituted, &source, &digest).is_err());

        let mut substituted = probe;
        substituted.capture_profile_id = "capture-other".to_owned();
        assert!(validate_probe_source_binding(&substituted, &source, &digest).is_err());
    }

    #[test]
    fn probe_rejects_multiple_video_streams() {
        let streams = vec![video_stream(0), video_stream(1)];
        assert!(unique_video_stream(&streams).is_err());
    }

    #[test]
    fn packet_probe_rejects_non_ffv1_video() {
        let mut stream = video_stream(0);
        stream.codec_name = Some("h264".to_owned());
        let (observed, _) = observed_stream_contract(&stream).unwrap();
        assert!(validate_packet_index_contract(&observed).is_err());
    }

    #[test]
    fn input_format_rejects_secondary_resource_manifests() {
        let temporary = tempdir().unwrap();
        let playlist = temporary.path().join("source.media");
        fs::write(&playlist, b"#EXTM3U\nhttps://example.invalid/segment.ts\n").unwrap();
        assert!(validate_input_format(&playlist).is_err());
    }

    #[test]
    fn reserved_private_output_names_are_rejected() {
        let temporary = tempdir().unwrap();
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            private_new_path_parent(&temporary.path().join("ordinary-output")).unwrap(),
            temporary.path()
        );
        for name in [
            ".scorepeek-frame-staging-user",
            ".scorepeek-file-claim-user",
            ".scorepeek-output-claim-user",
            INCOMPLETE_MARKER,
        ] {
            assert!(private_new_path_parent(&temporary.path().join(name)).is_err());
        }
        for basename in [b"../outside".as_slice(), b".".as_slice(), b"a/b".as_slice()] {
            assert!(validate_claim_basename(basename).is_err());
        }
    }

    #[test]
    fn directory_publication_never_removes_an_existing_destination() {
        let temporary = tempdir().unwrap();
        let private = temporary.path().join("private");
        fs::create_dir(&private).unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
        let staging = private.join("staging");
        fs::create_dir(&staging).unwrap();
        fs::write(staging.join("manifest.json"), b"new").unwrap();
        let output = private.join("output");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("sentinel"), b"existing").unwrap();

        assert!(publish_extraction(&staging, &output, &private).is_err());
        assert_eq!(fs::read(output.join("sentinel")).unwrap(), b"existing");
        assert!(!fs::read_dir(&private).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".scorepeek-output-claim-")
        }));
    }

    #[test]
    fn output_lock_recovers_owned_staging_and_incomplete_claim() {
        let temporary = tempdir().unwrap();
        let private = temporary.path().join("private");
        fs::create_dir(&private).unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
        let extraction_staging = private.join(format!("{EXTRACT_STAGING_PREFIX}stale"));
        fs::create_dir(&extraction_staging).unwrap();
        fs::set_permissions(&extraction_staging, fs::Permissions::from_mode(0o700)).unwrap();
        write_owned_marker(&extraction_staging, STAGING_MARKER, STAGING_MARKER_BYTES).unwrap();
        let file_claim = private.join(format!("{FILE_CLAIM_PREFIX}stale"));
        let mut file_claim_bytes = FILE_CLAIM_MARKER_BYTES.to_vec();
        file_claim_bytes.extend_from_slice(b"stale-output.json");
        fs::write(&file_claim, file_claim_bytes).unwrap();
        fs::set_permissions(&file_claim, fs::Permissions::from_mode(0o600)).unwrap();
        let file_staging = private.join(format!("{FILE_CLAIM_PREFIX}stale.staging"));
        fs::write(&file_staging, b"stale").unwrap();
        fs::set_permissions(&file_staging, fs::Permissions::from_mode(0o600)).unwrap();
        let basename = b"incomplete";
        let claim = private.join(format!(
            ".scorepeek-output-claim-{}",
            digest_bytes(basename)
        ));
        fs::write(&claim, basename).unwrap();
        fs::set_permissions(&claim, fs::Permissions::from_mode(0o600)).unwrap();
        fs::create_dir(private.join("incomplete")).unwrap();
        fs::set_permissions(
            private.join("incomplete"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        write_owned_marker(
            &private.join("incomplete"),
            INCOMPLETE_MARKER,
            INCOMPLETE_MARKER_BYTES,
        )
        .unwrap();

        let (_, lock) = lock_output_parent(&private.join("new-output")).unwrap();
        drop(lock);
        assert!(
            !private
                .join(format!("{EXTRACT_STAGING_PREFIX}stale"))
                .exists()
        );
        assert!(!file_claim.exists());
        assert!(!file_staging.exists());
        assert!(!private.join("incomplete").exists());
        assert!(!claim.exists());
    }

    #[test]
    fn recovery_preserves_an_unmarked_claim_destination() {
        let temporary = tempdir().unwrap();
        let private = temporary.path().join("private");
        fs::create_dir(&private).unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
        let basename = b"existing";
        let claim = private.join(format!(
            ".scorepeek-output-claim-{}",
            digest_bytes(basename)
        ));
        fs::write(&claim, basename).unwrap();
        fs::set_permissions(&claim, fs::Permissions::from_mode(0o600)).unwrap();
        let destination = private.join("existing");
        fs::create_dir(&destination).unwrap();
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(destination.join("sentinel"), b"user-data").unwrap();

        let (_, lock) = lock_output_parent(&private.join("new-output")).unwrap();
        drop(lock);
        assert_eq!(
            fs::read(destination.join("sentinel")).unwrap(),
            b"user-data"
        );
        assert!(!claim.exists());
    }

    #[test]
    fn pinned_tools_probe_and_extract_synthetic_observed_media() {
        let temporary = tempdir().unwrap();
        let private = temporary.path().join("private");
        fs::create_dir(&private).unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
        let (store, source_manifest) = ingest_synthetic_media(&private);

        let probe_path = private.join("probe.json");
        let probe_summary = store.probe_media("fixture-media-1", &probe_path).unwrap();
        assert_eq!(probe_summary.frame_count, 3);
        assert!(probe_path.is_file());
        assert!(store.probe_media("fixture-media-1", &probe_path).is_err());
        let mut probe: MediaProbeManifest =
            serde_json::from_slice(&fs::read(&probe_path).unwrap()).unwrap();
        assert_eq!(probe.index_basis, FrameIndexBasis::Ffv1PacketOrder);
        let extraction_request = private.join("extract.json");
        fs::write(&extraction_request, serde_json::to_vec(&json!({
            "schema": EXTRACT_REQUEST_SCHEMA,
            "fixture_id": "fixture-media-1",
            "source_manifest_sha256": source_manifest.summary().unwrap().source_manifest_sha256,
            "media_probe_sha256": probe_summary.media_probe_sha256,
            "frames": [
                { "frame_id": "frame-media-1", "decode_index": 0, "source_pts": probe.frames[0].source_pts },
                { "frame_id": "frame-media-3", "decode_index": 2, "source_pts": probe.frames[2].source_pts }
            ]
        })).unwrap()).unwrap();
        let extraction_directory = private.join("extracted");
        let extraction = store
            .extract_frames(&probe_path, &extraction_request, &extraction_directory)
            .unwrap();
        assert_eq!(extraction.frame_count, 2);
        assert_eq!(extraction.extracted_bytes, 1920 * 1080 * 3 * 2);
        assert!(extraction_directory.is_dir());
        for name in ["frame-000000.ppm", "frame-000001.ppm", "manifest.json"] {
            assert!(extraction_directory.join(name).is_file());
        }
        assert!(
            store
                .extract_frames(&probe_path, &extraction_request, &extraction_directory)
                .is_err()
        );

        probe.observed.color_transfer = Some("bt709".to_owned());
        probe.observed.color_primaries = Some("bt709".to_owned());
        let (calibrated_probe_path, calibrated_request_path) =
            write_calibrated_probe_and_request(&private, &probe);
        assert_canonical_extraction(
            &store,
            &calibrated_probe_path,
            &calibrated_request_path,
            &private,
        );

        probe.frames[2].source_pts += 1;
        let tampered_probe_path = private.join("tampered-probe.json");
        let tampered_probe_bytes = canonical_json(&probe).unwrap();
        fs::write(&tampered_probe_path, &tampered_probe_bytes).unwrap();
        fs::set_permissions(&tampered_probe_path, fs::Permissions::from_mode(0o600)).unwrap();
        let tampered_probe_sha256 = digest_bytes(&tampered_probe_bytes);
        let tampered_request = private.join("tampered-extract.json");
        fs::write(
            &tampered_request,
            serde_json::to_vec(&json!({
                "schema": EXTRACT_REQUEST_SCHEMA,
                "fixture_id": "fixture-media-1",
                "source_manifest_sha256": source_manifest.summary().unwrap().source_manifest_sha256,
                "media_probe_sha256": tampered_probe_sha256,
                "frames": [
                    { "frame_id": "frame-media-3", "decode_index": 2, "source_pts": probe.frames[2].source_pts }
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let tampered_output = private.join("tampered-extracted");
        assert!(
            store
                .extract_frames(&tampered_probe_path, &tampered_request, &tampered_output)
                .is_err()
        );
        assert!(!tampered_output.exists());
    }

    fn write_calibrated_probe_and_request(
        private: &Path,
        probe: &MediaProbeManifest,
    ) -> (PathBuf, PathBuf) {
        let probe_path = private.join("calibrated-probe.json");
        let probe_bytes = canonical_json(probe).unwrap();
        fs::write(&probe_path, &probe_bytes).unwrap();
        fs::set_permissions(&probe_path, fs::Permissions::from_mode(0o600)).unwrap();
        let request_path = private.join("canonical-extract.json");
        fs::write(
            &request_path,
            canonical_json(&sample_request(probe, &digest_bytes(&probe_bytes))).unwrap(),
        )
        .unwrap();
        fs::set_permissions(&request_path, fs::Permissions::from_mode(0o600)).unwrap();
        (probe_path, request_path)
    }

    fn assert_canonical_extraction(
        store: &CorpusStore,
        probe_path: &Path,
        extraction_request: &Path,
        private: &Path,
    ) {
        let canonical_directory = private.join("canonical");
        let canonical = store
            .extract_canonical_frames(probe_path, extraction_request, &canonical_directory)
            .unwrap();
        assert_eq!(canonical.frame_count, 2);
        assert_eq!(canonical.extracted_bytes, 1920 * 1080 * 3 * 2);
        for name in [
            "frame-000000.ppm",
            "frame-000001.ppm",
            "manifest.json",
            "normalizer.json",
        ] {
            assert!(canonical_directory.join(name).is_file());
        }
        let normalizer_bytes = fs::read(canonical_directory.join("normalizer.json")).unwrap();
        assert_eq!(
            digest_bytes(&normalizer_bytes),
            canonical.normalizer_artifact_sha256
        );
        let normalizer: DomainNormalizerArtifact =
            serde_json::from_slice(&normalizer_bytes).unwrap();
        normalizer.validate().unwrap();
    }

    fn sample_probe() -> MediaProbeManifest {
        MediaProbeManifest {
            schema: PROBE_SCHEMA.to_owned(),
            fixture_id: "fixture-1".to_owned(),
            source_manifest_sha256: "a".repeat(64),
            source: ContentRef {
                sha256: "b".repeat(64),
                bytes: 1,
            },
            capture_profile_id: "capture-test".to_owned(),
            toolchain: ToolchainIdentity {
                ffmpeg_version: FFMPEG_VERSION.to_owned(),
                ffmpeg_sha256: "c".repeat(64),
                ffprobe_sha256: "d".repeat(64),
            },
            observed: ObservedMediaContract {
                input_format: INPUT_FORMAT.to_owned(),
                codec_name: "ffv1".to_owned(),
                pixel_format: "bgr0".to_owned(),
                width: 320,
                height: 180,
                source_time_base: TimeBase {
                    numerator: 1,
                    denominator: 1000,
                },
                color_range: None,
                color_space: None,
                color_transfer: None,
                color_primaries: None,
            },
            video_stream_index: 0,
            index_basis: FrameIndexBasis::Ffv1PacketOrder,
            frames: vec![
                ProbedFrame {
                    decode_index: 0,
                    source_pts: 0,
                },
                ProbedFrame {
                    decode_index: 1,
                    source_pts: 333,
                },
                ProbedFrame {
                    decode_index: 2,
                    source_pts: 667,
                },
            ],
        }
    }

    fn ingest_synthetic_media(private: &Path) -> (CorpusStore, SourceManifest) {
        let source = private.join("source.mkv");
        let status = Command::new(find_executable("ffmpeg").unwrap())
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=1920x1080:r=3:d=1",
                "-c:v",
                "ffv1",
                "-pix_fmt",
                "yuv420p",
                "-color_range",
                "tv",
                "-colorspace",
                "bt709",
                "-color_trc",
                "bt709",
                "-color_primaries",
                "bt709",
                source.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success());
        let ingest_request = private.join("ingest.json");
        fs::write(
            &ingest_request,
            serde_json::to_vec(&json!({
                "schema": "scorepeek-private-corpus-ingest-v2",
                "fixture_id": "fixture-media-1",
                "session_id": "session-media-1",
                "capture_profile_id": CALIBRATED_CAPTURE_PROFILE_SHA256
            }))
            .unwrap(),
        )
        .unwrap();
        let store = CorpusStore::new(private.join("store"));
        let manifest = store.ingest(source, ingest_request).unwrap();
        (store, manifest)
    }

    fn sample_request(probe: &MediaProbeManifest, digest: &str) -> FrameExtractionRequest {
        FrameExtractionRequest {
            schema: EXTRACT_REQUEST_SCHEMA.to_owned(),
            fixture_id: probe.fixture_id.clone(),
            source_manifest_sha256: probe.source_manifest_sha256.clone(),
            media_probe_sha256: digest.to_owned(),
            frames: vec![
                RequestedFrame {
                    frame_id: "frame-1".to_owned(),
                    decode_index: 0,
                    source_pts: 0,
                },
                RequestedFrame {
                    frame_id: "frame-2".to_owned(),
                    decode_index: 2,
                    source_pts: 667,
                },
            ],
        }
    }

    #[test]
    fn canonical_normalizer_requires_the_exact_calibrated_profile_and_toolchain() {
        let mut probe = sample_probe();
        probe.capture_profile_id = CALIBRATED_CAPTURE_PROFILE_SHA256.to_owned();
        probe.toolchain.ffmpeg_sha256 = CALIBRATED_FFMPEG_SHA256.to_owned();
        probe.observed.pixel_format = "yuv420p".to_owned();
        probe.observed.width = 1_920;
        probe.observed.height = 1_080;
        probe.observed.color_range = Some("tv".to_owned());
        probe.observed.color_space = Some("bt709".to_owned());
        probe.observed.color_transfer = Some("bt709".to_owned());
        probe.observed.color_primaries = Some("bt709".to_owned());
        assert!(DomainNormalizerArtifact::for_probe(&probe).is_ok());

        probe.capture_profile_id = CALIBRATED_GAMESCOPE_VKCAPTURE_PROFILE_SHA256.to_owned();
        assert!(DomainNormalizerArtifact::for_probe(&probe).is_ok());

        probe.capture_profile_id = "e".repeat(64);
        assert!(DomainNormalizerArtifact::for_probe(&probe).is_err());
        probe.capture_profile_id = CALIBRATED_CAPTURE_PROFILE_SHA256.to_owned();
        probe.toolchain.ffmpeg_sha256 = "f".repeat(64);
        assert!(DomainNormalizerArtifact::for_probe(&probe).is_err());
        probe.toolchain.ffmpeg_sha256 = CALIBRATED_FFMPEG_SHA256.to_owned();
        probe.observed.color_transfer = None;
        assert!(DomainNormalizerArtifact::for_probe(&probe).is_err());
    }

    fn video_stream(index: u32) -> FfprobeStream {
        FfprobeStream {
            index: Some(index),
            codec_type: Some("video".to_owned()),
            width: Some(1920),
            height: Some(1080),
            time_base: Some("1/1000".to_owned()),
            codec_name: Some("ffv1".to_owned()),
            pix_fmt: Some("bgr0".to_owned()),
            color_range: None,
            color_space: None,
            color_transfer: None,
            color_primaries: None,
        }
    }
}
