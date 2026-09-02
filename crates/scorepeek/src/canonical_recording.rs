use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use scorepeek::recognition::ScreenClass;
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::diagnostic_live::BoundCanonicalFrame;

const WINDOW_FRAMES: usize = 10;
const SEGMENT_FRAMES: usize = 600;
const FINISH_TIMEOUT: Duration = Duration::from_secs(30);
const STDERR_LIMIT: usize = 64 * 1024;
const MIB: usize = 1024 * 1024;
pub const DEFAULT_RECORDING_MEMORY_MIB: usize = 1024;
pub const MIN_RECORDING_MEMORY_MIB: usize = 128;
pub const MAX_RECORDING_MEMORY_MIB: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordingMemoryLimit {
    bytes: u64,
}

impl RecordingMemoryLimit {
    pub fn from_mib(mib: usize) -> Result<Self, String> {
        if !(MIN_RECORDING_MEMORY_MIB..=MAX_RECORDING_MEMORY_MIB).contains(&mib) {
            return Err(format!(
                "recording memory must be between {MIN_RECORDING_MEMORY_MIB} and {MAX_RECORDING_MEMORY_MIB} MiB"
            ));
        }
        let bytes = u64::try_from(mib)
            .ok()
            .and_then(|value| value.checked_mul(MIB as u64))
            .ok_or_else(|| "recording memory byte count overflows".to_owned())?;
        Ok(Self { bytes })
    }

    pub fn default_limit() -> Self {
        Self::from_mib(DEFAULT_RECORDING_MEMORY_MIB)
            .expect("the registered recording memory default is valid")
    }

    pub const fn bytes(self) -> u64 {
        self.bytes
    }
}

struct RecordingMemoryAccount {
    limit: u64,
    current: AtomicU64,
    high_water: AtomicU64,
    degraded: AtomicBool,
    memory_limit_exceeded: AtomicBool,
}

impl RecordingMemoryAccount {
    fn new(limit: RecordingMemoryLimit) -> Self {
        Self {
            limit: limit.bytes(),
            current: AtomicU64::new(0),
            high_water: AtomicU64::new(0),
            degraded: AtomicBool::new(false),
            memory_limit_exceeded: AtomicBool::new(false),
        }
    }

    fn try_reserve(&self, bytes: u64) -> bool {
        let reserved = self
            .current
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(bytes)
                    .filter(|next| *next <= self.limit)
            });
        if let Ok(previous) = reserved {
            self.high_water
                .fetch_max(previous.saturating_add(bytes), Ordering::Relaxed);
            true
        } else {
            self.degraded.store(true, Ordering::Release);
            self.memory_limit_exceeded.store(true, Ordering::Release);
            false
        }
    }

    fn release(&self, bytes: u64) {
        self.current.fetch_sub(bytes, Ordering::AcqRel);
    }

    fn mark_degraded(&self) {
        self.degraded.store(true, Ordering::Release);
    }
}

struct MemoryReservation {
    memory: Arc<RecordingMemoryAccount>,
    bytes: u64,
}

impl MemoryReservation {
    fn try_new(memory: Arc<RecordingMemoryAccount>, bytes: u64) -> Result<Self, String> {
        if memory.try_reserve(bytes) {
            Ok(Self { memory, bytes })
        } else {
            Err("recording memory limit reached".to_owned())
        }
    }
}

