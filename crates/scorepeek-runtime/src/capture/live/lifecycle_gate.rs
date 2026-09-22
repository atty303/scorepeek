use super::*;

pub fn parse_lifecycle_runs(value: &OsStr) -> Result<u32, String> {
    let runs = parse_u64(value, "capture lifecycle run count")?;
    if !(u64::from(MIN_LIFECYCLE_RUNS)..=u64::from(MAX_LIFECYCLE_RUNS)).contains(&runs) {
        return Err(format!(
            "capture lifecycle run count must be between {MIN_LIFECYCLE_RUNS} and {MAX_LIFECYCLE_RUNS}"
        ));
    }
    u32::try_from(runs).map_err(|_| "capture lifecycle run count is too large".to_owned())
}

pub(super) fn parse_u64(value: &OsStr, label: &str) -> Result<u64, String> {
    let value = value
        .to_str()
        .ok_or_else(|| format!("{label} must be UTF-8"))?;
    value
        .parse::<u64>()
        .map_err(|_| format!("{label} must be an integer"))
}

#[cfg(test)]
pub fn run_gamescope_live_gate(duration_ms: u64) -> GamescopeLiveGateReport {
    run_gamescope_live_gate_with_interval(duration_ms, 0)
}

#[cfg(test)]
pub fn run_gamescope_live_gate_with_interval(
    duration_ms: u64,
    consumer_interval_ms: u64,
) -> GamescopeLiveGateReport {
    let mut sink = BoundedDiagnosticSink::default();
    let mut consumed_frames = 0_u64;
    let mut first_sequence = None;
    let mut last_sequence = None;

    let lease = match acquire_gamescope_source(DISCOVERY_TIMEOUT, &mut sink) {
        Ok(lease) => lease,
        Err(error) => {
            return report(
                duration_ms,
                consumer_interval_ms,
                consumed_frames,
                first_sequence,
                last_sequence,
                Some(error.error_type()),
                sink,
            );
        }
    };
    let mut receiver =
        match start_uncalibrated_gamescope_receiver(lease, RECEIVER_START_TIMEOUT, &mut sink) {
            Ok(receiver) => receiver,
            Err(error) => {
                return report(
                    duration_ms,
                    consumer_interval_ms,
                    consumed_frames,
                    first_sequence,
                    last_sequence,
                    Some(error.error_type()),
                    sink,
                );
            }
        };

    consume_latest(
        &mut receiver,
        &mut consumed_frames,
        &mut first_sequence,
        &mut last_sequence,
    );
    let steady_started = Instant::now();
    let mut last_consumed = steady_started;
    let requested_duration = Duration::from_millis(duration_ms);
    let consumer_interval = Duration::from_millis(consumer_interval_ms);
    let mut terminal = None;
    while steady_started.elapsed() < requested_duration {
        let remaining = requested_duration.saturating_sub(steady_started.elapsed());
        if let Err(error) = receiver.poll(remaining, &mut sink) {
            terminal = Some(error.error_type());
            break;
        }
        if last_consumed.elapsed() >= consumer_interval {
            consume_latest(
                &mut receiver,
                &mut consumed_frames,
                &mut first_sequence,
                &mut last_sequence,
            );
            last_consumed = Instant::now();
        }
    }

    consume_latest(
        &mut receiver,
        &mut consumed_frames,
        &mut first_sequence,
        &mut last_sequence,
    );

    if let Err(error) = receiver.shutdown(&mut sink) {
        terminal.get_or_insert(error.error_type());
    }
    report(
        duration_ms,
        consumer_interval_ms,
        consumed_frames,
        first_sequence,
        last_sequence,
        terminal,
        sink,
    )
}

#[cfg(test)]
pub fn run_gamescope_lifecycle_gate(
    duration_ms: u64,
    requested_runs: u32,
    consumer_interval_ms: u64,
) -> GamescopeLifecycleGateReport {
    let resources_before_first_run = process_resource_snapshot().ok();
    let mut resources_after_warmup = None;
    let mut maximum_resources_after_run = None;
    let mut resources_after_final_run = None;
    let mut runs = Vec::with_capacity(requested_runs as usize);
    let mut capture_failed = false;
    let mut resource_unavailable = resources_before_first_run.is_none();
    let mut overwrite_observed = false;

    for run in 1..=requested_runs {
        let report = run_gamescope_live_gate_with_interval(duration_ms, consumer_interval_ms);
        let summary = summarize_run(run, &report);
        overwrite_observed |= summary.overwritten_frames > 0;
        capture_failed |= !report.succeeded();
        runs.push(summary);

        match process_resource_snapshot() {
            Ok(resources) => {
                if run == 1 {
                    resources_after_warmup = Some(resources);
                }
                maximum_resources_after_run
                    .get_or_insert(resources)
                    .update_maximum(resources);
                resources_after_final_run = Some(resources);
            }
            Err(()) => resource_unavailable = true,
        }
        if capture_failed {
            break;
        }
    }

    let error_type = lifecycle_error_type(
        capture_failed,
        resource_unavailable,
        consumer_interval_ms > 0 && !overwrite_observed,
    );
    GamescopeLifecycleGateReport {
        schema: "scorepeek-gamescope-lifecycle-gate-v1",
        status: if error_type.is_some() {
            LiveGateStatus::Error
        } else {
            LiveGateStatus::Success
        },
        error_type,
        requested_duration_ms: duration_ms,
        consumer_interval_ms,
        requested_runs,
        completed_runs: u32::try_from(runs.len()).unwrap_or(u32::MAX),
        overwrite_observed,
        resources_before_first_run,
        resources_after_warmup,
        maximum_resources_after_run,
        resources_after_final_run,
        runs,
    }
}

