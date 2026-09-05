use crate::{
    config::{OverlayConfig, empty_canvas, save_atomic},
    runtime::Backend,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{BufRead as _, BufReader, Write as _},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const LEASE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Request {
    Acquire {
        canvas_id: String,
        editor_id: String,
    },
    KeepAlive {
        canvas_id: String,
        editor_id: String,
    },
    Release {
        canvas_id: String,
        editor_id: String,
    },
    ReplaceCanvas {
        canvas_id: String,
        editor_id: String,
        expected_revision: u64,
        presentation: scorepeek_overlay_ui::CanvasPresentation,
    },
    SetGeometry {
        canvas_id: String,
        editor_id: String,
        expected_revision: u64,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    SetOutput {
        canvas_id: String,
        editor_id: String,
        expected_revision: u64,
        output: String,
    },
    SetUnknownGrace {
        canvas_id: String,
        editor_id: String,
        expected_revision: u64,
        unknown_grace_ms: u32,
    },
    ListCanvases {
        backend: Backend,
    },
    AddCanvas {
        backend: Backend,
        expected_revision: u64,
        canvas_id: String,
    },
    DeleteCanvas {
        backend: Backend,
        expected_revision: u64,
        canvas_id: String,
    },
    SetCanvasEnabled {
        backend: Backend,
        expected_revision: u64,
        canvas_id: String,
        enabled: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CanvasSummary {
    pub id: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Response {
    pub ok: bool,
    pub readonly: bool,
    pub canvas: Option<scorepeek_overlay_ui::CanvasPresentation>,
    pub error: Option<String>,
    #[serde(default)]
    pub canvases: Vec<CanvasSummary>,
    pub backend_revision: Option<u64>,
    pub settings_revision: Option<u64>,
    pub unknown_grace_ms: Option<u32>,
}

struct Lease {
    editor_id: String,
    touched: Instant,
}
struct State {
    config: OverlayConfig,
    leases: BTreeMap<String, Lease>,
}

pub struct Controller {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Controller {
    /// Starts the parent-owned configuration writer.
    /// # Errors
    /// Returns socket or worker creation errors.
    pub fn start(path: &Path, config: OverlayConfig) -> Result<Self, String> {
        let runtime =
            std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
        let socket = runtime.join(format!(
            "scorepeek-overlay-control-{}.sock",
            std::process::id()
        ));
        if socket.exists() {
            std::fs::remove_file(&socket).map_err(|error| error.to_string())?;
        }
        let listener = UnixListener::bind(&socket)
            .map_err(|error| format!("bind {}: {error}", socket.display()))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let config_path = path.to_owned();
        let worker = std::thread::Builder::new()
            .name("overlay-config-writer".into())
            .spawn(move || {
                let state = Arc::new(Mutex::new(State {
                    config,
                    leases: BTreeMap::new(),
                }));
                while !stopping.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => handle(stream, &config_path, &state),
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(25));
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            path: socket,
            stop,
            worker: Some(worker),
        })
    }
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = UnixStream::connect(&self.path);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn handle(mut stream: UnixStream, path: &Path, shared: &Mutex<State>) {
    let response = read_request(&stream)
        .and_then(|request| apply(request, path, shared))
        .unwrap_or_else(|error| Response {
            ok: false,
            readonly: true,
            canvas: None,
            error: Some(error),
            canvases: Vec::new(),
            backend_revision: None,
            settings_revision: None,
            unknown_grace_ms: None,
        });
    if let Ok(mut bytes) = serde_json::to_vec(&response) {
        bytes.push(b'\n');
        let _ = stream.write_all(&bytes);
    }
}

fn read_request(stream: &UnixStream) -> Result<Request, String> {
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&line).map_err(|error| format!("overlay control request: {error}"))
}

#[allow(clippy::too_many_lines)]
fn apply(request: Request, path: &Path, shared: &Mutex<State>) -> Result<Response, String> {
    let mut state = shared
        .lock()
        .map_err(|_| "overlay control lock poisoned".to_owned())?;
    state
        .leases
        .retain(|_, lease| lease.touched.elapsed() < LEASE_TIMEOUT);
    match request {
        Request::Acquire {
            canvas_id,
            editor_id,
        } => {
            let canvas = state
                .config
                .canvases
                .iter()
                .find(|canvas| canvas.id == canvas_id)
                .cloned()
                .ok_or("canvas not found")?;
            let readonly = state
                .leases
                .get(&canvas_id)
                .is_some_and(|lease| lease.editor_id != editor_id);
            if !readonly {
                state.leases.insert(
                    canvas_id,
                    Lease {
                        editor_id,
                        touched: Instant::now(),
                    },
                );
            }
            Ok(Response {
                ok: true,
                readonly,
                canvas: Some(canvas.presentation()),
                error: None,
                canvases: Vec::new(),
                backend_revision: None,
                settings_revision: Some(state.config.settings_revision),
                unknown_grace_ms: Some(state.config.unknown_grace_ms),
            })
        }
        Request::KeepAlive {
            canvas_id,
            editor_id,
        } => {
            let lease = state
                .leases
                .get_mut(&canvas_id)
                .filter(|lease| lease.editor_id == editor_id)
                .ok_or("editor lease lost")?;
            lease.touched = Instant::now();
            Ok(Response {
                ok: true,
                readonly: false,
                canvas: None,
                error: None,
                canvases: Vec::new(),
                backend_revision: None,
                settings_revision: None,
                unknown_grace_ms: None,
            })
        }
        Request::Release {
            canvas_id,
            editor_id,
        } => {
            if state
                .leases
                .get(&canvas_id)
                .is_some_and(|lease| lease.editor_id == editor_id)
            {
                state.leases.remove(&canvas_id);
            }
            Ok(Response {
                ok: true,
                readonly: false,
                canvas: None,
                error: None,
                canvases: Vec::new(),
                backend_revision: None,
                settings_revision: None,
                unknown_grace_ms: None,
            })
        }
        Request::ReplaceCanvas {
            canvas_id,
            editor_id,
            expected_revision,
            presentation,
        } => {
            if presentation.id != canvas_id {
                return Err("canvas presentation id mismatch".into());
            }
            let lease = state
                .leases
                .get_mut(&canvas_id)
                .filter(|lease| lease.editor_id == editor_id)
                .ok_or("editor lease lost")?;
            lease.touched = Instant::now();
            let index = state
                .config
                .canvases
                .iter()
                .position(|current| current.id == canvas_id)
                .ok_or("canvas not found")?;
            let current = &state.config.canvases[index];
            if current.revision != expected_revision {
                return Err("canvas revision conflict".into());
            }
            let mut canvas = current.clone();
            canvas.skin = presentation.skin;
            canvas.show_on.clone_from(&presentation.show_on);
            canvas.opacity_percent = presentation.opacity_percent;
            canvas.z = presentation.z;
            canvas.widgets = presentation
                .widgets
                .iter()
                .map(|widget| crate::config::Widget {
                    id: widget.id.clone(),
                    kind: match widget.kind {
                        scorepeek_overlay_ui::WidgetKind::Status => {
                            crate::config::WidgetKind::Status
                        }
                        scorepeek_overlay_ui::WidgetKind::Selection => {
                            crate::config::WidgetKind::Selection
                        }
                        scorepeek_overlay_ui::WidgetKind::Score => crate::config::WidgetKind::Score,
                        scorepeek_overlay_ui::WidgetKind::HistoryList => {
                            crate::config::WidgetKind::HistoryList
                        }
                        scorepeek_overlay_ui::WidgetKind::HistoryGraph => {
                            crate::config::WidgetKind::HistoryGraph
                        }
                    },
                    x: widget.x,
                    y: widget.y,
                    width: widget.width,
                    height: widget.height,
                    z: widget.z,
                    settings: crate::config::WidgetSettings {
                        history_count: widget.settings.history_count,
                        graph_months: widget.settings.graph_months,
                    },
                })
                .collect();
            canvas.revision = expected_revision
                .checked_add(1)
                .ok_or("canvas revision exhausted")?;
            let previous = state.config.canvases[index].clone();
            state.config.canvases[index] = canvas.clone();
            if let Err(error) = save_atomic(path, &state.config) {
                state.config.canvases[index] = previous;
                return Err(error);
            }
            Ok(Response {
                ok: true,
                readonly: false,
                canvas: Some(canvas.presentation()),
                error: None,
                canvases: Vec::new(),
                backend_revision: None,
                settings_revision: None,
                unknown_grace_ms: None,
            })
        }
        Request::SetGeometry {
            canvas_id,
            editor_id,
            expected_revision,
            x,
            y,
            width,
            height,
        } => {
            let lease = state
                .leases
                .get_mut(&canvas_id)
                .filter(|lease| lease.editor_id == editor_id)
                .ok_or("editor lease lost")?;
            lease.touched = Instant::now();
            let index = state
                .config
                .canvases
                .iter()
                .position(|canvas| canvas.id == canvas_id)
                .ok_or("canvas not found")?;
            if state.config.canvases[index].revision != expected_revision {
                return Err("canvas revision conflict".into());
            }
            let previous = state.config.canvases[index].clone();
            let canvas = &mut state.config.canvases[index];
            canvas.x = x;
            canvas.y = y;
            canvas.initial_placement = None;
            canvas.width = width.max(32);
            canvas.height = height.max(32);
            canvas.revision = canvas
                .revision
                .checked_add(1)
                .ok_or("canvas revision exhausted")?;
            let response = canvas.presentation();
            if let Err(error) = save_atomic(path, &state.config) {
                state.config.canvases[index] = previous;
                return Err(error);
            }
            Ok(Response {
                ok: true,
                readonly: false,
                canvas: Some(response),
                error: None,
                canvases: Vec::new(),
                backend_revision: None,
                settings_revision: None,
                unknown_grace_ms: None,
            })
        }
        Request::SetOutput {
            canvas_id,
            editor_id,
            expected_revision,
            output,
        } => {
            let lease = state
                .leases
                .get_mut(&canvas_id)
                .filter(|lease| lease.editor_id == editor_id)
                .ok_or("editor lease lost")?;
            lease.touched = Instant::now();
            let index = state
                .config
                .canvases
                .iter()
                .position(|canvas| canvas.id == canvas_id)
                .ok_or("canvas not found")?;
            if state.config.canvases[index].revision != expected_revision {
                return Err("canvas revision conflict".into());
            }
            let previous = state.config.canvases[index].clone();
            let canvas = &mut state.config.canvases[index];
            canvas.output = Some(output);
            canvas.revision = canvas
                .revision
                .checked_add(1)
                .ok_or("canvas revision exhausted")?;
            let response = canvas.presentation();
            if let Err(error) = save_atomic(path, &state.config) {
                state.config.canvases[index] = previous;
                return Err(error);
            }
            Ok(Response {
                ok: true,
                readonly: false,
                canvas: Some(response),
                error: None,
                canvases: Vec::new(),
                backend_revision: None,
                settings_revision: None,
                unknown_grace_ms: None,
            })
        }
        Request::SetUnknownGrace {
            canvas_id,
            editor_id,
            expected_revision,
            unknown_grace_ms,
        } => {
            let lease = state
                .leases
                .get_mut(&canvas_id)
                .filter(|lease| lease.editor_id == editor_id)
                .ok_or("editor lease lost")?;
            lease.touched = Instant::now();
            if state.config.settings_revision != expected_revision {
                return Err("overlay settings revision conflict".into());
            }
            if unknown_grace_ms > 10_000 {
                return Err("overlay unknown_grace_ms must be at most 10000".into());
            }
            let previous = state.config.clone();
            state.config.unknown_grace_ms = unknown_grace_ms;
            state.config.settings_revision = expected_revision
                .checked_add(1)
                .ok_or("overlay settings revision exhausted")?;
            persist_or_rollback(path, &mut state.config, previous)?;
            Ok(Response {
                ok: true,
                readonly: false,
                canvas: None,
                error: None,
                canvases: Vec::new(),
                backend_revision: None,
                settings_revision: Some(state.config.settings_revision),
                unknown_grace_ms: Some(state.config.unknown_grace_ms),
            })
        }
        Request::ListCanvases { backend } => Ok(manager_response(&state.config, backend)),
        Request::AddCanvas {
            backend,
            expected_revision,
            canvas_id,
        } => {
            require_backend_revision(&state.config, backend, expected_revision)?;
            let previous = state.config.clone();
            state.config.canvases.push(empty_canvas(canvas_id, backend));
            state.config.backend_revisions.increment(backend)?;
            persist_or_rollback(path, &mut state.config, previous)?;
            Ok(manager_response(&state.config, backend))
        }
        Request::DeleteCanvas {
            backend,
            expected_revision,
            canvas_id,
        } => {
            require_backend_revision(&state.config, backend, expected_revision)?;
            let previous = state.config.clone();
            state
                .config
                .canvases
                .retain(|canvas| !(canvas.backend == backend && canvas.id == canvas_id));
            if state.config.canvases.len() == previous.canvases.len() {
                return Err("canvas not found".into());
            }
            state.config.backend_revisions.increment(backend)?;
            persist_or_rollback(path, &mut state.config, previous)?;
            state.leases.remove(&canvas_id);
            Ok(manager_response(&state.config, backend))
        }
        Request::SetCanvasEnabled {
            backend,
            expected_revision,
            canvas_id,
            enabled,
        } => {
            require_backend_revision(&state.config, backend, expected_revision)?;
            let previous = state.config.clone();
            let canvas = state
                .config
                .canvases
                .iter_mut()
                .find(|canvas| canvas.backend == backend && canvas.id == canvas_id)
                .ok_or("canvas not found")?;
            canvas.enabled = enabled;
            state.config.backend_revisions.increment(backend)?;
            persist_or_rollback(path, &mut state.config, previous)?;
            Ok(manager_response(&state.config, backend))
        }
    }
}

fn require_backend_revision(
    config: &OverlayConfig,
    backend: Backend,
    expected: u64,
) -> Result<(), String> {
    if config.backend_revisions.get(backend) != expected {
        return Err("backend canvas-list revision conflict".into());
    }
    Ok(())
}

fn persist_or_rollback(
    path: &Path,
    config: &mut OverlayConfig,
    previous: OverlayConfig,
) -> Result<(), String> {
    if let Err(error) = save_atomic(path, config) {
        *config = previous;
        return Err(error);
    }
    Ok(())
}

fn manager_response(config: &OverlayConfig, backend: Backend) -> Response {
    Response {
        ok: true,
        readonly: false,
        canvas: None,
        error: None,
        canvases: config
            .canvases
            .iter()
            .filter(|canvas| canvas.backend == backend)
            .map(|canvas| CanvasSummary {
                id: canvas.id.clone(),
                enabled: canvas.enabled,
            })
            .collect(),
        backend_revision: Some(config.backend_revisions.get(backend)),
        settings_revision: Some(config.settings_revision),
        unknown_grace_ms: Some(config.unknown_grace_ms),
    }
}

/// Sends one typed request to the parent-owned configuration writer.
/// # Errors
/// Returns connection, serialization, I/O, or response decoding errors.
pub fn request(path: &Path, request: &Request) -> Result<Response, String> {
    let mut stream =
        UnixStream::connect(path).map_err(|error| format!("overlay control connect: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| error.to_string())?;
    let mut bytes = serde_json::to_vec(request).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .map_err(|error| error.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&line).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_and_revision_serialize_canvas_edits() {
        let root =
            std::env::temp_dir().join(format!("scorepeek-overlay-control-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("overlay.toml");
        let config = OverlayConfig::initial();
        let shared = Mutex::new(State {
            config,
            leases: BTreeMap::new(),
        });
        let first = apply(
            Request::Acquire {
                canvas_id: "obs-status".into(),
                editor_id: "first".into(),
            },
            &path,
            &shared,
        )
        .unwrap();
        assert!(!first.readonly);
        let second = apply(
            Request::Acquire {
                canvas_id: "obs-status".into(),
                editor_id: "second".into(),
            },
            &path,
            &shared,
        )
        .unwrap();
        assert!(second.readonly);
        let mut presentation = first.canvas.unwrap();
        presentation.skin = scorepeek_overlay_ui::Skin::DjBlackbox;
        let saved = apply(
            Request::ReplaceCanvas {
                canvas_id: "obs-status".into(),
                editor_id: "first".into(),
                expected_revision: presentation.revision,
                presentation,
            },
            &path,
            &shared,
        )
        .unwrap();
        assert_eq!(saved.canvas.unwrap().revision, 1);
        assert!(
            apply(
                Request::SetGeometry {
                    canvas_id: "obs-status".into(),
                    editor_id: "first".into(),
                    expected_revision: 0,
                    x: 0,
                    y: 0,
                    width: 560,
                    height: 1040,
                },
                &path,
                &shared,
            )
            .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backend_revision_guards_canvas_management() {
        let root =
            std::env::temp_dir().join(format!("scorepeek-overlay-manager-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("overlay.toml");
        let shared = Mutex::new(State {
            config: OverlayConfig::initial(),
            leases: BTreeMap::new(),
        });
        let added = apply(
            Request::AddCanvas {
                backend: Backend::Obs,
                expected_revision: 0,
                canvas_id: "obs-empty".into(),
            },
            &path,
            &shared,
        )
        .unwrap();
        assert_eq!(added.backend_revision, Some(1));
        assert_eq!(added.canvases.len(), 5);
        assert!(
            apply(
                Request::DeleteCanvas {
                    backend: Backend::Obs,
                    expected_revision: 0,
                    canvas_id: "obs-empty".into(),
                },
                &path,
                &shared,
            )
            .is_err()
        );
        apply(
            Request::DeleteCanvas {
                backend: Backend::Obs,
                expected_revision: 1,
                canvas_id: "obs-empty".into(),
            },
            &path,
            &shared,
        )
        .unwrap();
        assert!(
            apply(
                Request::SetCanvasEnabled {
                    backend: Backend::Obs,
                    expected_revision: 2,
                    canvas_id: "obs-missing".into(),
                    enabled: false,
                },
                &path,
                &shared,
            )
            .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn global_grace_uses_its_own_revision_under_a_canvas_lease() {
        let root =
            std::env::temp_dir().join(format!("scorepeek-overlay-settings-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("overlay.toml");
        let shared = Mutex::new(State {
            config: OverlayConfig::initial(),
            leases: BTreeMap::new(),
        });
        apply(
            Request::Acquire {
                canvas_id: "wayland-status".into(),
                editor_id: "editor".into(),
            },
            &path,
            &shared,
        )
        .unwrap();
        let changed = apply(
            Request::SetUnknownGrace {
                canvas_id: "wayland-status".into(),
                editor_id: "editor".into(),
                expected_revision: 0,
                unknown_grace_ms: 2_000,
            },
            &path,
            &shared,
        )
        .unwrap();
        assert_eq!(changed.settings_revision, Some(1));
        assert_eq!(changed.unknown_grace_ms, Some(2_000));
        assert!(
            apply(
                Request::SetUnknownGrace {
                    canvas_id: "wayland-status".into(),
                    editor_id: "editor".into(),
                    expected_revision: 0,
                    unknown_grace_ms: 500,
                },
                &path,
                &shared,
            )
            .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
