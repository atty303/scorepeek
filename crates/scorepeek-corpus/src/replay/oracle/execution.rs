use super::*;

pub fn replay_corpus(store: &Path) -> Result<CorpusReplaySummary, CorpusError> {
    replay_corpus_with_options(store, CorpusReplayOptions::default())
}

pub(super) fn normalize_observation_stream(bytes: &[u8]) -> Result<(Vec<u8>, u64), CorpusError> {
    let mut normalized = Vec::new();
    let mut count = 0_u64;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if count >= MAX_NDJSON_RECORDS as u64 {
            return invalid("corpus observation count exceeds its bound");
        }
        if line.len() > MAX_NDJSON_RECORD_BYTES || line.last() != Some(&b'\n') {
            return invalid("corpus observation record exceeds its bound");
        }
        let value: Value = serde_json::from_slice(line)?;
        let schema = value["schema"].as_str().ok_or_else(|| {
            CorpusError::InvalidRequest("corpus observation schema is unavailable".into())
        })?;
        if schema != "scorepeek-recognition-observation-v23" {
            return invalid("corpus observation source schema differs");
        }
        let sequence = value["tick_sequence"].as_u64().ok_or_else(|| {
            CorpusError::InvalidRequest("corpus observation sequence is invalid".into())
        })?;
        let timestamp_ms = value["source_timestamp_ms"]
            .as_u64()
            .or_else(|| {
                value
                    .pointer("/timing/source_pts_ms")
                    .and_then(Value::as_u64)
            })
            .or_else(|| {
                value
                    .pointer("/timing/monotonic_end_ms")
                    .and_then(Value::as_u64)
            })
            .ok_or_else(|| {
                CorpusError::InvalidRequest("corpus observation timestamp is invalid".into())
            })?;
        let screen = value["screen"]
            .as_str()
            .or_else(|| value.pointer("/decision/screen").and_then(Value::as_str))
            .or_else(|| value.pointer("/fields/screen").and_then(Value::as_str))
            .ok_or_else(|| {
                CorpusError::InvalidRequest("corpus observation screen is invalid".into())
            })?;
        let record = serde_json::json!({
            "schema": CORPUS_OBSERVATION_SCHEMA,
            "tick_sequence": sequence,
            "source_timestamp_ms": timestamp_ms,
            "screen": screen,
            "fields": value.get("fields").cloned().unwrap_or(Value::Null),
            "decision": value.get("decision").cloned().unwrap_or(Value::Null),
            "song_id": value.get("song_id").cloned().unwrap_or(Value::Null),
        });
        normalized.extend_from_slice(&canonical_json(&record)?);
        if normalized.len() as u64 > MAX_CORPUS_OBSERVATION_BYTES {
            return invalid("normalized corpus observations exceed their byte bound");
        }
        count = count.saturating_add(1);
    }
    if count == 0 {
        return invalid("corpus observation stream is empty");
    }
    Ok((normalized, count))
}

pub fn replay_corpus_with_options(
    store: &Path,
    options: CorpusReplayOptions,
) -> Result<CorpusReplaySummary, CorpusError> {
    let segment_resolver = SegmentResolver {
        remote: SegmentRemote::from_environment()?,
        local_segment_decodes: Arc::new(AtomicU64::new(0)),
    };
    let available_parallelism = thread::available_parallelism().map_or(1, usize::from);
    if !(MINIMUM_REPLAY_MEMORY_MIB..=MAXIMUM_REPLAY_MEMORY_MIB).contains(&options.memory_mib)
        || options
            .text_workers
            .is_some_and(|workers| workers == 0 || workers > available_parallelism)
    {
        return invalid("corpus replay worker or memory configuration is invalid");
    }
    let Some((generation_sha256, suite)) = load_active_suite(store)? else {
        return invalid("active regression suite is unavailable");
    };
    let mut episodes = 0;
    let mut canonical_frames = 0;
    let mut negatives = 0;
    let bundle = scorepeek_runtime::resources::model::acquire::ensure_small_model(None, |_| {})
        .map_err(|error| CorpusError::InvalidReplay(format!("model cache failed: {error}")))?;
    let catalog_root = default_catalog_root()?;
    let diagnostic_root = tempfile::tempdir()?;
    let mut replay_failures = Vec::new();
    if suite.schema == SUITE_SCHEMA {
        return replay_canonical_suite(
            store,
            generation_sha256,
            &suite,
            options,
            &CanonicalReplayEnvironment {
                bundle: &bundle,
                catalog_root: &catalog_root,
                diagnostic_root: diagnostic_root.path(),
                segment_resolver: &segment_resolver,
            },
        );
    }
    for (session_index, entry) in suite.entries.iter().enumerate() {
        let (session, session_bytes) = read_json::<CaptureSession>(
            &store
                .join("sessions")
                .join(format!("{}.json", entry.session_sha256)),
        )?;
        let (label, label_bytes) = read_regression_label(
            &store
                .join("labels")
                .join(format!("{}.json", entry.label_sha256)),
        )?;
        if session.schema != SESSION_SCHEMA
            || digest(&session_bytes) != entry.session_sha256
            || digest(&label_bytes) != entry.label_sha256
            || label.session_sha256 != entry.session_sha256
            || !matches!(
                session.completeness.as_str(),
                "complete" | "partial" | "dropped"
            )
            || session.processed_ticks.saturating_add(session.busy_skips) == 0
            || label.episodes.windows(2).any(|pair| {
                pair[0]
                    .stable_sequences
                    .last()
                    .zip(pair[1].stable_sequences.first())
                    .is_none_or(|(left, right)| left >= right)
            })
        {
            return invalid("suite entry binding is invalid");
        }
        let frame_map = session_frame_map(&session);
        let binding = session_binding(store, &session)?;
        replay_normalization_pairs(store, &session)?;
        let descriptor = scorepeek_core::diagnostics::DiagnosticRunDescriptor {
            run_id: format!("corpus-replay-{session_index}"),
            monotonic_start_ms: 0,
            resource: scorepeek_core::diagnostics::DiagnosticResource {
                program: "scorepeek",
                version: env!("CARGO_PKG_VERSION"),
                build_sha256: "0".repeat(64),
            },
            binding: scorepeek_core::diagnostics::DiagnosticBinding {
                capture_generation: 1,
                capture_profile_sha256: binding.capture_profile_sha256.clone(),
                normalizer_sha256: binding.normalizer_sha256.clone(),
                canonical_layout_sha256: scorepeek_core::replay::CanonicalLayout::sha256(),
                catalog_sha256: session.catalog_sha256.clone(),
                model_sha256: scorepeek_core::replay::LIVE_MODEL_SHA256.to_owned(),
                runtime_sha256: scorepeek_core::replay::LIVE_RUNTIME_SHA256.to_owned(),
                replay: None,
            },
        };
        let mut recognition = ReplayRecognitionSession::start_registered(
            diagnostic_root.path(),
            descriptor,
            scorepeek_core::diagnostics::DiagnosticPolicy {
                enabled: false,
                ..scorepeek_core::diagnostics::DiagnosticPolicy::default()
            },
            &catalog_root,
            &bundle,
        )
        .map_err(|error| {
            CorpusError::InvalidReplay(format!("production recognizer could not start: {error:?}"))
        })?;
        for artifact_sha256 in frame_map.values() {
            let pixels = read_canonical_object(store, artifact_sha256)?;
            scorepeek_core::replay::inspect_canonical_rgb8(&pixels)
                .map_err(|_| CorpusError::InvalidReplay("scene predicate failed".to_owned()))?;
            canonical_frames += 1;
        }
        for sequence in &label.negative_frames {
            let pixels = read_canonical_object(
                store,
                frame_map.get(sequence).ok_or_else(|| {
                    CorpusError::InvalidReplay("labeled negative frame is unavailable".to_owned())
                })?,
            )?;
            if scorepeek_core::replay::inspect_canonical_rgb8(&pixels)
                .map_err(|_| CorpusError::InvalidReplay("scene predicate failed".to_owned()))?
                .screen
                != scorepeek_core::replay::ScreenClass::Unknown
            {
                return invalid_replay("negative frame is no longer unknown");
            }
            negatives += 1;
        }
        for episode in &label.episodes {
            if episode.stable_sequences.is_empty() {
                return invalid_replay("episode has no stable frame");
            }
            for sequence in &episode.stable_sequences {
                let pixels = read_canonical_object(
                    store,
                    frame_map.get(sequence).ok_or_else(|| {
                        CorpusError::InvalidReplay("stable frame is unavailable".to_owned())
                    })?,
                )?;
                if scorepeek_core::replay::inspect_canonical_rgb8(&pixels)
                    .map_err(|_| CorpusError::InvalidReplay("scene predicate failed".to_owned()))?
                    .screen
                    != scorepeek_core::replay::ScreenClass::Result
                {
                    return invalid_replay("stable result frame is no longer a result");
                }
                let frame = scorepeek_runtime::diagnostics::live::BoundCanonicalFrame::for_replay(
                    1,
                    *sequence,
                    sequence.saturating_mul(100),
                    binding.capture_profile_sha256.clone(),
                    binding.normalizer_sha256.clone(),
                    pixels.into_boxed_slice(),
                )
                .map_err(|_| {
                    CorpusError::InvalidReplay("canonical replay frame is invalid".to_owned())
                })?;
                let inspected = recognition.inspect(&frame).map_err(|_| {
                    CorpusError::InvalidReplay("production frame inspection failed".to_owned())
                })?;
                let ReplayFieldSubmission::Submitted(pending) = inspected.field_submission else {
                    return invalid_replay("stable result frame was not submitted for OCR");
                };
                let ReplayFieldPoll::Ready { observation, .. } =
                    recognition.wait_field_observation(&pending, Duration::from_secs(5))
                else {
                    return invalid_replay("production OCR did not complete");
                };
                let output = observation.output().as_ref().map_err(|error| {
                    CorpusError::InvalidReplay(format!("production OCR failed: {error}"))
                })?;
                let scorepeek_core::replay::ScreenFieldObservations::Result(fields) =
                    output.fields()
                else {
                    return invalid_replay("stable frame produced non-result fields");
                };
                let observed_song = output
                    .result_resolution()
                    .and_then(scorepeek_core::replay::ResultSongResolution::accepted_song_id)
                    .map(|song| song.as_uuid().to_string());
                let clear_type_matches = !episode_requires_clear_type(episode)
                    || output.clear_type() == Some(episode.expected_clear_type.as_str());
                if !clear_type_matches
                    || observed_song.as_deref() != Some(&episode.expected_song_id)
                {
                    replay_failures.push(format!(
                        "episode {} tick {} differs: expected song={} clear_type={:?}, observed song={} clear_type={:?} title_ocr={:?} artist_ocr={:?} resolution={:?}",
                        episode.episode_id,
                        sequence,
                        episode.expected_song_id,
                        episode.expected_clear_type,
                        observed_song.as_deref().unwrap_or("unresolved"),
                        output.clear_type().unwrap_or("unresolved"),
                        fields.title.open_text,
                        fields.artist.open_text,
                        output.result_resolution(),
                    ));
                }
                let expected = &episode.expected_result;
                let Some(expected_play_options) = expected.play_options.as_deref() else {
                    return invalid_replay("validated result label lacks play option truth");
                };
                if !expected_play_options_match(&fields.play_options.parsed, expected_play_options)
                {
                    replay_failures.push(format!(
                        "episode {} tick {} play options differ: expected={:?}, observed={:?}",
                        episode.episode_id,
                        sequence,
                        expected.play_options,
                        fields.play_options.parsed,
                    ));
                }
                match output.result_chart_resolution() {
                    Some(ResultChartResolution::Accepted {
                        chart,
                        current_score,
                        ..
                    }) => {
                        if !expected_play_side_matches(
                            fields.panel_side,
                            &expected.play_side,
                        )
                            || !play_mode_matches_type(
                                &expected.play_mode,
                                expected.play_type,
                            )
                            || chart.key.play_type != expected.play_type
                            || chart.key.difficulty != expected.difficulty
                            || chart.level != expected.level
                            || chart.notes != expected.notes
                            || *current_score != expected.current_score
                        {
                            replay_failures.push(format!(
                                "episode {} tick {} result context differs: expected={:?}, observed_chart={:?}, observed_score={}",
                                episode.episode_id, sequence, expected, chart, current_score,
                            ));
                        }
                    }
                    resolution => replay_failures.push(format!(
                        "episode {} tick {} result context unresolved: expected={:?}, raw difficulty={:?} level={:?} notes={:?} score={:?}, parsed={:?}, resolution={:?}, numeric={:?}",
                        episode.episode_id,
                        sequence,
                        expected,
                        fields.difficulty.open_text,
                        fields.level.open_text,
                        fields.notes.open_text,
                        fields.current_score.open_text,
                        output.parsed_result_fields(),
                        resolution,
                        output.numeric_batch(),
                    )),
                }
                if let (
                    Some(expected_judgments),
                    Some(expected_miss_count),
                    Some(expected_timing),
                    Some(expected_combo_break),
                    Some(expected_previous_best),
                ) = (
                    expected.judgments.as_ref(),
                    expected.miss_count.as_ref(),
                    expected.timing.as_ref(),
                    expected.combo_break.as_ref(),
                    expected.previous_best.as_ref(),
                ) {
                    match output.result_performance_resolution() {
                        Some(ResultPerformanceResolution::Accepted {
                            judgments,
                            miss_count,
                            timing,
                            combo_break,
                            previous_best,
                            ..
                        }) if judgments == expected_judgments
                            && optional_supplemental_matches(miss_count, expected_miss_count)
                            && optional_supplemental_matches(&timing.fast, &expected_timing.fast)
                            && optional_supplemental_matches(&timing.slow, &expected_timing.slow)
                            && optional_supplemental_matches(combo_break, expected_combo_break)
                            && optional_previous_matches(
                                &previous_best.clear_type,
                                &expected_previous_best.clear_type,
                            )
                            && optional_previous_matches(
                                &previous_best.score,
                                &expected_previous_best.score,
                            )
                            && optional_previous_matches(
                                &previous_best.miss_count,
                                &expected_previous_best.miss_count,
                            ) => {}
                        resolution => replay_failures.push(format!(
                            "episode {} tick {} performance differs: expected judgments={:?} miss_count={:?} timing={:?} combo_break={:?} previous_best={:?}, observed={:?}, numeric={:?}",
                            episode.episode_id,
                            sequence,
                            expected_judgments,
                            expected_miss_count,
                            expected_timing,
                            expected_combo_break,
                            expected_previous_best,
                            resolution,
                            output.numeric_batch(),
                        )),
                    }
                }
            }
            episodes += 1;
        }
        let finish = recognition.finish(
            scorepeek_core::diagnostics::DiagnosticRunStatus::Success,
            1,
            Duration::from_secs(5),
        );
        if !finish.field_observer_complete {
            return invalid_replay("production recognizer did not finish cleanly");
        }
    }
    if !replay_failures.is_empty() {
        return Err(CorpusError::InvalidReplay(replay_failures.join("; ")));
    }
    Ok(CorpusReplaySummary {
        schema: "scorepeek-private-corpus-replay-v4",
        generation_sha256,
        session_count: suite.entries.len(),
        episode_count: episodes,
        canonical_frames,
        negative_frames: negatives,
        text_workers: 0,
        preprocess_workers: 0,
        decode_workers: 0,
        maximum_active_sessions: 0,
        maximum_concurrent_decoders: 0,
        decoder_children: 0,
        maximum_blocked_sessions: 0,
        completed_sessions: suite.entries.len(),
        memory_limit_bytes: 0,
        tracked_memory_peak_bytes: 0,
        process_rss_peak_bytes: 0,
        ffmpeg_rss_peak_total_bytes: 0,
        decoder_details: Vec::new(),
        decode_consumer_wait_us: 0,
        preprocess_queue_wait_us: 0,
        preprocess_wall_us: 0,
        screen_classification_us: 0,
        crop_prepare_us: 0,
        field_queue_wait_us: 0,
        text_batch_wall_us: 0,
        maximum_text_worker_inference_us: 0,
        text_worker_busy_us: 0,
        numeric_inference_us: 0,
        field_join_us: 0,
        catalog_projection_us: 0,
        field_frame_wall_us: 0,
        ordered_commit_wait_us: 0,
        decoder_slot_wait_us: 0,
        memory_wait_us: 0,
        sessions: Vec::new(),
        corpus_wall_us: 0,
        local_segment_decodes: 0,
        remote_segment_downloads: 0,
        remote_downloaded_bytes: 0,
    })
}