impl Drop for MemoryReservation {
    fn drop(&mut self) {
        self.memory.release(self.bytes);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingHealthState {
    Active,
    Pressured,
    Degraded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct RecordingHealthSnapshot {
    pub state: RecordingHealthState,
    pub memory_limit_bytes: u64,
    pub memory_used_bytes: u64,
    pub memory_high_water_bytes: u64,
    pub dropped_frames: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalRecordingCompleteness {
    Complete,
    Partial,
}

#[derive(Debug)]
pub struct CanonicalRecordingOutcome {
    pub manifest_sha256: Option<String>,
    pub completeness: CanonicalRecordingCompleteness,
    pub final_health: RecordingHealthSnapshot,
}

#[derive(Clone)]
struct RecordedFrame {
    sequence: u64,
    source_sequence: u64,
    monotonic_ms: u64,
    screen: ScreenClass,
    semantic_episode_id: Option<u64>,
    pixels: Arc<Box<[u8]>>,
    memory: Option<Arc<RecordingMemoryAccount>>,
    reserved_bytes: u64,
}

impl Drop for RecordedFrame {
    fn drop(&mut self) {
        if let Some(memory) = &self.memory {
            memory.release(self.reserved_bytes);
        }
    }
}

enum Message {
    Frame(RecordedFrame),
}

pub struct CanonicalRecordingWorker {
    sender: Sender<Message>,
    worker: JoinHandle<CanonicalRecordingOutcome>,
    dropped: Arc<AtomicU64>,
    memory: Arc<RecordingMemoryAccount>,
}

impl CanonicalRecordingWorker {
    pub fn preflight() -> Result<(), String> {
        inspect_ffmpeg().map(|_| ())
    }

    pub(crate) fn start_named(
        root: &Path,
        directory_name: &str,
        memory_limit: RecordingMemoryLimit,
    ) -> Result<Self, String> {
        let ffmpeg = inspect_ffmpeg()?;
        let directory = root.join(directory_name);
        if directory.symlink_metadata().is_ok() {
            return Err("canonical recording session already exists".to_owned());
        }
        DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|error| format!("canonical recording directory failed: {error}"))?;
        let tick_index = TickIndexWriter::create(&directory.join("canonical-ticks.ndjson"))
            .inspect_err(|_| {
                let _ = std::fs::remove_dir(&directory);
            })?;
        let (sender, receiver) = mpsc::channel();
        let dropped = Arc::new(AtomicU64::new(0));
        let worker_dropped = Arc::clone(&dropped);
        let memory = Arc::new(RecordingMemoryAccount::new(memory_limit));
        let metadata_memory = MemoryReservation::try_new(Arc::clone(&memory), 64 * 1024)?;
        let worker_memory = Arc::clone(&memory);
        let worker_directory = directory.clone();
        let worker = thread::Builder::new()
            .name("scorepeek-canonical-recorder".to_owned())
            .spawn(move || {
                Recorder::new(
                    worker_directory,
                    worker_dropped,
                    ffmpeg,
                    worker_memory,
                    Some(tick_index),
                    metadata_memory,
                )
                .run(&receiver)
            })
            .map_err(|error| {
                let _ = std::fs::remove_file(directory.join("canonical-ticks.ndjson"));
                let _ = std::fs::remove_dir(&directory);
                format!("canonical recorder worker failed: {error}")
            })?;
        Ok(Self {
            sender,
            worker,
            dropped,
            memory,
        })
    }

    pub fn offer(
        &self,
        frame: &BoundCanonicalFrame,
        screen: ScreenClass,
        semantic_episode_id: Option<u64>,
    ) -> bool {
        let frame_bytes = (crate::diagnostic_recording::CANONICAL_BYTES
            + std::mem::size_of::<RecordedFrame>()
            + std::mem::size_of::<Message>()
            + 512) as u64;
        if !self.memory.try_reserve(frame_bytes) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let message = Message::Frame(RecordedFrame {
            sequence: frame.sequence(),
            source_sequence: frame.source_sequence(),
            monotonic_ms: frame.monotonic_end_ms(),
            screen,
            semantic_episode_id,
            pixels: frame.shared_pixels(),
            memory: Some(Arc::clone(&self.memory)),
            reserved_bytes: frame_bytes,
        });
        if self.sender.send(message).is_ok() {
            true
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            self.memory.mark_degraded();
            false
        }
    }

    pub fn health(&self) -> RecordingHealthSnapshot {
        health_snapshot(&self.memory, &self.dropped)
    }

    pub fn finish(self) -> CanonicalRecordingOutcome {
        let Self {
            sender,
            worker,
            dropped,
            memory,
        } = self;
        drop(sender);
        if let Ok(mut outcome) = worker.join() {
            outcome.final_health = health_snapshot(&memory, &dropped);
            outcome
        } else {
            memory.mark_degraded();
            CanonicalRecordingOutcome {
                manifest_sha256: None,
                completeness: CanonicalRecordingCompleteness::Partial,
                final_health: health_snapshot(&memory, &dropped),
            }
        }
    }
}

fn health_snapshot(
    memory: &RecordingMemoryAccount,
    dropped: &AtomicU64,
) -> RecordingHealthSnapshot {
    let memory_used_bytes = memory.current.load(Ordering::Acquire);
    let dropped_frames = dropped.load(Ordering::Relaxed);
    let state = if dropped_frames > 0 || memory.degraded.load(Ordering::Acquire) {
        RecordingHealthState::Degraded
    } else if memory_used_bytes.saturating_mul(4) >= memory.limit.saturating_mul(3) {
        RecordingHealthState::Pressured
    } else {
        RecordingHealthState::Active
    };
    RecordingHealthSnapshot {
        state,
        memory_limit_bytes: memory.limit,
        memory_used_bytes,
        memory_high_water_bytes: memory.high_water.load(Ordering::Relaxed),
        dropped_frames,
    }
}

#[derive(Serialize)]
struct TickRecord {
    sequence: u64,
    source_sequence: u64,
    monotonic_ms: u64,
    screen: ScreenClass,
    semantic_episode_id: Option<u64>,
    disposition: &'static str,
}

#[derive(Serialize)]
struct SegmentRecord {
    path: String,
    first_sequence: u64,
    last_sequence: u64,
    frames: usize,
    raw_rgb24_sha256: String,
    encoded_sha256: String,
    bytes: u64,
}

#[derive(Serialize)]
struct Manifest<'a> {
    schema: &'static str,
    completeness: CanonicalRecordingCompleteness,
    ffmpeg_sha256: String,
    ffmpeg_version: &'a str,
    tick_index_sha256: String,
    tick_count: usize,
    segments: &'a [SegmentRecord],
    dropped_frames: u64,
    completeness_reasons: Vec<&'static str>,
    memory_limit_bytes: u64,
    memory_high_water_bytes: u64,
    integrity_verification: &'static str,
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent completeness causes and the test-only dry mode are orthogonal"
)]
struct Recorder {
    directory: PathBuf,
    previous_screen: Option<ScreenClass>,
    after_remaining: usize,
    ring: VecDeque<PendingFrame>,
    #[cfg(test)]
    ticks: Vec<TickRecord>,
    tick_index: Option<TickIndexWriter>,
    tick_index_sha256: Option<String>,
    tick_count: usize,
    tick_index_failure: bool,
    segments: Vec<SegmentRecord>,
    segment_memory: Vec<MemoryReservation>,
    encoder: Option<SegmentEncoder>,
    last_retained_sequence: Option<u64>,
    dropped_frames: u64,
    partial: bool,
    encoder_failure: bool,
    chronology_reset: bool,
    shutdown_timeout: bool,
    external_dropped: Arc<AtomicU64>,
    ffmpeg: ToolIdentity,
    observed_ticks: usize,
    previous_sequence: Option<u64>,
    previous_monotonic_ms: Option<u64>,
    dry_run: bool,
    memory: Arc<RecordingMemoryAccount>,
    _metadata_memory: MemoryReservation,
}

struct PendingFrame {
    frame: RecordedFrame,
    retained: bool,
}

struct ToolIdentity {
    path: PathBuf,
    sha256: String,
    version: String,
}

struct TickIndexWriter {
    file: File,
    digest: Sha256,
    count: usize,
}

impl TickIndexWriter {
    fn create(path: &Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| format!("canonical tick index create failed: {error}"))?;
        Ok(Self {
            file,
            digest: Sha256::new(),
            count: 0,
        })
    }

