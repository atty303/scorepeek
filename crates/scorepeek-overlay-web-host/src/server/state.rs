//! Shared server state and editor-control connection ownership.

use crate::bridge::data::Feed;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

pub(crate) struct Shared {
    pub(crate) canvases: Mutex<Vec<crate::config::Canvas>>,
    pub(crate) control_socket: std::path::PathBuf,
    pub(crate) feed: Feed,
    pub(crate) changed: Arc<Notify>,
    pub(crate) skins: crate::skin::StoreRoot,
}

pub(crate) struct EditorConnection {
    shared: Arc<Shared>,
    id: String,
    owns_lease: std::cell::Cell<bool>,
}
impl EditorConnection {
    pub(crate) fn new(shared: Arc<Shared>) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            shared,
            owns_lease: std::cell::Cell::new(false),
            id: format!(
                "obs-socket-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
        }
    }
    pub(crate) fn request(
        &self,
        mut request: crate::control::Request,
    ) -> Result<crate::control::Response, String> {
        use crate::control::Request;
        let (backend, editor) = match &mut request {
            Request::AcquireBackend { backend, editor_id }
            | Request::KeepAliveBackend { backend, editor_id }
            | Request::ReleaseBackend { backend, editor_id }
            | Request::UpdateBackendDraft {
                backend, editor_id, ..
            }
            | Request::CommitBackend {
                backend, editor_id, ..
            } => (backend, Some(editor_id)),
            Request::GetBackend { backend } => (backend, None),
        };
        if *backend != crate::bridge::data::Backend::Obs {
            return Err("stage control only accepts the OBS backend".into());
        }
        if let Some(editor) = editor {
            editor.clone_from(&self.id);
        }
        let publishes = !matches!(request, Request::ReleaseBackend { .. }) || self.owns_lease.get();
        let response = crate::control::request(&self.shared.control_socket, &request)?;
        if matches!(request, Request::AcquireBackend { .. }) && response.ok && !response.readonly {
            self.owns_lease.set(true);
        }
        if response.readonly || matches!(request, Request::ReleaseBackend { .. }) && response.ok {
            self.owns_lease.set(false);
        }
        if publishes && response.ok && !response.readonly && response.generation.is_some() {
            let mut canvases = self
                .shared
                .canvases
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for presentation in &response.canvases {
                if let Some(canvas) = canvases
                    .iter_mut()
                    .find(|canvas| canvas.id == presentation.id)
                {
                    canvas.apply_presentation(presentation);
                } else {
                    let mut canvas = crate::config::empty_canvas(
                        presentation.id.clone(),
                        crate::bridge::data::Backend::Obs,
                        presentation.skin,
                    );
                    canvas.apply_presentation(presentation);
                    canvases.push(canvas);
                }
            }
            canvases.retain(|canvas| response.canvases.iter().any(|item| item.id == canvas.id));
        }
        self.shared.changed.notify_waiters();
        Ok(response)
    }
}
impl Drop for EditorConnection {
    fn drop(&mut self) {
        if !self.owns_lease.get() {
            return;
        }
        let result = self.request(crate::control::Request::ReleaseBackend {
            backend: crate::bridge::data::Backend::Obs,
            editor_id: self.id.clone(),
        });
        crate::diagnostics::emit(
            "overlay_editor_connection",
            &serde_json::json!({
                "status": if result.as_ref().is_ok_and(|response| response.ok) { "released" } else { "release_failed" },
            }),
        );
    }
}