pub(super) fn optional_supplemental_matches<T: PartialEq>(
    observed: &SupplementalResultValue<T>,
    expected: &SupplementalResultValue<T>,
) -> bool {
    observed == expected || matches!(observed, SupplementalResultValue::Unknown { .. })
}

struct ReplayPending {
    pending: ReplayPendingObservation,
    _memory: ReplayPendingMemory,
}

struct PreparedReplayFrame {
    pixels: Box<[u8]>,
    recognition: ReplayPreparedFrame,
    memory: ReplayPendingMemory,
    queue_wait_us: u64,
    wall_us: u64,
}

struct PendingReplayPreprocess {
    tick: CanonicalTick,
    receiver: mpsc::Receiver<Result<PreparedReplayFrame, String>>,
}

struct ReplayPreprocessJob {
    pixels: Box<[u8]>,
    memory: ReplayPendingMemory,
    queued_at: Instant,
    output: mpsc::SyncSender<Result<PreparedReplayFrame, String>>,
}

struct ReplayPreprocessPoolInner {
    senders: Vec<mpsc::Sender<Option<ReplayPreprocessJob>>>,
    cursor: AtomicUsize,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

#[derive(Clone)]
pub(super) struct ReplayPreprocessPool {
    inner: Arc<ReplayPreprocessPoolInner>,
}

impl ReplayPreprocessPool {
    fn start(workers: usize) -> Self {
        let mut senders = Vec::with_capacity(workers);
        let mut handles = Vec::with_capacity(workers);
        for _worker_id in 0..workers {
            let (sender, receiver) = mpsc::channel::<Option<ReplayPreprocessJob>>();
            senders.push(sender);
            handles.push(thread::spawn(move || {
                while let Ok(Some(job)) = receiver.recv() {
                    let queue_wait_us = duration_us(job.queued_at.elapsed());
                    let started = Instant::now();
                    let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        ReplayPreparedFrame::prepare_since(&job.pixels, job.queued_at)
                    }));
                    let result = match prepared {
                        Ok(Ok(recognition)) => Ok(PreparedReplayFrame {
                            pixels: job.pixels,
                            recognition,
                            memory: job.memory,
                            queue_wait_us,
                            wall_us: duration_us(started.elapsed()),
                        }),
                        Ok(Err(error)) => {
                            Err(format!("canonical replay preprocessing failed: {error:?}"))
                        }
                        Err(_) => Err("canonical replay preprocessing panicked".to_owned()),
                    };
                    let _ = job.output.send(result);
                }
            }));
        }
        Self {
            inner: Arc::new(ReplayPreprocessPoolInner {
                senders,
                cursor: AtomicUsize::new(0),
                handles: Mutex::new(handles),
            }),
        }
    }

    fn submit(
        &self,
        tick: CanonicalTick,
        pixels: Box<[u8]>,
        memory: ReplayPendingMemory,
    ) -> Result<PendingReplayPreprocess, CorpusError> {
        let (output, receiver) = mpsc::sync_channel(1);
        let index = self.inner.cursor.fetch_add(1, Ordering::Relaxed) % self.inner.senders.len();
        self.inner.senders[index]
            .send(Some(ReplayPreprocessJob {
                pixels,
                memory,
                queued_at: Instant::now(),
                output,
            }))
            .map_err(|_| {
                CorpusError::InvalidReplay("canonical replay preprocessing stopped".to_owned())
            })?;
        Ok(PendingReplayPreprocess { tick, receiver })
    }
}

impl Drop for ReplayPreprocessPoolInner {
    fn drop(&mut self) {
        for sender in &self.senders {
            let _ = sender.send(None);
        }
        for handle in self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
        {
            let _ = handle.join();
        }
    }
}

#[derive(Default)]
#[allow(
    clippy::struct_field_names,
    reason = "every replay duration field includes its serialized microsecond unit"
)]
struct ReplayMeasurements {
    decode_consumer_wait_us: u64,
    preprocess_queue_wait_us: u64,
    preprocess_wall_us: u64,
    screen_classification_us: u64,
    crop_prepare_us: u64,
    field_queue_wait_us: u64,
    text_batch_wall_us: u64,
    maximum_text_worker_inference_us: u64,
    text_worker_busy_us: u64,
    numeric_inference_us: u64,
    field_join_us: u64,
    catalog_projection_us: u64,
    field_frame_wall_us: u64,
    ordered_commit_wait_us: u64,
}

struct PreparedReplaySession {
    index: usize,
    session: CaptureSession,
    label: RegressionLabel,
    binding: SessionBinding,
}

#[derive(Clone)]
struct QueuedReplaySession {
    index: usize,
    session_sha256: String,
    label_sha256: String,
    memory_wait_started: Instant,
    memory_wait_us: u64,
}

struct ReplaySessionRuntime {
    session: CaptureSession,
    label: RegressionLabel,
    binding: SessionBinding,
    recognition: Option<ReplayRecognitionSession>,
    event_stream: ReplayEventStream,
    timeline: scorepeek_core::replay::TimelineDriver,
    pending: VecDeque<ReplayPending>,
    measurements: ReplayMeasurements,
    failures: Vec<String>,
    canonical: CanonicalRecordingManifest,
    retained: Vec<CanonicalTick>,
    segment_index: usize,
    prefetched_segments: VecDeque<PrefetchedReplaySegment>,
    retained_offset: usize,
    canonical_frames: usize,
    last_sequence: u64,
    last_monotonic_ms: u64,
    session_id: String,
    session_started: Instant,
    decoder_slot_wait_us: u64,
    memory_wait_us: u64,
    _memory: ReplaySessionMemory,
}

impl Drop for ReplaySessionRuntime {
    fn drop(&mut self) {
        if let Some(recognition) = self.recognition.take() {
            let _ = recognition.finish_offline(
                scorepeek_core::diagnostics::DiagnosticRunStatus::Error,
                self.last_monotonic_ms,
            );
        }
    }
}

pub(super) struct ReplayEventStream {
    pub(super) output: scorepeek_runtime::events::server::RoutineOutput,
    pub(super) observer: Option<JoinHandle<Result<ReplayObserved, String>>>,
}

impl ReplayEventStream {
    fn finish(&mut self, status: &str) -> Result<ReplayObserved, String> {
        self.output.finish_diagnostics(status);
        self.observer
            .take()
            .ok_or_else(|| "replay diagnostic observer is not active".to_owned())?
            .join()
            .map_err(|_| "replay diagnostic observer panicked".to_owned())?
    }
}

impl Drop for ReplayEventStream {
    fn drop(&mut self) {
        if self.observer.is_some() {
            let _ = self.finish("error");
        }
    }
}

enum ReplayWork {
    Queued(QueuedReplaySession),
    Prepared(Box<PreparedReplaySession>, u64),
    Active(Box<ReplaySessionRuntime>),
}

struct ScheduledReplayWork {
    index: usize,
    queued_at: Instant,
    work: ReplayWork,
}

enum ReplayStep {
    Continue(Box<ReplaySessionRuntime>),
    Finalize(Box<ReplaySessionRuntime>),
}

enum ReplayWorkerResult {
    Step {
        index: usize,
        session_key: String,
        result: Result<ReplayStep, CorpusError>,
    },
    Finalized {
        index: usize,
        session_key: String,
        result: Result<ReplaySessionOutcome, CorpusError>,
    },
}

struct ReplaySessionOutcome {
    trace: Option<Box<TraceStatus>>,
    session_key: String,
    music_select_best_snapshots: usize,
    episode_count: usize,
    canonical_frames: usize,
    negative_frames: usize,
    measurements: ReplayMeasurements,
    failures: Vec<String>,
    wall_us: u64,
    decoder_slot_wait_us: u64,
    memory_wait_us: u64,
}