    fn write(&mut self, tick: &TickRecord) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(tick)
            .map_err(|_| "canonical tick serialization failed".to_owned())?;
        bytes.push(b'\n');
        self.file
            .write_all(&bytes)
            .map_err(|error| format!("canonical tick index write failed: {error}"))?;
        self.digest.update(&bytes);
        self.count = self.count.saturating_add(1);
        Ok(())
    }

    fn finish(self) -> Result<(String, usize), String> {
        self.file
            .sync_all()
            .map_err(|error| format!("canonical tick index sync failed: {error}"))?;
        Ok((hex_digest(self.digest.finalize().as_slice()), self.count))
    }
}

impl Recorder {
    fn new(
        directory: PathBuf,
        external_dropped: Arc<AtomicU64>,
        ffmpeg: ToolIdentity,
        memory: Arc<RecordingMemoryAccount>,
        tick_index: Option<TickIndexWriter>,
        metadata_memory: MemoryReservation,
    ) -> Self {
        Self {
            directory,
            previous_screen: None,
            after_remaining: WINDOW_FRAMES,
            ring: VecDeque::with_capacity(WINDOW_FRAMES),
            #[cfg(test)]
            ticks: Vec::new(),
            tick_index,
            tick_index_sha256: None,
            tick_count: 0,
            tick_index_failure: false,
            segments: Vec::new(),
            segment_memory: Vec::new(),
            encoder: None,
            last_retained_sequence: None,
            dropped_frames: 0,
            partial: false,
            encoder_failure: false,
            chronology_reset: false,
            shutdown_timeout: false,
            external_dropped,
            ffmpeg,
            observed_ticks: 0,
            previous_sequence: None,
            previous_monotonic_ms: None,
            dry_run: false,
            memory,
            _metadata_memory: metadata_memory,
        }
    }

    fn run(mut self, receiver: &mpsc::Receiver<Message>) -> CanonicalRecordingOutcome {
        while let Ok(message) = receiver.recv() {
            match message {
                Message::Frame(frame) => self.observe(frame),
            }
        }
        self.retain_session_tail();
        self.close_segment();
        self.finish_tick_index();
        self.dropped_frames = self
            .dropped_frames
            .saturating_add(self.external_dropped.load(Ordering::Relaxed));
        let completeness = if self.partial || self.dropped_frames > 0 {
            CanonicalRecordingCompleteness::Partial
        } else {
            CanonicalRecordingCompleteness::Complete
        };
        let manifest_sha256 = self.publish(completeness).ok();
        CanonicalRecordingOutcome {
            manifest_sha256,
            completeness,
            final_health: health_snapshot(&self.memory, &self.external_dropped),
        }
    }

