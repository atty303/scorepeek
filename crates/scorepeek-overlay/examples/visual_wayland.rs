use scorepeek_overlay::{
    control::Controller,
    runtime::{Backend, Config},
};
use serde_json::json;
use std::{
    io::{BufRead as _, BufReader, Write as _},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

struct TimedLease(Duration);

impl std::io::Read for TimedLease {
    fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
        std::thread::sleep(self.0);
        Ok(0)
    }
}

fn fixture_event(sequence: u64, screen: &str) -> serde_json::Value {
    json!({
        "schema": "scorepeek-event-v3",
        "invocation_id": "overlay-visual-wayland",
        "sequence": sequence,
        "event_id": format!("overlay-visual-wayland:{sequence}"),
        "emitted_monotonic_ms": sequence * 1_000,
        "emitted_unix_ms": 1_000 + i64::try_from(sequence).unwrap_or(i64::MAX) * 1_000,
        "capture": null,
        "event": "screen_state_changed",
        "state": {
            "screen_episode_id": sequence,
            "screen": screen,
            "suspended": false,
        },
    })
}

struct IntegrationFeed {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<std::io::Result<()>>>,
    event_socket: PathBuf,
    trigger_socket: PathBuf,
}

impl IntegrationFeed {
    fn shutdown(&mut self) -> Result<(), String> {
        self.stop.store(true, Ordering::Release);
        let joined = self.handle.take().map_or(Ok(()), |handle| {
            handle
                .join()
                .map_err(|_| "integration feed panicked".to_owned())?
                .map_err(|error| format!("integration feed failed: {error}"))
        });
        let event_cleanup = remove_fixture_socket(&self.event_socket, "event");
        let trigger_cleanup = remove_fixture_socket(&self.trigger_socket, "trigger");
        joined?;
        event_cleanup?;
        trigger_cleanup?;
        Ok(())
    }

    fn finish(mut self) -> Result<(), String> {
        self.shutdown()
    }
}

impl Drop for IntegrationFeed {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn remove_fixture_socket(path: &Path, name: &str) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove fixture {name} socket: {error}")),
    }
}

fn accept_until_stopped(
    listener: &UnixListener,
    stop: &AtomicBool,
) -> std::io::Result<Option<UnixStream>> {
    listener.set_nonblocking(true)?;
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => return Ok(Some(stream)),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}

fn start_integration_feed(socket: &Path, trigger: &Path) -> std::io::Result<IntegrationFeed> {
    let listener = UnixListener::bind(socket)?;
    let trigger_listener = match UnixListener::bind(trigger) {
        Ok(listener) => listener,
        Err(error) => {
            let _ = std::fs::remove_file(socket);
            return Err(error);
        }
    };
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let handle = match std::thread::Builder::new()
        .name("overlay-wayland-fixture-feed".into())
        .spawn(move || {
            let Some(mut stream) = accept_until_stopped(&listener, &thread_stop)? else {
                return Ok(());
            };
            let snapshot = json!({
                "schema": "scorepeek-event-snapshot-v3",
                "invocation_id": "overlay-visual-wayland",
                "next_sequence": 1,
                "status": {
                    "watcher": "session_active",
                    "capture": null,
                    "catalog": "ready",
                    "model": "ready",
                    "scores": null,
                    "recording": null,
                    "last_session_outcome": null,
                },
                "result": {
                    "schema": "scorepeek-event-v3",
                    "invocation_id": "overlay-visual-wayland",
                    "sequence": 0,
                    "event_id": "overlay-visual-wayland:0",
                    "emitted_monotonic_ms": 0,
                    "emitted_unix_ms": 1_000,
                    "capture": null,
                    "event": "result_changed",
                    "source_sequence": 0,
                    "state": {"status": "inactive"},
                },
                "screen_state": null,
                "music_selection": null,
                "music_select_best": null,
            });
            writeln!(stream, "{snapshot}")?;
            stream.flush()?;
            let Some(trigger) = accept_until_stopped(&trigger_listener, &thread_stop)? else {
                return Ok(());
            };
            trigger.set_read_timeout(Some(Duration::from_millis(100)))?;
            let mut commands = BufReader::new(trigger).lines();
            for (sequence, expected) in [(1, "music_select"), (2, "play"), (3, "music_select")] {
                let command = loop {
                    if thread_stop.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    match commands.next() {
                        Some(Ok(command)) => break command,
                        Some(Err(error))
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) => {}
                        Some(Err(error)) => return Err(error),
                        None => {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "fixture trigger closed before all screen transitions",
                            ));
                        }
                    }
                };
                if command != expected {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("expected fixture transition {expected}, got {command}"),
                    ));
                }
                let screen = expected;
                writeln!(stream, "{}", fixture_event(sequence, screen))?;
                stream.flush()?;
            }
            Ok(())
        }) {
        Ok(handle) => handle,
        Err(error) => {
            let _ = std::fs::remove_file(socket);
            let _ = std::fs::remove_file(trigger);
            return Err(error);
        }
    };
    Ok(IntegrationFeed {
        stop,
        handle: Some(handle),
        event_socket: socket.to_owned(),
        trigger_socket: trigger.to_owned(),
    })
}