#[derive(Default)]
pub(super) struct ReplayObserved {
    music_selections: Vec<(u64, scorepeek_core::event::MusicSelectionState)>,
    confirmed_results: Vec<scorepeek_core::event::ResultDomainEvent>,
    music_select_best_snapshots: usize,
    trace: Option<TraceStatus>,
}

#[derive(Default)]
pub(super) struct ReplayDecodeActivity {
    pub(super) active: AtomicUsize,
    pub(super) maximum_active: AtomicUsize,
    pub(super) children: AtomicUsize,
    pub(super) tracked_bytes: AtomicU64,
    pub(super) tracked_peak_bytes: AtomicU64,
    pub(super) next_decoder_id: AtomicUsize,
    pub(super) process_rss_peak_bytes: AtomicU64,
    pub(super) ffmpeg_rss_peak_total_bytes: AtomicU64,
    pub(super) ffmpeg_current_rss: Mutex<BTreeMap<usize, u64>>,
    pub(super) live_pids: Mutex<BTreeMap<usize, u32>>,
    pub(super) decoder_details: Mutex<Vec<CorpusReplayDecoderSummary>>,
}

impl ReplayDecodeActivity {
    pub(super) fn reserve_decoder(&self) -> ReplayDecoderMemory<'_> {
        let bytes = self
            .tracked_bytes
            .fetch_add(DECODER_RESERVATION_BYTES as u64, Ordering::AcqRel)
            + DECODER_RESERVATION_BYTES as u64;
        self.tracked_peak_bytes.fetch_max(bytes, Ordering::AcqRel);
        ReplayDecoderMemory { activity: self }
    }

    pub(super) fn enter<'a>(
        &'a self,
        process_id: u32,
        memory: ReplayDecoderMemory<'a>,
    ) -> ReplayDecodeGuard<'a> {
        let decoder_id = self.next_decoder_id.fetch_add(1, Ordering::AcqRel);
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.maximum_active.fetch_max(active, Ordering::AcqRel);
        self.children.fetch_add(1, Ordering::AcqRel);
        self.live_pids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(decoder_id, process_id);
        ReplayDecodeGuard {
            activity: self,
            decoder_id,
            started: Instant::now(),
            rss_peak_bytes: AtomicU64::new(0),
            finished: std::sync::atomic::AtomicBool::new(false),
            _memory: memory,
        }
    }

    fn reserve_pending(self: &Arc<Self>) -> ReplayPendingMemory {
        let bytes = self.tracked_bytes.fetch_add(
            PENDING_FIELD_FRAME_RESERVATION_BYTES as u64,
            Ordering::AcqRel,
        ) + PENDING_FIELD_FRAME_RESERVATION_BYTES as u64;
        self.tracked_peak_bytes.fetch_max(bytes, Ordering::AcqRel);
        ReplayPendingMemory {
            activity: Arc::clone(self),
        }
    }

    fn reserve_session(self: &Arc<Self>) -> ReplaySessionMemory {
        let bytes = self
            .tracked_bytes
            .fetch_add(SESSION_STATE_RESERVATION_BYTES as u64, Ordering::AcqRel)
            + SESSION_STATE_RESERVATION_BYTES as u64;
        self.tracked_peak_bytes.fetch_max(bytes, Ordering::AcqRel);
        ReplaySessionMemory {
            activity: Arc::clone(self),
        }
    }
}

pub(super) struct ReplayDecoderMemory<'a> {
    activity: &'a ReplayDecodeActivity,
}

impl Drop for ReplayDecoderMemory<'_> {
    fn drop(&mut self) {
        self.activity
            .tracked_bytes
            .fetch_sub(DECODER_RESERVATION_BYTES as u64, Ordering::AcqRel);
    }
}

pub(super) struct ReplayDecodeGuard<'a> {
    activity: &'a ReplayDecodeActivity,
    decoder_id: usize,
    started: Instant,
    rss_peak_bytes: AtomicU64,
    finished: std::sync::atomic::AtomicBool,
    _memory: ReplayDecoderMemory<'a>,
}

impl ReplayDecodeGuard<'_> {
    pub(super) fn sample_rss(&self, child_id: u32) {
        if let Some(bytes) = process_rss_bytes(child_id) {
            self.rss_peak_bytes.fetch_max(bytes, Ordering::AcqRel);
            let mut current = self
                .activity
                .ffmpeg_current_rss
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            current.insert(self.decoder_id, bytes);
            self.activity
                .ffmpeg_rss_peak_total_bytes
                .fetch_max(current.values().copied().sum(), Ordering::AcqRel);
        } else {
            self.activity
                .ffmpeg_current_rss
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&self.decoder_id);
        }
        if let Some(bytes) = process_rss_bytes(std::process::id()) {
            self.activity
                .process_rss_peak_bytes
                .fetch_max(bytes, Ordering::AcqRel);
        }
    }

    pub(super) fn finish(&self) {
        if self.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        self.activity
            .live_pids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.decoder_id);
        self.activity
            .ffmpeg_current_rss
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.decoder_id);
        self.activity
            .decoder_details
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(CorpusReplayDecoderSummary {
                decoder_id: self.decoder_id,
                wall_us: u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX),
                rss_peak_bytes: self.rss_peak_bytes.load(Ordering::Acquire),
            });
        self.activity.active.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Drop for ReplayDecodeGuard<'_> {
    fn drop(&mut self) {
        self.finish();
    }
}

pub(super) fn process_rss_bytes(process_id: u32) -> Option<u64> {
    let status = fs::read_to_string(format!("/proc/{process_id}/status")).ok()?;
    let kib = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .split_ascii_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    kib.checked_mul(1024)
}

struct ReplayPendingMemory {
    activity: Arc<ReplayDecodeActivity>,
}

struct ReplaySessionMemory {
    activity: Arc<ReplayDecodeActivity>,
}

impl Drop for ReplaySessionMemory {
    fn drop(&mut self) {
        self.activity
            .tracked_bytes
            .fetch_sub(SESSION_STATE_RESERVATION_BYTES as u64, Ordering::AcqRel);
    }
}

impl Drop for ReplayPendingMemory {
    fn drop(&mut self) {
        self.activity.tracked_bytes.fetch_sub(
            PENDING_FIELD_FRAME_RESERVATION_BYTES as u64,
            Ordering::AcqRel,
        );
    }
}