    fn observe(&mut self, frame: RecordedFrame) {
        let sequence = frame.sequence;
        let monotonic_ms = frame.monotonic_ms;
        let screen = frame.screen;
        let chronology_reset = self
            .previous_sequence
            .is_some_and(|previous| sequence <= previous)
            || self
                .previous_monotonic_ms
                .is_some_and(|previous| monotonic_ms < previous);
        if chronology_reset {
            self.partial = true;
            self.chronology_reset = true;
            self.memory.mark_degraded();
            while let Some(mut pending) = self.ring.pop_front() {
                pending.retained = true;
                self.finalize_tick(pending);
            }
            self.close_segment();
            self.last_retained_sequence = None;
            self.previous_screen = None;
            self.after_remaining = WINDOW_FRAMES;
        }
        let changed = self
            .previous_screen
            .is_none_or(|previous| previous != screen);
        if changed {
            for buffered in &mut self.ring {
                buffered.retained = true;
            }
            self.after_remaining = WINDOW_FRAMES - 1;
        }
        let always = matches!(
            screen,
            ScreenClass::MusicSelect | ScreenClass::DecideTransition | ScreenClass::Result
        );
        let retained =
            self.observed_ticks < WINDOW_FRAMES || always || changed || self.after_remaining > 0;
        if !changed && self.after_remaining > 0 {
            self.after_remaining -= 1;
        }
        self.ring.push_back(PendingFrame { frame, retained });
        if self.ring.len() > WINDOW_FRAMES
            && let Some(pending) = self.ring.pop_front()
        {
            self.finalize_tick(pending);
        }
        self.previous_screen = Some(screen);
        self.previous_sequence = Some(sequence);
        self.previous_monotonic_ms = Some(monotonic_ms);
        self.observed_ticks = self.observed_ticks.saturating_add(1);
    }

    fn finalize_tick(&mut self, pending: PendingFrame) {
        let PendingFrame { frame, retained } = pending;
        if retained {
            self.retain(&frame);
        }
        let tick = TickRecord {
            sequence: frame.sequence,
            source_sequence: frame.source_sequence,
            monotonic_ms: frame.monotonic_ms,
            screen: frame.screen,
            semantic_episode_id: frame.semantic_episode_id,
            disposition: if retained {
                "retained"
            } else {
                match frame.screen {
                    ScreenClass::Play => "play_interior",
                    ScreenClass::ModeSelect => "mode_select_interior",
                    ScreenClass::Unknown => "unknown_interior",
                    _ => "retained",
                }
            },
        };
        if self.dry_run {
            #[cfg(test)]
            self.ticks.push(tick);
        } else if self
            .tick_index
            .as_mut()
            .is_none_or(|writer| writer.write(&tick).is_err())
        {
            self.partial = true;
            self.tick_index_failure = true;
            self.memory.mark_degraded();
        }
    }

    fn retain_session_tail(&mut self) {
        while let Some(mut pending) = self.ring.pop_front() {
            pending.retained = true;
            self.finalize_tick(pending);
        }
    }

