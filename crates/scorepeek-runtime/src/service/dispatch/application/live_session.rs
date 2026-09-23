use super::*;

pub(super) fn run_startup_stage<T>(
    diagnostics: &diagnostic_stream::DiagnosticSink,
    stage: &str,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    match operation() {
        Ok(value) => {
            diagnostics.record(
                "run_startup_stage",
                &serde_json::json!({"stage":stage, "status":"success"}),
                true,
            );
            Ok(value)
        }
        Err(error) => {
            diagnostics.record(
                "run_startup_stage",
                &serde_json::json!({"stage":stage, "status":"error", "error":error}),
                true,
            );
            Err(error)
        }
    }
}

pub(super) fn check_startup_stop(
    diagnostics: &diagnostic_stream::DiagnosticSink,
    monitor: &live_control::SignalStopMonitor,
) -> Result<(), String> {
    if !monitor.stop_requested() {
        return Ok(());
    }
    diagnostics.record(
        "run_startup_stage",
        &serde_json::json!({"stage":"interrupt", "status":"cancel"}),
        true,
    );
    if monitor.interrupted() {
        Err(INTERRUPTED_ERROR.to_owned())
    } else {
        Err(TERMINATED_ERROR.to_owned())
    }
}

pub(super) fn settle_startup_result<T>(
    diagnostics: &mut diagnostic_stream::RunDiagnostics,
    monitor: &live_control::SignalStopMonitor,
    result: Result<T, String>,
) -> Result<T, String> {
    let sink = diagnostics.sink();
    if let Err(error) = check_startup_stop(&sink, monitor) {
        diagnostics.finish("cancel");
        return Err(error);
    }
    result
}

pub(super) fn settle_output_startup_result<T>(
    output: &mut routine_output::RoutineOutput,
    monitor: &live_control::SignalStopMonitor,
    result: Result<T, String>,
) -> Result<T, String> {
    if !monitor.stop_requested() {
        return result;
    }
    output.record_diagnostic(
        "run_startup_stage",
        &serde_json::json!({"stage":"interrupt", "status":"cancel"}),
        true,
    );
    output.finish_diagnostics("cancel");
    if monitor.interrupted() {
        Err(INTERRUPTED_ERROR.to_owned())
    } else {
        Err(TERMINATED_ERROR.to_owned())
    }
}