pub(super) fn lifecycle_error_type(
    capture_failed: bool,
    resource_unavailable: bool,
    expected_overwrite_missing: bool,
) -> Option<LifecycleGateErrorType> {
    if capture_failed {
        Some(LifecycleGateErrorType::CaptureRunFailed)
    } else if resource_unavailable {
        Some(LifecycleGateErrorType::ProcessResourceUnavailable)
    } else if expected_overwrite_missing {
        Some(LifecycleGateErrorType::ExpectedOverwriteMissing)
    } else {
        None
    }
}

pub(super) fn summarize_run(run: u32, report: &GamescopeLiveGateReport) -> LifecycleRunSummary {
    let mut summary = LifecycleRunSummary {
        run,
        status: report.status,
        error_type: report.error_type,
        consumed_frames: report.consumed_frames,
        received_frames: 0,
        overwritten_frames: 0,
        last_sequence: report.last_sequence,
        maximum_gap_ns: 0,
        diagnostic_fact_count: u32::try_from(report.diagnostic_facts.len()).unwrap_or(u32::MAX),
        dropped_diagnostic_facts: report.dropped_diagnostic_facts,
        phases: LifecyclePhaseSummary::default(),
    };
    for fact in &report.diagnostic_facts {
        let succeeded = fact.status == CaptureDiagnosticStatus::Success;
        match (&fact.operation, &fact.detail) {
            (
                CaptureDiagnosticOperation::SteadyReception,
                CaptureDiagnosticDetail::SteadyReception {
                    received_frames,
                    overwritten_frames,
                    last_sequence,
                    maximum_gap_ns,
                },
            ) => {
                summary.received_frames = *received_frames;
                summary.overwritten_frames = *overwritten_frames;
                summary.last_sequence = *last_sequence;
                summary.maximum_gap_ns = *maximum_gap_ns;
            }
            (CaptureDiagnosticOperation::StreamNegotiation, _) if succeeded => {
                summary.phases.negotiation = LifecyclePhaseStatus::Success;
            }
            (CaptureDiagnosticOperation::FirstFrame, _) if succeeded => {
                summary.phases.first_frame = LifecyclePhaseStatus::Success;
            }
            (CaptureDiagnosticOperation::ReceiverShutdown, _) if succeeded => {
                summary.phases.receiver_shutdown = LifecyclePhaseStatus::Success;
            }
            (CaptureDiagnosticOperation::Shutdown, _) if succeeded => {
                summary.phases.provider_shutdown = LifecyclePhaseStatus::Success;
            }
            _ => {}
        }
    }
    summary
}

pub(super) fn process_resource_snapshot() -> Result<ProcessResourceSnapshot, ()> {
    Ok(ProcessResourceSnapshot {
        open_file_descriptors: count_proc_entries("/proc/self/fd")?,
        threads: count_proc_entries("/proc/self/task")?,
        resident_bytes: resident_bytes()?,
    })
}

pub(super) fn count_proc_entries(path: &str) -> Result<u64, ()> {
    let entries = fs::read_dir(path).map_err(|_| ())?;
    let mut count = 0_u64;
    for entry in entries {
        entry.map_err(|_| ())?;
        count = count.checked_add(1).ok_or(())?;
    }
    Ok(count)
}

pub(super) fn resident_bytes() -> Result<u64, ()> {
    let file = File::open("/proc/self/status").map_err(|_| ())?;
    let mut status = String::new();
    file.take(MAX_PROC_STATUS_BYTES)
        .read_to_string(&mut status)
        .map_err(|_| ())?;
    let line = status
        .lines()
        .find(|line| line.starts_with("VmRSS:"))
        .ok_or(())?;
    let mut fields = line.split_ascii_whitespace();
    if fields.next() != Some("VmRSS:") {
        return Err(());
    }
    let kibibytes = fields.next().ok_or(())?.parse::<u64>().map_err(|_| ())?;
    if fields.next() != Some("kB") || fields.next().is_some() {
        return Err(());
    }
    kibibytes.checked_mul(1024).ok_or(())
}

pub(super) fn consume_latest(
    receiver: &mut scorepeek::capture::UncalibratedPipeWireReceiver,
    consumed_frames: &mut u64,
    first_sequence: &mut Option<u64>,
    last_sequence: &mut Option<u64>,
) {
    let Some(frame) = receiver.take_latest_frame() else {
        return;
    };
    *consumed_frames = consumed_frames.saturating_add(1);
    first_sequence.get_or_insert(frame.sequence());
    *last_sequence = Some(frame.sequence());
}

pub(super) fn report(
    duration_ms: u64,
    consumer_interval_ms: u64,
    consumed_frames: u64,
    first_sequence: Option<u64>,
    last_sequence: Option<u64>,
    error_type: Option<CaptureErrorType>,
    sink: BoundedDiagnosticSink,
) -> GamescopeLiveGateReport {
    GamescopeLiveGateReport {
        schema: "scorepeek-gamescope-live-gate-v2",
        status: if error_type.is_some() {
            LiveGateStatus::Error
        } else {
            LiveGateStatus::Success
        },
        requested_duration_ms: duration_ms,
        consumer_interval_ms,
        consumed_frames,
        first_sequence,
        last_sequence,
        error_type,
        diagnostic_facts: sink.facts,
        dropped_diagnostic_facts: sink.dropped,
    }
}