    fn retain(&mut self, frame: &RecordedFrame) {
        if self
            .last_retained_sequence
            .is_some_and(|sequence| frame.sequence <= sequence)
        {
            return;
        }
        if self.dry_run {
            self.last_retained_sequence = Some(frame.sequence);
            return;
        }
        if self.encoder_failure {
            self.external_dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if self
            .encoder
            .as_ref()
            .is_some_and(|encoder| encoder.frames == SEGMENT_FRAMES)
        {
            self.close_segment();
        }
        if self.encoder_failure {
            self.external_dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if self.encoder.is_none() {
            if let Ok(encoder) = SegmentEncoder::start(
                &self.ffmpeg.path,
                &self.directory,
                self.segments.len(),
                frame.sequence,
                Arc::clone(&self.memory),
            ) {
                self.encoder = Some(encoder);
            } else {
                self.partial = true;
                self.encoder_failure = true;
                self.memory.mark_degraded();
                self.external_dropped.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        let write_error = self
            .encoder
            .as_mut()
            .and_then(|encoder| encoder.write(frame).err());
        if let Some(error) = write_error {
            let mut lost_frames = 1_u64;
            if let Some(encoder) = self.encoder.take() {
                lost_frames = lost_frames.saturating_add(encoder.frames as u64);
                encoder.abort();
            }
            self.partial = true;
            self.encoder_failure = true;
            self.memory.mark_degraded();
            self.shutdown_timeout |= error.contains("timed out");
            self.external_dropped
                .fetch_add(lost_frames, Ordering::Relaxed);
            return;
        }
        self.last_retained_sequence = Some(frame.sequence);
    }

    fn close_segment(&mut self) {
        if let Some(encoder) = self.encoder.take() {
            let path = encoder.path.clone();
            let encoded_frames = encoder.frames as u64;
            match encoder.finish() {
                Ok(segment) => {
                    if let Ok(reservation) =
                        MemoryReservation::try_new(Arc::clone(&self.memory), 4 * 1024)
                    {
                        self.segment_memory.push(reservation);
                        self.segments.push(segment);
                    } else {
                        self.partial = true;
                        self.external_dropped
                            .fetch_add(encoded_frames, Ordering::Relaxed);
                        let _ = std::fs::remove_file(path);
                    }
                }
                Err(error) => {
                    self.partial = true;
                    self.encoder_failure = true;
                    self.memory.mark_degraded();
                    self.shutdown_timeout |= error.contains("timed out");
                    self.external_dropped
                        .fetch_add(encoded_frames, Ordering::Relaxed);
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }

    fn finish_tick_index(&mut self) {
        let Some(writer) = self.tick_index.take() else {
            return;
        };
        if let Ok((digest, count)) = writer.finish() {
            self.tick_index_sha256 = Some(digest);
            self.tick_count = count;
        } else {
            self.partial = true;
            self.tick_index_failure = true;
            self.memory.mark_degraded();
        }
    }

    fn publish(&self, completeness: CanonicalRecordingCompleteness) -> Result<String, String> {
        let mut completeness_reasons = Vec::new();
        if self.dropped_frames > 0 {
            completeness_reasons.push("frame_loss");
        }
        if self.encoder_failure {
            completeness_reasons.push("encoder_failure");
        }
        if self.shutdown_timeout {
            completeness_reasons.push("shutdown_timeout");
        }
        if self.memory.memory_limit_exceeded.load(Ordering::Acquire) {
            completeness_reasons.push("memory_limit");
        }
        if self.tick_index_failure {
            completeness_reasons.push("tick_index_failure");
        }
        if self.chronology_reset {
            completeness_reasons.push("chronology_reset");
        }
        let manifest = Manifest {
            schema: "scorepeek-canonical-session-recording-v2",
            completeness,
            ffmpeg_sha256: self.ffmpeg.sha256.clone(),
            ffmpeg_version: &self.ffmpeg.version,
            tick_index_sha256: self
                .tick_index_sha256
                .clone()
                .unwrap_or_else(|| "0".repeat(64)),
            tick_count: self.tick_count,
            segments: &self.segments,
            dropped_frames: self.dropped_frames,
            completeness_reasons,
            memory_limit_bytes: self.memory.limit,
            memory_high_water_bytes: self.memory.high_water.load(Ordering::Relaxed),
            integrity_verification: "deferred_to_import",
        };
        let mut bytes = serde_json::to_vec(&manifest)
            .map_err(|_| "canonical manifest serialization failed".to_owned())?;
        bytes.push(b'\n');
        write_new(&self.directory.join("canonical-manifest.json"), &bytes)?;
        Ok(digest_bytes(&bytes))
    }
}

struct SegmentEncoder {
    child: Child,
    writer: Option<SegmentWriter>,
    path: PathBuf,
    first_sequence: u64,
    last_sequence: u64,
    frames: usize,
    raw_digest: Sha256,
    stderr: JoinHandle<Vec<u8>>,
    _stderr_memory: MemoryReservation,
}

struct SegmentWriter {
    sender: SyncSender<WriterMessage>,
    worker: JoinHandle<()>,
}

enum WriterMessage {
    Frame {
        pixels: Arc<Box<[u8]>>,
        completion: mpsc::Sender<Result<(), String>>,
    },
    Close,
}

fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

impl SegmentEncoder {
    fn start(
        ffmpeg: &Path,
        root: &Path,
        index: usize,
        first_sequence: u64,
        memory: Arc<RecordingMemoryAccount>,
    ) -> Result<Self, String> {
        let stderr_memory = MemoryReservation::try_new(memory, STDERR_LIMIT as u64)?;
        let path = root.join(format!("segment-{index:04}.mkv"));
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("segment output failed: {error}"))?;
        let mut child = match Command::new(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "-video_size",
                "1920x1080",
                "-framerate",
                "10",
                "-i",
                "pipe:0",
                "-an",
                "-c:v",
                "libx264rgb",
                "-crf",
                "0",
                "-preset",
                "ultrafast",
                "-g",
                "10",
                "-keyint_min",
                "10",
                "-sc_threshold",
                "0",
                "-f",
                "matroska",
                "pipe:1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::from(output))
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                return Err(format!("ffmpeg encoder start failed: {error}"));
            }
        };
        let Some(stdin) = child.stdin.take() else {
            kill_and_reap(&mut child);
            let _ = std::fs::remove_file(&path);
            return Err("ffmpeg stdin unavailable".to_owned());
        };
        let Some(stderr_pipe) = child.stderr.take() else {
            kill_and_reap(&mut child);
            let _ = std::fs::remove_file(&path);
            return Err("ffmpeg stderr unavailable".to_owned());
        };
        let stderr = bounded_reader(stderr_pipe);
        let writer = match SegmentWriter::start(stdin) {
            Ok(writer) => writer,
            Err(error) => {
                kill_and_reap(&mut child);
                let _ = stderr.join();
                let _ = std::fs::remove_file(&path);
                return Err(error);
            }
        };
        Ok(Self {
            child,
            writer: Some(writer),
            path,
            first_sequence,
            last_sequence: first_sequence,
            frames: 0,
            raw_digest: Sha256::new(),
            stderr,
            _stderr_memory: stderr_memory,
        })
    }

    fn write(&mut self, frame: &RecordedFrame) -> Result<(), String> {
        self.writer
            .as_ref()
            .ok_or_else(|| "ffmpeg stdin closed".to_owned())?
            .write(Arc::clone(&frame.pixels))?;
        self.raw_digest.update(frame.pixels.as_ref());
        self.last_sequence = frame.sequence;
        self.frames += 1;
        Ok(())
    }

    fn finish(mut self) -> Result<SegmentRecord, String> {
        if let Some(writer) = self.writer.take()
            && let Err(error) = writer.finish()
        {
            return self.abort_with(error);
        }
        let started = Instant::now();
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(error) => return self.abort_with(error.to_string()),
            }
            if started.elapsed() >= FINISH_TIMEOUT {
                return self.abort_with("ffmpeg encoder timed out".to_owned());
            }
            thread::sleep(Duration::from_millis(10));
        };
        if !status.success() {
            let stderr = self.stderr.join().unwrap_or_default();
            let _ = std::fs::remove_file(&self.path);
            return Err(format!(
                "ffmpeg encoder failed: {}",
                String::from_utf8_lossy(&stderr)
            ));
        }
        let _ = self.stderr.join();
        let metadata = match self.path.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                let _ = std::fs::remove_file(&self.path);
                return Err(error.to_string());
            }
        };
        let raw_rgb24_sha256 = hex_digest(self.raw_digest.finalize().as_slice());
        let Some(filename) = self.path.file_name().and_then(|name| name.to_str()) else {
            let _ = std::fs::remove_file(&self.path);
            return Err("segment filename invalid".to_owned());
        };
        let encoded_sha256 = match digest_file(&self.path) {
            Ok(digest) => digest,
            Err(error) => {
                let _ = std::fs::remove_file(&self.path);
                return Err(error);
            }
        };
        Ok(SegmentRecord {
            path: filename.to_owned(),
            first_sequence: self.first_sequence,
            last_sequence: self.last_sequence,
            frames: self.frames,
            raw_rgb24_sha256,
            encoded_sha256,
            bytes: metadata.len(),
        })
    }

    fn abort(mut self) {
        let writer = self.writer.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(writer) = writer {
            writer.abort();
        }
        let _ = self.stderr.join();
        let _ = std::fs::remove_file(self.path);
    }

    fn abort_with(mut self, error: String) -> Result<SegmentRecord, String> {
        let writer = self.writer.take();
        kill_and_reap(&mut self.child);
        if let Some(writer) = writer {
            writer.abort();
        }
        let _ = self.stderr.join();
        let _ = std::fs::remove_file(self.path);
        Err(error)
    }
}

impl SegmentWriter {
    fn start(mut stdin: ChildStdin) -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("scorepeek-canonical-ffmpeg-stdin".to_owned())
            .spawn(move || {
                while let Ok(message) = receiver.recv() {
                    match message {
                        WriterMessage::Frame { pixels, completion } => {
                            let result = stdin
                                .write_all(pixels.as_ref())
                                .map_err(|error| format!("ffmpeg frame write failed: {error}"));
                            let failed = result.is_err();
                            let _ = completion.send(result);
                            if failed {
                                return;
                            }
                        }
                        WriterMessage::Close => return,
                    }
                }
            })
            .map_err(|error| format!("ffmpeg stdin writer start failed: {error}"))?;
        Ok(Self { sender, worker })
    }