#[allow(clippy::too_many_lines)]
pub(super) fn parse_routine_run_options(options: &[OsString]) -> Result<RoutineRunOptions, String> {
    let mut capture = None;
    let mut node_name = None;
    let mut crop = scorepeek::capture::EdgeCrop {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let mut crop_seen = [false; 4];
    let mut recording = false;
    let mut recording_retention = RecordingRetention::Selective;
    let mut scores_db = None;
    let mut no_scores = false;
    let mut recording_memory_mib = None;
    let mut overlays = OverlayOptions::default();
    let mut index = 0;
    while index < options.len() {
        match options[index].to_str() {
            Some("--overlay-config") if overlays.config_path.is_none() => {
                let option = options[index].to_str().unwrap_or_default();
                index += 1;
                let value = options
                    .get(index)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| format!("{option} requires a value"))?;
                overlays.config_path = Some(PathBuf::from(value));
            }
            Some("--overlay-wayland") if !overlays.wayland => overlays.wayland = true,
            Some("--overlay-wayland-edit") if !overlays.wayland_edit => {
                overlays.wayland = true;
                overlays.wayland_edit = true;
            }
            Some("--overlay-obs") if !overlays.obs => overlays.obs = true,
            Some("--record") if !recording => recording = true,
            Some("--record-all") if !recording => {
                recording = true;
                recording_retention = RecordingRetention::All;
            }
            Some("--no-scores") if !no_scores => no_scores = true,
            Some("--scores-db") if scores_db.is_none() => {
                index += 1;
                let Some(value) = options.get(index).filter(|value| !value.is_empty()) else {
                    return Err("--scores-db requires a database path".to_owned());
                };
                scores_db = Some(PathBuf::from(value));
            }
            Some("--capture") if capture.is_none() => {
                index += 1;
                let Some(value) = options.get(index).and_then(|value| value.to_str()) else {
                    return Err("--capture requires pipewire or vulkan-layer".to_owned());
                };
                capture = Some(value);
            }
            Some("--node-name") if node_name.is_none() => {
                index += 1;
                let Some(value) = options
                    .get(index)
                    .and_then(|value| value.to_str())
                    .filter(|value| !value.is_empty())
                else {
                    return Err("--node-name requires a non-empty UTF-8 value".to_owned());
                };
                node_name = Some(value);
            }
            Some(option @ ("--crop-left" | "--crop-top" | "--crop-right" | "--crop-bottom")) => {
                let crop_index = match option {
                    "--crop-left" => 0,
                    "--crop-top" => 1,
                    "--crop-right" => 2,
                    _ => 3,
                };
                if crop_seen[crop_index] {
                    return Err(format!("duplicate run option: {option}"));
                }
                crop_seen[crop_index] = true;
                index += 1;
                let Some(value) = options.get(index).and_then(|value| value.to_str()) else {
                    return Err(format!("{option} requires a pixel count"));
                };
                let value = value
                    .parse::<u32>()
                    .map_err(|_| format!("{option} requires a non-negative integer"))?;
                match crop_index {
                    0 => crop.left = value,
                    1 => crop.top = value,
                    2 => crop.right = value,
                    _ => crop.bottom = value,
                }
            }
            Some("--record-memory-mib") if recording_memory_mib.is_none() => {
                index += 1;
                let Some(value) = options.get(index).and_then(|value| value.to_str()) else {
                    return Err("--record-memory-mib requires an integer MiB value".to_owned());
                };
                recording_memory_mib =
                    Some(value.parse::<usize>().map_err(|_| {
                        "--record-memory-mib requires an integer MiB value".to_owned()
                    })?);
            }
            Some(option) => return Err(format!("unknown or duplicate run option: {option}")),
            None => return Err("run option must be UTF-8".to_owned()),
        }
        index += 1;
    }
    if no_scores && scores_db.is_some() {
        return Err("--scores-db conflicts with --no-scores".to_owned());
    }
    if recording_memory_mib.is_some() && !recording {
        return Err("--record-memory-mib requires --record or --record-all".to_owned());
    }
    let recording_memory_limit = RecordingMemoryLimit::from_mib(
        recording_memory_mib.unwrap_or(DEFAULT_RECORDING_MEMORY_MIB),
    )?;
    let capture = match (capture, node_name) {
        (Some("pipewire"), Some(node_name)) => RoutineCapture::Pipewire {
            node_name: node_name.to_owned(),
        },
        (Some("pipewire"), None) => {
            return Err("--capture pipewire requires --node-name".to_owned());
        }
        (Some("vulkan-layer"), None) => RoutineCapture::VulkanLayer,
        (Some("vulkan-layer"), Some(_)) => {
            return Err("--node-name is only valid with --capture pipewire".to_owned());
        }
        (Some(value), _) => return Err(format!("unsupported capture backend: {value}")),
        (None, _) => {
            return Err(
                "scorepeek run requires --capture pipewire or --capture vulkan-layer".to_owned(),
            );
        }
    };
    Ok(RoutineRunOptions {
        overlays,
        capture,
        crop,
        scores_db,
        no_scores,
        recording,
        recording_memory_limit,
        recording_retention,
    })
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the admitted run keeps parsed options and its diagnostic ownership explicit"
)]
pub(super) fn run_routine_live_session(
    capture: &RoutineCapture,
    crop: scorepeek::capture::EdgeCrop,
    recording: &str,
    recording_memory_limit: RecordingMemoryLimit,
    recording_retention: RecordingRetention,
    scores_db: Option<&Path>,
    no_scores: bool,
    overlays: OverlayOptions,
    bundle: &Path,
    config_path: &Path,
    invocation_id: String,
    mut diagnostics: diagnostic_stream::RunDiagnostics,
    monitor: &live_control::SignalStopMonitor,
) -> Result<(), String> {
    let recording_enabled = recording == "enabled";
    let diagnostic_sink = diagnostics.sink();
    diagnostic_sink.record(
        "canonical_recording_config",
        &serde_json::json!({
            "enabled": recording_enabled,
            "retention": recording_retention,
            "memory_limit_bytes": recording_memory_limit.bytes(),
        }),
        true,
    );
    let catalog_paths_result = run_startup_stage(&diagnostic_sink, "catalog_paths", || {
        catalog_paths(
            env::var_os("XDG_DATA_HOME").as_deref(),
            env::var_os("XDG_CACHE_HOME").as_deref(),
            env::var_os("HOME").as_deref(),
        )
    });
    let (catalog_root, _) = settle_startup_result(&mut diagnostics, monitor, catalog_paths_result)?;
    let catalog_url_result = run_startup_stage(&diagnostic_sink, "catalog_url", || {
        crate::resources::catalog::acquire::resolve_effective_url(config_path)
            .map_err(|error| error.to_string())
    });
    let effective_catalog_url =
        settle_startup_result(&mut diagnostics, monitor, catalog_url_result)?;
    let prepared_catalog_result = run_startup_stage(&diagnostic_sink, "active_catalog", || {
        crate::resources::catalog::acquire::prepare(
            &catalog_root,
            &effective_catalog_url,
            |event| {
                record_catalog_update(&diagnostic_sink, &event);
            },
        )
        .map_err(|error| error.to_string())
    });
    let prepared_catalog =
        settle_startup_result(&mut diagnostics, monitor, prepared_catalog_result)?;
    let run_catalog_digest = prepared_catalog.active.digest;
    settle_startup_result(&mut diagnostics, monitor, Ok(()))?;
    let background_catalog_update = prepared_catalog.background_due.then(|| {
        let background_root = catalog_root.clone();
        let background_url = effective_catalog_url.clone();
        let background_sink = diagnostic_sink.clone();
        let background_stop = monitor.stop_token();
        std::thread::Builder::new()
            .name("catalog-update-low-priority".to_owned())
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(250));
                if background_stop.load(std::sync::atomic::Ordering::Acquire) {
                    return;
                }
                let _ = crate::resources::catalog::schedule::update_background(
                    &background_root,
                    &background_url,
                    |event| record_catalog_update(&background_sink, &event),
                );
            })
    });
    let background_catalog_update = match background_catalog_update {
        Some(Ok(handle)) => Some(handle),
        Some(Err(error)) => {
            diagnostic_sink.record(
                "catalog_update",
                &serde_json::json!({
                    "mode": "background",
                    "stage": "resolve",
                    "status": "error",
                    "error_type": "worker_start_failed",
                    "source_url_sha256": effective_catalog_url.fingerprint(),
                }),
                true,
            );
            eprintln!("scorepeek: background catalog update unavailable: {error}");
            None
        }
        None => None,
    };
    settle_startup_result(&mut diagnostics, monitor, Ok(()))?;
    if recording_enabled {
        let recorder_result = run_startup_stage(&diagnostic_sink, "canonical_recorder", || {
            canonical_recording::CanonicalRecordingWorker::preflight()
        });
        settle_startup_result(&mut diagnostics, monitor, recorder_result)?;
    }
    let state_result = run_startup_stage(&diagnostic_sink, "state", || {
        local_profiles::state_paths(recording_enabled)
    });
    let state = settle_startup_result(&mut diagnostics, monitor, state_result)?;
    let identity_result = run_startup_stage(&diagnostic_sink, "executable_identity", || {
        current_executable_sha256()
    });
    let build_sha256 = settle_startup_result(&mut diagnostics, monitor, identity_result)?;
    let stop = monitor.stop_token();
    let output_result =
        routine_output::RoutineOutput::start(invocation_id.clone(), recording_enabled, diagnostics);
    let mut output = match output_result {
        Ok(output) => output,
        Err(error) => {
            let (message, mut diagnostics) = error.into_parts();
            return settle_startup_result(&mut diagnostics, monitor, Err(message));
        }
    };
    settle_output_startup_result(&mut output, monitor, Ok(()))?;
    let scores_result = (|| {
        if no_scores {
            return Ok(None);
        }
        let path = if let Some(path) = scores_db {
            path.to_path_buf()
        } else {
            let root = env::var_os("XDG_DATA_HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .or_else(|| {
                    env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
                })
                .ok_or_else(|| "scores database requires XDG_DATA_HOME or HOME".to_owned())?;
            root.join("scorepeek/scores.sqlite3")
        };
        let path =
            std::path::absolute(path).map_err(|error| format!("scores database path: {error}"))?;
        output.enable_scores(&path)?;
        Ok(Some(path))
    })();
    let scores_path = settle_output_startup_result(&mut output, monitor, scores_result)?;
    let mut overlay_children = crate::overlay::supervisor::Children::default();
    let overlay_config_path = overlays
        .config_path
        .unwrap_or_else(scorepeek_overlay_wayland::config::default_path);
    let requested_backends = [
        overlays
            .wayland
            .then_some(scorepeek_overlay_wayland::bridge::data::Backend::Wayland),
        overlays
            .obs
            .then_some(scorepeek_overlay_wayland::bridge::data::Backend::Obs),
    ];
    settle_output_startup_result(&mut output, monitor, Ok(()))?;
    let loaded_overlay_config_result = if overlays.wayland || overlays.obs {
        Some(scorepeek_overlay_wayland::config::load_or_create(
            &overlay_config_path,
        ))
        .transpose()
    } else {
        Ok(None)
    };
    let loaded_overlay_config =
        settle_output_startup_result(&mut output, monitor, loaded_overlay_config_result)?;
    let overlay_controller = loaded_overlay_config
        .as_ref()
        .map(|(config, _)| {
            crate::config::control::Controller::start(&overlay_config_path, config.clone())
        })
        .transpose();
    let overlay_controller =
        settle_output_startup_result(&mut output, monitor, overlay_controller)?;
    let warning_result = (|| {
        if let Some((_, issues)) = &loaded_overlay_config {
            for issue in issues {
                output.warning(format!(
                    "overlay canvas {} ignored: {}",
                    issue.canvas_id, issue.message
                ))?;
            }
        }
        Ok(())
    })();
    settle_output_startup_result(&mut output, monitor, warning_result)?;
    for backend in requested_backends.into_iter().flatten() {
        settle_output_startup_result(&mut output, monitor, Ok(()))?;
        let (overlay_config, _) = loaded_overlay_config
            .as_ref()
            .expect("overlay config loaded");
        let validation_result = overlay_config.validated().map(|validated| {
            validated
                .0
                .into_iter()
                .filter(|canvas| canvas.backend == backend)
                .collect::<Vec<_>>()
        });
        let canvases = settle_output_startup_result(&mut output, monitor, validation_result)?;
        let started = output
            .event_socket_path()
            .ok_or_else(|| "event socket unavailable".to_owned())
            .and_then(|socket| {
                let executable = std::env::current_exe().map_err(|error| error.to_string())?;
                overlay_children.start(
                    &executable,
                    &scorepeek_overlay_wayland::bridge::data::Config {
                        backend,
                        canvases,
                        config_path: overlay_config_path.clone(),
                        control_socket: overlay_controller
                            .as_ref()
                            .expect("overlay controller started")
                            .path()
                            .to_owned(),
                        skin_store: scorepeek_overlay_wayland::skin::StoreRoot::discover()
                            .path()
                            .to_owned(),
                        socket: socket.to_path_buf(),
                        invocation: invocation_id.clone(),
                        scores_db: scores_path.clone(),
                        listen: overlay_config
                            .obs_listen
                            .parse()
                            .map_err(|error| format!("overlay obs_listen: {error}"))?,
                        unknown_grace_ms: overlay_config.unknown_grace_ms,
                        edit_on_start: backend
                            == scorepeek_overlay_wayland::bridge::data::Backend::Wayland
                            && (overlays.wayland_edit
                                || !overlay_config
                                    .canvases
                                    .iter()
                                    .any(|canvas| canvas.backend == backend)),
                    },
                )
            });
        if let Err(error) = settle_output_startup_result(&mut output, monitor, started) {
            if error == INTERRUPTED_ERROR || error == TERMINATED_ERROR {
                return Err(error);
            }
            let publish_result = output.publish(&RunEvent {
                schema: RUN_EVENT_SCHEMA.to_owned(),
                kind: RunEventKind::OverlayObserved {
                    observation: serde_json::json!({
                        "backend": format!("{backend:?}"),
                        "operation": "spawn",
                        "error_type": "start_failed",
                        "error": error,
                    }),
                },
            });
            settle_output_startup_result(&mut output, monitor, publish_result)?;
            return Err(format!(
                "{backend:?} overlay initialization failed: {error}"
            ));
        }
    }
    let watcher_started = output.publish(&RunEvent {
        schema: RUN_EVENT_SCHEMA.to_owned(),
        kind: RunEventKind::WatcherStarted {
            invocation_id: invocation_id.clone(),
        },
    });
    settle_output_startup_result(&mut output, monitor, watcher_started)?;

    let mut lifetimes = routine_watcher::SourceLifetimes::new();
    settle_output_startup_result(&mut output, monitor, Ok(()))?;
    let vulkan_listener_result = match capture {
        RoutineCapture::VulkanLayer => run_startup_stage(
            &diagnostic_sink,
            "vulkan_capture_socket",
            scorepeek::capture::vulkan::VulkanListener::bind_default,
        )
        .map(Some),
        RoutineCapture::Pipewire { .. } => Ok(None),
    };
    let vulkan_listener =
        settle_output_startup_result(&mut output, monitor, vulkan_listener_result)?;
    let mut vulkan_generation = 0_u64;
    let mut announced = None;
    while !stop.load(std::sync::atomic::Ordering::Acquire) {
        output.refresh_scores()?;
        output.refresh_overlays(&mut overlay_children, overlay_controller.as_ref())?;
        let mut vulkan_session = None;
        let decision = match capture {
            RoutineCapture::Pipewire { node_name } => {
                let Ok(snapshot) = scorepeek::capture::snapshot_pipewire_sources(
                    node_name,
                    std::time::Duration::from_millis(500),
                ) else {
                    announce_watcher_state(
                        &mut announced,
                        routine_watcher::WatcherState::RemoteUnavailable,
                        "PipeWire is unavailable; scorepeek will keep waiting",
                        &mut output,
                    )?;
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    continue;
                };
                lifetimes.observe(snapshot)
            }
            RoutineCapture::VulkanLayer => {
                let accepted = vulkan_listener
                    .as_ref()
                    .expect("Vulkan listener exists")
                    .accept(std::time::Duration::from_millis(500));
                let Some(session) = (match accepted {
                    Ok(session) => session,
                    Err(failure) => {
                        let (category, producer_status) = failure.diagnostic();
                        let fact = serde_json::to_value(
                            scorepeek::capture::CaptureDiagnosticFact {
                                sequence: 0,
                                monotonic_start_ms: 0,
                                monotonic_end_ms: 0,
                                operation:
                                    scorepeek::capture::CaptureDiagnosticOperation::SourceAcquisition,
                                status: scorepeek::capture::CaptureDiagnosticStatus::Error,
                                error_type: Some(
                                    scorepeek::capture::CaptureErrorType::ReceiverFailed,
                                ),
                                detail:
                                    scorepeek::capture::CaptureDiagnosticDetail::VulkanFailure {
                                        category,
                                        producer_status,
                                    },
                            },
                        )
                        .map_err(|error| format!("serialize Vulkan admission failure: {error}"))?;
                        output.record_diagnostic("capture", &fact, true);
                        return Err(format!("Vulkan producer admission failed ({category})"));
                    }
                }) else {
                    announce_watcher_state(
                        &mut announced,
                        routine_watcher::WatcherState::WaitingForSource,
                        "waiting for a Vulkan-layer producer",
                        &mut output,
                    )?;
                    continue;
                };
                vulkan_generation = vulkan_generation.saturating_add(1);
                vulkan_session = Some(session);
                routine_watcher::WatchDecision::Admit {
                    node_id: 0,
                    generation: vulkan_generation,
                }
            }
        };
        if stop.load(std::sync::atomic::Ordering::Acquire) {
            break;
        }
        match decision {
            routine_watcher::WatchDecision::WaitAbsent
            | routine_watcher::WatchDecision::WaitConsumed => {
                announce_watcher_state(
                    &mut announced,
                    routine_watcher::WatcherState::WaitingForSource,
                    "waiting for the selected capture source",
                    &mut output,
                )?;
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            routine_watcher::WatchDecision::WaitAmbiguous => {
                announce_watcher_state(
                    &mut announced,
                    routine_watcher::WatcherState::AmbiguousSources,
                    "multiple matching PipeWire sources are present; waiting for exactly one",
                    &mut output,
                )?;
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            routine_watcher::WatchDecision::Admit {
                node_id,
                generation,
            } => {
                let session_id = format!("{invocation_id}-session-{generation}");
                let diagnostic_run_root = output.diagnostic_run_root().map(Path::to_path_buf);
                let session_paths = match state
                    .start_recording_session(diagnostic_run_root.as_deref(), &session_id)
                {
                    Ok(paths) => paths,
                    Err(error) => {
                        output.warning(format!("recording degraded for this session: {error}"))?;
                        None
                    }
                };
                let runtime_capture = match capture {
                    RoutineCapture::Pipewire { node_name } => {
                        capture_live::RuntimeCaptureInput::Pipewire {
                            node_name,
                            crop,
                            expected_node_id: Some(node_id),
                        }
                    }
                    RoutineCapture::VulkanLayer => capture_live::RuntimeCaptureInput::VulkanLayer {
                        session: Box::new(vulkan_session.take().expect("admitted Vulkan session")),
                        crop,
                    },
                };
                if stop.load(std::sync::atomic::Ordering::Acquire) {
                    if let Some(paths) = session_paths.as_ref()
                        && let Err(error) = paths.cleanup()
                    {
                        output.warning(error)?;
                    }
                    break;
                }
                let mut started = false;
                let mut emit = |emission: LiveSessionEmission| {
                    output.refresh_scores()?;
                    output.refresh_overlays(&mut overlay_children, overlay_controller.as_ref())?;
                    let output_started = std::time::Instant::now();
                    if let Some(identity) = emission.diagnostic_identity.as_ref() {
                        output.record_diagnostic("capture_session_identity", identity, true);
                    }
                    if let Some(fact) = emission.diagnostic_capture_fact.as_ref() {
                        output.record_diagnostic("capture", fact, true);
                        return Ok(capture_live::LiveEventProcessingTiming::default());
                    }
                    let event = run_event_from_live_emission(emission)?;
                    if matches!(&event.kind, RunEventKind::SessionStarted { .. }) {
                        started = true;
                    }
                    let output_overhead_us =
                        u64::try_from(output_started.elapsed().as_micros()).unwrap_or(u64::MAX);
                    let timing = output.publish_timed(&event)?;
                    Ok(capture_live::LiveEventProcessingTiming {
                        screen_resolver_us: timing.screen_resolver_us,
                        attempt_resolver_us: timing.attempt_resolver_us,
                        output_us: Some(
                            timing
                                .output_us
                                .unwrap_or(0)
                                .saturating_add(output_overhead_us),
                        ),
                    })
                };
                let report = execute_live_session(
                    &catalog_root,
                    bundle,
                    &session_id,
                    &build_sha256,
                    &run_catalog_digest,
                    recording_memory_limit,
                    recording_retention,
                    session_paths.as_ref().map(|paths| paths.root.as_path()),
                    runtime_capture,
                    &stop,
                    &mut emit,
                )?;
                if report.output_failed() {
                    return Err("live result output failed".to_owned());
                }
                if state.recording_enabled {
                    record_canonical_publication_summary(
                        &output,
                        session_paths.as_ref().map(|paths| paths.root.as_path()),
                    );
                }
                if started {
                    let stop_reason = report.stop_reason();
                    let (outcome, readmit_same_node, fatal) =
                        routine_session_disposition(stop_reason);
                    if matches!(capture, RoutineCapture::Pipewire { .. }) {
                        lifetimes.generation_ended(node_id, readmit_same_node);
                    }
                    announced = None;
                    output.publish(&RunEvent {
                        schema: RUN_EVENT_SCHEMA.to_owned(),
                        kind: RunEventKind::SessionFinished {
                            session_id: session_id.clone(),
                            outcome: outcome.to_owned(),
                            report: serde_json::to_value(&report).map_err(|error| {
                                format!("live report serialization failed: {error}")
                            })?,
                        },
                    })?;
                    if state.recording_enabled {
                        if report.canonical_recording_is_complete() {
                            if let Some(session_paths) = session_paths.as_ref() {
                                output.publish(&RunEvent {
                                    schema: RUN_EVENT_SCHEMA.to_owned(),
                                    kind: RunEventKind::RecordingCompleted {
                                        session_id: session_id.clone(),
                                        directory: session_paths.root.display().to_string(),
                                    },
                                })?;
                            }
                        } else {
                            output.status_recording_degraded()?;
                            output
                                .warning("canonical recording is partial and cannot be imported")?;
                        }
                    }
                    if fatal {
                        return Err(report
                            .failure_detail()
                            .unwrap_or("capture session failed")
                            .to_owned());
                    }
                } else {
                    let startup_report = serde_json::to_value(&report).map_err(|error| {
                        format!("capture startup report serialization failed: {error}")
                    })?;
                    output.record_diagnostic("capture_startup_failure", &startup_report, true);
                    if let Some(paths) = session_paths.as_ref()
                        && let Err(error) = paths.cleanup()
                    {
                        output.warning(error)?;
                    }
                    match report.startup_retry() {
                        Some(capture_live::LiveSessionStartupRetry::Admission)
                            if report
                                .capture_error_type()
                                .is_some_and(transient_admission_capture_error) =>
                        {
                            announce_watcher_state(
                                &mut announced,
                                routine_watcher::WatcherState::WaitingForSource,
                                "capture source disappeared during admission; scorepeek will keep waiting",
                                &mut output,
                            )?;
                        }
                        Some(capture_live::LiveSessionStartupRetry::Admission) => {
                            return Err(report.startup_failure_summary());
                        }
                        Some(capture_live::LiveSessionStartupRetry::Catalog) => {
                            announce_watcher_state(
                                &mut announced,
                                routine_watcher::WatcherState::CatalogUnavailable,
                                "active catalog changed or is temporarily unavailable; scorepeek will retry",
                                &mut output,
                            )?;
                        }
                        None => return Err(report.startup_failure_summary()),
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
        }
    }
    overlay_children.shutdown();
    output.refresh_overlays(&mut overlay_children, overlay_controller.as_ref())?;
    output.publish(&RunEvent {
        schema: RUN_EVENT_SCHEMA.to_owned(),
        kind: RunEventKind::WatcherStopped {
            invocation_id,
            reason: "signal".to_owned(),
        },
    })?;
    if let Some(handle) = background_catalog_update {
        let _ = handle.join();
    }
    output.finish_diagnostics("cancel");
    if monitor.interrupted() {
        Err(INTERRUPTED_ERROR.to_owned())
    } else {
        Ok(())
    }
}

pub(super) fn record_catalog_update(
    sink: &diagnostic_stream::DiagnosticSink,
    event: &crate::resources::catalog::acquire::UpdateEvent,
) {
    if let Ok(value) = serde_json::to_value(event) {
        sink.record("catalog_update", &value, true);
    }
}

pub(super) fn routine_session_disposition(
    reason: Option<capture_live::LiveSessionStopReason>,
) -> (&'static str, bool, bool) {
    match reason {
        Some(capture_live::LiveSessionStopReason::RequestedSignal) => ("stopped", false, false),
        Some(
            capture_live::LiveSessionStopReason::SourceEnded
            | capture_live::LiveSessionStopReason::SourceContractChanged,
        ) => ("source_ended", true, false),
        Some(capture_live::LiveSessionStopReason::TerminalFailure) | None => ("error", false, true),
    }
}

pub(super) const fn transient_admission_capture_error(
    error_type: scorepeek::capture::CaptureErrorType,
) -> bool {
    matches!(
        error_type,
        scorepeek::capture::CaptureErrorType::SourceUnavailable
            | scorepeek::capture::CaptureErrorType::SourceLost
            | scorepeek::capture::CaptureErrorType::StreamLost
    )
}

pub(super) fn announce_watcher_state(
    announced: &mut Option<routine_watcher::WatcherState>,
    state: routine_watcher::WatcherState,
    message: &str,
    output: &mut routine_output::RoutineOutput,
) -> Result<(), String> {
    output.watcher_state(state.as_str(), None, message)?;
    if *announced != Some(state) {
        *announced = Some(state);
    }
    Ok(())
}

pub(super) fn record_canonical_publication_summary(
    output: &routine_output::RoutineOutput,
    canonical_root: Option<&Path>,
) {
    let Some(root) = canonical_root else {
        output.record_diagnostic(
            "recording_summary",
            &serde_json::json!({
                "status": "unavailable", "error_type": "recording_root_unavailable"
            }),
            true,
        );
        return;
    };
    let manifest_path = root.join("canonical-manifest.json");
    let manifest = std::fs::read(&manifest_path)
        .ok()
        .filter(|bytes| bytes.len() <= 1024 * 1024)
        .and_then(|bytes| {
            serde_json::from_slice::<
                    scorepeek_core::canonical_recording::CanonicalRecordingManifest,
                >(&bytes)
                .ok()
                .map(|manifest| (manifest, bytes.len() as u64))
        });
    let Some((manifest, manifest_bytes)) = manifest else {
        output.record_diagnostic(
            "recording_summary",
            &serde_json::json!({
                "status": "unavailable", "error_type": "manifest_unavailable", "locator": root
            }),
            true,
        );
        return;
    };
    let retained = manifest
        .segments
        .iter()
        .fold(0_u64, |count, segment| count.saturating_add(segment.frames));
    let segment_bytes = manifest
        .segments
        .iter()
        .fold(0_u64, |bytes, segment| bytes.saturating_add(segment.bytes));
    output.record_diagnostic("recording_summary", &serde_json::json!({
        "status": if manifest.completeness == scorepeek_core::canonical_recording::Completion::Complete { "complete" } else { "partial" },
        "locator": root,
        "session_id": manifest.session_id,
        "retained_frames": retained,
        "elided_ticks": manifest.tick_count.saturating_sub(retained),
        "bytes": manifest_bytes.saturating_add(manifest.tick_index.bytes).saturating_add(segment_bytes),
    }), true);
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) fn execute_live_session(
    catalog_root: &Path,
    bundle_root: &Path,
    session_id: &str,
    build_sha256: &str,
    catalog_sha256: &str,
    recording_memory_limit: RecordingMemoryLimit,
    recording_retention: RecordingRetention,
    canonical_recording_root: Option<&Path>,
    runtime_capture: capture_live::RuntimeCaptureInput<'_>,
    stop: &std::sync::atomic::AtomicBool,
    emit: &mut impl FnMut(
        LiveSessionEmission,
    ) -> Result<capture_live::LiveEventProcessingTiming, String>,
) -> Result<capture_live::CaptureSessionReport, String> {
    let execution_context = crate::service::session::recognition::RecognitionExecutionContext::new(
        session_id.to_owned(),
        CanonicalLayout::sha256(),
        catalog_sha256.to_owned(),
        recognition_title::LIVE_MODEL_SHA256.to_owned(),
        recognition_title::LIVE_RUNTIME_SHA256.to_owned(),
    )
    .ok_or_else(|| "recognition session context is invalid".to_owned())?;
    let descriptor = DiagnosticRunDescriptor {
        run_id: execution_context.session_id.clone(),
        monotonic_start_ms: 0,
        resource: DiagnosticResource {
            program: "scorepeek",
            version: env!("CARGO_PKG_VERSION"),
            build_sha256: build_sha256.to_owned(),
        },
        binding: DiagnosticBinding {
            canonical_layout_sha256: execution_context.canonical_layout_sha256.clone(),
            catalog_sha256: execution_context.catalog_sha256.clone(),
            model_sha256: execution_context.model_sha256.clone(),
            runtime_sha256: execution_context.runtime_sha256.clone(),
            replay: None,
        },
    };
    let policy = DiagnosticPolicy {
        enabled: false,
        ..DiagnosticPolicy::default()
    };
    let resource_revisions = serde_json::json!({
        "canonical_layout_sha256": descriptor.binding.canonical_layout_sha256,
        "catalog_sha256": descriptor.binding.catalog_sha256,
        "model_sha256": descriptor.binding.model_sha256,
        "runtime_sha256": descriptor.binding.runtime_sha256,
    });
    let report = capture_live::run_runtime_live_session(
        capture_live::LiveCaptureSessionConfig {
            execution_context,
            descriptor,
            diagnostic_policy: policy,
            diagnostic_root: Path::new("/"),
            diagnostic_directory_name: Some("capture"),
            catalog_root,
            bundle_root,
            canonical_recording_root,
            recording_memory_limit,
            recording_retention,
            runtime_capture,
        },
        stop,
        &mut |event| {
            let started = std::time::Instant::now();
            let diagnostic_identity = match event {
                capture_live::CaptureSessionEvent::Started { source_evidence } => {
                    Some(serde_json::json!({
                        "session_id": session_id,
                        "source": source_evidence,
                        "resources": resource_revisions,
                    }))
                }
                _ => None,
            };
            let diagnostic_capture_fact = match event {
                capture_live::CaptureSessionEvent::CaptureDiagnostic { fact } => {
                    Some(serde_json::to_value(fact).map_err(|error| {
                        format!("capture diagnostic serialization failed: {error}")
                    })?)
                }
                _ => None,
            };
            let authority_joint_evidence = match &event {
                capture_live::CaptureSessionEvent::Observation { output, .. } => {
                    Some(output.joint_evidence().clone())
                }
                _ => None,
            };
            let value = live_session_event_value(Some(session_id), None, event)?;
            let serialization_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            let mut timing = emit(LiveSessionEmission {
                value,
                authority_joint_evidence,
                diagnostic_identity,
                diagnostic_capture_fact,
            })?;
            timing.add(capture_live::LiveEventProcessingTiming {
                screen_resolver_us: None,
                attempt_resolver_us: None,
                output_us: Some(serialization_us),
            });
            Ok(timing)
        },
    );
    Ok(report)
}