fn replay_canonical_suite(
    store: &Path,
    generation_sha256: String,
    suite: &RegressionSuite,
    options: CorpusReplayOptions,
    environment: &CanonicalReplayEnvironment<'_>,
) -> Result<CorpusReplaySummary, CorpusError> {
    let replay_started = std::time::Instant::now();
    let available_parallelism = std::thread::available_parallelism().map_or(1, usize::from);
    let text_workers = options.text_workers.unwrap_or_else(|| {
        scorepeek_core::model::session::recommended_text_worker_count(
            scorepeek_core::model::session::RecognitionExecutionMode::Offline,
            available_parallelism,
        )
    });
    let preprocess_workers = (available_parallelism / 4).clamp(1, 8);
    let memory_limit_bytes = options.memory_mib.saturating_mul(1024 * 1024);
    let memory_decode_slots = (memory_limit_bytes
        / (DECODER_RESERVATION_BYTES
            + 2 * (SESSION_STATE_RESERVATION_BYTES + PENDING_FIELD_FRAME_RESERVATION_BYTES)))
        .max(1);
    let decode_workers = suite
        .entries
        .len()
        .min((available_parallelism / 4).max(1))
        .min(memory_decode_slots);
    if decode_workers == 0 {
        return invalid_replay("canonical replay suite is empty");
    }
    let queue_epoch = Instant::now();
    let mut queued = suite
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| QueuedReplaySession {
            index,
            session_sha256: entry.session_sha256.clone(),
            label_sha256: entry.label_sha256.clone(),
            memory_wait_started: queue_epoch,
            memory_wait_us: 0,
        })
        .collect::<VecDeque<_>>();
    let mut bootstrap_failures: Vec<(usize, String, CorpusError)> = Vec::new();
    let (first_source, first, shared) = loop {
        let Some(source) = queued.pop_front() else {
            bootstrap_failures.sort_by_key(|(index, _, _)| *index);
            return Err(CorpusError::InvalidReplay(
                bootstrap_failures
                    .into_iter()
                    .map(|(index, key, error)| format!("session[{index}] {key}: {error}"))
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        };
        let prepared = match load_prepared_replay_session(store, &source) {
            Ok(prepared) => prepared,
            Err(error) => {
                bootstrap_failures.push((source.index, source.session_sha256.clone(), error));
                continue;
            }
        };
        let descriptor = replay_descriptor(prepared.index, &prepared.session, &prepared.binding);
        match RuntimeReplaySharedResources::load(
            &descriptor,
            environment.catalog_root,
            environment.bundle,
            text_workers,
        ) {
            Ok(shared) => {
                let catalog_sha256 = prepared.session.catalog_sha256.clone();
                break (
                    source,
                    prepared,
                    Arc::new(ReplaySharedResources::new(
                        catalog_sha256,
                        Arc::new(shared),
                        environment.catalog_root,
                        environment.bundle,
                    )),
                );
            }
            Err(error) => bootstrap_failures.push((
                source.index,
                source.session_sha256.clone(),
                CorpusError::InvalidReplay(format!(
                    "shared production recognizer could not start: {error}"
                )),
            )),
        }
    };
    let maximum_active_sessions = suite
        .entries
        .len()
        .min(decode_workers.saturating_mul(2).max(1));
    let pending_slots = memory_limit_bytes
        .saturating_sub(decode_workers.saturating_mul(DECODER_RESERVATION_BYTES))
        .saturating_sub(maximum_active_sessions.saturating_mul(SESSION_STATE_RESERVATION_BYTES))
        / PENDING_FIELD_FRAME_RESERVATION_BYTES;
    let per_session_pending_limit = pending_slots
        .checked_div(maximum_active_sessions)
        .unwrap_or(1)
        .max(1)
        .min(text_workers.saturating_mul(2));
    let decode_activity = Arc::new(ReplayDecodeActivity::default());
    let preprocess_pool = ReplayPreprocessPool::start(preprocess_workers);
    let memory_wait_epoch = Instant::now();
    for source in &mut queued {
        source.memory_wait_started = memory_wait_epoch;
    }
    let mut ready = VecDeque::new();
    ready.push_back(ScheduledReplayWork {
        index: first.index,
        queued_at: Instant::now(),
        work: ReplayWork::Prepared(Box::new(first), first_source.memory_wait_us),
    });
    while ready.len() < maximum_active_sessions {
        let Some(source) = queued.pop_front() else {
            break;
        };
        ready.push_back(ScheduledReplayWork {
            index: source.index,
            queued_at: Instant::now(),
            work: ReplayWork::Queued(source),
        });
    }
    let (work_sender, work_receiver) = mpsc::channel::<Option<ScheduledReplayWork>>();
    let work_receiver = Arc::new(Mutex::new(work_receiver));
    let (result_sender, result_receiver) = mpsc::channel::<ReplayWorkerResult>();
    let (finalize_sender, finalize_receiver) =
        mpsc::channel::<Option<(usize, String, ReplaySessionRuntime)>>();
    let finalize_receiver = Arc::new(Mutex::new(finalize_receiver));
    let trace = options
        .trace_dir
        .map(|path| Arc::new(Mutex::new(ReplayTrace::new(path, &generation_sha256))));
    let mut handles = Vec::with_capacity(decode_workers);
    for _ in 0..decode_workers {
        let receiver = Arc::clone(&work_receiver);
        let result_sender = result_sender.clone();
        let shared = Arc::clone(&shared);
        let activity = Arc::clone(&decode_activity);
        let preprocess_pool = preprocess_pool.clone();
        let store = store.to_owned();
        let diagnostic_root = environment.diagnostic_root.to_owned();
        let segment_resolver = environment.segment_resolver.clone();
        let trace = trace.clone();
        handles.push(thread::spawn(move || {
            loop {
                let message = receiver
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .recv();
                let Ok(Some(work)) = message else { break };
                let index = work.index;
                let session_key = match &work.work {
                    ReplayWork::Queued(source) => source.session_sha256.clone(),
                    ReplayWork::Prepared(prepared, _) => prepared.session.source_session_id.clone(),
                    ReplayWork::Active(runtime) => runtime.session_id.clone(),
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let context = ReplayStepContext {
                        store: &store,
                        diagnostic_root: &diagnostic_root,
                        shared: &shared,
                        decode_activity: &activity,
                        preprocess_pool: &preprocess_pool,
                        outstanding_limit: per_session_pending_limit,
                        segment_resolver: &segment_resolver,
                        trace: trace.as_ref(),
                    };
                    execute_replay_step(&context, work)
                }))
                .unwrap_or_else(|_| {
                    Err(CorpusError::InvalidReplay(
                        "canonical replay worker panicked".to_owned(),
                    ))
                });
                if result_sender
                    .send(ReplayWorkerResult::Step {
                        index,
                        session_key,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
    let mut finalizer_handles = Vec::with_capacity(decode_workers);
    for _ in 0..decode_workers {
        let receiver = Arc::clone(&finalize_receiver);
        let result_sender = result_sender.clone();
        finalizer_handles.push(thread::spawn(move || {
            loop {
                let message = receiver
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .recv();
                let Ok(Some((index, session_key, runtime))) = message else {
                    break;
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    finalize_replay_session(runtime)
                }))
                .unwrap_or_else(|_| {
                    Err(CorpusError::InvalidReplay(
                        "canonical replay finalizer panicked".to_owned(),
                    ))
                });
                if result_sender
                    .send(ReplayWorkerResult::Finalized {
                        index,
                        session_key,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
    drop(result_sender);
    let mut inflight = 0usize;
    let mut active_sessions = ready.len();
    let mut maximum_blocked_sessions = 0usize;
    let mut completed = bootstrap_failures.len();
    let mut results = Vec::with_capacity(suite.entries.len());
    results.extend(
        bootstrap_failures
            .drain(..)
            .map(|(index, key, error)| (index, key, Err(error))),
    );
    while completed < suite.entries.len() {
        while inflight < decode_workers {
            let Some(work) = ready.pop_front() else { break };
            work_sender.send(Some(work)).map_err(|_| {
                CorpusError::InvalidReplay("canonical replay scheduler stopped".to_owned())
            })?;
            inflight = inflight.saturating_add(1);
        }
        maximum_blocked_sessions = maximum_blocked_sessions.max(
            queued
                .len()
                .saturating_add(ready.len())
                .saturating_add(inflight.saturating_sub(decode_workers)),
        );
        let worker_result = result_receiver.recv().map_err(|_| {
            CorpusError::InvalidReplay("canonical replay worker stopped".to_owned())
        })?;
        match worker_result {
            ReplayWorkerResult::Step {
                index,
                session_key,
                result,
            } => {
                inflight = inflight.saturating_sub(1);
                match result {
                    Ok(ReplayStep::Continue(runtime)) => ready.push_back(ScheduledReplayWork {
                        index,
                        queued_at: Instant::now(),
                        work: ReplayWork::Active(runtime),
                    }),
                    Ok(ReplayStep::Finalize(runtime)) => finalize_sender
                        .send(Some((index, session_key, *runtime)))
                        .map_err(|_| {
                            CorpusError::InvalidReplay(
                                "canonical replay finalizer stopped".to_owned(),
                            )
                        })?,
                    Err(error) => {
                        results.push((index, session_key, Err(error)));
                        active_sessions = active_sessions.saturating_sub(1);
                        completed = completed.saturating_add(1);
                    }
                }
            }
            ReplayWorkerResult::Finalized {
                index,
                session_key,
                result,
            } => {
                results.push((index, session_key, result));
                active_sessions = active_sessions.saturating_sub(1);
                completed = completed.saturating_add(1);
            }
        }
        while active_sessions < maximum_active_sessions {
            let Some(mut source) = queued.pop_front() else {
                break;
            };
            source.memory_wait_us = source.memory_wait_us.saturating_add(
                u64::try_from(source.memory_wait_started.elapsed().as_micros()).unwrap_or(u64::MAX),
            );
            ready.push_back(ScheduledReplayWork {
                index: source.index,
                queued_at: Instant::now(),
                work: ReplayWork::Queued(source),
            });
            active_sessions = active_sessions.saturating_add(1);
        }
    }
    for _ in 0..decode_workers {
        let _ = work_sender.send(None);
        let _ = finalize_sender.send(None);
    }
    drop(work_sender);
    for handle in handles {
        if handle.join().is_err() {
            results.push((
                usize::MAX,
                "worker".to_owned(),
                Err(CorpusError::InvalidReplay(
                    "canonical replay worker panicked".to_owned(),
                )),
            ));
        }
    }
    drop(finalize_sender);
    for handle in finalizer_handles {
        if handle.join().is_err() {
            results.push((
                usize::MAX,
                "finalizer".to_owned(),
                Err(CorpusError::InvalidReplay(
                    "canonical replay finalizer panicked".to_owned(),
                )),
            ));
        }
    }
    results.sort_by_key(|(index, _, _)| *index);
    let mut measurements = ReplayMeasurements::default();
    let mut episode_count = 0usize;
    let mut canonical_frames = 0usize;
    let mut negative_frames = 0usize;
    let mut failures = Vec::new();
    let mut sessions = Vec::with_capacity(suite.entries.len());
    let mut decoder_slot_wait_us = 0_u64;
    let mut memory_wait_us = 0_u64;
    for (index, session_key, result) in results {
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                failures.push(format!("session[{index}] {session_key}: {error}"));
                continue;
            }
        };
        episode_count = episode_count.saturating_add(outcome.episode_count);
        canonical_frames = canonical_frames.saturating_add(outcome.canonical_frames);
        negative_frames = negative_frames.saturating_add(outcome.negative_frames);
        measurements.text_batch_wall_us = measurements
            .text_batch_wall_us
            .saturating_add(outcome.measurements.text_batch_wall_us);
        measurements.field_queue_wait_us = measurements
            .field_queue_wait_us
            .saturating_add(outcome.measurements.field_queue_wait_us);
        measurements.maximum_text_worker_inference_us = measurements
            .maximum_text_worker_inference_us
            .max(outcome.measurements.maximum_text_worker_inference_us);
        measurements.numeric_inference_us = measurements
            .numeric_inference_us
            .saturating_add(outcome.measurements.numeric_inference_us);
        measurements.field_join_us = measurements
            .field_join_us
            .saturating_add(outcome.measurements.field_join_us);
        measurements.catalog_projection_us = measurements
            .catalog_projection_us
            .saturating_add(outcome.measurements.catalog_projection_us);
        measurements.decode_consumer_wait_us = measurements
            .decode_consumer_wait_us
            .saturating_add(outcome.measurements.decode_consumer_wait_us);
        measurements.preprocess_queue_wait_us = measurements
            .preprocess_queue_wait_us
            .saturating_add(outcome.measurements.preprocess_queue_wait_us);
        measurements.preprocess_wall_us = measurements
            .preprocess_wall_us
            .saturating_add(outcome.measurements.preprocess_wall_us);
        measurements.screen_classification_us = measurements
            .screen_classification_us
            .saturating_add(outcome.measurements.screen_classification_us);
        measurements.crop_prepare_us = measurements
            .crop_prepare_us
            .saturating_add(outcome.measurements.crop_prepare_us);
        measurements.text_worker_busy_us = measurements
            .text_worker_busy_us
            .saturating_add(outcome.measurements.text_worker_busy_us);
        measurements.field_frame_wall_us = measurements
            .field_frame_wall_us
            .saturating_add(outcome.measurements.field_frame_wall_us);
        measurements.ordered_commit_wait_us = measurements
            .ordered_commit_wait_us
            .saturating_add(outcome.measurements.ordered_commit_wait_us);
        failures.extend(
            outcome
                .failures
                .iter()
                .map(|failure| format!("{}: {failure}", outcome.session_key)),
        );
        decoder_slot_wait_us = decoder_slot_wait_us.saturating_add(outcome.decoder_slot_wait_us);
        memory_wait_us = memory_wait_us.saturating_add(outcome.memory_wait_us);
        sessions.push(CorpusReplaySessionSummary {
            trace: outcome.trace,
            session_key: outcome.session_key,
            music_select_best_snapshots: outcome.music_select_best_snapshots,
            wall_us: outcome.wall_us,
            canonical_frames: outcome.canonical_frames,
        });
    }
    if !failures.is_empty() {
        return Err(CorpusError::InvalidReplay(failures.join("; ")));
    }
    let mut decoder_details = decode_activity
        .decoder_details
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    decoder_details.sort_by_key(|decoder| decoder.decoder_id);
    Ok(CorpusReplaySummary {
        schema: "scorepeek-private-corpus-replay-v4",
        generation_sha256,
        session_count: suite.entries.len(),
        episode_count,
        canonical_frames,
        negative_frames,
        text_workers,
        preprocess_workers,
        decode_workers,
        maximum_active_sessions,
        maximum_concurrent_decoders: decode_activity.maximum_active.load(Ordering::Acquire),
        decoder_children: decode_activity.children.load(Ordering::Acquire),
        maximum_blocked_sessions,
        completed_sessions: suite.entries.len(),
        memory_limit_bytes: u64::try_from(memory_limit_bytes).unwrap_or(u64::MAX),
        tracked_memory_peak_bytes: decode_activity.tracked_peak_bytes.load(Ordering::Acquire),
        process_rss_peak_bytes: decode_activity
            .process_rss_peak_bytes
            .load(Ordering::Acquire),
        ffmpeg_rss_peak_total_bytes: decode_activity
            .ffmpeg_rss_peak_total_bytes
            .load(Ordering::Acquire),
        decoder_details,
        decode_consumer_wait_us: measurements.decode_consumer_wait_us,
        preprocess_queue_wait_us: measurements.preprocess_queue_wait_us,
        preprocess_wall_us: measurements.preprocess_wall_us,
        screen_classification_us: measurements.screen_classification_us,
        crop_prepare_us: measurements.crop_prepare_us,
        field_queue_wait_us: measurements.field_queue_wait_us,
        text_batch_wall_us: measurements.text_batch_wall_us,
        maximum_text_worker_inference_us: measurements.maximum_text_worker_inference_us,
        text_worker_busy_us: measurements.text_worker_busy_us,
        numeric_inference_us: measurements.numeric_inference_us,
        field_join_us: measurements.field_join_us,
        catalog_projection_us: measurements.catalog_projection_us,
        field_frame_wall_us: measurements.field_frame_wall_us,
        ordered_commit_wait_us: measurements.ordered_commit_wait_us,
        decoder_slot_wait_us,
        memory_wait_us,
        sessions,
        corpus_wall_us: u64::try_from(replay_started.elapsed().as_micros()).unwrap_or(u64::MAX),
        local_segment_decodes: environment
            .segment_resolver
            .local_segment_decodes
            .load(Ordering::Acquire),
        remote_segment_downloads: environment
            .segment_resolver
            .remote
            .as_ref()
            .map_or(0, |remote| remote.metrics().downloaded_segments),
        remote_downloaded_bytes: environment
            .segment_resolver
            .remote
            .as_ref()
            .map_or(0, |remote| remote.metrics().downloaded_bytes),
    })
}

pub(super) fn replay_descriptor(
    session_index: usize,
    session: &CaptureSession,
    binding: &SessionBinding,
) -> scorepeek_core::diagnostics::DiagnosticRunDescriptor {
    scorepeek_core::diagnostics::DiagnosticRunDescriptor {
        run_id: format!("canonical-corpus-replay-{session_index}"),
        monotonic_start_ms: 0,
        resource: scorepeek_core::diagnostics::DiagnosticResource {
            program: "scorepeek",
            version: env!("CARGO_PKG_VERSION"),
            build_sha256: "0".repeat(64),
        },
        binding: scorepeek_core::diagnostics::DiagnosticBinding {
            capture_generation: session.capture_generation,
            capture_profile_sha256: binding.capture_profile_sha256.clone(),
            normalizer_sha256: binding.normalizer_sha256.clone(),
            canonical_layout_sha256: scorepeek_core::replay::CanonicalLayout::sha256(),
            catalog_sha256: session.catalog_sha256.clone(),
            model_sha256: scorepeek_core::replay::LIVE_MODEL_SHA256.to_owned(),
            runtime_sha256: scorepeek_core::replay::LIVE_RUNTIME_SHA256.to_owned(),
            replay: None,
        },
    }
}

fn load_prepared_replay_session(
    store: &Path,
    source: &QueuedReplaySession,
) -> Result<PreparedReplaySession, CorpusError> {
    let (session, session_bytes) = read_json::<CaptureSession>(
        &store
            .join("sessions")
            .join(format!("{}.json", source.session_sha256)),
    )?;
    let (label, label_bytes) = read_regression_label(
        &store
            .join("labels")
            .join(format!("{}.json", source.label_sha256)),
    )?;
    if session.schema != SESSION_SCHEMA
        || session.completeness != "complete"
        || digest(&session_bytes) != source.session_sha256
        || digest(&label_bytes) != source.label_sha256
        || label.session_sha256 != source.session_sha256
    {
        return invalid("canonical suite entry binding is invalid");
    }
    let binding = session_binding(store, &session)?;
    Ok(PreparedReplaySession {
        index: source.index,
        session,
        label,
        binding,
    })
}

pub(super) fn start_replay_observer(
    stream_observer: scorepeek_runtime::diagnostics::inspect::DiagnosticObserver,
    index: usize,
    trace: Option<&Arc<Mutex<ReplayTrace>>>,
    #[cfg(test)] fail_spawn: bool,
) -> Result<JoinHandle<Result<ReplayObserved, String>>, String> {
    #[cfg(test)]
    if fail_spawn {
        finish_abandoned_trace_session(trace, index)?;
        return Err("replay diagnostic observer could not be started: injected failure".to_owned());
    }
    let observer_trace = trace.cloned();
    match thread::Builder::new()
        .name(format!("scorepeek-replay-observer-{index}"))
        .spawn(move || {
            let mut collected = ReplayObserved::default();
            let stream = stream_observer.consume(|record| {
                if record["operation"] == "diagnostic_record_rejected"
                    && record["data"]["source_operation"] == "run_event"
                    && record["data"]["error_type"] == "record_too_large"
                {
                    return Err("diagnostic run event exceeded the stream record bound".to_owned());
                }
                if record["operation"] != "run_event" {
                    return Ok(());
                }
                let data = record
                    .get("data")
                    .ok_or_else(|| "diagnostic run event has no data".to_owned())?;
                let event = scorepeek_core::event::RunEvent::from_value(data.clone())?;
                if matches!(
                    event.kind,
                    scorepeek_core::event::RunEventKind::FieldObservation { .. }
                ) {
                    return Ok(());
                }
                if let Some(trace) = &observer_trace {
                    trace
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .observe(index, &event)?;
                }
                match event.kind {
                    scorepeek_core::event::RunEventKind::MusicSelectionChanged {
                        source_sequence,
                        state,
                        ..
                    } => collected.music_selections.push((source_sequence, state)),
                    scorepeek_core::event::RunEventKind::ResultChanged {
                        state: scorepeek_core::event::ResultState::Confirmed { result, .. },
                        ..
                    } => collected.confirmed_results.push(*result),
                    scorepeek_core::event::RunEventKind::MusicSelectBestObserved { .. } => {
                        collected.music_select_best_snapshots =
                            collected.music_select_best_snapshots.saturating_add(1);
                    }
                    _ => {}
                }
                Ok(())
            });
            if let Some(trace) = observer_trace {
                let finisher = trace
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .finish_session(index)?;
                collected.trace = Some(finisher.finish());
            }
            stream?;
            Ok(collected)
        }) {
        Ok(observer) => Ok(observer),
        Err(error) => {
            finish_abandoned_trace_session(trace, index)?;
            Err(format!(
                "replay diagnostic observer could not be started: {error}"
            ))
        }
    }
}

fn finish_abandoned_trace_session(
    trace: Option<&Arc<Mutex<ReplayTrace>>>,
    index: usize,
) -> Result<(), String> {
    if let Some(trace) = trace {
        let finisher = trace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .finish_session(index)?;
        let _ = finisher.finish();
    }
    Ok(())
}

fn start_replay_session(
    store: &Path,
    diagnostic_root: &Path,
    prepared: PreparedReplaySession,
    shared: Arc<RuntimeReplaySharedResources>,
    decode_activity: &Arc<ReplayDecodeActivity>,
    memory_wait_us: u64,
    trace: Option<&Arc<Mutex<ReplayTrace>>>,
) -> Result<ReplaySessionRuntime, CorpusError> {
    let PreparedReplaySession {
        index: session_index,
        session,
        label,
        binding,
    } = prepared;
    let session_id = session.source_session_id.clone();
    let canonical_manifest_object =
        session_object_for_source(store, &session, "recognition/canonical-manifest.json")?;
    let (canonical, _) = read_json::<CanonicalRecordingManifest>(&canonical_manifest_object)?;
    if canonical.completeness != "complete" || canonical.dropped_frames != 0 {
        return invalid_replay("canonical session is incomplete");
    }
    let tick_object =
        session_object_for_source(store, &session, "recognition/canonical-ticks.ndjson")?;
    let retained = read_canonical_ticks(&tick_object)?
        .into_iter()
        .filter(|tick| tick.disposition == "retained")
        .collect::<Vec<_>>();
    let run_id = format!("corpus-{session_index}");
    let runtime_root = diagnostic_root.join(format!("runtime-{session_index}"));
    let diagnostics = scorepeek_runtime::diagnostics::inspect::RunDiagnostics::start_ephemeral_at(
        &runtime_root,
        &run_id,
    );
    let observer = scorepeek_runtime::diagnostics::inspect::DiagnosticObserver::connect_at(
        &runtime_root,
        None,
    )
    .map_err(CorpusError::InvalidReplay)?;
    if let Some(trace) = trace {
        trace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .start_session(session_index, &session_id)
            .map_err(CorpusError::InvalidReplay)?;
    }
    let observer = start_replay_observer(
        observer,
        session_index,
        trace,
        #[cfg(test)]
        false,
    )
    .map_err(CorpusError::InvalidReplay)?;
    let output = scorepeek_runtime::events::server::RoutineOutput::start_headless_with_diagnostics(
        format!("corpus-{session_index}"),
        session.profile_sha256.clone(),
        diagnostics,
    );
    let descriptor = replay_descriptor(session_index, &session, &binding);
    let mut runtime = ReplaySessionRuntime {
        session,
        label,
        binding,
        recognition: None,
        event_stream: ReplayEventStream {
            output,
            observer: Some(observer),
        },
        timeline: scorepeek_core::replay::TimelineDriver::default(),
        pending: VecDeque::new(),
        measurements: ReplayMeasurements::default(),
        failures: Vec::new(),
        canonical,
        retained,
        segment_index: 0,
        prefetched_segments: VecDeque::new(),
        retained_offset: 0,
        canonical_frames: 0,
        last_sequence: 0,
        last_monotonic_ms: 0,
        session_id,
        session_started: Instant::now(),
        decoder_slot_wait_us: 0,
        memory_wait_us,
        _memory: decode_activity.reserve_session(),
    };
    runtime
        .event_stream
        .output
        .publish(&scorepeek_core::event::RunEvent {
            schema: scorepeek_core::event::RUN_EVENT_SCHEMA.to_owned(),
            kind: scorepeek_core::event::RunEventKind::SessionStarted {
                session_id: Some(runtime.session_id.clone()),
                capture_generation: runtime.session.capture_generation,
                capture_profile_sha256: runtime.binding.capture_profile_sha256.clone(),
                normalizer_artifact_sha256: runtime.binding.normalizer_sha256.clone(),
            },
        })
        .map_err(CorpusError::InvalidReplay)?;
    let recognition = ReplayRecognitionSession::start_registered_shared(
        diagnostic_root,
        descriptor,
        scorepeek_core::diagnostics::DiagnosticPolicy {
            enabled: false,
            ..scorepeek_core::diagnostics::DiagnosticPolicy::default()
        },
        &shared,
    )
    .map_err(|error| {
        CorpusError::InvalidReplay(format!("production recognizer could not start: {error:?}"))
    })?;
    runtime.recognition = Some(recognition);
    Ok(runtime)
}

fn execute_replay_step(
    context: &ReplayStepContext<'_>,
    scheduled: ScheduledReplayWork,
) -> Result<ReplayStep, CorpusError> {
    let slot_wait_us = u64::try_from(scheduled.queued_at.elapsed().as_micros()).unwrap_or(u64::MAX);
    match scheduled.work {
        ReplayWork::Queued(source) => {
            let prepared = load_prepared_replay_session(context.store, &source)?;
            let shared =
                context
                    .shared
                    .for_session(prepared.index, &prepared.session, &prepared.binding)?;
            let mut runtime = start_replay_session(
                context.store,
                context.diagnostic_root,
                prepared,
                shared,
                context.decode_activity,
                source.memory_wait_us,
                context.trace,
            )?;
            runtime.decoder_slot_wait_us =
                runtime.decoder_slot_wait_us.saturating_add(slot_wait_us);
            process_replay_segment(
                context.store,
                &mut runtime,
                context.decode_activity,
                context.preprocess_pool,
                context.outstanding_limit,
                context.segment_resolver,
            )?;
            Ok(
                if runtime.segment_index == runtime.canonical.segments.len() {
                    ReplayStep::Finalize(Box::new(runtime))
                } else {
                    ReplayStep::Continue(Box::new(runtime))
                },
            )
        }
        ReplayWork::Prepared(prepared, memory_wait_us) => {
            let shared =
                context
                    .shared
                    .for_session(prepared.index, &prepared.session, &prepared.binding)?;
            let mut runtime = start_replay_session(
                context.store,
                context.diagnostic_root,
                *prepared,
                shared,
                context.decode_activity,
                memory_wait_us,
                context.trace,
            )?;
            runtime.decoder_slot_wait_us =
                runtime.decoder_slot_wait_us.saturating_add(slot_wait_us);
            process_replay_segment(
                context.store,
                &mut runtime,
                context.decode_activity,
                context.preprocess_pool,
                context.outstanding_limit,
                context.segment_resolver,
            )?;
            Ok(
                if runtime.segment_index == runtime.canonical.segments.len() {
                    ReplayStep::Finalize(Box::new(runtime))
                } else {
                    ReplayStep::Continue(Box::new(runtime))
                },
            )
        }
        ReplayWork::Active(mut runtime) => {
            runtime.decoder_slot_wait_us =
                runtime.decoder_slot_wait_us.saturating_add(slot_wait_us);
            process_replay_segment(
                context.store,
                &mut runtime,
                context.decode_activity,
                context.preprocess_pool,
                context.outstanding_limit,
                context.segment_resolver,
            )?;
            Ok(
                if runtime.segment_index == runtime.canonical.segments.len() {
                    ReplayStep::Finalize(runtime)
                } else {
                    ReplayStep::Continue(runtime)
                },
            )
        }
    }
}

fn process_replay_segment(
    store: &Path,
    runtime: &mut ReplaySessionRuntime,
    decode_activity: &Arc<ReplayDecodeActivity>,
    preprocess_pool: &ReplayPreprocessPool,
    outstanding_limit: usize,
    segment_resolver: &SegmentResolver,
) -> Result<(), CorpusError> {
    let segment = runtime
        .canonical
        .segments
        .get(runtime.segment_index)
        .cloned()
        .ok_or_else(|| CorpusError::InvalidReplay("canonical segment is unavailable".to_owned()))?;
    let expected = runtime
        .retained
        .get(runtime.retained_offset..runtime.retained_offset.saturating_add(segment.frames))
        .ok_or_else(|| {
            CorpusError::InvalidReplay("canonical segment exceeds retained tick index".to_owned())
        })?
        .to_vec();
    let object = match runtime.prefetched_segments.pop_front() {
        Some(prefetched) if prefetched.segment_index == runtime.segment_index => {
            prefetched.finish()?
        }
        Some(_) => return invalid_replay("canonical segment prefetch order differs"),
        None => segment_resolver.resolve(
            store,
            &runtime.session,
            &format!("recognition/{}", segment.path),
        )?,
    };
    fill_replay_segment_prefetch(store, runtime, segment_resolver);
    let mut preprocessing = VecDeque::new();
    let decoded_digest = decode_resolved_canonical_frames(
        &object,
        segment.frames,
        DecodeContext::Replay,
        Some(decode_activity),
        |index, pixels, decode_consumer_wait_us| {
            runtime.measurements.decode_consumer_wait_us = runtime
                .measurements
                .decode_consumer_wait_us
                .saturating_add(decode_consumer_wait_us);
            let tick = expected.get(index).ok_or_else(|| {
                CorpusError::InvalidReplay("canonical decoded frame exceeds tick index".to_owned())
            })?;
            preprocessing.push_back(preprocess_pool.submit(
                tick.clone(),
                pixels,
                decode_activity.reserve_pending(),
            )?);
            while preprocessing.len() >= outstanding_limit {
                commit_replay_preprocessed(runtime, &mut preprocessing, true, outstanding_limit)?;
            }
            commit_replay_preprocessed(runtime, &mut preprocessing, false, outstanding_limit)
        },
    )?;
    while !preprocessing.is_empty() {
        commit_replay_preprocessed(runtime, &mut preprocessing, true, outstanding_limit)?;
    }
    if let Some(expected) = &segment.raw_rgb24_sha256
        && decoded_digest != *expected
    {
        return invalid_replay("canonical segment decoded pixel digest differs");
    }
    runtime.retained_offset = runtime.retained_offset.saturating_add(segment.frames);
    runtime.segment_index = runtime.segment_index.saturating_add(1);
    if runtime.segment_index == runtime.canonical.segments.len()
        && runtime.retained_offset != runtime.retained.len()
    {
        return invalid_replay("canonical segment coverage differs");
    }
    Ok(())
}

fn fill_replay_segment_prefetch(
    store: &Path,
    runtime: &mut ReplaySessionRuntime,
    segment_resolver: &SegmentResolver,
) {
    let mut next_segment_index = runtime.prefetched_segments.back().map_or_else(
        || runtime.segment_index.saturating_add(1),
        |item| item.segment_index.saturating_add(1),
    );
    while runtime.prefetched_segments.len() < REPLAY_SEGMENT_PREFETCH {
        let Some(next_segment) = runtime.canonical.segments.get(next_segment_index) else {
            break;
        };
        runtime
            .prefetched_segments
            .push_back(PrefetchedReplaySegment::start(
                next_segment_index,
                store.to_owned(),
                runtime.session.clone(),
                format!("recognition/{}", next_segment.path),
                segment_resolver.clone(),
            ));
        next_segment_index = next_segment_index.saturating_add(1);
    }
}

fn commit_replay_preprocessed(
    runtime: &mut ReplaySessionRuntime,
    preprocessing: &mut VecDeque<PendingReplayPreprocess>,
    wait: bool,
    outstanding_limit: usize,
) -> Result<(), CorpusError> {
    let Some(front) = preprocessing.front() else {
        return Ok(());
    };
    let prepared = if wait {
        front
            .receiver
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| {
                CorpusError::InvalidReplay("canonical replay preprocessing timed out".to_owned())
            })?
    } else {
        match front.receiver.try_recv() {
            Ok(prepared) => prepared,
            Err(mpsc::TryRecvError::Empty) => return Ok(()),
            Err(mpsc::TryRecvError::Disconnected) => {
                return invalid_replay("canonical replay preprocessing stopped");
            }
        }
    };
    let pending = preprocessing
        .pop_front()
        .expect("prepared replay queue has a front");
    let prepared = prepared.map_err(CorpusError::InvalidReplay)?;
    process_replay_frame(runtime, &pending.tick, prepared, outstanding_limit)
}

fn process_replay_frame(
    runtime: &mut ReplaySessionRuntime,
    tick: &CanonicalTick,
    prepared: PreparedReplayFrame,
    outstanding_limit: usize,
) -> Result<(), CorpusError> {
    while runtime.pending.len() >= outstanding_limit {
        commit_replay_pending(
            runtime.recognition.as_mut().expect("recognizer is active"),
            &mut runtime.pending,
            &mut runtime.event_stream.output,
            true,
            &runtime.session_id,
            runtime.session.capture_generation,
            &mut runtime.measurements,
        )?;
    }
    commit_replay_pending(
        runtime.recognition.as_mut().expect("recognizer is active"),
        &mut runtime.pending,
        &mut runtime.event_stream.output,
        false,
        &runtime.session_id,
        runtime.session.capture_generation,
        &mut runtime.measurements,
    )?;
    let PreparedReplayFrame {
        pixels,
        recognition: prepared_recognition,
        memory,
        queue_wait_us,
        wall_us,
    } = prepared;
    runtime.measurements.preprocess_queue_wait_us = runtime
        .measurements
        .preprocess_queue_wait_us
        .saturating_add(queue_wait_us);
    runtime.measurements.preprocess_wall_us = runtime
        .measurements
        .preprocess_wall_us
        .saturating_add(wall_us);
    runtime.measurements.screen_classification_us = runtime
        .measurements
        .screen_classification_us
        .saturating_add(prepared_recognition.screen_classification_us());
    runtime.measurements.crop_prepare_us = runtime
        .measurements
        .crop_prepare_us
        .saturating_add(prepared_recognition.crop_prepare_us().unwrap_or(0));
    let frame = scorepeek_runtime::diagnostics::live::BoundCanonicalFrame::for_replay(
        runtime.session.capture_generation,
        tick.sequence,
        tick.monotonic_ms,
        runtime.binding.capture_profile_sha256.clone(),
        runtime.binding.normalizer_sha256.clone(),
        pixels,
    )
    .map_err(|_| CorpusError::InvalidReplay("canonical replay frame is invalid".to_owned()))?;
    let inspected = runtime
        .recognition
        .as_mut()
        .expect("recognizer is active")
        .inspect_prepared(&frame, prepared_recognition)
        .map_err(|_| CorpusError::InvalidReplay("production frame inspection failed".to_owned()))?;
    let screen = inspected.screen();
    for episode in &runtime.label.episodes {
        if episode
            .attempt
            .as_ref()
            .and_then(|attempt| attempt.play_span)
            .is_some_and(|span| {
                tick.sequence == span.first_sequence || tick.sequence == span.last_sequence
            })
            && screen != ScreenClass::Play
        {
            runtime.failures.push(format!(
                "episode {} PLAY endpoint {} classified as {screen:?}",
                episode.episode_id, tick.sequence,
            ));
        }
    }
    let timeline_step = runtime
        .timeline
        .observe(screen.into(), tick.sequence, tick.monotonic_ms);
    runtime
        .event_stream
        .output
        .publish(&scorepeek_core::event::RunEvent {
            schema: scorepeek_core::event::RUN_EVENT_SCHEMA.to_owned(),
            kind: scorepeek_core::event::RunEventKind::RawScreenObserved {
                session_id: Some(runtime.session_id.clone()),
                capture_generation: Some(runtime.session.capture_generation),
                semantic_episode_id: timeline_step.active_episode_id,
                sequence: tick.sequence,
                monotonic_start_ms: tick.monotonic_ms,
                monotonic_end_ms: tick.monotonic_ms,
                screen: replay_screen_name(screen).to_owned(),
                result_presence: inspected.result_presence(),
                play_presence: inspected.play_presence(),
                unknown_reason: (screen == ScreenClass::Unknown)
                    .then(|| "predicate_not_matched".to_owned()),
            },
        })
        .map_err(CorpusError::InvalidReplay)?;
    apply_replay_timeline_actions(
        timeline_step.actions,
        runtime.recognition.as_mut().expect("recognizer is active"),
        &mut runtime.pending,
        &mut runtime.event_stream.output,
        &runtime.session_id,
        runtime.session.capture_generation,
        tick.sequence,
        tick.monotonic_ms,
        &mut runtime.measurements,
    )?;
    match inspected.field_submission {
        ReplayFieldSubmission::NotApplicable => {}
        ReplayFieldSubmission::Submitted(mut field) => {
            let episode_id = runtime.timeline.active_episode_id().ok_or_else(|| {
                CorpusError::InvalidReplay("field observation has no semantic episode".to_owned())
            })?;
            field.bind_screen_episode(episode_id);
            runtime.pending.push_back(ReplayPending {
                pending: field,
                _memory: memory,
            });
        }
        ReplayFieldSubmission::BusySkipped => {
            return invalid_replay("offline replay skipped field OCR as busy");
        }
        ReplayFieldSubmission::Rejected(error) => {
            return Err(CorpusError::InvalidReplay(format!(
                "offline replay rejected field OCR: {error:?}"
            )));
        }
    }
    runtime.canonical_frames = runtime.canonical_frames.saturating_add(1);
    runtime.last_sequence = tick.sequence;
    runtime.last_monotonic_ms = tick.monotonic_ms;
    Ok(())
}

fn finalize_replay_session(
    mut runtime: ReplaySessionRuntime,
) -> Result<ReplaySessionOutcome, CorpusError> {
    let finish_actions = runtime.timeline.finish();
    if finish_actions.is_empty() {
        drain_replay_pending(
            runtime.recognition.as_mut().expect("recognizer is active"),
            &mut runtime.pending,
            &mut runtime.event_stream.output,
            &runtime.session_id,
            runtime.session.capture_generation,
            &mut runtime.measurements,
        )?;
    } else {
        apply_replay_timeline_actions(
            finish_actions,
            runtime.recognition.as_mut().expect("recognizer is active"),
            &mut runtime.pending,
            &mut runtime.event_stream.output,
            &runtime.session_id,
            runtime.session.capture_generation,
            runtime.last_sequence,
            runtime.last_monotonic_ms,
            &mut runtime.measurements,
        )?;
    }
    runtime
        .event_stream
        .output
        .publish(&scorepeek_core::event::RunEvent {
            schema: scorepeek_core::event::RUN_EVENT_SCHEMA.to_owned(),
            kind: scorepeek_core::event::RunEventKind::SessionFinished {
                session_id: runtime.session_id.clone(),
                capture_generation: runtime.session.capture_generation,
                outcome: "replayed".to_owned(),
                report: serde_json::json!({}),
            },
        })
        .map_err(CorpusError::InvalidReplay)?;
    let recognition = runtime.recognition.take().expect("recognizer is active");
    let finish = recognition.finish(
        scorepeek_core::diagnostics::DiagnosticRunStatus::Success,
        runtime.last_monotonic_ms,
        Duration::from_secs(30),
    );
    let observed = runtime
        .event_stream
        .finish("success")
        .map_err(CorpusError::InvalidReplay)?;
    if !finish.field_observer_complete {
        return invalid_replay("production recognizer did not finish cleanly");
    }
    validate_music_selection_oracle(
        &runtime.label,
        &observed.music_selections,
        &mut runtime.failures,
    );
    validate_semantic_oracle(
        &runtime.label,
        &observed.confirmed_results,
        &mut runtime.failures,
    );
    Ok(ReplaySessionOutcome {
        trace: observed.trace.map(Box::new),
        session_key: runtime.session_id.clone(),
        music_select_best_snapshots: observed.music_select_best_snapshots,
        episode_count: runtime.label.episodes.len(),
        canonical_frames: runtime.canonical_frames,
        negative_frames: runtime.label.negative_frames.len(),
        measurements: std::mem::take(&mut runtime.measurements),
        failures: std::mem::take(&mut runtime.failures),
        wall_us: u64::try_from(runtime.session_started.elapsed().as_micros()).unwrap_or(u64::MAX),
        decoder_slot_wait_us: runtime.decoder_slot_wait_us,
        memory_wait_us: runtime.memory_wait_us,
    })
}

#[allow(
    clippy::too_many_arguments,
    reason = "the replay adapter executes one shared timeline action against bound session state"
)]
fn apply_replay_timeline_actions(
    actions: Vec<scorepeek_core::replay::TimelineAction>,
    recognition: &mut ReplayRecognitionSession,
    pending: &mut VecDeque<ReplayPending>,
    output: &mut scorepeek_runtime::events::server::RoutineOutput,
    session_id: &str,
    generation: u64,
    sequence: u64,
    monotonic_ms: u64,
    measurements: &mut ReplayMeasurements,
) -> Result<(), CorpusError> {
    for action in actions {
        match action {
            scorepeek_core::replay::TimelineAction::Semantic { episode, phase } => {
                publish_replay_semantic(
                    output,
                    session_id,
                    generation,
                    episode,
                    sequence,
                    monotonic_ms,
                    phase,
                )?;
            }
            scorepeek_core::replay::TimelineAction::DrainAdmitted { .. } => {
                drain_replay_pending(
                    recognition,
                    pending,
                    output,
                    session_id,
                    generation,
                    measurements,
                )?;
            }
        }
    }
    Ok(())
}

fn publish_replay_semantic(
    output: &mut scorepeek_runtime::events::server::RoutineOutput,
    session_id: &str,
    generation: u64,
    episode: scorepeek_core::replay::SemanticScreenEpisode,
    sequence: u64,
    monotonic_ms: u64,
    phase: scorepeek_core::session::timeline::SemanticEpisodePhase,
) -> Result<(), CorpusError> {
    output
        .publish(&scorepeek_core::event::RunEvent {
            schema: scorepeek_core::event::RUN_EVENT_SCHEMA.to_owned(),
            kind: scorepeek_core::event::RunEventKind::SemanticScreenEpisodeChanged {
                session_id: Some(session_id.to_owned()),
                capture_generation: Some(generation),
                screen_episode_id: episode.id,
                sequence,
                monotonic_end_ms: monotonic_ms,
                screen: replay_screen_name(episode.screen).to_owned(),
                phase,
            },
        })
        .map_err(CorpusError::InvalidReplay)
}

fn drain_replay_pending(
    recognition: &mut ReplayRecognitionSession,
    pending: &mut VecDeque<ReplayPending>,
    output: &mut scorepeek_runtime::events::server::RoutineOutput,
    session_id: &str,
    generation: u64,
    measurements: &mut ReplayMeasurements,
) -> Result<(), CorpusError> {
    while !pending.is_empty() {
        commit_replay_pending(
            recognition,
            pending,
            output,
            true,
            session_id,
            generation,
            measurements,
        )?;
    }
    Ok(())
}

fn commit_replay_pending(
    recognition: &mut ReplayRecognitionSession,
    pending: &mut VecDeque<ReplayPending>,
    output: &mut scorepeek_runtime::events::server::RoutineOutput,
    wait: bool,
    session_id: &str,
    generation: u64,
    measurements: &mut ReplayMeasurements,
) -> Result<(), CorpusError> {
    let Some(front) = pending.front() else {
        return Ok(());
    };
    let wait_started = Instant::now();
    let poll = if wait {
        recognition.wait_field_observation(&front.pending, Duration::from_secs(30))
    } else {
        recognition.poll_field_observation(&front.pending)
    };
    if wait {
        measurements.ordered_commit_wait_us = measurements
            .ordered_commit_wait_us
            .saturating_add(u64::try_from(wait_started.elapsed().as_micros()).unwrap_or(u64::MAX));
    }
    let ReplayFieldPoll::Ready {
        observation,
        frame_processing_wall_us,
        screen_episode_id,
    } = poll
    else {
        return if !wait && matches!(poll, ReplayFieldPoll::Pending) {
            Ok(())
        } else {
            invalid_replay("production OCR did not complete in sequence order")
        };
    };
    pending.pop_front();
    let sequence = observation.sequence();
    let monotonic_start_ms = observation.monotonic_start_ms();
    let monotonic_end_ms = observation.monotonic_end_ms();
    let observation = observation
        .into_output()
        .map_err(|error| CorpusError::InvalidReplay(format!("production OCR failed: {error}")))?;
    let processing = observation.processing_timing();
    measurements.field_queue_wait_us = measurements
        .field_queue_wait_us
        .saturating_add(processing.field_queue_wait_us);
    measurements.text_batch_wall_us = measurements
        .text_batch_wall_us
        .saturating_add(processing.text_batch_wall_us);
    measurements.maximum_text_worker_inference_us = measurements
        .maximum_text_worker_inference_us
        .max(processing.maximum_text_worker_inference_us);
    measurements.text_worker_busy_us = measurements
        .text_worker_busy_us
        .saturating_add(processing.text_worker_busy_us);
    measurements.numeric_inference_us = measurements
        .numeric_inference_us
        .saturating_add(processing.numeric_recognition_us.unwrap_or(0));
    measurements.field_join_us = measurements
        .field_join_us
        .saturating_add(processing.join_us);
    measurements.catalog_projection_us = measurements
        .catalog_projection_us
        .saturating_add(processing.catalog_evidence_us);
    measurements.field_frame_wall_us = measurements
        .field_frame_wall_us
        .saturating_add(frame_processing_wall_us);
    output
        .publish(
            &scorepeek_core::event::run_event_from_field_observation(
                session_id,
                generation,
                screen_episode_id,
                sequence,
                monotonic_start_ms,
                monotonic_end_ms,
                &observation,
            )
            .map_err(CorpusError::InvalidReplay)?,
        )
        .map_err(CorpusError::InvalidReplay)
}

fn replay_screen_name(screen: ScreenClass) -> &'static str {
    match screen {
        ScreenClass::Title => "title",
        ScreenClass::Result => "result",
        ScreenClass::MusicSelect => "music_select",
        ScreenClass::ModeSelect => "mode_select",
        ScreenClass::DecideTransition => "decide_transition",
        ScreenClass::Play => "play",
        ScreenClass::Unknown => "unknown",
    }
}

fn for_each_canonical_session_frame(
    store: &Path,
    session: &CaptureSession,
    observe: impl FnMut(&CanonicalTick, Box<[u8]>) -> Result<(), CorpusError> + Send,
) -> Result<(), CorpusError> {
    let resolver = SegmentResolver {
        remote: SegmentRemote::from_environment()?,
        local_segment_decodes: Arc::new(AtomicU64::new(0)),
    };
    for_each_canonical_session_frame_with_activity(store, session, None, &resolver, observe)
}

fn for_each_canonical_session_frame_with_activity(
    store: &Path,
    session: &CaptureSession,
    activity: Option<&ReplayDecodeActivity>,
    resolver: &SegmentResolver,
    mut observe: impl FnMut(&CanonicalTick, Box<[u8]>) -> Result<(), CorpusError> + Send,
) -> Result<(), CorpusError> {
    let canonical_manifest_object =
        session_object_for_source(store, session, "recognition/canonical-manifest.json")?;
    let (canonical, _) = read_json::<CanonicalRecordingManifest>(&canonical_manifest_object)?;
    if canonical.completeness != "complete" || canonical.dropped_frames != 0 {
        return invalid_replay("canonical session is incomplete");
    }
    let tick_object =
        session_object_for_source(store, session, "recognition/canonical-ticks.ndjson")?;
    let ticks = read_canonical_ticks(&tick_object)?;
    let retained = ticks
        .iter()
        .filter(|tick| tick.disposition == "retained")
        .collect::<Vec<_>>();
    let mut offset = 0usize;
    for segment in &canonical.segments {
        let object = resolver.resolve(store, session, &format!("recognition/{}", segment.path))?;
        let expected = retained
            .get(offset..offset.saturating_add(segment.frames))
            .ok_or_else(|| {
                CorpusError::InvalidReplay(
                    "canonical segment exceeds retained tick index".to_owned(),
                )
            })?;
        let decoded_digest = decode_resolved_canonical_frames(
            &object,
            segment.frames,
            DecodeContext::Replay,
            activity,
            |index, pixels, _| {
                let tick = expected.get(index).ok_or_else(|| {
                    CorpusError::InvalidReplay(
                        "canonical decoded frame exceeds tick index".to_owned(),
                    )
                })?;
                observe(tick, pixels)
            },
        )?;
        if let Some(expected) = &segment.raw_rgb24_sha256
            && decoded_digest != *expected
        {
            return invalid_replay("canonical segment decoded pixel digest differs");
        }
        offset = offset.saturating_add(segment.frames);
    }
    if offset != retained.len() {
        return invalid_replay("canonical segment coverage differs");
    }
    Ok(())
}

pub(super) fn for_each_session_canonical_frame(
    store: &Path,
    session: &CaptureSession,
    mut observe: impl FnMut(u64, Box<[u8]>) -> Result<(), CorpusError> + Send,
) -> Result<(), CorpusError> {
    if session
        .artifacts
        .iter()
        .any(|artifact| artifact.source_path == "recognition/canonical-manifest.json")
    {
        return for_each_canonical_session_frame(store, session, |tick, pixels| {
            observe(tick.sequence, pixels)
        });
    }
    for frame in &session.canonical_frames {
        let encoded = fs::read(store.join("objects").join(&frame.artifact_sha256))?;
        if digest(&encoded) != frame.artifact_sha256 {
            return invalid("canonical session frame digest differs");
        }
        let (header, pixels) = qoi::decode_to_vec(encoded)
            .map_err(|_| CorpusError::InvalidRequest("canonical session QOI is invalid".into()))?;
        if header.width != 1_920 || header.height != 1_080 || pixels.len() != 1_920 * 1_080 * 3 {
            return invalid("canonical session frame is not canonical RGB8");
        }
        observe(frame.sequence, pixels.into_boxed_slice())?;
    }
    Ok(())
}

pub(super) fn session_object_for_source(
    store: &Path,
    session: &CaptureSession,
    source_path: &str,
) -> Result<PathBuf, CorpusError> {
    let artifact = session
        .artifacts
        .iter()
        .find(|artifact| artifact.source_path == source_path)
        .ok_or_else(|| {
            CorpusError::InvalidReplay(format!(
                "canonical session artifact is unavailable: {source_path}"
            ))
        })?;
    let object = store.join("objects").join(&artifact.sha256);
    verify_file(&object, &artifact.sha256, artifact.bytes)?;
    Ok(object)
}

impl SegmentResolver {
    pub(super) fn resolve(
        &self,
        store: &Path,
        session: &CaptureSession,
        source_path: &str,
    ) -> Result<ResolvedSegment, CorpusError> {
        let artifact = session
            .artifacts
            .iter()
            .find(|artifact| artifact.source_path == source_path)
            .ok_or_else(|| {
                CorpusError::InvalidReplay(format!(
                    "canonical session artifact is unavailable: {source_path}"
                ))
            })?;
        let object = store.join("objects").join(&artifact.sha256);
        match object.symlink_metadata() {
            Ok(_) => {
                verify_file(&object, &artifact.sha256, artifact.bytes)?;
                self.local_segment_decodes.fetch_add(1, Ordering::AcqRel);
                Ok(ResolvedSegment::Local(object))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let remote = self.remote.as_ref().ok_or_else(|| {
                    CorpusError::InvalidReplay(
                        "remote remote_not_configured: canonical segment is not local".to_owned(),
                    )
                })?;
                remote
                    .materialize(&artifact.sha256, artifact.bytes)
                    .map(ResolvedSegment::Remote)
                    .map_err(|error| CorpusError::InvalidReplay(error.to_string()))
            }
            Err(error) => Err(error.into()),
        }
    }
}

fn validate_semantic_oracle(
    label: &RegressionLabel,
    emitted: &[scorepeek_core::event::ResultDomainEvent],
    failures: &mut Vec<String>,
) {
    let accepted = label
        .episodes
        .iter()
        .filter(|episode| episode_expects_result_event(episode))
        .collect::<Vec<_>>();
    if emitted.len() != accepted.len() {
        failures.push(format!(
            "session {} event count differs: expected {}, observed {}",
            label.session_sha256,
            accepted.len(),
            emitted.len()
        ));
    }
    let mut actual_by_key = BTreeMap::<String, u64>::new();
    for (index, episode) in accepted.iter().enumerate() {
        let attempt = episode.attempt.as_ref().expect("accepted attempt exists");
        let Some(event) = emitted.get(index) else {
            failures.push(format!(
                "attempt {} is missing its ordered result event",
                attempt.attempt_key
            ));
            continue;
        };
        if !result_event_matches(event, episode) {
            failures.push(format!(
                "attempt {} result payload or play-options order differs",
                attempt.attempt_key
            ));
        }
        let expected_parent = attempt
            .parent_attempt_key
            .as_ref()
            .and_then(|key| actual_by_key.get(key))
            .copied();
        if event.parent_attempt_id != expected_parent {
            failures.push(format!(
                "attempt {} parent relation differs",
                attempt.attempt_key
            ));
        }
        actual_by_key.insert(attempt.attempt_key.clone(), event.attempt_id);
    }
}

pub(super) fn episode_expects_result_event(episode: &RegressionEpisode) -> bool {
    episode
        .attempt
        .as_ref()
        .is_some_and(|attempt| matches!(attempt.outcome, AttemptOutcome::Accepted))
}

pub(super) fn episode_requires_clear_type(episode: &RegressionEpisode) -> bool {
    episode
        .attempt
        .as_ref()
        .is_none_or(|attempt| !matches!(attempt.outcome, AttemptOutcome::NoResult))
}

fn validate_music_selection_oracle(
    label: &RegressionLabel,
    selections: &[(u64, scorepeek_core::event::MusicSelectionState)],
    failures: &mut Vec<String>,
) {
    use scorepeek_core::event::MusicSelectionState;
    for episode in &label.episodes {
        let Some(span) = episode
            .attempt
            .as_ref()
            .and_then(|attempt| attempt.select_span)
        else {
            continue;
        };
        let latest = selections
            .iter()
            .filter(|(source_sequence, _)| *source_sequence <= span.last_sequence)
            .map(|(_, state)| state)
            .next_back();
        let expected = &episode.expected_result;
        let matches = match latest {
            Some(MusicSelectionState::Selected {
                scorepeek_song_id,
                play_type,
                difficulty,
                level,
                notes,
                ..
            }) => {
                serde_json::to_value(scorepeek_song_id).ok()
                    == Some(Value::String(episode.expected_song_id.clone()))
                    && *play_type == expected.play_type
                    && *difficulty == expected.difficulty
                    && *level == expected.level
                    && *notes == expected.notes
            }
            _ => false,
        };
        if !matches {
            failures.push(format!(
                "episode {} SELECT state differs at sequence {}: {latest:?}",
                episode.episode_id, span.last_sequence,
            ));
        }
    }
}

fn result_event_matches(
    event: &scorepeek_core::event::ResultDomainEvent,
    episode: &RegressionEpisode,
) -> bool {
    let expected = &episode.expected_result;
    let expected_song = serde_json::from_value::<scorepeek_core::replay::ScorepeekSongId>(
        Value::String(episode.expected_song_id.clone()),
    )
    .ok();
    event.contract == "scorepeek-result-detected-v4"
        && Some(event.scorepeek_song_id) == expected_song
        && event.clear_type == episode.expected_clear_type
        && match event.play_side {
            scorepeek_core::replay::PlaySide::OnePlayer => expected.play_side == "one_player",
            scorepeek_core::replay::PlaySide::TwoPlayer => expected.play_side == "two_player",
        }
        && event.play_mode == expected.play_mode
        && event.play_type == expected.play_type
        && event.difficulty == expected.difficulty
        && event.level == expected.level
        && event.notes == expected.notes
        && event.current_score == expected.current_score
        && expected.judgments.as_ref() == Some(&event.judgments)
        && expected.miss_count.as_ref() == Some(&event.miss_count)
        && expected.timing.as_ref() == Some(&event.timing)
        && expected.combo_break.as_ref() == Some(&event.combo_break)
        && expected.previous_best.as_ref() == Some(&event.previous_best)
        && matches!(
            (&event.play_options, expected.play_options.as_deref()),
            (PlayOptions::Known { values }, Some(expected)) if values == expected
        )
}

pub(super) fn optional_previous_matches<T: PartialEq>(
    observed: &PreviousBestValue<T>,
    expected: &PreviousBestValue<T>,
) -> bool {
    observed == expected || matches!(observed, PreviousBestValue::Unknown { .. })
}

pub(super) fn expected_play_options_match(observed: &PlayOptions, expected: &[PlayOption]) -> bool {
    matches!(observed, PlayOptions::Known { values } if values == expected)
}

#[derive(Deserialize)]
pub(super) struct SessionBinding {
    pub(super) capture_profile_sha256: String,
    pub(super) normalizer_sha256: String,
}

#[derive(Deserialize)]
struct SessionRunDocument {
    binding: SessionBinding,
}

pub(super) fn session_binding(
    store: &Path,
    session: &CaptureSession,
) -> Result<SessionBinding, CorpusError> {
    let run = session
        .artifacts
        .iter()
        .find(|artifact| artifact.source_path == "capture/run.json")
        .ok_or_else(|| {
            CorpusError::InvalidReplay("session run binding is unavailable".to_owned())
        })?;
    let (document, _) = read_json::<SessionRunDocument>(&store.join("objects").join(&run.sha256))?;
    if !valid_sha256(&document.binding.capture_profile_sha256)
        || !valid_sha256(&document.binding.normalizer_sha256)
    {
        return invalid_replay("session run binding is invalid");
    }
    Ok(document.binding)
}

fn replay_normalization_pairs(store: &Path, session: &CaptureSession) -> Result<(), CorpusError> {
    if session.normalization_pairs.is_empty() {
        return Ok(());
    }
    let profile_artifact = session
        .artifacts
        .iter()
        .find(|artifact| artifact.source_path == "capture/profile.json")
        .ok_or_else(|| {
            CorpusError::InvalidReplay("normalization profile is unavailable".to_owned())
        })?;
    let profile_bytes = fs::read(store.join("objects").join(&profile_artifact.sha256))?;
    let profile = scorepeek_runtime::capture::GamescopeProfileBinding::parse(
        &profile_bytes,
        &profile_artifact.sha256,
    )
    .map_err(|_| CorpusError::InvalidReplay("normalization profile is invalid".to_owned()))?;
    if profile.capture_profile_sha256() != session.profile_sha256 {
        return invalid_replay("normalization profile binding differs");
    }
    let frame_map = session_frame_map(session);
    for pair in &session.normalization_pairs {
        if frame_map.get(&pair.sequence) != Some(&pair.canonical_sha256) {
            return invalid_replay("normalization pair canonical binding differs");
        }
        let observed_path = store.join("objects").join(&pair.observed_sha256);
        let observed_pixels = u64::from(profile.observed_width())
            .checked_mul(u64::from(profile.observed_height()))
            .ok_or_else(|| {
                CorpusError::InvalidReplay("observed QOI dimensions overflow".to_owned())
            })?;
        let encoded_bound = observed_pixels
            .checked_mul(5)
            .and_then(|bytes| bytes.checked_add(22))
            .map_or(MAX_ARTIFACT_BYTES, |bytes| bytes.min(MAX_ARTIFACT_BYTES));
        let encoded = read_bounded_qoi_with_limit(&observed_path, encoded_bound)?;
        let header = qoi::decode_header(&encoded)
            .map_err(|_| CorpusError::InvalidReplay("observed QOI header is invalid".to_owned()))?;
        if header.width != profile.observed_width() || header.height != profile.observed_height() {
            return invalid_replay("observed QOI dimensions differ from the profile");
        }
        let (header, rgb) = qoi::decode_to_vec(&encoded)
            .map_err(|_| CorpusError::InvalidReplay("observed QOI is invalid".to_owned()))?;
        if rgb.len()
            != usize::try_from(header.width)
                .unwrap_or(usize::MAX)
                .saturating_mul(usize::try_from(header.height).unwrap_or(usize::MAX))
                .saturating_mul(3)
        {
            return invalid_replay("observed QOI dimensions differ from the profile");
        }
        let mut bgrx = Vec::with_capacity((rgb.len() / 3).saturating_mul(4));
        for pixel in rgb.chunks_exact(3) {
            bgrx.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 0]);
        }
        let stride = profile
            .observed_width()
            .checked_mul(4)
            .ok_or_else(|| CorpusError::InvalidReplay("observed stride overflows".to_owned()))?;
        let normalized = profile
            .geometry()
            .normalize_bgrx_bytes(
                &bgrx,
                profile.observed_width(),
                profile.observed_height(),
                stride,
            )
            .map_err(|_| {
                CorpusError::InvalidReplay("production normalization failed".to_owned())
            })?;
        let canonical = read_canonical_object(store, &pair.canonical_sha256)?;
        if normalized.as_ref() != canonical {
            return invalid_replay("observed-to-canonical normalization regressed");
        }
    }
    Ok(())
}

fn default_catalog_root() -> Result<PathBuf, CorpusError> {
    if let Some(data) = env::var_os("XDG_DATA_HOME") {
        let path = PathBuf::from(data);
        if path.is_absolute() {
            return Ok(path.join("scorepeek/catalog"));
        }
        return invalid_replay("XDG_DATA_HOME must be absolute");
    }
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| CorpusError::InvalidReplay("HOME is required".to_owned()))?;
    Ok(home.join(".local/share/scorepeek/catalog"))
}

pub(super) fn parse_timestamp_ms(value: &str) -> Result<u64, CorpusError> {
    let (seconds, fraction) = value.split_once('.').unwrap_or((value, ""));
    if seconds.starts_with('-')
        || seconds.is_empty()
        || !seconds.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return invalid("video frame timestamp is invalid");
    }
    let seconds = seconds
        .parse::<u64>()
        .map_err(|_| CorpusError::InvalidRequest("video frame timestamp overflows".to_owned()))?;
    let mut milliseconds = 0_u64;
    for (index, byte) in fraction.bytes().take(3).enumerate() {
        milliseconds =
            milliseconds.saturating_add(u64::from(byte - b'0') * [100_u64, 10, 1][index]);
    }
    seconds
        .checked_mul(1_000)
        .and_then(|whole| whole.checked_add(milliseconds))
        .ok_or_else(|| CorpusError::InvalidRequest("video frame timestamp overflows".to_owned()))
}

fn video_timeline_event(
    event: &str,
    session_id: &str,
    source_timestamp_ms: u64,
    previous_source_timestamp_ms: u64,
) -> Value {
    serde_json::json!({
        "schema":"scorepeek-private-diagnostic-event-v1", "event":event,
        "session_id":session_id, "capture_generation":1,
        "source_timestamp_ms":source_timestamp_ms,
        "previous_source_timestamp_ms":previous_source_timestamp_ms,
    })
}

fn profile_root() -> Result<PathBuf, CorpusError> {
    if let Some(config) = env::var_os("XDG_CONFIG_HOME") {
        let path = PathBuf::from(config);
        if path.is_absolute() {
            return Ok(path.join("scorepeek/profiles"));
        }
        return invalid("XDG_CONFIG_HOME must be absolute");
    }
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| CorpusError::InvalidRequest("HOME is required".to_owned()))?;
    Ok(home.join(".config/scorepeek/profiles"))
}