    fn write(&self, pixels: Arc<Box<[u8]>>) -> Result<(), String> {
        let (completion, receiver) = mpsc::channel();
        self.sender
            .send(WriterMessage::Frame { pixels, completion })
            .map_err(|_| "ffmpeg stdin writer is unavailable".to_owned())?;
        receiver
            .recv_timeout(FINISH_TIMEOUT)
            .map_err(|_| "ffmpeg frame write timed out".to_owned())?
    }

    fn finish(self) -> Result<(), String> {
        self.sender
            .send(WriterMessage::Close)
            .map_err(|_| "ffmpeg stdin writer is unavailable".to_owned())?;
        self.worker
            .join()
            .map_err(|_| "ffmpeg stdin writer panicked".to_owned())
    }

    fn abort(self) {
        drop(self.sender);
        let _ = self.worker.join();
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("canonical artifact create failed: {error}"))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("canonical artifact write failed: {error}"))
}

fn digest_bytes(bytes: &[u8]) -> String {
    hex_digest(Sha256::digest(bytes).as_slice())
}

fn digest_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0; 64 * 1024].into_boxed_slice();
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex_digest(digest.finalize().as_slice()))
}

fn inspect_ffmpeg() -> Result<ToolIdentity, String> {
    let path = executable_on_path("ffmpeg")?;
    let encoder = Command::new(&path)
        .args(["-hide_banner", "-h", "encoder=libx264rgb"])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("ffmpeg encoder preflight failed: {error}"))?;
    if !encoder.status.success() || !String::from_utf8_lossy(&encoder.stdout).contains("libx264rgb")
    {
        return Err("ffmpeg does not provide libx264rgb".to_owned());
    }
    let version = Command::new(&path)
        .arg("-version")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("ffmpeg version preflight failed: {error}"))?;
    if !version.status.success() {
        return Err("ffmpeg version preflight failed".to_owned());
    }
    let version = String::from_utf8_lossy(&version.stdout)
        .lines()
        .next()
        .ok_or_else(|| "ffmpeg version output is empty".to_owned())?
        .to_owned();
    Ok(ToolIdentity {
        sha256: digest_file(&path)?,
        path,
        version,
    })
}

