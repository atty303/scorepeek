use crate::{
    bridge::data::Backend,
    config::{Canvas, OverlayConfig, empty_canvas, save_atomic},
};
use scorepeek_overlay::CanvasPresentation;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
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
const DIAGNOSTIC_CAPACITY: usize = 128;

pub use scorepeek_overlay::editor::protocol::{Request, Response};

struct Lease {
    editor_id: String,
    touched: Instant,
    projection_generation: u64,
    draft: Vec<CanvasPresentation>,
}
struct State {
    config: OverlayConfig,
    leases: BTreeMap<Backend, Lease>,
    diagnostics: VecDeque<serde_json::Value>,
    dropped_diagnostics: u64,
}

pub struct Controller {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<State>>,
    worker: Option<JoinHandle<()>>,
    _config_lock: std::fs::File,
}

impl Controller {
    /// Starts the parent-owned, backend-transactional configuration writer.
    /// # Errors
    /// Returns socket or worker creation errors.
    pub fn start(path: &Path, config: OverlayConfig) -> Result<Self, String> {
        let config_lock = acquire_config_lock(path)?;
        let parent = path
            .parent()
            .ok_or("overlay config lock has no parent directory")?;
        let socket = parent.join(format!(".overlay-control-{}.sock", std::process::id()));
        if socket.exists() {
            std::fs::remove_file(&socket).map_err(|error| error.to_string())?;
        }
        let listener = UnixListener::bind(&socket)
            .map_err(|error| format!("bind {}: {error}", socket.display()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let config_path = path.to_owned();
        let state = Arc::new(Mutex::new(State {
            config,
            leases: BTreeMap::new(),
            diagnostics: VecDeque::new(),
            dropped_diagnostics: 0,
        }));
        let worker_state = Arc::clone(&state);
        let worker = std::thread::Builder::new()
            .name("overlay-config-writer".into())
            .spawn(move || {
                while !stopping.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => handle(stream, &config_path, &worker_state),
                        Err(error) => {
                            if let Ok(mut state) = worker_state.lock() {
                                state.observe(
                                    "overlay_controller",
                                    serde_json::json!({
                                        "status":"failed", "error_type":"accept", "error":error.to_string()
                                    }),
                                );
                            }
                            break;
                        }
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            path: socket,
            stop,
            state,
            worker: Some(worker),
            _config_lock: config_lock,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Takes bounded parent-process diagnostics without writing to terminal streams.
    #[must_use]
    pub fn take_observations(&self) -> Vec<serde_json::Value> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut observations = state.diagnostics.drain(..).collect::<Vec<_>>();
        if state.dropped_diagnostics > 0 {
            observations.push(serde_json::json!({
                "operation":"overlay_controller_diagnostics",
                "data":{"status":"dropped","count":state.dropped_diagnostics}
            }));
            state.dropped_diagnostics = 0;
        }
        observations
    }
}

fn acquire_config_lock(path: &Path) -> Result<std::fs::File, String> {
    let lock_path = path.with_extension("toml.lock");
    let parent = lock_path
        .parent()
        .ok_or("overlay config lock has no parent directory")?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| format!("open {}: {error}", lock_path.display()))?;
    file.try_lock().map_err(|error| {
        format!(
            "overlay config {} is already owned by another scorepeek process: {error}",
            path.display()
        )
    })?;
    Ok(file)
}

impl State {
    fn observe(&mut self, operation: &str, data: serde_json::Value) {
        if self.diagnostics.len() == DIAGNOSTIC_CAPACITY {
            self.dropped_diagnostics = self.dropped_diagnostics.saturating_add(1);
        } else {
            self.diagnostics.push_back(serde_json::Value::Object(
                [
                    ("operation".to_owned(), operation.into()),
                    ("data".to_owned(), data),
                ]
                .into_iter()
                .collect(),
            ));
        }
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
    let response = match read_request(&stream) {
        Ok(request) => {
            let identity = request_identity(&request);
            apply(request, path, shared)
                .unwrap_or_else(|error| failed_response(shared, identity, error))
        }
        Err(error) => Response {
            ok: false,
            readonly: true,
            error: Some(error),
            canvases: Vec::new(),
            generation: None,
            dirty: false,
        },
    };
    if let Ok(mut bytes) = serde_json::to_vec(&response) {
        bytes.push(b'\n');
        let _ = stream.write_all(&bytes);
    }
}

fn request_identity(request: &Request) -> Option<(Backend, String)> {
    match request {
        Request::AcquireBackend { backend, editor_id }
        | Request::KeepAliveBackend { backend, editor_id }
        | Request::ReleaseBackend { backend, editor_id }
        | Request::UpdateBackendDraft {
            backend, editor_id, ..
        }
        | Request::CommitBackend {
            backend, editor_id, ..
        } => Some((*backend, editor_id.clone())),
        Request::GetBackend { .. } => None,
    }
}

fn failed_response(
    shared: &Mutex<State>,
    identity: Option<(Backend, String)>,
    error: String,
) -> Response {
    let Ok(state) = shared.lock() else {
        return Response {
            ok: false,
            readonly: true,
            error: Some(error),
            canvases: Vec::new(),
            generation: None,
            dirty: false,
        };
    };
    let Some((backend, editor_id)) = identity else {
        return Response {
            ok: false,
            readonly: true,
            error: Some(error),
            canvases: Vec::new(),
            generation: None,
            dirty: false,
        };
    };
    let owns_lease = state
        .leases
        .get(&backend)
        .is_some_and(|lease| lease.editor_id == editor_id);
    let mut response = if owns_lease {
        lease_response(&state, backend)
    } else {
        backend_response(&state.config, backend, true)
    };
    response.ok = false;
    response.readonly = !owns_lease;
    response.error = Some(error);
    response
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
        Request::AcquireBackend { backend, editor_id } => {
            if let Some(lease) = state
                .leases
                .get_mut(&backend)
                .filter(|lease| lease.editor_id == editor_id)
            {
                lease.touched = Instant::now();
                state.observe(
                    "overlay_editor_lease",
                    serde_json::json!({"backend":backend,"status":"retained"}),
                );
                return Ok(lease_response(&state, backend));
            }
            let occupied = state
                .leases
                .get(&backend)
                .is_some_and(|lease| lease.editor_id != editor_id);
            if occupied {
                state.observe(
                    "overlay_editor_lease",
                    serde_json::json!({"backend":backend,"status":"rejected"}),
                );
                return Err("workspace editor is already active".into());
            }
            {
                let projection_generation = state.config.projection_generations.get(backend);
                let draft = state
                    .config
                    .canvases
                    .iter()
                    .filter(|canvas| canvas.backend == backend)
                    .map(Canvas::presentation)
                    .collect();
                state.leases.insert(
                    backend,
                    Lease {
                        editor_id,
                        touched: Instant::now(),
                        projection_generation,
                        draft,
                    },
                );
            }
            state.observe(
                "overlay_editor_lease",
                serde_json::json!({
                    "backend": backend, "status": "acquired"
                }),
            );
            Ok(lease_response(&state, backend))
        }
        Request::KeepAliveBackend { backend, editor_id } => {
            require_lease(&mut state, backend, &editor_id)?;
            Ok(empty_response(false))
        }
        Request::ReleaseBackend { backend, editor_id } => {
            if state
                .leases
                .get(&backend)
                .is_some_and(|lease| lease.editor_id != editor_id)
            {
                return Ok(empty_response(false));
            }
            if state.leases.remove(&backend).is_some() {
                state.observe(
                    "overlay_editor_lease",
                    serde_json::json!({
                        "backend": backend, "status":"released"
                    }),
                );
            }
            Ok(backend_response(&state.config, backend, false))
        }
        Request::GetBackend { backend } => Ok(backend_response(&state.config, backend, true)),
        Request::UpdateBackendDraft {
            backend,
            editor_id,
            canvases,
        } => {
            let replacements = build_replacements(&state.config, backend, canvases.clone())?;
            drop(replacements);
            let lease = state
                .leases
                .get_mut(&backend)
                .filter(|lease| lease.editor_id == editor_id)
                .ok_or("editor lease lost")?;
            lease.touched = Instant::now();
            lease.draft = canvases;
            lease.projection_generation = lease.projection_generation.saturating_add(1);
            Ok(lease_response(&state, backend))
        }
        Request::CommitBackend {
            backend,
            editor_id,
            canvases,
        } => {
            require_lease(&mut state, backend, &editor_id)?;
            let replacements = build_replacements(&state.config, backend, canvases)?;
            let mut candidate = state.config.clone();
            candidate
                .canvases
                .retain(|canvas| canvas.backend != backend);
            candidate.canvases.extend(replacements);
            candidate.projection_generations.increment(backend)?;
            if let Err(error) = save_atomic(path, &candidate) {
                state.observe(
                    "overlay_editor_commit",
                    serde_json::json!({
                        "backend": backend, "status":"failed", "error":error
                    }),
                );
                return Err(error);
            }
            state.config = candidate;
            let canvas_count = state
                .config
                .canvases
                .iter()
                .filter(|canvas| canvas.backend == backend)
                .count();
            state.observe(
                "overlay_editor_commit",
                serde_json::json!({
                    "backend": backend, "status":"saved",
                    "canvas_count":canvas_count
                }),
            );
            Ok(backend_response(&state.config, backend, false))
        }
    }
}

fn empty_response(readonly: bool) -> Response {
    Response {
        ok: true,
        readonly,
        error: None,
        canvases: Vec::new(),
        generation: None,
        dirty: false,
    }
}

fn require_lease(state: &mut State, backend: Backend, editor_id: &str) -> Result<(), String> {
    let lease = state
        .leases
        .get_mut(&backend)
        .filter(|lease| lease.editor_id == editor_id)
        .ok_or("editor lease lost")?;
    lease.touched = Instant::now();
    Ok(())
}

fn build_replacements(
    config: &OverlayConfig,
    backend: Backend,
    presentations: Vec<CanvasPresentation>,
) -> Result<Vec<Canvas>, String> {
    let mut ids = BTreeSet::new();
    let mut replacements = Vec::with_capacity(presentations.len());
    for presentation in presentations {
        if !ids.insert(presentation.id.clone()) {
            return Err("canvas ids must be unique within a backend workspace".into());
        }
        let mut canvas = config
            .canvases
            .iter()
            .find(|canvas| canvas.backend == backend && canvas.id == presentation.id)
            .cloned()
            .unwrap_or_else(|| empty_canvas(presentation.id.clone(), backend, presentation.skin));
        canvas.apply_presentation(&presentation);
        replacements.push(canvas);
    }
    let mut candidate = config.clone();
    candidate
        .canvases
        .retain(|canvas| canvas.backend != backend);
    candidate.canvases.extend(replacements.clone());
    let (_, issues) = candidate.validated()?;
    if let Some(issue) = issues.first() {
        return Err(format!("canvas {}: {}", issue.canvas_id, issue.message));
    }
    Ok(replacements)
}

fn backend_response(config: &OverlayConfig, backend: Backend, readonly: bool) -> Response {
    Response {
        ok: true,
        readonly,
        error: None,
        canvases: config
            .canvases
            .iter()
            .filter(|canvas| canvas.backend == backend)
            .map(Canvas::presentation)
            .collect(),
        generation: Some(config.projection_generations.get(backend)),
        dirty: false,
    }
}

fn lease_response(state: &State, backend: Backend) -> Response {
    let lease = state.leases.get(&backend).expect("lease exists");
    let saved = state
        .config
        .canvases
        .iter()
        .filter(|canvas| canvas.backend == backend)
        .map(Canvas::presentation)
        .collect::<Vec<_>>();
    Response {
        ok: true,
        readonly: false,
        error: None,
        canvases: lease.draft.clone(),
        generation: Some(lease.projection_generation),
        dirty: lease.draft != saved,
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

    fn blackbox_skin() -> scorepeek_overlay::Skin {
        "dev.atty303.scorepeek.skin.dj-blackbox".parse().unwrap()
    }

    fn cyan_skin() -> scorepeek_overlay::Skin {
        "dev.atty303.scorepeek.skin.cyan-system".parse().unwrap()
    }

    fn fixture(name: &str) -> (PathBuf, Mutex<State>) {
        let root = std::env::temp_dir().join(format!(
            "scorepeek-overlay-control-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        (
            root.join("overlay.toml"),
            Mutex::new(State {
                config: crate::config::visual_debug_config(cyan_skin()),
                leases: BTreeMap::new(),
                diagnostics: VecDeque::new(),
                dropped_diagnostics: 0,
            }),
        )
    }

    #[test]
    fn controller_holds_one_process_lifetime_writer_lock_per_config() {
        let root = std::env::temp_dir().join(format!(
            "scorepeek-overlay-writer-lock-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("overlay.toml");
        let first = acquire_config_lock(&path).unwrap();
        let error = acquire_config_lock(&path).unwrap_err();
        assert!(error.contains("already owned"), "{error}");
        drop(first);
        let reopened = acquire_config_lock(&path).unwrap();
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backend_lease_serializes_editors_and_commit_is_atomic() {
        let (path, shared) = fixture("atomic");
        let first = apply(
            Request::AcquireBackend {
                backend: Backend::Obs,
                editor_id: "first".into(),
            },
            &path,
            &shared,
        )
        .unwrap();
        assert!(!first.readonly);
        assert!(!first.dirty);
        assert_eq!(
            apply(
                Request::AcquireBackend {
                    backend: Backend::Obs,
                    editor_id: "second".into()
                },
                &path,
                &shared
            )
            .unwrap_err(),
            "workspace editor is already active"
        );
        let mut draft = first.canvases;
        draft[0].skin = blackbox_skin();
        let updated = apply(
            Request::UpdateBackendDraft {
                backend: Backend::Obs,
                editor_id: "first".into(),
                canvases: draft.clone(),
            },
            &path,
            &shared,
        )
        .unwrap();
        assert!(updated.dirty);
        let reacquired = apply(
            Request::AcquireBackend {
                backend: Backend::Obs,
                editor_id: "first".into(),
            },
            &path,
            &shared,
        )
        .unwrap();
        assert!(reacquired.dirty);
        assert_eq!(reacquired.canvases, updated.canvases);
        let saved = apply(
            Request::CommitBackend {
                backend: Backend::Obs,
                editor_id: "first".into(),
                canvases: draft,
            },
            &path,
            &shared,
        )
        .unwrap();
        assert!(!saved.dirty);
        assert_eq!(saved.generation, Some(1));
        assert_eq!(saved.canvases[0].skin, blackbox_skin());
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("dj-blackbox")
        );
        let released = apply(
            Request::ReleaseBackend {
                backend: Backend::Obs,
                editor_id: "first".into(),
            },
            &path,
            &shared,
        )
        .unwrap();
        assert!(!released.dirty);
        assert_eq!(released.canvases, saved.canvases);
        let diagnostics = shared
            .lock()
            .unwrap()
            .diagnostics
            .drain(..)
            .collect::<Vec<_>>();
        assert!(diagnostics.iter().any(|record| {
            record["operation"] == "overlay_editor_commit" && record["data"]["status"] == "saved"
        }));
        assert!(diagnostics.iter().all(|record| {
            matches!(
                record["operation"].as_str(),
                Some("overlay_editor_lease" | "overlay_editor_commit")
            )
        }));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn expired_editor_release_does_not_restore_over_the_new_owners_draft() {
        let (path, shared) = fixture("expired-release");
        let acquire = |editor: &str| {
            apply(
                Request::AcquireBackend {
                    backend: Backend::Obs,
                    editor_id: editor.into(),
                },
                &path,
                &shared,
            )
            .unwrap()
        };
        let saved = acquire("expired").canvases;
        shared
            .lock()
            .unwrap()
            .leases
            .get_mut(&Backend::Obs)
            .unwrap()
            .touched = Instant::now().checked_sub(LEASE_TIMEOUT).unwrap();
        let mut draft = acquire("current").canvases;
        draft[0].skin = blackbox_skin();
        let updated = apply(
            Request::UpdateBackendDraft {
                backend: Backend::Obs,
                editor_id: "current".into(),
                canvases: draft.clone(),
            },
            &path,
            &shared,
        )
        .unwrap();
        assert!(updated.dirty);
        let release = |editor: &str| {
            apply(
                Request::ReleaseBackend {
                    backend: Backend::Obs,
                    editor_id: editor.into(),
                },
                &path,
                &shared,
            )
            .unwrap()
        };
        let stale = release("expired");
        assert!(stale.ok);
        assert!(stale.canvases.is_empty());
        assert_eq!(stale.generation, None);
        let current = acquire("current");
        assert!(current.dirty);
        assert_eq!(current.canvases, draft);
        let restored = release("current");
        assert_eq!(restored.canvases, saved);
        assert_eq!(restored.generation, Some(0));
        assert!(!restored.dirty);
        let repeated = release("current");
        assert_eq!(repeated.canvases, saved);
        assert_eq!(repeated.generation, Some(0));
        acquire("abandoned");
        apply(
            Request::UpdateBackendDraft {
                backend: Backend::Obs,
                editor_id: "abandoned".into(),
                canvases: draft,
            },
            &path,
            &shared,
        )
        .unwrap();
        shared
            .lock()
            .unwrap()
            .leases
            .get_mut(&Backend::Obs)
            .unwrap()
            .touched = Instant::now().checked_sub(LEASE_TIMEOUT).unwrap();
        let abandoned = release("abandoned");
        assert_eq!(abandoned.canvases, saved);
        assert_eq!(abandoned.generation, Some(0));
        assert!(!abandoned.dirty);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn restoring_the_saved_backend_document_clears_dirty() {
        let (path, shared) = fixture("restore-dirty");
        let acquired = apply(
            Request::AcquireBackend {
                backend: Backend::Wayland,
                editor_id: "editor".into(),
            },
            &path,
            &shared,
        )
        .unwrap();
        let saved = acquired.canvases.clone();
        let mut changed = acquired.canvases;
        changed[0].opacity_percent = 25;
        let changed = apply(
            Request::UpdateBackendDraft {
                backend: Backend::Wayland,
                editor_id: "editor".into(),
                canvases: changed,
            },
            &path,
            &shared,
        )
        .unwrap();
        assert!(changed.dirty);
        let restored = apply(
            Request::UpdateBackendDraft {
                backend: Backend::Wayland,
                editor_id: "editor".into(),
                canvases: saved,
            },
            &path,
            &shared,
        )
        .unwrap();
        assert!(!restored.dirty);
        assert_eq!(
            restored.generation,
            changed.generation.map(|value| value + 1)
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn failed_commit_keeps_lease_and_previous_document() {
        let (path, shared) = fixture("failure");
        let acquired = apply(
            Request::AcquireBackend {
                backend: Backend::Wayland,
                editor_id: "editor".into(),
            },
            &path,
            &shared,
        )
        .unwrap();
        let mut invalid = acquired.canvases;
        invalid[0].widgets[0].x = -3;
        let error = apply(
            Request::CommitBackend {
                backend: Backend::Wayland,
                editor_id: "editor".into(),
                canvases: invalid,
            },
            &path,
            &shared,
        )
        .unwrap_err();
        let response = failed_response(&shared, Some((Backend::Wayland, "editor".into())), error);
        assert!(!response.ok);
        assert!(!response.readonly);
        assert!(!response.canvases.is_empty());
        assert!(
            apply(
                Request::KeepAliveBackend {
                    backend: Backend::Wayland,
                    editor_id: "editor".into()
                },
                &path,
                &shared
            )
            .is_ok()
        );
        assert!(!path.exists());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
