use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::Read as _;
use std::time::{Duration, Instant};

use scorepeek::capture::{
    CaptureDiagnosticDetail, CaptureDiagnosticFact, CaptureDiagnosticOperation,
    CaptureDiagnosticSink, CaptureDiagnosticStatus, CaptureErrorType, GamescopeProfileBinding,
    GamescopeSessionProvenance, acquire_gamescope_source, acquire_gamescope_source_for_session,
    admit_gamescope_profile, start_uncalibrated_gamescope_receiver,
};
use serde::Serialize;

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);
const RECEIVER_START_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_GATE_DURATION_MS: u64 = 60_000;
const MAX_CONSUMER_INTERVAL_MS: u64 = 60_000;
const MIN_LIFECYCLE_RUNS: u32 = 2;
const MAX_LIFECYCLE_RUNS: u32 = 100;
const MAX_DIAGNOSTIC_FACTS: usize = 32;
const MAX_PROC_STATUS_BYTES: u64 = 64 * 1024;
const MAX_BINDING_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum LiveGateStatus {
    Success,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum LifecycleGateErrorType {
    CaptureRunFailed,
    ProcessResourceUnavailable,
    ExpectedOverwriteMissing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum BindingAdmissionGateErrorType {
    BindingUnavailable,
    BindingInvalid,
    CaptureFailed,
    AdmissionRejected,
    ShutdownFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
struct ProcessResourceSnapshot {
    open_file_descriptors: u64,
    threads: u64,
    resident_bytes: u64,
}

impl ProcessResourceSnapshot {
    fn update_maximum(&mut self, observed: Self) {
        self.open_file_descriptors = self
            .open_file_descriptors
            .max(observed.open_file_descriptors);
        self.threads = self.threads.max(observed.threads);
        self.resident_bytes = self.resident_bytes.max(observed.resident_bytes);
    }
}

#[derive(Debug, Serialize)]
pub struct GamescopeLiveGateReport {
    schema: &'static str,
    status: LiveGateStatus,
    requested_duration_ms: u64,
    consumer_interval_ms: u64,
    consumed_frames: u64,
    first_sequence: Option<u64>,
    last_sequence: Option<u64>,
    error_type: Option<CaptureErrorType>,
    diagnostic_facts: Vec<CaptureDiagnosticFact>,
    dropped_diagnostic_facts: u64,
}

#[derive(Debug, Serialize)]
struct LifecycleRunSummary {
    run: u32,
    status: LiveGateStatus,
    error_type: Option<CaptureErrorType>,
    consumed_frames: u64,
    received_frames: u64,
    overwritten_frames: u64,
    last_sequence: Option<u64>,
    maximum_gap_ns: u64,
    diagnostic_fact_count: u32,
    dropped_diagnostic_facts: u64,
    phases: LifecyclePhaseSummary,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum LifecyclePhaseStatus {
    #[default]
    NotObserved,
    Success,
}

#[derive(Debug, Default, Serialize)]
struct LifecyclePhaseSummary {
    negotiation: LifecyclePhaseStatus,
    first_frame: LifecyclePhaseStatus,
    receiver_shutdown: LifecyclePhaseStatus,
    provider_shutdown: LifecyclePhaseStatus,
}

#[derive(Debug, Serialize)]
pub struct GamescopeLifecycleGateReport {
    schema: &'static str,
    status: LiveGateStatus,
    error_type: Option<LifecycleGateErrorType>,
    requested_duration_ms: u64,
    consumer_interval_ms: u64,
    requested_runs: u32,
    completed_runs: u32,
    overwrite_observed: bool,
    resources_before_first_run: Option<ProcessResourceSnapshot>,
    resources_after_warmup: Option<ProcessResourceSnapshot>,
    maximum_resources_after_run: Option<ProcessResourceSnapshot>,
    resources_after_final_run: Option<ProcessResourceSnapshot>,
    runs: Vec<LifecycleRunSummary>,
}

#[derive(Debug, Serialize)]
pub struct GamescopeBindingAdmissionGateReport {
    schema: &'static str,
    status: LiveGateStatus,
    error_type: Option<BindingAdmissionGateErrorType>,
    capture_error_type: Option<CaptureErrorType>,
    capture_profile_sha256: Option<String>,
    normalizer_artifact_sha256: Option<String>,
    diagnostic_facts: Vec<CaptureDiagnosticFact>,
    dropped_diagnostic_facts: u64,
}

impl GamescopeLiveGateReport {
    pub const fn succeeded(&self) -> bool {
        matches!(self.status, LiveGateStatus::Success)
    }
}

impl GamescopeLifecycleGateReport {
    pub const fn succeeded(&self) -> bool {
        matches!(self.status, LiveGateStatus::Success)
    }
}

impl GamescopeBindingAdmissionGateReport {
    pub const fn succeeded(&self) -> bool {
        matches!(self.status, LiveGateStatus::Success)
    }
}

#[derive(Default)]
struct BoundedDiagnosticSink {
    facts: Vec<CaptureDiagnosticFact>,
    dropped: u64,
}

impl CaptureDiagnosticSink for BoundedDiagnosticSink {
    fn record(&mut self, fact: CaptureDiagnosticFact) {
        if self.facts.len() < MAX_DIAGNOSTIC_FACTS {
            self.facts.push(fact);
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }
}

pub fn parse_duration_ms(value: &OsStr) -> Result<u64, String> {
    let duration = parse_u64(value, "capture live gate duration")?;
    if !(1..=MAX_GATE_DURATION_MS).contains(&duration) {
        return Err(format!(
            "capture live gate duration must be between 1 and {MAX_GATE_DURATION_MS} ms"
        ));
    }
    Ok(duration)
}

pub fn parse_consumer_interval_ms(value: &OsStr) -> Result<u64, String> {
    let interval = parse_u64(value, "capture consumer interval")?;
    if interval > MAX_CONSUMER_INTERVAL_MS {
        return Err(format!(
            "capture consumer interval must be between 0 and {MAX_CONSUMER_INTERVAL_MS} ms"
        ));
    }
    Ok(interval)
}

pub fn run_gamescope_binding_admission_gate(
    binding_path: &std::path::Path,
    expected_binding_sha256: &str,
    session: GamescopeSessionProvenance,
) -> GamescopeBindingAdmissionGateReport {
    let binding = match read_binding(binding_path, expected_binding_sha256) {
        Ok(binding) => binding,
        Err(error_type) => {
            return binding_admission_report(error_type, None, BoundedDiagnosticSink::default());
        }
    };
    let mut sink = BoundedDiagnosticSink::default();
    let lease = match acquire_gamescope_source_for_session(DISCOVERY_TIMEOUT, session, &mut sink) {
        Ok(lease) => lease,
        Err(error) => {
            return binding_admission_report(
                BindingAdmissionGateErrorType::CaptureFailed,
                Some(error.error_type()),
                sink,
            );
        }
    };
    let receiver =
        match start_uncalibrated_gamescope_receiver(lease, RECEIVER_START_TIMEOUT, &mut sink) {
            Ok(receiver) => receiver,
            Err(error) => {
                return binding_admission_report(
                    BindingAdmissionGateErrorType::CaptureFailed,
                    Some(error.error_type()),
                    sink,
                );
            }
        };
    match admit_gamescope_profile(receiver, binding, &mut sink) {
        Ok(lease) => {
            let digests = (
                lease.capture_profile_sha256().to_owned(),
                lease.normalizer_artifact_sha256().to_owned(),
            );
            if let Err(error) = lease.shutdown(&mut sink) {
                return binding_admission_report(
                    BindingAdmissionGateErrorType::ShutdownFailed,
                    Some(error.error_type()),
                    sink,
                );
            }
            GamescopeBindingAdmissionGateReport {
                schema: "scorepeek-gamescope-binding-admission-gate-v1",
                status: LiveGateStatus::Success,
                error_type: None,
                capture_error_type: None,
                capture_profile_sha256: Some(digests.0),
                normalizer_artifact_sha256: Some(digests.1),
                diagnostic_facts: sink.facts,
                dropped_diagnostic_facts: sink.dropped,
            }
        }
        Err(failure) => {
            let error_type = failure.error_type();
            let _ = failure.shutdown(&mut sink);
            binding_admission_report(
                BindingAdmissionGateErrorType::AdmissionRejected,
                Some(error_type),
                sink,
            )
        }
    }
}

fn read_binding(
    path: &std::path::Path,
    expected_sha256: &str,
) -> Result<GamescopeProfileBinding, BindingAdmissionGateErrorType> {
    let file = File::open(path).map_err(|_| BindingAdmissionGateErrorType::BindingUnavailable)?;
    let metadata = file
        .metadata()
        .map_err(|_| BindingAdmissionGateErrorType::BindingUnavailable)?;
    let maximum_bytes = u64::try_from(MAX_BINDING_BYTES).unwrap_or(u64::MAX);
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum_bytes {
        return Err(BindingAdmissionGateErrorType::BindingInvalid);
    }
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| BindingAdmissionGateErrorType::BindingInvalid)?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(maximum_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| BindingAdmissionGateErrorType::BindingUnavailable)?;
    if u64::try_from(bytes.len()).ok() != Some(metadata.len()) {
        return Err(BindingAdmissionGateErrorType::BindingInvalid);
    }
    GamescopeProfileBinding::parse(&bytes, expected_sha256)
        .map_err(|_| BindingAdmissionGateErrorType::BindingInvalid)
}

fn binding_admission_report(
    error_type: BindingAdmissionGateErrorType,
    capture_error_type: Option<CaptureErrorType>,
    sink: BoundedDiagnosticSink,
) -> GamescopeBindingAdmissionGateReport {
    GamescopeBindingAdmissionGateReport {
        schema: "scorepeek-gamescope-binding-admission-gate-v1",
        status: LiveGateStatus::Error,
        error_type: Some(error_type),
        capture_error_type,
        capture_profile_sha256: None,
        normalizer_artifact_sha256: None,
        diagnostic_facts: sink.facts,
        dropped_diagnostic_facts: sink.dropped,
    }
}

pub fn parse_lifecycle_runs(value: &OsStr) -> Result<u32, String> {
    let runs = parse_u64(value, "capture lifecycle run count")?;
    if !(u64::from(MIN_LIFECYCLE_RUNS)..=u64::from(MAX_LIFECYCLE_RUNS)).contains(&runs) {
        return Err(format!(
            "capture lifecycle run count must be between {MIN_LIFECYCLE_RUNS} and {MAX_LIFECYCLE_RUNS}"
        ));
    }
    u32::try_from(runs).map_err(|_| "capture lifecycle run count is too large".to_owned())
}

fn parse_u64(value: &OsStr, label: &str) -> Result<u64, String> {
    let value = value
        .to_str()
        .ok_or_else(|| format!("{label} must be UTF-8"))?;
    value
        .parse::<u64>()
        .map_err(|_| format!("{label} must be an integer"))
}

pub fn run_gamescope_live_gate(duration_ms: u64) -> GamescopeLiveGateReport {
    run_gamescope_live_gate_with_interval(duration_ms, 0)
}

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

fn lifecycle_error_type(
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

fn summarize_run(run: u32, report: &GamescopeLiveGateReport) -> LifecycleRunSummary {
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

fn process_resource_snapshot() -> Result<ProcessResourceSnapshot, ()> {
    Ok(ProcessResourceSnapshot {
        open_file_descriptors: count_proc_entries("/proc/self/fd")?,
        threads: count_proc_entries("/proc/self/task")?,
        resident_bytes: resident_bytes()?,
    })
}

fn count_proc_entries(path: &str) -> Result<u64, ()> {
    let entries = fs::read_dir(path).map_err(|_| ())?;
    let mut count = 0_u64;
    for entry in entries {
        entry.map_err(|_| ())?;
        count = count.checked_add(1).ok_or(())?;
    }
    Ok(count)
}

fn resident_bytes() -> Result<u64, ()> {
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

fn consume_latest(
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

fn report(
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

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;

    use scorepeek::capture::{
        CaptureDiagnosticDetail, CaptureDiagnosticFact, CaptureDiagnosticOperation,
        CaptureDiagnosticSink, CaptureDiagnosticStatus, CaptureErrorType, CaptureSourceKind,
        FractionalRectangle, GamescopeProfileBinding, GamescopeProfileBindingAuthoringInput,
        RationalCoordinate, UncalibratedMemoryType, UncalibratedVideoContract,
    };

    use super::{
        BoundedDiagnosticSink, GamescopeLiveGateReport, LifecycleGateErrorType,
        LifecyclePhaseStatus, LiveGateStatus, MAX_DIAGNOSTIC_FACTS, lifecycle_error_type,
        parse_consumer_interval_ms, parse_duration_ms, parse_lifecycle_runs,
        process_resource_snapshot, read_binding, run_gamescope_binding_admission_gate,
        summarize_run,
    };

    #[test]
    fn duration_is_explicitly_bounded() {
        assert_eq!(parse_duration_ms(OsStr::new("1")).unwrap(), 1);
        assert_eq!(parse_duration_ms(OsStr::new("60000")).unwrap(), 60_000);
        assert!(parse_duration_ms(OsStr::new("0")).is_err());
        assert!(parse_duration_ms(OsStr::new("60001")).is_err());
        assert!(parse_duration_ms(OsStr::new("forever")).is_err());
    }

    #[test]
    fn consumer_interval_is_explicitly_bounded() {
        assert_eq!(parse_consumer_interval_ms(OsStr::new("0")).unwrap(), 0);
        assert_eq!(
            parse_consumer_interval_ms(OsStr::new("60000")).unwrap(),
            60_000
        );
        assert!(parse_consumer_interval_ms(OsStr::new("60001")).is_err());
        assert!(parse_consumer_interval_ms(OsStr::new("sometimes")).is_err());
    }

    #[test]
    fn binding_gate_reads_only_digest_selected_bounded_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("binding.json");
        let video = UncalibratedVideoContract {
            width: 4,
            height: 2,
            framerate_num: 60,
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
        let authored = GamescopeProfileBinding::author(GamescopeProfileBindingAuthoringInput {
            calibration_evidence_sha256: "1".repeat(64),
            environment_id: "test-machine".to_owned(),
            gamescope_version: "3.16.19".to_owned(),
            backend_id: "sdl".to_owned(),
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
        })
        .unwrap();
        fs::write(&path, &authored.bytes).unwrap();

        assert!(read_binding(&path, &authored.artifact_sha256).is_ok());
        assert!(read_binding(&path, &"f".repeat(64)).is_err());
        fs::write(&path, vec![0; super::MAX_BINDING_BYTES + 1]).unwrap();
        assert!(read_binding(&path, &authored.artifact_sha256).is_err());
    }

    #[test]
    fn binding_gate_failure_report_omits_paths_and_session_values() {
        let session = scorepeek::capture::GamescopeSessionProvenance::new(
            scorepeek::capture::GamescopeSessionProvenanceInput {
                environment_id: "PRIVATE-ENVIRONMENT".to_owned(),
                gamescope_version: "PRIVATE-VERSION".to_owned(),
                backend_id: "PRIVATE-BACKEND".to_owned(),
                output_width: 4,
                output_height: 2,
                nested_width: 4,
                nested_height: 2,
                nested_refresh_hz: 60,
                scaler: "auto".to_owned(),
                filter: "linear".to_owned(),
            },
        )
        .unwrap();
        let path = std::path::Path::new("/PRIVATE/BINDING/PATH");
        let report = run_gamescope_binding_admission_gate(path, &"f".repeat(64), session);
        let encoded = serde_json::to_string(&report).unwrap();

        assert!(!encoded.contains("PRIVATE"));
        assert!(encoded.contains("binding_unavailable"));
    }

    #[test]
    fn lifecycle_run_count_is_explicitly_bounded() {
        assert_eq!(parse_lifecycle_runs(OsStr::new("2")).unwrap(), 2);
        assert_eq!(parse_lifecycle_runs(OsStr::new("100")).unwrap(), 100);
        assert!(parse_lifecycle_runs(OsStr::new("1")).is_err());
        assert!(parse_lifecycle_runs(OsStr::new("101")).is_err());
        assert!(parse_lifecycle_runs(OsStr::new("many")).is_err());
    }

    #[test]
    fn lifecycle_error_precedence_is_stable() {
        assert_eq!(
            lifecycle_error_type(true, true, true),
            Some(LifecycleGateErrorType::CaptureRunFailed)
        );
        assert_eq!(
            lifecycle_error_type(false, true, true),
            Some(LifecycleGateErrorType::ProcessResourceUnavailable)
        );
        assert_eq!(
            lifecycle_error_type(false, false, true),
            Some(LifecycleGateErrorType::ExpectedOverwriteMissing)
        );
        assert_eq!(lifecycle_error_type(false, false, false), None);
    }

    #[test]
    fn lifecycle_summary_extracts_only_typed_receiver_facts() {
        let report = GamescopeLiveGateReport {
            schema: "test",
            status: LiveGateStatus::Success,
            requested_duration_ms: 100,
            consumer_interval_ms: 25,
            consumed_frames: 2,
            first_sequence: Some(0),
            last_sequence: Some(4),
            error_type: None,
            diagnostic_facts: vec![
                CaptureDiagnosticFact {
                    sequence: 0,
                    monotonic_start_ms: 0,
                    monotonic_end_ms: 1,
                    operation: CaptureDiagnosticOperation::StreamNegotiation,
                    status: CaptureDiagnosticStatus::Success,
                    error_type: None,
                    detail: CaptureDiagnosticDetail::StreamNegotiation {
                        format: "BGRx",
                        requested_framerate_num: 60,
                        requested_framerate_denom: 1,
                        width: 1280,
                        height: 720,
                        framerate_num: 60,
                        framerate_denom: 1,
                        maximum_framerate_num: 60,
                        maximum_framerate_denom: 1,
                        pixel_aspect_num: 1,
                        pixel_aspect_denom: 1,
                        chroma_site: 0,
                        color_range: 0,
                        color_matrix: 0,
                        transfer_function: 0,
                        color_primaries: 0,
                    },
                },
                CaptureDiagnosticFact {
                    sequence: 1,
                    monotonic_start_ms: 1,
                    monotonic_end_ms: 2,
                    operation: CaptureDiagnosticOperation::FirstFrame,
                    status: CaptureDiagnosticStatus::Success,
                    error_type: None,
                    detail: CaptureDiagnosticDetail::FirstFrame {
                        memory_type: "mem_fd",
                        stride: 5120,
                        byte_count: 3_686_400,
                    },
                },
                CaptureDiagnosticFact {
                    sequence: 2,
                    monotonic_start_ms: 2,
                    monotonic_end_ms: 100,
                    operation: CaptureDiagnosticOperation::SteadyReception,
                    status: CaptureDiagnosticStatus::Success,
                    error_type: None,
                    detail: CaptureDiagnosticDetail::SteadyReception {
                        received_frames: 5,
                        overwritten_frames: 3,
                        last_sequence: Some(4),
                        maximum_gap_ns: 17_000_000,
                    },
                },
                CaptureDiagnosticFact {
                    sequence: 3,
                    monotonic_start_ms: 100,
                    monotonic_end_ms: 101,
                    operation: CaptureDiagnosticOperation::ReceiverShutdown,
                    status: CaptureDiagnosticStatus::Success,
                    error_type: None,
                    detail: CaptureDiagnosticDetail::ReceiverShutdown {
                        received_frames: 5,
                        overwritten_frames: 3,
                    },
                },
                CaptureDiagnosticFact {
                    sequence: 4,
                    monotonic_start_ms: 101,
                    monotonic_end_ms: 102,
                    operation: CaptureDiagnosticOperation::Shutdown,
                    status: CaptureDiagnosticStatus::Success,
                    error_type: None,
                    detail: CaptureDiagnosticDetail::Shutdown {
                        source: CaptureSourceKind::GamescopeDefaultRemote,
                    },
                },
            ],
            dropped_diagnostic_facts: 0,
        };

        let summary = summarize_run(7, &report);
        assert_eq!(summary.run, 7);
        assert_eq!(summary.received_frames, 5);
        assert_eq!(summary.overwritten_frames, 3);
        assert_eq!(summary.last_sequence, Some(4));
        assert_eq!(summary.maximum_gap_ns, 17_000_000);
        assert_lifecycle_phases_succeeded(&summary);
        assert_eq!(summary.error_type, None::<CaptureErrorType>);
    }

    fn assert_lifecycle_phases_succeeded(summary: &super::LifecycleRunSummary) {
        assert_eq!(summary.phases.negotiation, LifecyclePhaseStatus::Success);
        assert_eq!(summary.phases.first_frame, LifecyclePhaseStatus::Success);
        assert_eq!(
            summary.phases.receiver_shutdown,
            LifecyclePhaseStatus::Success
        );
        assert_eq!(
            summary.phases.provider_shutdown,
            LifecyclePhaseStatus::Success
        );
    }

    #[test]
    fn process_resources_are_available_on_linux() {
        let snapshot = process_resource_snapshot().unwrap();
        assert!(snapshot.open_file_descriptors > 0);
        assert!(snapshot.threads > 0);
        assert!(snapshot.resident_bytes > 0);
    }

    #[test]
    fn diagnostic_sink_drops_new_facts_at_capacity() {
        let mut sink = BoundedDiagnosticSink::default();
        for sequence in 0..=MAX_DIAGNOSTIC_FACTS as u64 {
            sink.record(CaptureDiagnosticFact {
                sequence,
                monotonic_start_ms: 0,
                monotonic_end_ms: 0,
                operation: CaptureDiagnosticOperation::Shutdown,
                status: CaptureDiagnosticStatus::Success,
                error_type: None,
                detail: CaptureDiagnosticDetail::Shutdown {
                    source: CaptureSourceKind::GamescopeDefaultRemote,
                },
            });
        }
        assert_eq!(sink.facts.len(), MAX_DIAGNOSTIC_FACTS);
        assert_eq!(sink.dropped, 1);
    }
}
