use super::*;

pub(super) fn canonical_tick_follows(previous: Option<(u64, u64)>, tick: &CanonicalTick) -> bool {
    previous.is_none_or(|(sequence, monotonic)| {
        tick.sequence > sequence && tick.monotonic_ms >= monotonic
    })
}

pub(super) fn decode_canonical_segment(
    path: &Path,
    frames: usize,
) -> Result<(String, usize), CorpusError> {
    let digest = decode_canonical_frames(path, frames, DecodeContext::Verify, |_, _| Ok(()))?;
    Ok((digest, frames))
}

#[derive(Clone, Copy)]
pub(super) enum DecodeContext {
    Verify,
    Replay,
}

pub(super) enum DecodeSource<'a> {
    Path(&'a Path),
    File(File),
}

fn decode_canonical_frames(
    path: &Path,
    expected_frames: usize,
    context: DecodeContext,
    observe: impl FnMut(usize, Box<[u8]>) -> Result<(), CorpusError> + Send,
) -> Result<String, CorpusError> {
    decode_canonical_frames_with_activity(path, expected_frames, context, None, observe)
}

pub(super) fn decode_canonical_frames_with_activity(
    path: &Path,
    expected_frames: usize,
    context: DecodeContext,
    activity: Option<&ReplayDecodeActivity>,
    mut observe: impl FnMut(usize, Box<[u8]>) -> Result<(), CorpusError> + Send,
) -> Result<String, CorpusError> {
    decode_canonical_frames_with_program_and_timing(
        path,
        expected_frames,
        context,
        activity,
        OsStr::new("ffmpeg"),
        move |index, pixels, _| observe(index, pixels),
    )
}

fn decode_canonical_frames_with_activity_and_timing(
    path: &Path,
    expected_frames: usize,
    context: DecodeContext,
    activity: Option<&ReplayDecodeActivity>,
    observe: impl FnMut(usize, Box<[u8]>, u64) -> Result<(), CorpusError> + Send,
) -> Result<String, CorpusError> {
    decode_canonical_frames_with_program_and_timing(
        path,
        expected_frames,
        context,
        activity,
        OsStr::new("ffmpeg"),
        observe,
    )
}

pub(super) fn decode_canonical_frames_with_program(
    path: &Path,
    expected_frames: usize,
    context: DecodeContext,
    activity: Option<&ReplayDecodeActivity>,
    program: &OsStr,
    mut observe: impl FnMut(usize, Box<[u8]>) -> Result<(), CorpusError> + Send,
) -> Result<String, CorpusError> {
    decode_canonical_frames_with_program_and_timing(
        path,
        expected_frames,
        context,
        activity,
        program,
        move |index, pixels, _| observe(index, pixels),
    )
}

fn decode_canonical_frames_with_program_and_timing(
    path: &Path,
    expected_frames: usize,
    context: DecodeContext,
    activity: Option<&ReplayDecodeActivity>,
    program: &OsStr,
    observe: impl FnMut(usize, Box<[u8]>, u64) -> Result<(), CorpusError> + Send,
) -> Result<String, CorpusError> {
    decode_canonical_source_with_program_and_timing(
        DecodeSource::Path(path),
        expected_frames,
        context,
        activity,
        program,
        observe,
    )
}

pub(super) fn decode_resolved_canonical_frames(
    source: &ResolvedSegment,
    expected_frames: usize,
    context: DecodeContext,
    activity: Option<&ReplayDecodeActivity>,
    observe: impl FnMut(usize, Box<[u8]>, u64) -> Result<(), CorpusError> + Send,
) -> Result<String, CorpusError> {
    let source = match source {
        ResolvedSegment::Local(path) => DecodeSource::Path(path),
        ResolvedSegment::Remote(segment) => DecodeSource::File(segment.input()?),
    };
    decode_canonical_source_with_program_and_timing(
        source,
        expected_frames,
        context,
        activity,
        OsStr::new("ffmpeg"),
        observe,
    )
}

