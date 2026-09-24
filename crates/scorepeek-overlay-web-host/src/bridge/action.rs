pub use scorepeek_overlay_runtime::control::*;

#[cfg(test)]
pub(crate) use test_authority::Controller;

#[cfg(test)]
mod test_authority {
    use super::{Request, Response};
    use crate::{
        config::{Canvas, OverlayConfig},
        host::lifecycle::Backend,
    };
    use scorepeek_overlay::CanvasPresentation;
    use std::{
        io::{BufRead as _, BufReader, Write as _},
        os::unix::net::{UnixListener, UnixStream},
        path::{Path, PathBuf},
        sync::{Arc, Mutex},
        thread::JoinHandle,
    };

    struct Lease {
        editor_id: String,
        draft: Vec<CanvasPresentation>,
        generation: u64,
    }

    struct State {
        document: OverlayConfig,
        lease: Option<Lease>,
    }

    pub(crate) struct Controller {
        path: PathBuf,
        state: Arc<Mutex<State>>,
        worker: Option<JoinHandle<()>>,
    }

    impl Controller {
        pub(crate) fn start(path: &Path, document: OverlayConfig) -> Result<Self, String> {
            let socket = path.with_extension("test-control.sock");
            let listener = UnixListener::bind(&socket).map_err(|error| error.to_string())?;
            let state = Arc::new(Mutex::new(State {
                document,
                lease: None,
            }));
            let worker_state = Arc::clone(&state);
            let worker = std::thread::spawn(move || {
                while let Ok((mut stream, _)) = listener.accept() {
                    let mut line = String::new();
                    if BufReader::new(&stream).read_line(&mut line).is_err() || line.is_empty() {
                        break;
                    }
                    let response = serde_json::from_str(&line)
                        .map_err(|error| error.to_string())
                        .and_then(|request| apply(&worker_state, request))
                        .unwrap_or_else(|error| Response {
                            ok: false,
                            readonly: true,
                            error: Some(error),
                            canvases: Vec::new(),
                            generation: None,
                            dirty: false,
                        });
                    if let Ok(mut bytes) = serde_json::to_vec(&response) {
                        bytes.push(b'\n');
                        let _ = stream.write_all(&bytes);
                    }
                }
            });
            Ok(Self {
                path: socket,
                state,
                worker: Some(worker),
            })
        }

        pub(crate) fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for Controller {
        fn drop(&mut self) {
            let _ = UnixStream::connect(&self.path);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
            let _ = std::fs::remove_file(&self.path);
            drop(self.state.lock());
        }
    }

    fn saved(state: &State) -> Vec<CanvasPresentation> {
        state
            .document
            .canvases
            .iter()
            .filter(|canvas| canvas.backend == Backend::Obs)
            .map(Canvas::presentation)
            .collect()
    }

    fn response(state: &State, readonly: bool) -> Response {
        let saved = saved(state);
        let (canvases, generation, dirty) = state.lease.as_ref().map_or_else(
            || (saved.clone(), None, false),
            |lease| {
                (
                    lease.draft.clone(),
                    Some(lease.generation),
                    lease.draft != saved,
                )
            },
        );
        Response {
            ok: true,
            readonly,
            error: None,
            canvases,
            generation,
            dirty,
        }
    }

    fn apply(state: &Mutex<State>, request: Request) -> Result<Response, String> {
        let mut state = state
            .lock()
            .map_err(|_| "test authority poisoned".to_owned())?;
        match request {
            Request::AcquireBackend {
                backend: Backend::Obs,
                editor_id,
            } => {
                let readonly = state
                    .lease
                    .as_ref()
                    .is_some_and(|lease| lease.editor_id != editor_id);
                if state.lease.is_none() {
                    state.lease = Some(Lease {
                        editor_id,
                        draft: saved(&state),
                        generation: 1,
                    });
                }
                Ok(response(&state, readonly))
            }
            Request::KeepAliveBackend { editor_id, .. } => Ok(response(
                &state,
                state
                    .lease
                    .as_ref()
                    .is_none_or(|lease| lease.editor_id != editor_id),
            )),
            Request::ReleaseBackend { editor_id, .. } => {
                if state
                    .lease
                    .as_ref()
                    .is_some_and(|lease| lease.editor_id == editor_id)
                {
                    state.lease = None;
                }
                Ok(response(&state, false))
            }
            Request::GetBackend { .. } => Ok(response(&state, true)),
            Request::UpdateBackendDraft {
                editor_id,
                canvases,
                ..
            }
            | Request::CommitBackend {
                editor_id,
                canvases,
                ..
            } => {
                let lease = state
                    .lease
                    .as_mut()
                    .filter(|lease| lease.editor_id == editor_id)
                    .ok_or_else(|| "test editor lease is not owned".to_owned())?;
                lease.draft = canvases;
                lease.generation = lease.generation.saturating_add(1);
                Ok(response(&state, false))
            }
            Request::AcquireBackend { .. } => Err("test authority supports OBS only".to_owned()),
        }
    }
}