fn find_profile_bytes(expected_sha256: &str) -> Result<Vec<u8>, CorpusError> {
    if !valid_sha256(expected_sha256) {
        return invalid("capture profile digest is invalid");
    }
    for entry in fs::read_dir(profile_root()?)? {
        let entry = entry?;
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            let bytes = fs::read(entry.path())?;
            let file_sha256 = digest(&bytes);
            if scorepeek_runtime::capture::GamescopeProfileBinding::parse(&bytes, &file_sha256)
                .is_ok_and(|profile| profile.capture_profile_sha256() == expected_sha256)
            {
                return Ok(bytes);
            }
        }
    }
    invalid("capture profile bound by the diagnostic is unavailable")
}

fn field_json(fields: &scorepeek_core::replay::ScreenFieldObservations) -> Value {
    match fields {
        scorepeek_core::replay::ScreenFieldObservations::Title(fields) => {
            serde_json::json!({
                "screen":"title", "game_version":fields.game_version.open_text
            })
        }
        scorepeek_core::replay::ScreenFieldObservations::Result(fields) => {
            serde_json::json!({
                "screen":"result", "title":fields.title.open_text, "artist":fields.artist.open_text,
                "clear_type":fields.clear_type.open_text
            })
        }
        scorepeek_core::replay::ScreenFieldObservations::MusicSelect(fields) => {
            serde_json::json!({
                "screen":"music_select", "central_title":fields.central_title.open_text,
                "artist":fields.artist.open_text, "active_list_title":fields.active_list_title.open_text
            })
        }
    }
}