pub(super) fn decode_canonical_source_with_program_and_timing(
    source: DecodeSource<'_>,
    expected_frames: usize,
    context: DecodeContext,
    activity: Option<&ReplayDecodeActivity>,
    program: &OsStr,
    mut observe: impl FnMut(usize, Box<[u8]>, u64) -> Result<(), CorpusError> + Send,
) -> Result<String, CorpusError> {
    let decoder_memory = activity.map(ReplayDecodeActivity::reserve_decoder);
    let mut command = Command::new(program);
    command.args(["-hide_banner", "-loglevel", "error", "-threads", "1", "-i"]);
    match source {
        DecodeSource::Path(path) => {
            command.arg(path).stdin(Stdio::null());
        }
        DecodeSource::File(file) => {
            command.arg("pipe:0").stdin(Stdio::from(file));
        }
    }
    let child = command
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| decode_error(context, format!("ffmpeg decode failed: {error}")))?;
    let mut child = ReapedChild(child);
    let activity_guard = activity
        .zip(decoder_memory)
        .map(|(activity, memory)| activity.enter(child.id(), memory));
    let Some(mut stdout) = child.stdout.take() else {
        kill_and_reap(&mut child);
        return Err(decode_error(
            context,
            "ffmpeg decoder stdout is unavailable".to_owned(),
        ));
    };
    let Some(stderr) = child.stderr.take() else {
        kill_and_reap(&mut child);
        return Err(decode_error(
            context,
            "ffmpeg decoder stderr is unavailable".to_owned(),
        ));
    };
    let stderr = bounded_decode_stderr(stderr);
    if let Some(activity) = &activity_guard {
        activity.sample_rss(child.id());
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || -> Result<String, String> {
        let mut digest = Sha256::new();
        for _ in 0..expected_frames {
            let mut pixels = vec![0u8; 1920 * 1080 * 3].into_boxed_slice();
            stdout
                .read_exact(&mut pixels)
                .map_err(|error| format!("canonical RGB frame read failed: {error}"))?;
            digest.update(&pixels);
            sender
                .send((Instant::now(), pixels))
                .map_err(|_| "canonical decoder consumer stopped".to_owned())?;
        }
        let mut extra = [0_u8; 1];
        if stdout
            .read(&mut extra)
            .map_err(|error| format!("canonical RGB trailer read failed: {error}"))?
            != 0
        {
            return Err("canonical segment decoded more frames than declared".to_owned());
        }
        Ok(hex_digest(digest.finalize().as_slice()))
    });
    let mut last_progress = Instant::now();
    for index in 0..expected_frames {
        let pixels = loop {
            if let Some(activity) = &activity_guard {
                activity.sample_rss(child.id());
            }
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(decoded) => break decoded,
                Err(RecvTimeoutError::Timeout)
                    if last_progress.elapsed() < CANONICAL_DECODE_TIMEOUT => {}
                Err(RecvTimeoutError::Timeout) => {
                    drop(receiver);
                    abort_decoder(&mut child, reader, stderr);
                    return Err(decode_error(
                        context,
                        "canonical decode timed out".to_owned(),
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let detail = finish_failed_decoder(&mut child, reader, stderr);
                    return Err(decode_error(context, detail));
                }
            }
        };
        let (decoded_at, pixels) = pixels;
        let decode_consumer_wait_us = duration_us(decoded_at.elapsed());
        let mut callback_timed_out = false;
        let callback_started = Instant::now();
        let callback = thread::scope(|scope| {
            let (callback_sender, callback_receiver) = mpsc::sync_channel(1);
            let observer = &mut observe;
            let callback = scope.spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    observer(index, pixels, decode_consumer_wait_us)
                }));
                let _ = callback_sender.send(result);
            });
            let result = loop {
                if let Some(activity) = &activity_guard {
                    activity.sample_rss(child.id());
                }
                match callback_receiver.recv_timeout(Duration::from_millis(10)) {
                    Ok(result) => break Some(result),
                    Err(RecvTimeoutError::Timeout)
                        if callback_started.elapsed() < CANONICAL_DECODE_TIMEOUT => {}
                    Err(RecvTimeoutError::Timeout) => {
                        kill_and_reap(&mut child);
                        callback_timed_out = true;
                        break None;
                    }
                    Err(RecvTimeoutError::Disconnected) => break None,
                }
            };
            let _ = callback.join();
            result
        });
        if callback_timed_out {
            drop(receiver);
            abort_decoder(&mut child, reader, stderr);
            return Err(decode_error(
                context,
                "canonical decode timed out while consuming a frame".to_owned(),
            ));
        }
        match callback {
            Some(Ok(Ok(()))) => last_progress = Instant::now(),
            Some(Ok(Err(error))) => {
                drop(receiver);
                abort_decoder(&mut child, reader, stderr);
                return Err(error);
            }
            Some(Err(_)) | None => {
                drop(receiver);
                abort_decoder(&mut child, reader, stderr);
                return Err(decode_error(
                    context,
                    "canonical decoder consumer panicked".to_owned(),
                ));
            }
        }
    }
    let status = loop {
        if let Some(activity) = &activity_guard {
            activity.sample_rss(child.id());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                abort_decoder(&mut child, reader, stderr);
                return Err(decode_error(
                    context,
                    format!("ffmpeg wait failed: {error}"),
                ));
            }
        }
        if last_progress.elapsed() >= CANONICAL_DECODE_TIMEOUT {
            drop(receiver);
            abort_decoder(&mut child, reader, stderr);
            return Err(decode_error(
                context,
                "canonical decode timed out".to_owned(),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    };
    if let Some(activity) = &activity_guard {
        activity.finish();
    }
    drop(receiver);
    let stderr_bytes = stderr.join().unwrap_or_default();
    let digest = reader
        .join()
        .map_err(|_| decode_error(context, "canonical decoder reader panicked".to_owned()))?
        .map_err(|detail| decode_error(context, detail))?;
    if !status.success() {
        return Err(decode_error(
            context,
            format!(
                "ffmpeg canonical decode failed: {}",
                String::from_utf8_lossy(&stderr_bytes)
            ),
        ));
    }
    Ok(digest)
}

fn decode_error(context: DecodeContext, detail: String) -> CorpusError {
    match context {
        DecodeContext::Verify => CorpusError::InvalidRequest(detail),
        DecodeContext::Replay => CorpusError::InvalidReplay(detail),
    }
}

fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

struct ReapedChild(Child);

impl Deref for ReapedChild {
    type Target = Child;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ReapedChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for ReapedChild {
    fn drop(&mut self) {
        kill_and_reap(&mut self.0);
    }
}

fn abort_decoder(
    child: &mut Child,
    reader: JoinHandle<Result<String, String>>,
    stderr: JoinHandle<Vec<u8>>,
) {
    kill_and_reap(child);
    let _ = reader.join();
    let _ = stderr.join();
}

fn finish_failed_decoder(
    child: &mut Child,
    reader: JoinHandle<Result<String, String>>,
    stderr: JoinHandle<Vec<u8>>,
) -> String {
    kill_and_reap(child);
    let reader = reader.join();
    let stderr = stderr.join().unwrap_or_default();
    match reader {
        Ok(Err(detail)) => detail,
        Err(_) => "canonical decoder reader panicked".to_owned(),
        Ok(Ok(_)) => format!(
            "ffmpeg canonical decode ended before all frames: stderr={}",
            String::from_utf8_lossy(&stderr)
        ),
    }
}

fn bounded_decode_stderr(mut stderr: impl std::io::Read + Send + 'static) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut retained = Vec::new();
        let mut buffer = [0_u8; 4096];
        while let Ok(read) = stderr.read(&mut buffer) {
            if read == 0 {
                break;
            }
            let available = CANONICAL_DECODE_STDERR_BYTES.saturating_sub(retained.len());
            retained.extend_from_slice(&buffer[..read.min(available)]);
        }
        retained
    })
}