fn load_document(
    source_config: Option<std::ffi::OsString>,
    integration_fixture: bool,
) -> Result<scorepeek_overlay::config::OverlayConfig, Box<dyn std::error::Error>> {
    if integration_fixture {
        let mut document = scorepeek_overlay::config::visual_debug_config(
            "dev.atty303.scorepeek.skin.cyan-system".parse()?,
        );
        for (index, canvas) in document
            .canvases
            .iter_mut()
            .filter(|canvas| canvas.backend == Backend::Wayland)
            .enumerate()
        {
            canvas.output = if index % 2 == 0 {
                "HEADLESS-1"
            } else {
                "HEADLESS-2"
            }
            .into();
            canvas
                .skin_properties
                .insert("background".into(), serde_json::json!("animated"));
            if canvas.id == "wayland-status" {
                canvas.show_on = Some(scorepeek_overlay_ui::editor_model::SCREENS.to_vec());
            }
            if let Some(widget) = canvas.widgets.first().cloned() {
                let mut extra = widget;
                extra.id = format!("{}-nested-extra", canvas.id);
                extra.y = extra.y.saturating_add(
                    i32::try_from(extra.height.saturating_add(24)).unwrap_or(i32::MAX),
                );
                canvas.widgets.push(extra);
            }
        }
        return Ok(document);
    }
    let Some(source_config) = source_config else {
        return Ok(scorepeek_overlay::config::OverlayConfig::initial());
    };
    let bytes = std::fs::read(&source_config)?;
    let document: scorepeek_overlay::config::OverlayConfig = toml::from_slice(&bytes)?;
    if document.schema_version != scorepeek_overlay::config::SCHEMA_VERSION {
        return Err("SOURCE_CONFIG.toml must already use the current schema".into());
    }
    document.validated()?;
    Ok(document)
}

fn parse_args() -> Result<(std::path::PathBuf, u64, Option<std::ffi::OsString>), String> {
    let mut args = std::env::args_os().skip(1);
    let config_path = args
        .next()
        .ok_or("usage: visual_wayland CONFIG.toml [SECONDS]")?;
    let seconds = args.next().map_or(Ok(30_u64), |value| {
        value
            .to_str()
            .ok_or("SECONDS must be UTF-8")?
            .parse::<u64>()
            .map_err(|error| format!("SECONDS: {error}"))
    })?;
    let source_config = args.next();
    if seconds == 0 || args.next().is_some() {
        return Err("usage: visual_wayland CONFIG.toml [SECONDS>0] [SOURCE_CONFIG.toml]".into());
    }
    Ok((
        std::path::PathBuf::from(config_path),
        seconds,
        source_config,
    ))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (config_path, seconds, source_config) = parse_args()?;
    let parent = config_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent)?;
    let integration_fixture =
        source_config.as_deref() == Some(std::ffi::OsStr::new("--integration-fixture"));
    let document = load_document(source_config, integration_fixture)?;
    let event_socket = parent.join("events.sock");
    let feed_trigger = parent.join("feed-trigger.sock");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&config_path)
        .map_err(|error| format!("CONFIG.toml must not already exist: {error}"))?;
    file.write_all(toml::to_string_pretty(&document)?.as_bytes())?;
    file.sync_all()?;
    let controller = Controller::start(&config_path, document.clone())?;
    let fixture_feed = integration_fixture
        .then(|| start_integration_feed(&event_socket, &feed_trigger))
        .transpose()?;
    let runtime_config = || Config {
        backend: Backend::Wayland,
        canvases: document
            .canvases
            .iter()
            .filter(|canvas| canvas.backend == Backend::Wayland)
            .cloned()
            .collect(),
        config_path: config_path.clone(),
        control_socket: controller.path().to_owned(),
        skin_store: scorepeek_overlay::skin::StoreRoot::discover()
            .path()
            .to_owned(),
        socket: if integration_fixture {
            event_socket.clone()
        } else {
            std::env::temp_dir().join(format!(
                "scorepeek-visual-absent-{}.sock",
                std::process::id()
            ))
        },
        invocation: "overlay-visual-wayland".into(),
        scores_db: None,
        listen: document.obs_listen.parse().expect("fixture listen address"),
        unknown_grace_ms: document.unknown_grace_ms,
        edit_on_start: true,
    };
    let scenario = if integration_fixture {
        use scorepeek_overlay::native::NativeEditorScenarioStep;
        use scorepeek_overlay_ui::editor::EditorAction;

        vec![
            NativeEditorScenarioStep {
                after: Duration::from_secs(1),
                target_canvas: "wayland-status",
                name: "canvas_output_moved",
                action: EditorAction::Output("HEADLESS-2".into()),
            },
            NativeEditorScenarioStep {
                after: Duration::from_millis(400),
                target_canvas: "wayland-status",
                name: "canvas_visibility_none",
                action: EditorAction::CanvasVisibleNone,
            },
            NativeEditorScenarioStep {
                after: Duration::from_millis(400),
                target_canvas: "wayland-status",
                name: "canvas_visibility_all",
                action: EditorAction::CanvasVisibleAll,
            },
            NativeEditorScenarioStep {
                after: Duration::from_millis(400),
                target_canvas: "wayland-status",
                name: "canvas_deleted",
                action: EditorAction::DeleteCanvas,
            },
        ]
    } else {
        Vec::new()
    };
    eprintln!("Wayland editor will remain open for {seconds} seconds.");
    let result = scorepeek_overlay::native::run_with_editor_scenario(
        runtime_config(),
        TimedLease(Duration::from_secs(seconds)),
        scenario,
    );
    if let Some(feed) = fixture_feed {
        feed.finish()?;
    }
    result?;
    drop(controller);
    Ok(())
}