fn executable_on_path(name: &str) -> Result<PathBuf, String> {
    let path = std::env::var_os("PATH").ok_or_else(|| "PATH is not set".to_owned())?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return candidate
                .canonicalize()
                .map_err(|error| format!("ffmpeg path resolution failed: {error}"));
        }
    }
    Err("ffmpeg is not available on PATH".to_owned())
}

fn bounded_reader(mut reader: impl std::io::Read + Send + 'static) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut retained = Vec::new();
        let mut buffer = [0; 4096];
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 {
                break;
            }
            let available = STDERR_LIMIT.saturating_sub(retained.len());
            retained.extend_from_slice(&buffer[..read.min(available)]);
        }
        retained
    })
}

#[cfg(test)]
fn decode_segment(ffmpeg: &Path, path: &Path) -> Result<(String, usize), String> {
    let mut child = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("ffmpeg decoder start failed: {error}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "ffmpeg decoder stdout unavailable".to_owned())?;
    let stderr = bounded_reader(
        child
            .stderr
            .take()
            .ok_or_else(|| "ffmpeg decoder stderr unavailable".to_owned())?,
    );
    let decoded = thread::spawn(move || {
        let mut digest = Sha256::new();
        let mut bytes = 0usize;
        let mut buffer = vec![0; 64 * 1024].into_boxed_slice();
        loop {
            let read = stdout
                .read(&mut buffer)
                .map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            bytes = bytes.saturating_add(read);
            digest.update(&buffer[..read]);
        }
        Ok::<_, String>((hex_digest(digest.finalize().as_slice()), bytes))
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if started.elapsed() >= FINISH_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            let _ = decoded.join();
            let _ = stderr.join();
            return Err("ffmpeg decoder timed out".to_owned());
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stderr = stderr.join().unwrap_or_default();
    let (digest, bytes) = decoded
        .join()
        .map_err(|_| "ffmpeg decoder output reader panicked".to_owned())??;
    if !status.success() {
        return Err(format!(
            "ffmpeg decoder failed: {}",
            String::from_utf8_lossy(&stderr)
        ));
    }
    let frame_bytes = crate::diagnostic_recording::CANONICAL_BYTES;
    if bytes % frame_bytes != 0 {
        return Err("ffmpeg decoded a partial RGB24 frame".to_owned());
    }
    Ok((digest, bytes / frame_bytes))
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to a string cannot fail");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorder() -> Recorder {
        let memory = Arc::new(RecordingMemoryAccount::new(
            RecordingMemoryLimit::default_limit(),
        ));
        let metadata_memory = MemoryReservation::try_new(Arc::clone(&memory), 64 * 1024).unwrap();
        let mut recorder = Recorder::new(
            PathBuf::new(),
            Arc::new(AtomicU64::new(0)),
            ToolIdentity {
                path: PathBuf::new(),
                sha256: "0".repeat(64),
                version: "test".to_owned(),
            },
            memory,
            None,
            metadata_memory,
        );
        recorder.dry_run = true;
        recorder
    }

    fn frame(sequence: u64, screen: ScreenClass) -> RecordedFrame {
        RecordedFrame {
            sequence,
            source_sequence: sequence,
            monotonic_ms: sequence.saturating_mul(100),
            screen,
            semantic_episode_id: None,
            pixels: Arc::new(Vec::new().into_boxed_slice()),
            memory: None,
            reserved_bytes: 0,
        }
    }

    #[test]
    fn stable_play_interior_is_elided_but_session_and_transition_windows_are_retained() {
        let mut recorder = recorder();
        for sequence in 1..=40 {
            recorder.observe(frame(sequence, ScreenClass::Play));
        }
        recorder.observe(frame(41, ScreenClass::Result));
        recorder.retain_session_tail();

        assert!(
            recorder.ticks[..10]
                .iter()
                .all(|tick| tick.disposition == "retained")
        );
        assert!(
            recorder.ticks[10..30]
                .iter()
                .all(|tick| tick.disposition == "play_interior")
        );
        assert!(
            recorder.ticks[30..]
                .iter()
                .all(|tick| tick.disposition == "retained")
        );
    }

    #[test]
    fn music_select_decide_and_result_are_never_elided() {
        for screen in [
            ScreenClass::MusicSelect,
            ScreenClass::DecideTransition,
            ScreenClass::Result,
        ] {
            let mut recorder = recorder();
            for sequence in 1..=50 {
                recorder.observe(frame(sequence, screen));
            }
            recorder.retain_session_tail();
            assert!(
                recorder
                    .ticks
                    .iter()
                    .all(|tick| tick.disposition == "retained")
            );
        }
    }

    #[test]
    fn unknown_interior_is_typed_and_known_resume_keeps_both_sides() {
        let mut recorder = recorder();
        for sequence in 1..=40 {
            recorder.observe(frame(sequence, ScreenClass::Unknown));
        }
        recorder.observe(frame(41, ScreenClass::Play));
        recorder.retain_session_tail();
        assert!(
            recorder.ticks[10..30]
                .iter()
                .all(|tick| tick.disposition == "unknown_interior")
        );
        assert!(
            recorder.ticks[30..]
                .iter()
                .all(|tick| tick.disposition == "retained")
        );
    }

    #[test]
    fn shared_memory_limit_rejects_only_while_full_and_keeps_degraded_sticky() {
        let account = RecordingMemoryAccount::new(RecordingMemoryLimit { bytes: 100 });
        assert!(account.try_reserve(80));
        assert!(!account.try_reserve(30));
        assert_eq!(account.current.load(Ordering::Acquire), 80);
        account.release(80);
        assert!(account.try_reserve(30));
        assert_eq!(account.high_water.load(Ordering::Relaxed), 80);
        assert!(account.degraded.load(Ordering::Acquire));
        assert!(account.memory_limit_exceeded.load(Ordering::Acquire));
    }

    #[test]
    fn chronology_reset_flushes_the_old_tail_but_degrades_the_session() {
        let mut recorder = recorder();
        for sequence in 1..=25 {
            recorder.observe(frame(sequence, ScreenClass::Play));
        }
        recorder.observe(frame(1, ScreenClass::Result));
        recorder.retain_session_tail();

        assert_eq!(recorder.ticks.len(), 26);
        assert!(
            recorder.ticks[15..25]
                .iter()
                .all(|tick| tick.disposition == "retained")
        );
        assert_eq!(recorder.ticks[25].sequence, 1);
        assert_eq!(recorder.ticks[25].disposition, "retained");
        assert!(recorder.partial);
        assert!(recorder.chronology_reset);
    }

    #[test]
    fn encoder_failure_drops_later_retained_frames_without_restarting_a_child() {
        let mut recorder = recorder();
        recorder.dry_run = false;
        recorder.encoder_failure = true;

        for sequence in 1..=5 {
            recorder.retain(&frame(sequence, ScreenClass::Result));
        }

        assert!(recorder.encoder.is_none());
        assert_eq!(recorder.external_dropped.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn tick_index_is_streamed_with_a_digest_and_count() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ticks.ndjson");
        let mut writer = TickIndexWriter::create(&path).unwrap();
        let tick = TickRecord {
            sequence: 7,
            source_sequence: 70,
            monotonic_ms: 700,
            screen: ScreenClass::Result,
            semantic_episode_id: Some(3),
            disposition: "retained",
        };
        writer.write(&tick).unwrap();
        let (actual_digest, count) = writer.finish().unwrap();
        let bytes = std::fs::read(path).unwrap();

        assert_eq!(count, 1);
        assert_eq!(actual_digest, digest_bytes(&bytes));
        assert!(bytes.ends_with(b"\n"));
        assert!(!bytes[..bytes.len() - 1].contains(&b'\n'));
    }

    #[test]
    fn ffmpeg_segment_is_pixel_exact_after_lossless_decode() {
        let ffmpeg = inspect_ffmpeg().unwrap();
        let root = tempfile::tempdir().unwrap();
        let pixels = (0..crate::diagnostic_recording::CANONICAL_BYTES)
            .map(|index| u8::try_from(index % 251).unwrap())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let frame = RecordedFrame {
            sequence: 7,
            source_sequence: 70,
            monotonic_ms: 700,
            screen: ScreenClass::Result,
            semantic_episode_id: Some(3),
            pixels: Arc::new(pixels),
            memory: None,
            reserved_bytes: 0,
        };
        let memory = Arc::new(RecordingMemoryAccount::new(
            RecordingMemoryLimit::default_limit(),
        ));
        let mut encoder = SegmentEncoder::start(&ffmpeg.path, root.path(), 0, 7, memory).unwrap();
        encoder.write(&frame).unwrap();
        let segment = encoder.finish().unwrap();
        assert_eq!(segment.frames, 1);
        assert_eq!(segment.first_sequence, 7);
        assert_eq!(segment.last_sequence, 7);
        assert_eq!(
            segment.raw_rgb24_sha256,
            digest_bytes(frame.pixels.as_ref())
        );
        let (decoded_digest, decoded_frames) =
            decode_segment(&ffmpeg.path, &root.path().join(&segment.path)).unwrap();
        assert_eq!(decoded_digest, segment.raw_rgb24_sha256);
        assert_eq!(decoded_frames, segment.frames);
    }

    #[test]
    fn stdin_writer_reports_a_broken_child_and_can_be_reaped() {
        let mut child = Command::new("true")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let writer = SegmentWriter::start(stdin).unwrap();
        assert!(child.wait().unwrap().success());
        assert!(writer.write(Arc::new(vec![1].into_boxed_slice())).is_err());
        writer.abort();
    }

    #[test]
    fn encoder_spawn_failure_removes_the_owned_segment_file() {
        let root = tempfile::tempdir().unwrap();
        let memory = Arc::new(RecordingMemoryAccount::new(
            RecordingMemoryLimit::default_limit(),
        ));
        assert!(
            SegmentEncoder::start(
                Path::new("/scorepeek-test/nonexistent-ffmpeg"),
                root.path(),
                0,
                1,
                memory,
            )
            .is_err()
        );
        assert!(!root.path().join("segment-0000.mkv").exists());
    }
}
