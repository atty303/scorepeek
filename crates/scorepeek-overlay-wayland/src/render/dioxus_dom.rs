mod assets;
mod frame_turn;
mod profile;
mod skin_session;
pub(crate) mod text;
use assets::{
    EmbeddedSkinAssets, SkinAssetCache, namespace_native_skin_output, namespace_skin_css,
};
use frame_turn::{
    NativeDisplaySurfaceState, NativeDisplayTurnInput, NativeEditorStageTurnInput,
    NativeFrameBoundary, NativeSurfaceReadiness, run_native_display_turn,
    run_native_editor_stage_turn,
};
pub(crate) use profile::FrameWorkProfile;
use profile::{FrameWorkSample, WorkStat};
use scorepeek_overlay::editor::effect::{
    EditorCanvas, EditorSelectionMetrics, EditorSurface, PlacementPreview, SurfaceAction,
};
#[cfg(test)]
use scorepeek_overlay::editor::model::EditorEffectKind;
use scorepeek_overlay::editor::model::{
    EditorBackendReply, EditorEffect, EditorInput, EditorSession, StageProjection,
};
use scorepeek_overlay::editor::runtime::{EditorRuntime, use_editor_runtime};
use skin_session::{
    EditorSkinPreview, EditorSkinReconciliation, NativeDisplaySkin, create_editor_skin_preview,
    create_native_display_skin, reconcile_editor_skin_previews, render_native_display_skin,
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::Arc,
    task::{Wake, Waker},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::bridge::data::{Config, Feed};
use crate::host::event_loop::{Event, OutputDescription};
use crate::input::pointer::PointerInput;
use crate::render::blitz::{
    NativeEventConsumer, dispatch_native_event, poll_native_document,
    poll_native_document_for_frame,
};
#[cfg(test)]
use crate::render::frame::NativeFramePresenter;
use crate::render::frame::WindowPresenter;
use crate::render::vello::{paint_native_scene, resolve_with_loaded_resources};
#[cfg(test)]
use crate::window::geometry::{editor_geometry, editor_panel_width};
use crate::window::surface::Shell;
use anyrender::{CompositeAlphaMode, ImageRenderer, WindowRenderer};
use anyrender_vello::{VelloRendererOptions, VelloWindowRenderer};
use blitz_dom::DocumentConfig;
use blitz_traits::shell::{ColorScheme, Viewport};
use dioxus::prelude::*;
use dioxus_core::VirtualDom;
use dioxus_native_dom::DioxusDocument;
use scorepeek_overlay::OverlayState;
#[cfg(test)]
use scorepeek_overlay::WidgetLayout;
use scorepeek_overlay::editor::{EditorAction, EditorOutput, EditorPanel};
use serde::{Deserialize, Serialize};
use smithay_client_toolkit::reexports::calloop::ping::{Ping, make_ping};

#[derive(Default)]
struct EditorSkinUpdates {
    pending: bool,
    requests: u64,
    renders: u64,
}

impl EditorSkinUpdates {
    fn request(&mut self) {
        self.pending = true;
        self.requests = self.requests.saturating_add(1);
    }

    fn take_if_ready(&mut self, frame: bool, dragging: bool) -> bool {
        if !self.pending || (!frame && dragging) {
            return false;
        }
        self.pending = false;
        true
    }
}

fn editor_skin_root_id(canvas_id: &str) -> String {
    use std::fmt::Write as _;
    let mut id = String::from("scorepeek-editor-skin-");
    for byte in canvas_id.bytes() {
        let _ = write!(id, "{byte:02x}");
    }
    id
}

fn worker_needs_replacement(
    id: &str,
    output: Option<&str>,
    desired_ids: &std::collections::BTreeSet<String>,
    projected: &[crate::config::Canvas],
    finished: bool,
) -> bool {
    !desired_ids.contains(id)
        || projected
            .iter()
            .find(|canvas| canvas.id == id)
            .is_some_and(|canvas| Some(canvas.output.as_str()) != output)
        || finished
}

#[derive(Debug, Default, PartialEq, Eq)]
struct WorkerReconciliation {
    stop_join: Vec<String>,
    start: Vec<String>,
}

fn reconcile_worker_lifecycle<'a>(
    workers: impl IntoIterator<Item = (&'a str, Option<&'a str>, bool)>,
    projected: &[crate::config::Canvas],
) -> WorkerReconciliation {
    let desired_ids = projected
        .iter()
        .map(|canvas| canvas.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let live = workers
        .into_iter()
        .map(|(id, output, finished)| (id.to_owned(), output.map(str::to_owned), finished))
        .collect::<Vec<_>>();
    let stop_join = live
        .iter()
        .filter(|(id, output, finished)| {
            worker_needs_replacement(id, output.as_deref(), &desired_ids, projected, *finished)
        })
        .map(|(id, _, _)| id.clone())
        .collect::<Vec<_>>();
    let retained = live
        .iter()
        .filter(|(id, _, _)| !stop_join.contains(id))
        .map(|(id, _, _)| id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let start = projected
        .iter()
        .filter(|canvas| !retained.contains(canvas.id.as_str()))
        .map(|canvas| canvas.id.clone())
        .collect();
    WorkerReconciliation { stop_join, start }
}

fn update_display_visibility(
    projection: Reactive<NativeDocumentProjection>,
    screen: scorepeek_overlay::ScreenView,
) -> Option<bool> {
    let update = {
        let current = projection.borrow();
        match &*current {
            NativeDocumentProjection::Display { canvas, visible } => {
                let next_visible =
                    scorepeek_overlay::canvas_visible(canvas.show_on.as_deref(), screen);
                (*visible != next_visible).then(|| (canvas.clone(), next_visible))
            }
            NativeDocumentProjection::Editor(_) => None,
        }
    };
    let (canvas, visible) = update?;
    projection.set(NativeDocumentProjection::Display { canvas, visible });
    Some(visible)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProjectionCacheKey {
    editing: bool,
    session_id: u64,
    revision: u64,
}

#[derive(Default)]
struct NativeProjectionCache {
    key: Option<ProjectionCacheKey>,
    canvases: Vec<crate::config::Canvas>,
    rebuilds: u64,
}

impl NativeProjectionCache {
    fn resolve<'a>(
        &'a mut self,
        fallback_skin: Option<scorepeek_overlay::Skin>,
        session: &EditorSession,
        desired: &[crate::config::Canvas],
    ) -> Result<&'a [crate::config::Canvas], String> {
        let key = ProjectionCacheKey {
            editing: session.editing,
            session_id: session.session_id,
            revision: session.revision,
        };
        if self.key == Some(key) {
            return Ok(&self.canvases);
        }
        self.canvases = if session.editing {
            let output_descriptions = session
                .outputs
                .iter()
                .map(|output| OutputDescription {
                    name: output.name.clone(),
                    model: output.model.clone(),
                    logical_size: output.logical_size,
                })
                .collect::<Vec<_>>();
            let draft = session
                .draft
                .iter()
                .map(|presentation| {
                    let mut canvas = crate::config::empty_canvas(
                        presentation.id.clone(),
                        crate::bridge::data::Backend::Wayland,
                        presentation.skin,
                    );
                    canvas.apply_presentation(presentation);
                    canvas
                })
                .collect::<Vec<_>>();
            editor_stage_canvases(fallback_skin, &draft, &output_descriptions)?
        } else {
            desired.to_vec()
        };
        self.key = Some(key);
        self.rebuilds = self.rebuilds.saturating_add(1);
        crate::diagnostics::emit(
            "native_projection_rebuilt",
            &serde_json::json!({
                "session_id": key.session_id,
                "revision": key.revision,
                "editing": key.editing,
                "surface_count": self.canvases.len(),
                "rebuild_count": self.rebuilds,
            }),
        );
        Ok(&self.canvases)
    }
}

fn editor_skin_presentation_changed(
    before: &scorepeek_overlay::CanvasPresentation,
    after: &scorepeek_overlay::CanvasPresentation,
) -> bool {
    before.id != after.id
        || before.skin != after.skin
        || before.skin_properties != after.skin_properties
        || before.width != after.width
        || before.height != after.height
        || before.widgets != after.widgets
}

fn editor_skin_previews_changed(
    previews: &std::collections::BTreeMap<String, EditorSkinPreview>,
    stage: &StageProjection,
) -> bool {
    stage.canvases.len() != previews.len()
        || stage.canvases.iter().any(|canvas| {
            previews.get(&canvas.id).is_none_or(|preview| {
                editor_skin_presentation_changed(&preview.canvas.presentation(), canvas)
            })
        })
}

fn accepts_stage_projection(current: &StageProjection, candidate: &StageProjection) -> bool {
    current.session_id != candidate.session_id || candidate.revision > current.revision
}

fn accept_stage_projection_replica(
    projection: Reactive<NativeDocumentProjection>,
    document: &mut DioxusDocument,
    previews: &mut std::collections::BTreeMap<String, EditorSkinPreview>,
    assets: &SkinAssetCache,
    output: &str,
    candidate: &StageProjection,
) -> bool {
    let accepted = match &*projection.borrow() {
        NativeDocumentProjection::Editor(current) => {
            accepts_stage_projection(current, candidate) && current != candidate
        }
        NativeDocumentProjection::Display { .. } => false,
    };
    if !accepted {
        return false;
    }
    let live = candidate
        .canvases
        .iter()
        .map(|canvas| canvas.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let removed = previews
        .keys()
        .filter(|id| !live.contains(id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    for id in removed {
        if let Some(mut preview) = previews.remove(&id) {
            preview.tree.unmount(&mut document.inner.borrow_mut());
            assets.release_editor_owner(&id, output);
        }
    }
    projection.set(NativeDocumentProjection::Editor(candidate.clone()));
    true
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SurfaceRole {
    DisplayCanvas,
    EditorStage,
}

#[derive(Clone, Default)]
struct PublishedStages {
    by_output: std::collections::BTreeMap<String, StageProjection>,
    #[cfg(test)]
    publications: u64,
}

#[derive(Clone)]
struct EditorAuthorityProps {
    initial: Rc<RefCell<Option<EditorSession>>>,
    runtime: Rc<RefCell<Option<EditorRuntime>>>,
}

#[allow(clippy::needless_pass_by_value)]
fn editor_authority(props: EditorAuthorityProps) -> Element {
    let initial = props
        .initial
        .borrow_mut()
        .take()
        .expect("editor authority initializes exactly once");
    let runtime = use_editor_runtime(move || initial);
    *props.runtime.borrow_mut() = Some(runtime);
    rsx! {}
}

struct NativeEditorAuthority {
    document: DioxusDocument,
    runtime: EditorRuntime,
    published: Arc<std::sync::Mutex<PublishedStages>>,
}

impl NativeEditorAuthority {
    fn new(session: EditorSession, published: Arc<std::sync::Mutex<PublishedStages>>) -> Self {
        let initial = Rc::new(RefCell::new(Some(session)));
        let runtime = Rc::new(RefCell::new(None));
        let mut document = DioxusDocument::new(
            VirtualDom::new_with_props(
                editor_authority,
                EditorAuthorityProps {
                    initial,
                    runtime: Rc::clone(&runtime),
                },
            ),
            DocumentConfig::default(),
        );
        document.initial_build();
        let runtime = runtime
            .borrow()
            .as_ref()
            .copied()
            .expect("editor authority publishes its runtime during initial build");
        let mut authority = Self {
            document,
            runtime,
            published,
        };
        authority.publish();
        authority
    }

    fn dispatch(&mut self, input: EditorInput) -> Vec<EditorEffect> {
        let before = {
            let session = self.runtime.session.read();
            (session.session_id, session.revision)
        };
        crate::diagnostics::emit(
            "native_editor_action_received",
            &serde_json::json!({"session_id":before.0,"revision":before.1,"input":input.diagnostic_name()}),
        );
        let effects = self.runtime.dispatch.call(input);
        self.poll();
        let after = {
            let session = self.runtime.session.read();
            (session.session_id, session.revision)
        };
        crate::diagnostics::emit(
            "native_editor_action_reduced",
            &serde_json::json!({"session_id":after.0,"before_revision":before.1,"revision":after.1,"effect_count":effects.len()}),
        );
        if after != before {
            self.publish();
        }
        effects
    }

    fn poll(&mut self) {
        while poll_native_document(&mut self.document, Waker::noop()) {}
    }

    fn publish(&mut self) {
        self.poll();
        let stages = self.runtime.stages.read().clone();
        let session_id = self.runtime.session.read().session_id;
        let revision = self.runtime.session.read().revision;
        let output_count = stages.len();
        let mut published = self
            .published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        published.by_output = stages
            .into_iter()
            .map(|stage| (stage.output.name.clone(), stage))
            .collect();
        #[cfg(test)]
        {
            published.publications = published.publications.saturating_add(1);
        }
        crate::diagnostics::emit(
            "native_editor_projection_published",
            &serde_json::json!({"session_id":session_id,"revision":revision,"output_count":output_count}),
        );
    }

    fn session(&self) -> dioxus::signals::ReadableRef<'_, Signal<EditorSession>> {
        self.runtime.session.read()
    }
}

#[derive(Clone, Debug, PartialEq)]
struct InteractionCorrelation {
    run_id: String,
    interaction_id: u64,
    action: &'static str,
}

#[derive(Debug)]
enum CoordinatorCommand {
    EditorInput {
        input: EditorInput,
        correlation: Option<InteractionCorrelation>,
    },
    Open {
        output: Option<String>,
        canvas: String,
        preview_screen: Option<scorepeek_overlay::ScreenKind>,
    },
}

fn stop_workers<'a>(
    ids: impl IntoIterator<Item = &'a String>,
    workers: &std::collections::BTreeMap<String, impl WorkerControl>,
    wakes: &std::sync::Mutex<std::collections::BTreeMap<String, Ping>>,
) {
    for id in ids {
        if let Some(worker) = workers.get(id) {
            worker.request_stop();
        }
        if let Some(wake) = wakes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
        {
            wake.ping();
        }
    }
}

trait WorkerControl {
    fn request_stop(&self);
}

struct NativeWorker {
    output: Option<String>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    join: std::thread::JoinHandle<Result<(), String>>,
}

enum NativeWorkerStartup {
    Ready(String),
    Failed { canvas_id: String, error: String },
}

impl WorkerControl for NativeWorker {
    fn request_stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
    }
}

fn new_native_skin_runtime(
    package: &crate::skin::Package,
    report: &Rc<RefCell<RunReport>>,
    canvas_id: &str,
    output: Option<&str>,
    interaction_ids: &[u64],
) -> Result<crate::skin::Runtime, String> {
    let started = Instant::now();
    let result = crate::skin::Runtime::new_measured(package);
    match &result {
        Ok((_, timing)) => crate::diagnostics::emit(
            "native_skin_runtime_timing",
            &serde_json::json!({
                "run_id": report.borrow().run_id,
                "interaction_ids": interaction_ids,
                "canvas_id": canvas_id,
                "output": output,
                "cache_phase": timing.cache_phase,
                "engine_us": timing.engine_us,
                "module_us": timing.module_us,
                "cache_duration_us": timing.duration_us,
                "duration_us": duration_us(started.elapsed()),
                "status": "success",
            }),
        ),
        Err(_) => crate::diagnostics::emit(
            "native_skin_runtime_timing",
            &serde_json::json!({
                "run_id": report.borrow().run_id,
                "interaction_ids": interaction_ids,
                "canvas_id": canvas_id,
                "output": output,
                "duration_us": duration_us(started.elapsed()),
                "status": "error",
                "error_type": "skin_runtime_create_failed",
            }),
        ),
    }
    result.map(|(runtime, _)| runtime)
}

#[derive(Clone)]
struct NativeOverlayProps {
    initial: NativeDocumentProjection,
    published: Rc<RefCell<Option<Reactive<NativeDocumentProjection>>>>,
    port: NativeEditorPort,
}

#[derive(Clone, PartialEq)]
// Keeping both complete projections inline avoids an allocation on each native frame update.
#[allow(clippy::large_enum_variant)]
enum NativeDocumentProjection {
    Display {
        canvas: scorepeek_overlay::CanvasPresentation,
        visible: bool,
    },
    Editor(StageProjection),
}

#[derive(Clone)]
struct NativeEditorPort {
    coordinator: std::sync::mpsc::Sender<CoordinatorCommand>,
    source_output: Option<String>,
    run_id: String,
    sequence: Arc<std::sync::atomic::AtomicU64>,
}

impl NativeEditorPort {
    fn send(&self, input: EditorInput, action: &'static str) {
        let interaction_id = self
            .sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .saturating_add(1);
        let _ = self.coordinator.send(CoordinatorCommand::EditorInput {
            input,
            correlation: Some(InteractionCorrelation {
                run_id: self.run_id.clone(),
                interaction_id,
                action,
            }),
        });
    }
}

struct Reactive<T: 'static>(Signal<T>);

impl<T: 'static> Clone for Reactive<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: 'static> Copy for Reactive<T> {}

impl<T: 'static> Reactive<T> {
    fn borrow(&self) -> dioxus::signals::ReadableRef<'_, Signal<T>> {
        self.0.read()
    }

    fn set(&self, value: T) {
        let mut signal = self.0;
        signal.set(value);
    }
}

#[allow(clippy::cast_precision_loss)]
fn native_overlay(props: NativeOverlayProps) -> Element {
    let initial = props.initial.clone();
    let projection = Reactive(use_signal(move || initial));
    *props.published.borrow_mut() = Some(projection);
    let surface_port = props.port.clone();
    let onsurface = Callback::new(move |action: SurfaceAction| match &action {
        SurfaceAction::Enter(canvas) => {
            let current = projection.borrow();
            if let NativeDocumentProjection::Display {
                canvas: visible, ..
            } = &*current
            {
                let _ = surface_port.coordinator.send(CoordinatorCommand::Open {
                    output: surface_port.source_output.clone(),
                    canvas: canvas.clone().unwrap_or_else(|| visible.id.clone()),
                    preview_screen: None,
                });
            } else {
                surface_port.send(EditorInput::Surface(action), "surface");
            }
        }
        _ => surface_port.send(EditorInput::Surface(action), "surface"),
    });
    let action_port = props.port;
    let onaction = Callback::new(move |action: EditorAction| {
        let name = action.diagnostic_name();
        action_port.send(EditorInput::Action(action), name);
    });
    match &*projection.borrow() {
        NativeDocumentProjection::Display { canvas, visible } => rsx! {
            style { {scorepeek_overlay::HOST_CSS} }
            EditorSurface { onaction:onsurface,
                div { class:"canvas-content",style:format!("display:{};opacity:{}",if *visible{"block"}else{"none"},f32::from(canvas.opacity_percent)/100.0),
                    div { id:"scorepeek-skin-root", class:"scorepeek-skin-scope", "data-backend":"native", style:"position:absolute;inset:0" }
                }
            }
        },
        NativeDocumentProjection::Editor(stage) => {
            let selected = stage.selected_canvas.as_ref();
            rsx! {
                style { {scorepeek_overlay::HOST_CSS} }
                EditorSurface { onaction:onsurface,
                    div { id:"scorepeek-skin-root", style:"display:none" }
                    for canvas in &stage.canvases {
                        Fragment { key:"{canvas.id}",
                            EditorCanvas {canvas:canvas.clone(),editing:stage.interactive,selected:stage.interactive&&selected.is_some_and(|selected|selected.id==canvas.id),selected_widget:stage.selected_widget.clone(),onaction:onsurface,
                                div { id:editor_skin_root_id(&canvas.id), class:"scorepeek-skin-scope", "data-backend":"native", "data-skin-canvas":"{canvas.id}", style:"position:absolute;inset:0" }
                            }
                        }
                    }
                    if stage.interactive {
                        if let Some(canvas)=selected {
                            EditorSelectionMetrics { canvas:canvas.clone(), selected_widget:stage.selected_widget.clone() }
                        }
                        EditorPanel {
                            view:stage.view.clone(),
                            title:stage.title.clone(),
                            onaction,
                        }
                        if let Some(kind)=stage.placing {
                            if let Some(size) = stage.view.skins.iter().find(|skin| stage.selected_canvas.as_ref().is_some_and(|canvas| canvas.skin == skin.id)).and_then(|skin| skin.widget_defaults.get(kind.name())).map(|default| [default.width, default.height]) {
                                PlacementPreview {kind,point:stage.point.map(f64::from),size}
                            }
                        }
                    }
                    if let Some(notice)=&stage.notice {
                        div { id:"notice", class:"show error", role:"alert", "{notice}" }
                    }
                }
            }
        }
    }
}

struct CalloopWaker(Ping);

fn native_skin_input(
    canvas: &crate::config::Canvas,
    state: &OverlayState,
    manifest: &crate::skin::Manifest,
) -> serde_json::Value {
    let canvas_properties = manifest.effective_canvas_properties(&canvas.skin_properties);
    serde_json::json!({
        "schema":"scorepeek-skin-input-v2",
        "backend":"native",
        "monotonic_ms":skin_monotonic_ms(),
        "canvas":{"id":canvas.id,"skin":canvas.skin.name(),"width":canvas.width,"height":canvas.height,"properties":canvas_properties},
        "widgets":canvas.widgets.iter().map(|widget| { let kind = serde_json::to_value(widget.kind).ok().and_then(|value| value.as_str().map(str::to_owned)).unwrap_or_default(); let properties = manifest.effective_widget_properties(&kind,&widget.skin_properties); serde_json::json!({"id":widget.id,"kind":widget.kind,"x":widget.x,"y":widget.y,"width":widget.width,"height":widget.height,"settings":widget.settings,"properties":properties}) }).collect::<Vec<_>>(),
        "state":state,
    })
}

fn native_skin_input_presentation(
    canvas: &scorepeek_overlay::CanvasPresentation,
    state: &OverlayState,
    manifest: &crate::skin::Manifest,
) -> serde_json::Value {
    let canvas_properties = manifest.effective_canvas_properties(&canvas.skin_properties);
    serde_json::json!({
        "schema":"scorepeek-skin-input-v2",
        "backend":"native",
        "monotonic_ms":skin_monotonic_ms(),
        "canvas":{"id":canvas.id,"skin":canvas.skin.name(),"width":canvas.width,"height":canvas.height,"properties":canvas_properties},
        "widgets":canvas.widgets.iter().map(|widget| { let kind = serde_json::to_value(widget.kind).ok().and_then(|value| value.as_str().map(str::to_owned)).unwrap_or_default(); let properties = manifest.effective_widget_properties(&kind,&widget.skin_properties); serde_json::json!({"id":widget.id,"kind":widget.kind,"x":widget.x,"y":widget.y,"width":widget.width,"height":widget.height,"settings":widget.settings,"properties":properties}) }).collect::<Vec<_>>(),
        "state":state,
    })
}

fn skin_monotonic_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    u64::try_from(START.get_or_init(Instant::now).elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn skin_deadline(schedule: &crate::skin::Schedule, editing: bool) -> Option<Instant> {
    if editing {
        return None;
    }
    match schedule {
        crate::skin::Schedule::Idle => None,
        crate::skin::Schedule::NextFrame => Some(Instant::now()),
        crate::skin::Schedule::AfterMs { milliseconds } => {
            Instant::now().checked_add(Duration::from_millis(*milliseconds))
        }
    }
}

fn skin_error_type(error: &str) -> &'static str {
    if error.contains("timeout") {
        "hard_timeout"
    } else if error.contains("trap") {
        "trap"
    } else if error.contains("tree") || error.contains("JSON") {
        "invalid_output"
    } else {
        "runtime_error"
    }
}

#[cfg(test)]
fn embedded_editor_skins() -> Vec<scorepeek_overlay::editor::EditorSkin> {
    [
        include_str!("../../../../skins/cyan-system/skin.toml"),
        include_str!("../../../../skins/result-aurora/skin.toml"),
        include_str!("../../../../skins/dj-blackbox/skin.toml"),
    ]
    .into_iter()
    .filter_map(|source| {
        let manifest: crate::skin::Manifest = toml::from_str(source).ok()?;
        Some(scorepeek_overlay::editor::EditorSkin {
            id: manifest.id.parse().ok()?,
            name: manifest.name,
            release: manifest.release,
            preview: String::new(),
            preview_video: None,
            widget_defaults: serde_json::from_value(
                serde_json::to_value(manifest.widget_defaults).ok()?,
            )
            .ok()?,
            canvas_properties: serde_json::from_value(
                serde_json::to_value(manifest.canvas_properties).ok()?,
            )
            .ok()?,
            widget_properties: serde_json::from_value(
                serde_json::to_value(manifest.widget_properties).ok()?,
            )
            .ok()?,
        })
    })
    .collect()
}
#[allow(clippy::cast_possible_truncation)]
fn snap_i32(value: f64) -> i32 {
    ((value / 4.0).round() * 4.0).clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}
const fn grid_floor(value: u32) -> u32 {
    value / 4 * 4
}
fn maximum_grid_position(output: u32, extent: u32) -> i32 {
    i32::try_from(grid_floor(output.saturating_sub(extent))).unwrap_or(i32::MAX)
}

impl Wake for CalloopWaker {
    fn wake(self: Arc<Self>) {
        self.0.ping();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.ping();
    }
}

fn recent_same_failure(
    failure: &(Option<String>, Instant),
    canvas: &crate::config::Canvas,
) -> bool {
    failure.0.as_deref() == Some(canvas.output.as_str())
        && failure.1.elapsed() < Duration::from_secs(5)
}

#[derive(Debug, PartialEq, Eq)]
enum NativeWorkerExit {
    Completed,
    Failed {
        error_type: &'static str,
        error: String,
    },
}

fn classify_native_worker_exit(
    result: std::thread::Result<Result<(), String>>,
) -> NativeWorkerExit {
    match result {
        Ok(Ok(())) => NativeWorkerExit::Completed,
        Ok(Err(error)) => NativeWorkerExit::Failed {
            error_type: canvas_worker_error_type(&error),
            error,
        },
        Err(payload) => NativeWorkerExit::Failed {
            error_type: "panic",
            error: payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| {
                    payload
                        .downcast_ref::<&str>()
                        .map(|message| (*message).to_owned())
                })
                .unwrap_or_else(|| "unknown canvas worker panic".to_owned()),
        },
    }
}

fn emit_native_worker_failure(
    canvas_id: &str,
    output: Option<&str>,
    error_type: &str,
    error: &str,
) {
    crate::diagnostics::emit(
        "native_canvas_failed",
        &serde_json::json!({"canvas_id":canvas_id,"output":output,"status":"error","error_type":error_type,"error":error}),
    );
}

fn reap_native_workers_on_shutdown(
    workers: std::collections::BTreeMap<String, NativeWorker>,
) -> Result<(), String> {
    let mut first_failure = None;
    for (id, worker) in workers {
        let output = worker.output.clone();
        if let NativeWorkerExit::Failed { error_type, error } =
            classify_native_worker_exit(worker.join.join())
        {
            emit_native_worker_failure(&id, output.as_deref(), error_type, &error);
            first_failure.get_or_insert_with(|| {
                format!("native canvas worker {id} failed during shutdown ({error_type}): {error}")
            });
        }
    }
    first_failure.map_or(Ok(()), Err)
}

fn execute_native_editor_effect(
    effect: &EditorEffect,
    control_socket: &std::path::Path,
    editor_id: &str,
    fallback: &[scorepeek_overlay::CanvasPresentation],
) -> EditorBackendReply {
    let request = match effect {
        EditorEffect::Acquire => crate::bridge::action::Request::AcquireBackend {
            backend: crate::bridge::data::Backend::Wayland,
            editor_id: editor_id.to_owned(),
        },
        EditorEffect::KeepAlive => crate::bridge::action::Request::KeepAliveBackend {
            backend: crate::bridge::data::Backend::Wayland,
            editor_id: editor_id.to_owned(),
        },
        EditorEffect::Update { canvases } => crate::bridge::action::Request::UpdateBackendDraft {
            backend: crate::bridge::data::Backend::Wayland,
            editor_id: editor_id.to_owned(),
            canvases: canvases.clone(),
        },
        EditorEffect::Save { canvases } => crate::bridge::action::Request::CommitBackend {
            backend: crate::bridge::data::Backend::Wayland,
            editor_id: editor_id.to_owned(),
            canvases: canvases.clone(),
        },
        EditorEffect::Discard | EditorEffect::Close => {
            crate::bridge::action::Request::ReleaseBackend {
                backend: crate::bridge::data::Backend::Wayland,
                editor_id: editor_id.to_owned(),
            }
        }
    };
    let started = Instant::now();
    let response = crate::bridge::action::request(control_socket, &request);
    let request_name = match effect {
        EditorEffect::Acquire => "acquire_backend",
        EditorEffect::KeepAlive => "keep_alive_backend",
        EditorEffect::Update { .. } => "update_backend_draft",
        EditorEffect::Save { .. } => "commit_backend",
        EditorEffect::Discard | EditorEffect::Close => "release_backend",
    };
    crate::diagnostics::emit(
        "native_editor_effect",
        &serde_json::json!({
            "request": request_name,
            "effect": format!("{:?}", effect.kind()),
            "duration_us": duration_us(started.elapsed()),
            "status": if response.as_ref().is_ok_and(|reply| reply.ok) { "success" } else { "error" },
        }),
    );
    match response {
        Ok(response) => EditorBackendReply {
            ok: response.ok,
            readonly: response.readonly,
            error: response.error,
            canvases: if !response.ok
                || (response.canvases.is_empty()
                    && !matches!(
                        effect,
                        EditorEffect::Acquire
                            | EditorEffect::Update { .. }
                            | EditorEffect::Save { .. }
                    )) {
                fallback.to_vec()
            } else {
                response.canvases
            },
            dirty: response.dirty,
        },
        Err(error) => EditorBackendReply {
            ok: false,
            readonly: true,
            error: Some(error),
            canvases: fallback.to_vec(),
            dirty: true,
        },
    }
}

fn execute_native_editor_effects(
    authority: &mut NativeEditorAuthority,
    effects: Vec<EditorEffect>,
    control_socket: &std::path::Path,
    editor_id: &str,
) {
    let mut pending = std::collections::VecDeque::from(effects);
    while let Some(effect) = pending.pop_front() {
        let fallback = authority.session().draft.clone();
        let reply = execute_native_editor_effect(&effect, control_socket, editor_id, &fallback);
        pending.extend(authority.dispatch(EditorInput::BackendCompleted {
            effect: effect.kind(),
            requested_draft: effect.requested_draft().map(<[_]>::to_vec),
            reply,
        }));
    }
}

/// Runs the production native coordinator with a deterministic scenario input.
///
/// The driver only replaces human timing. Every action still crosses the
/// production coordinator and is reduced by the sole `EditorSession` authority.
/// # Errors
/// Returns Wayland, GPU or event-loop failures.
#[doc(hidden)]
#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
pub(crate) fn run_with_editor_scenario(
    config: Config,
    input: impl std::io::Read + Send + 'static,
    scenario: Vec<crate::host::run::NativeEditorScenarioStep>,
) -> Result<(), String> {
    use std::collections::BTreeMap;
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    crate::host::lifecycle::watch_parent(input, Arc::clone(&stop))?;
    let canvas_wakes = Arc::new(std::sync::Mutex::new(BTreeMap::<String, Ping>::new()));
    let waking = Arc::clone(&canvas_wakes);
    let feed = Feed::start(
        config.clone().into(),
        Arc::new(move || {
            for wake in waking
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .values()
            {
                wake.ping();
            }
        }),
        Arc::new(crate::diagnostics::emit),
    )
    .map_err(|error| error.to_string())?;
    let feed_state = Arc::clone(&feed.state);
    let feed_stop = Arc::clone(&feed.stop);
    let mut desired = config.canvases.clone();
    let mut workers = BTreeMap::<String, NativeWorker>::new();
    let mut failed = BTreeMap::<String, (Option<String>, Instant)>::new();
    let (coordinator_tx, coordinator_rx) = std::sync::mpsc::channel();
    let (startup_tx, startup_rx) = std::sync::mpsc::channel();
    let mut startup_pending = None::<std::collections::BTreeSet<String>>;
    let mut startup_complete = false;
    if !scenario.is_empty() {
        let scenario_tx = coordinator_tx.clone();
        std::thread::Builder::new()
            .name("overlay-wayland-scenario".into())
            .spawn(move || {
                for (interaction_id, step) in scenario.into_iter().enumerate() {
                    std::thread::sleep(step.after);
                    if scenario_tx
                        .send(CoordinatorCommand::EditorInput {
                            input: EditorInput::Action(EditorAction::SelectCanvas(
                                step.target_canvas.into(),
                            )),
                            correlation: Some(InteractionCorrelation {
                                run_id: "nested-editor-setup".into(),
                                interaction_id: u64::try_from(interaction_id).unwrap_or(u64::MAX),
                                action: "target_canvas_selected",
                            }),
                        })
                        .is_err()
                    {
                        break;
                    }
                    if scenario_tx
                        .send(CoordinatorCommand::EditorInput {
                            input: EditorInput::Action(step.action),
                            correlation: Some(InteractionCorrelation {
                                run_id: "nested-editor-scenario".into(),
                                interaction_id: u64::try_from(interaction_id).unwrap_or(u64::MAX),
                                action: step.name,
                            }),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|error| error.to_string())?;
    }
    let editor_id = format!("wayland-{}", std::process::id());
    let start_editing = config.edit_on_start || desired.is_empty();
    let skin_assets = Arc::new(SkinAssetCache::new(crate::skin::StoreRoot::new(
        config.skin_store.clone(),
    )));
    let installed_skins = skin_assets.installed_editor_skins()?;
    let fallback_skin = installed_skins.first().map(|skin| skin.id);
    let probe = desired.first().cloned().map_or_else(
        || {
            fallback_skin
                .map(editor_bootstrap)
                .ok_or_else(|| String::from("Wayland editor requires at least one installed skin"))
        },
        Ok,
    )?;
    let outputs = discover_editor_outputs(&probe)?;
    let initial = desired
        .iter()
        .map(crate::config::Canvas::presentation)
        .collect::<Vec<_>>();
    let viewport = outputs
        .first()
        .and_then(|output| output.logical_size)
        .unwrap_or([1920, 1080]);
    let mut session = EditorSession::new(initial, viewport, "wayland");
    session.set_session_id(
        u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros(),
        )
        .unwrap_or(u64::MAX),
    );
    session.set_skins(installed_skins);
    session.set_outputs(editor_outputs_from_descriptions(&outputs));
    let published_stages = Arc::new(std::sync::Mutex::new(PublishedStages::default()));
    let mut authority = NativeEditorAuthority::new(session, Arc::clone(&published_stages));
    if start_editing {
        let effects = authority.dispatch(EditorInput::Open {
            output: outputs.first().map(|output| output.name.clone()),
            canvas: desired.first().map(|canvas| canvas.id.clone()),
            preview: scorepeek_overlay::ScreenKind::MusicSelect,
        });
        execute_native_editor_effects(&mut authority, effects, &config.control_socket, &editor_id);
    }
    let mut was_editing = authority.session().editing;
    let mut projection_cache = NativeProjectionCache::default();
    let mut coordinator_work = FrameWorkProfile::default();
    let mut next_keepalive = Instant::now() + Duration::from_secs(5);
    while !stop.load(std::sync::atomic::Ordering::Acquire) {
        while let Ok(command) = coordinator_rx.try_recv() {
            match command {
                CoordinatorCommand::EditorInput { input, correlation } => {
                    if let Some(correlation) = correlation.as_ref() {
                        crate::diagnostics::emit(
                            "native_editor_input_transport",
                            &serde_json::json!({
                                "run_id": correlation.run_id,
                                "interaction_id": correlation.interaction_id,
                                "action": correlation.action,
                                "status": "received",
                            }),
                        );
                    }
                    let before_revision = authority.session().revision;
                    let effects = authority.dispatch(input);
                    execute_native_editor_effects(
                        &mut authority,
                        effects,
                        &config.control_socket,
                        &editor_id,
                    );
                    if let Some(correlation) = correlation {
                        let session = authority.session();
                        crate::diagnostics::emit(
                            "native_editor_input_applied",
                            &serde_json::json!({
                                "run_id": correlation.run_id,
                                "interaction_id": correlation.interaction_id,
                                "action": correlation.action,
                                "session_id": session.session_id,
                                "revision": session.revision,
                                "changed": session.revision > before_revision,
                                "status": "applied",
                            }),
                        );
                    }
                }
                CoordinatorCommand::Open {
                    output,
                    canvas,
                    preview_screen,
                } => {
                    let effects = authority.dispatch(EditorInput::Open {
                        output,
                        canvas: Some(canvas),
                        preview: preview_screen
                            .unwrap_or(scorepeek_overlay::ScreenKind::MusicSelect),
                    });
                    execute_native_editor_effects(
                        &mut authority,
                        effects,
                        &config.control_socket,
                        &editor_id,
                    );
                }
            }
            for wake in canvas_wakes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .values()
            {
                wake.ping();
            }
        }
        if authority.session().editing && Instant::now() >= next_keepalive {
            let effects = authority.dispatch(EditorInput::KeepAliveTick);
            execute_native_editor_effects(
                &mut authority,
                effects,
                &config.control_socket,
                &editor_id,
            );
            next_keepalive = Instant::now() + Duration::from_secs(5);
        }
        let editing = authority.session().editing;
        if was_editing && !editing {
            if let Ok(response) = crate::bridge::action::request(
                &config.control_socket,
                &crate::bridge::action::Request::GetBackend {
                    backend: crate::bridge::data::Backend::Wayland,
                },
            ) {
                desired = response
                    .canvases
                    .into_iter()
                    .map(|presentation| {
                        let mut canvas = crate::config::empty_canvas(
                            presentation.id.clone(),
                            crate::bridge::data::Backend::Wayland,
                            presentation.skin,
                        );
                        canvas.apply_presentation(&presentation);
                        canvas
                    })
                    .collect();
            }
            if desired.is_empty() {
                let output = authority
                    .session()
                    .outputs
                    .first()
                    .map(|output| output.name.clone());
                let effects = authority.dispatch(EditorInput::Open {
                    output,
                    canvas: None,
                    preview: scorepeek_overlay::ScreenKind::MusicSelect,
                });
                execute_native_editor_effects(
                    &mut authority,
                    effects,
                    &config.control_socket,
                    &editor_id,
                );
            }
        }
        was_editing = authority.session().editing;
        let editing = was_editing;
        let session = authority.session();
        let projection_started = Instant::now();
        let projected = projection_cache.resolve(fallback_skin, &session, &desired)?;
        if startup_pending.is_none() {
            startup_pending = Some(projected.iter().map(|canvas| canvas.id.clone()).collect());
        }
        coordinator_work.record("projection", projection_started.elapsed());
        let lifecycle_started = Instant::now();
        let reconciliation = reconcile_worker_lifecycle(
            workers.iter().map(|(id, worker)| {
                (
                    id.as_str(),
                    worker.output.as_deref(),
                    worker.join.is_finished(),
                )
            }),
            projected,
        );
        coordinator_work.record("surface_lifecycle", lifecycle_started.elapsed());
        stop_workers(reconciliation.stop_join.iter(), &workers, &canvas_wakes);
        for id in reconciliation.stop_join {
            if let Some(worker) = workers.remove(&id) {
                let output = worker.output.clone();
                match classify_native_worker_exit(worker.join.join()) {
                    NativeWorkerExit::Completed => {
                        failed.remove(&id);
                    }
                    NativeWorkerExit::Failed { error_type, error } => {
                        if let Some(canvas) = projected.iter().find(|canvas| canvas.id == id) {
                            failed
                                .insert(id.clone(), (Some(canvas.output.clone()), Instant::now()));
                        }
                        emit_native_worker_failure(&id, output.as_deref(), error_type, &error);
                    }
                }
            }
            canvas_wakes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&id);
        }
        for id in reconciliation.start {
            let Some(canvas) = projected.iter().find(|canvas| canvas.id == id) else {
                return Err(format!(
                    "worker reconciliation referenced missing projected canvas {id}"
                ));
            };
            if failed
                .get(&canvas.id)
                .is_some_and(|failure| recent_same_failure(failure, canvas))
            {
                continue;
            }
            let config_started = Instant::now();
            let mut canvas_config = config.clone();
            canvas_config.canvases = vec![canvas.clone()];
            coordinator_work.record("canvas_config", config_started.elapsed());
            let canvas_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stopping = Arc::clone(&canvas_stop);
            let state = Arc::clone(&feed_state);
            let stopped = Arc::clone(&feed_stop);
            let stages = Arc::clone(&published_stages);
            let wakes = Arc::clone(&canvas_wakes);
            let coordinator = coordinator_tx.clone();
            let startup = startup_tx.clone();
            let assets = Arc::clone(&skin_assets);
            let role = if editing {
                SurfaceRole::EditorStage
            } else {
                SurfaceRole::DisplayCanvas
            };
            let join = std::thread::Builder::new()
                .name(format!("overlay-wayland-{}", canvas.id))
                .spawn(move || {
                    run_canvas(
                        &canvas_config,
                        stopping,
                        state,
                        stopped,
                        stages,
                        &wakes,
                        coordinator,
                        role,
                        assets,
                        &startup,
                    )
                })
                .map_err(|error| error.to_string())?;
            workers.insert(
                canvas.id.clone(),
                NativeWorker {
                    output: Some(canvas.output.clone()),
                    stop: canvas_stop,
                    join,
                },
            );
        }
        while let Ok(event) = startup_rx.try_recv() {
            if startup_complete {
                continue;
            }
            match event {
                NativeWorkerStartup::Ready(canvas_id) => {
                    if let Some(pending) = startup_pending.as_mut() {
                        pending.remove(&canvas_id);
                    }
                }
                NativeWorkerStartup::Failed { canvas_id, error } => {
                    return Err(format!(
                        "native canvas worker {canvas_id} failed during initialization: {error}"
                    ));
                }
            }
        }
        if !startup_complete
            && startup_pending
                .as_ref()
                .is_some_and(std::collections::BTreeSet::is_empty)
        {
            crate::diagnostics::emit(
                "child_ready",
                &serde_json::json!({"backend":"wayland","status":"success"}),
            );
            startup_complete = true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let ids = workers.keys().cloned().collect::<Vec<_>>();
    stop_workers(ids.iter(), &workers, &canvas_wakes);
    let shutdown_result = reap_native_workers_on_shutdown(workers);
    if authority.session().editing {
        let effects = authority.dispatch(EditorInput::Action(EditorAction::Close));
        execute_native_editor_effects(&mut authority, effects, &config.control_socket, &editor_id);
    }
    crate::diagnostics::emit("native_coordinator_work", &coordinator_work);
    shutdown_result
}

fn discover_editor_outputs(
    canvas: &crate::config::Canvas,
) -> Result<Vec<OutputDescription>, String> {
    let ping = make_ping().map_err(|error| error.to_string())?;
    let shell = Shell::open(
        Some(canvas.output.as_str()),
        canvas.width,
        canvas.height,
        canvas.x,
        canvas.y,
        false,
        ping.1,
    )?;
    Ok(shell.output_descriptions)
}

fn editor_outputs_from_descriptions(outputs: &[OutputDescription]) -> Vec<EditorOutput> {
    outputs
        .iter()
        .map(|output| EditorOutput {
            name: output.name.clone(),
            model: output.model.clone(),
            logical_size: output.logical_size,
        })
        .collect()
}

fn editor_bootstrap(skin: scorepeek_overlay::Skin) -> crate::config::Canvas {
    let mut bootstrap = crate::config::empty_canvas(
        "__scorepeek-editor-bootstrap".into(),
        crate::bridge::data::Backend::Wayland,
        skin,
    );
    bootstrap.skin = skin;
    bootstrap.x = 0;
    bootstrap.y = 0;
    bootstrap.width = 1920;
    bootstrap.height = 1080;
    bootstrap.show_on = Some(Vec::new());
    bootstrap
}

fn editor_stage_canvases(
    fallback_skin: Option<scorepeek_overlay::Skin>,
    canvases: &[crate::config::Canvas],
    outputs: &[OutputDescription],
) -> Result<Vec<crate::config::Canvas>, String> {
    if outputs.is_empty() {
        return fallback_skin
            .map(editor_bootstrap)
            .map(|bootstrap| vec![bootstrap])
            .ok_or_else(|| "Wayland editor requires at least one installed skin".into());
    }
    let skin = if let Some(canvas) = canvases.first() {
        canvas.skin
    } else {
        fallback_skin.ok_or("Wayland editor requires at least one installed skin")?
    };
    Ok(editor_stage_projections(canvases, outputs, skin))
}

fn editor_stage_projections(
    canvases: &[crate::config::Canvas],
    outputs: &[OutputDescription],
    skin: scorepeek_overlay::Skin,
) -> Vec<crate::config::Canvas> {
    outputs
        .iter()
        .enumerate()
        .map(|(index, output)| {
            let mut id = format!("__scorepeek-editor-stage-{index}");
            while canvases.iter().any(|canvas| canvas.id == id) {
                id.push('_');
            }
            let mut stage =
                crate::config::empty_canvas(id, crate::bridge::data::Backend::Wayland, skin);
            stage.skin = skin;
            stage.output.clone_from(&output.name);
            stage.show_on = Some(Vec::new());
            stage.x = 0;
            stage.y = 0;
            if let Some([width, height]) = output.logical_size {
                stage.width = width.max(32);
                stage.height = height.max(32);
            }
            stage
        })
        .collect()
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn run_canvas(
    config: &Config,
    external_stop: Arc<std::sync::atomic::AtomicBool>,
    feed_state: Arc<std::sync::Mutex<OverlayState>>,
    feed_stop: Arc<std::sync::atomic::AtomicBool>,
    published_stages: Arc<std::sync::Mutex<PublishedStages>>,
    wakes: &Arc<std::sync::Mutex<std::collections::BTreeMap<String, Ping>>>,
    coordinator: std::sync::mpsc::Sender<CoordinatorCommand>,
    role: SurfaceRole,
    skin_assets: Arc<SkinAssetCache>,
    startup: &std::sync::mpsc::Sender<NativeWorkerStartup>,
) -> Result<(), String> {
    let canvas_id = config
        .canvases
        .first()
        .map_or_else(|| "unknown".into(), |canvas| canvas.id.clone());
    let result = run_canvas_inner(
        config,
        external_stop,
        feed_state,
        feed_stop,
        published_stages,
        wakes,
        coordinator,
        role,
        skin_assets,
        startup,
    );
    if let Err(error) = &result {
        let _ = startup.send(NativeWorkerStartup::Failed {
            canvas_id,
            error: error.clone(),
        });
    }
    result
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn run_canvas_inner(
    config: &Config,
    external_stop: Arc<std::sync::atomic::AtomicBool>,
    feed_state: Arc<std::sync::Mutex<OverlayState>>,
    feed_stop: Arc<std::sync::atomic::AtomicBool>,
    published_stages: Arc<std::sync::Mutex<PublishedStages>>,
    wakes: &Arc<std::sync::Mutex<std::collections::BTreeMap<String, Ping>>>,
    coordinator: std::sync::mpsc::Sender<CoordinatorCommand>,
    role: SurfaceRole,
    skin_assets: Arc<SkinAssetCache>,
    startup: &std::sync::mpsc::Sender<NativeWorkerStartup>,
) -> Result<(), String> {
    let startup_started = Instant::now();
    let report = Rc::new(RefCell::new(RunReport::new()));
    let ping = make_ping().map_err(|e| e.to_string())?;
    let wake = ping.0.clone();
    let mut canvas = config
        .canvases
        .first()
        .ok_or("Wayland overlay has no canvas")?
        .clone();
    report.borrow_mut().canvas_id = Some(canvas.id.clone());
    wakes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(canvas.id.clone(), wake);
    let configured_output = canvas.output.clone();
    let shell_started = Instant::now();
    let shell = Shell::open(
        Some(canvas.output.as_str()),
        canvas.width,
        canvas.height,
        canvas.x,
        canvas.y,
        false,
        ping.1,
    )?;
    crate::diagnostics::emit(
        "native_startup_timing",
        &serde_json::json!({
            "run_id": report.borrow().run_id,
            "canvas_id": canvas.id,
            "phase": "shell_opened",
            "duration_us": duration_us(shell_started.elapsed()),
            "elapsed_us": duration_us(startup_started.elapsed()),
        }),
    );
    let resolved_output = shell
        .output_name
        .as_deref()
        .filter(|selected_output| canvas.output != *selected_output);
    let output_changed = resolved_output.is_some();
    if let Some(selected_output) = resolved_output {
        if let Some([output_width, output_height]) = shell.output_logical_size {
            canvas.width = canvas.width.min(grid_floor(output_width));
            canvas.height = canvas.height.min(grid_floor(output_height));
            canvas.x = canvas
                .x
                .clamp(0, maximum_grid_position(output_width, canvas.width));
            canvas.y = canvas
                .y
                .clamp(0, maximum_grid_position(output_height, canvas.height));
        }
        crate::diagnostics::emit(
            "native_output_fallback",
            &serde_json::json!({
                "canvas_id": canvas.id,
                "configured_output": configured_output,
                "selected_output": selected_output,
                "status": "draft_required",
            }),
        );
    }
    canvas.x = shell.position[0];
    canvas.y = shell.position[1];
    if output_changed && let Some([output_width, output_height]) = shell.output_logical_size {
        if canvas.id.starts_with("__scorepeek-editor-") {
            canvas.x = 0;
            canvas.y = 0;
            canvas.width = output_width.max(32);
            canvas.height = output_height.max(32);
        }
        canvas.x = canvas
            .x
            .clamp(0, maximum_grid_position(output_width, canvas.width));
        canvas.y = canvas
            .y
            .clamp(0, maximum_grid_position(output_height, canvas.height));
    }
    let outputs = shell.output_descriptions.clone();
    report
        .borrow_mut()
        .output_name
        .clone_from(&shell.output_name);
    report
        .borrow_mut()
        .operations
        .push(if shell.fractional_scaling {
            "fractional_scale_enabled"
        } else {
            "integer_scale_fallback"
        });
    let renderer_started = Instant::now();
    let renderer = VelloWindowRenderer::with_options(
        VelloRendererOptions::default()
            .base_color(peniko::Color::TRANSPARENT)
            .composite_alpha_mode(CompositeAlphaMode::Transparent),
    );
    let renderer_duration = renderer_started.elapsed();
    crate::diagnostics::emit(
        "native_startup_timing",
        &serde_json::json!({
            "run_id": report.borrow().run_id,
            "canvas_id": canvas.id,
            "phase": "renderer_created",
            "duration_us": duration_us(renderer_duration),
            "elapsed_us": duration_us(startup_started.elapsed()),
        }),
    );
    report
        .borrow_mut()
        .operations
        .push("renderer_context_initialized");
    let app_started = Instant::now();
    let mut app = App::new(
        renderer,
        shell,
        Waker::from(Arc::new(CalloopWaker(ping.0))),
        feed_state,
        feed_stop,
        external_stop,
        canvas,
        skin_assets,
        outputs,
        Rc::clone(&report),
        published_stages,
        startup_started,
        coordinator,
        role,
    )?;
    crate::diagnostics::emit(
        "native_startup_timing",
        &serde_json::json!({
            "run_id": report.borrow().run_id,
            "canvas_id": app.surface_canvas.id,
            "phase": "app_initialized",
            "duration_us": duration_us(app_started.elapsed()),
            "elapsed_us": duration_us(startup_started.elapsed()),
        }),
    );
    let _ = startup.send(NativeWorkerStartup::Ready(app.surface_canvas.id.clone()));
    let result = app.run();
    let unmap = shutdown_native_surface(
        &mut app,
        |app| {
            app.unmount_skin_resources();
            crate::diagnostics::emit(
                "native_renderer_shutdown",
                &serde_json::json!({
                    "run_id": report.borrow().run_id,
                    "canvas_id": app.surface_canvas.id,
                    "output": app.surface_output,
                    "phase": "app_stopped",
                }),
            );
        },
        |app| {
            crate::diagnostics::emit(
                "native_renderer_shutdown",
                &serde_json::json!({
                    "run_id": report.borrow().run_id,
                    "canvas_id": app.surface_canvas.id,
                    "output": app.surface_output,
                    "phase": "suspend_requested",
                }),
            );
            crate::diagnostics::emit(
                "native_renderer_shutdown",
                &serde_json::json!({
                    "run_id": report.borrow().run_id,
                    "canvas_id": app.surface_canvas.id,
                    "output": app.surface_output,
                    "phase": "suspend_started",
                }),
            );
            app.renderer.suspend();
            crate::diagnostics::emit(
                "native_renderer_shutdown",
                &serde_json::json!({
                    "run_id": report.borrow().run_id,
                    "canvas_id": app.surface_canvas.id,
                    "output": app.surface_output,
                    "phase": "suspend_completed",
                }),
            );
        },
        |app| app.shell.unmap(),
    );
    crate::diagnostics::emit(
        "native_surface_unmap",
        &serde_json::json!({
            "run_id": report.borrow().run_id,
            "canvas_id": app.surface_canvas.id,
            "output": app.surface_output,
            "status": if unmap.is_ok() { "success" } else { "error" },
            "error_type": unmap.as_ref().err().map(|_| "wayland_flush_failed"),
        }),
    );
    let (result, secondary_failure) = native_shutdown_result(result, unmap);
    {
        let mut report = report.borrow_mut();
        report.paint_count = app.paint_count;
        report.render_calls = app.render_calls;
        report.editor_skin_update_requests = app.editor_skin_updates.requests;
        report.editor_skin_render_count = app.editor_skin_updates.renders;
        report.skin_package_open_count = app
            .skin_assets
            .open_count
            .load(std::sync::atomic::Ordering::Relaxed);
        report.skin_package_open_ns = app
            .skin_assets
            .open_ns
            .load(std::sync::atomic::Ordering::Relaxed);
        report.skin_package_clone_count = app
            .skin_assets
            .clone_count
            .load(std::sync::atomic::Ordering::Relaxed);
        report.skin_package_clone_ns = app
            .skin_assets
            .clone_ns
            .load(std::sync::atomic::Ordering::Relaxed);
        report.resource_lookup_count = app
            .skin_assets
            .resource_lookup_count
            .load(std::sync::atomic::Ordering::Relaxed);
        report.resource_lookup_ns = app
            .skin_assets
            .resource_lookup_ns
            .load(std::sync::atomic::Ordering::Relaxed);
        report.skin_runtime_create_count = app.skin_runtime_create_count;
        report.frame_work = app.frame_work.clone();
        report.elapsed_ms = u64::try_from(app.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let seconds = app.started.elapsed().as_secs_f64();
        report.effective_paint_hz = (seconds > 0.0).then(|| f64::from(app.paint_count) / seconds);
        report.status = if result.is_ok() { "complete" } else { "failed" };
        report.failure_type = result
            .as_ref()
            .err()
            .map(|error| canvas_worker_error_type(error));
        report.failure = result.as_ref().err().cloned();
        report.secondary_failure_type = secondary_failure.as_deref().map(canvas_worker_error_type);
        report.secondary_failure = secondary_failure;
        report.operations.push("shutdown");
        crate::diagnostics::emit("native_summary", &*report);
    }
    drop(app);
    result
}

struct App {
    // Renderer is dropped before the shell; its own Arc handle also retains ownership.
    renderer: VelloWindowRenderer,
    shell: Shell,
    document: DioxusDocument,
    pointer: PointerInput,
    projection: Reactive<NativeDocumentProjection>,
    published_stages: Arc<std::sync::Mutex<PublishedStages>>,
    waker: Waker,
    started: Instant,
    surface_state: NativeDisplaySurfaceState,
    paint_count: u32,
    render_calls: u32,
    full_layout_pending: bool,
    editor_skin_updates: EditorSkinUpdates,
    report: Rc<RefCell<RunReport>>,
    feed_state: Arc<std::sync::Mutex<OverlayState>>,
    current_state: OverlayState,
    feed_stop: Arc<std::sync::atomic::AtomicBool>,
    external_stop: Arc<std::sync::atomic::AtomicBool>,
    surface_canvas: crate::config::Canvas,
    surface_output: Option<String>,
    canvas: crate::config::Canvas,
    role: SurfaceRole,
    coordinator: std::sync::mpsc::Sender<CoordinatorCommand>,
    output_descriptions: Vec<OutputDescription>,
    surface_logical: [u32; 2],
    display_skin: Option<NativeDisplaySkin>,
    editor_skin_previews: std::collections::BTreeMap<String, EditorSkinPreview>,
    next_skin_render: Option<Instant>,
    skin_assets: Arc<SkinAssetCache>,
    skin_runtime_create_count: u64,
    frame_work: FrameWorkProfile,
    pending_frame_start: Option<FrameWorkSample>,
    startup_started: Instant,
    /// Renderer/protocol composition state used only to normalize Wayland key events to DOM.
    text_composition: TextComposition,
}

#[derive(Clone, Default, PartialEq, Eq)]
enum TextComposition {
    #[default]
    Idle,
    Active(String),
}

impl NativeEventConsumer for App {
    fn configure_event(
        &mut self,
        logical: [u32; 2],
        physical: [u32; 2],
        scale_120: u32,
    ) -> Result<(), String> {
        self.configure(logical, physical, scale_120)?;
        if self.editing()
            && let Some(output) = self.surface_output.clone()
        {
            let _ = self.coordinator.send(CoordinatorCommand::EditorInput {
                input: EditorInput::Resize {
                    output,
                    logical_size: logical,
                },
                correlation: None,
            });
        }
        Ok(())
    }

    fn pointer_motion_event(&mut self, point: [f64; 2]) {
        self.pointer
            .dispatch(&mut self.document, point, 0x110, None);
    }

    fn pointer_button_event(&mut self, button: u32, pressed: bool, point: [f64; 2]) {
        self.pointer_button(button, pressed, point[0], point[1]);
    }

    fn pointer_scroll_event(&mut self, delta: [f64; 2], point: [f64; 2]) {
        self.pointer.wheel(&mut self.document, point, delta);
    }

    fn text_event(&mut self, command: &scorepeek_overlay_wayland_handles::TextCommand) {
        self.input_command(command);
    }

    fn ime_event(&mut self, update: scorepeek_overlay_wayland_handles::TextUpdate) {
        self.input_composition(update);
    }

    fn keyboard_focus_event(&mut self, focused: bool) {
        if !focused {
            self.shell.set_text_input(None);
            self.set_text_composing(false);
        }
    }
}

impl App {
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn new(
        renderer: VelloWindowRenderer,
        mut shell: Shell,
        waker: Waker,
        feed_state: Arc<std::sync::Mutex<OverlayState>>,
        feed_stop: Arc<std::sync::atomic::AtomicBool>,
        external_stop: Arc<std::sync::atomic::AtomicBool>,
        canvas: crate::config::Canvas,
        skin_assets: Arc<SkinAssetCache>,
        outputs: Vec<OutputDescription>,
        report: Rc<RefCell<RunReport>>,
        published_stages: Arc<std::sync::Mutex<PublishedStages>>,
        startup_started: Instant,
        coordinator: std::sync::mpsc::Sender<CoordinatorCommand>,
        role: SurfaceRole,
    ) -> Result<Self, String> {
        let current_state = feed_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let initial = match role {
            SurfaceRole::DisplayCanvas => NativeDocumentProjection::Display {
                canvas: canvas.presentation(),
                visible: scorepeek_overlay::canvas_visible(
                    canvas.show_on.as_deref(),
                    current_state.screen,
                ),
            },
            SurfaceRole::EditorStage => published_stages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .by_output
                .get(
                    shell
                        .output_name
                        .as_deref()
                        .unwrap_or(canvas.output.as_str()),
                )
                .cloned()
                .map(NativeDocumentProjection::Editor)
                .ok_or_else(|| {
                    format!(
                        "editor projection missing for output {}",
                        shell
                            .output_name
                            .as_deref()
                            .unwrap_or(canvas.output.as_str())
                    )
                })?,
        };
        let published = Rc::new(RefCell::new(None));
        let port = NativeEditorPort {
            coordinator: coordinator.clone(),
            source_output: shell.output_name.clone(),
            run_id: report.borrow().run_id.clone(),
            sequence: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let display_package = if role == SurfaceRole::DisplayCanvas {
            Some(skin_assets.load(canvas.skin.name())?)
        } else {
            None
        };
        let vdom = VirtualDom::new_with_props(
            native_overlay,
            NativeOverlayProps {
                initial,
                published: Rc::clone(&published),
                port,
            },
        );
        let (document_config, _) =
            document_config_inner_with_handle(Arc::clone(&skin_assets), Vec::new());
        let mut document = DioxusDocument::new(vdom, document_config);
        document.initial_build();
        let projection = published
            .borrow()
            .as_ref()
            .copied()
            .expect("native overlay publishes its projection during initial build");
        let (display_skin, display_next_render) = if let Some(package) = display_package {
            let (skin, next) = create_native_display_skin(
                &mut document,
                &canvas,
                &package,
                &report,
                shell.output_name.as_deref(),
                &current_state,
            )?;
            (Some(skin), next)
        } else {
            (None, None)
        };
        let visible = match &*projection.borrow() {
            NativeDocumentProjection::Display { visible, .. } => *visible,
            NativeDocumentProjection::Editor(_) => true,
        };
        let interactive = matches!(&*projection.borrow(), NativeDocumentProjection::Editor(stage) if stage.interactive);
        let editor_presentations = match &*projection.borrow() {
            NativeDocumentProjection::Editor(projection) => projection.canvases.clone(),
            NativeDocumentProjection::Display { .. } => Vec::new(),
        };
        let mut editor_skin_previews =
            std::collections::BTreeMap::<String, EditorSkinPreview>::new();
        let mut editor_skin_updates = EditorSkinUpdates::default();
        let mut frame_work = FrameWorkProfile::default();
        for presentation in &editor_presentations {
            let Some(output) = shell.output_name.as_deref() else {
                return Err("editor skin preview requires an output owner".into());
            };
            if !skin_assets.acquire_editor_owner(&presentation.id, output) {
                editor_skin_updates.request();
                continue;
            }
            let preview = match create_editor_skin_preview(
                &mut document,
                presentation,
                &skin_assets,
                &report,
                shell.output_name.as_deref(),
                &current_state,
                &mut frame_work,
            ) {
                Ok(preview) => preview,
                Err(error) => {
                    skin_assets.release_editor_owner(&presentation.id, output);
                    for (id, mut mounted) in editor_skin_previews {
                        mounted.tree.unmount(&mut document.inner.borrow_mut());
                        skin_assets.release_editor_owner(&id, output);
                    }
                    return Err(error);
                }
            };
            editor_skin_previews.insert(presentation.id.clone(), preview);
        }
        let editor_next_render = editor_skin_previews
            .values()
            .filter_map(|preview| preview.next_render)
            .min();
        let editor_preview_count = u64::try_from(editor_skin_previews.len()).unwrap_or(u64::MAX);
        shell.set_input_enabled(surface_input_enabled(
            role == SurfaceRole::EditorStage,
            interactive,
            visible,
        ));
        shell.set_keyboard_enabled(role == SurfaceRole::EditorStage && interactive);
        let surface_logical = [canvas.width, canvas.height];
        let surface_output = shell.output_name.clone();
        Ok(Self {
            renderer,
            shell,
            document,
            pointer: PointerInput::default(),
            projection,
            published_stages,
            waker,
            started: Instant::now(),
            surface_state: NativeDisplaySurfaceState::AwaitingConfigure,
            paint_count: 0,
            render_calls: 0,
            full_layout_pending: false,
            editor_skin_updates,
            report,
            feed_state,
            current_state,
            feed_stop,
            external_stop,
            surface_canvas: canvas.clone(),
            surface_output,
            canvas,
            role,
            coordinator,
            output_descriptions: outputs,
            surface_logical,
            display_skin,
            editor_skin_previews,
            next_skin_render: if role == SurfaceRole::EditorStage {
                editor_next_render
            } else {
                display_next_render
            },
            skin_assets,
            skin_runtime_create_count: if role == SurfaceRole::DisplayCanvas {
                1
            } else {
                editor_preview_count
            },
            frame_work,
            pending_frame_start: None,
            startup_started,
            text_composition: TextComposition::Idle,
        })
    }

    fn editing(&self) -> bool {
        matches!(
            &*self.projection.borrow(),
            NativeDocumentProjection::Editor(_)
        )
    }

    fn unmount_skin_resources(&mut self) {
        for (id, preview) in &mut self.editor_skin_previews {
            preview.tree.unmount(&mut self.document.inner.borrow_mut());
            if let Some(output) = self.surface_output.as_deref() {
                self.skin_assets.release_editor_owner(id, output);
            }
        }
        self.editor_skin_previews.clear();
        if let Some(mut display) = self.display_skin.take() {
            display.tree.unmount(&mut self.document.inner.borrow_mut());
        }
        self.next_skin_render = None;
        crate::diagnostics::emit(
            "native_skin_resources_unmounted",
            &serde_json::json!({
                "run_id": self.report.borrow().run_id,
                "canvas_id": self.surface_canvas.id,
                "output": self.surface_output,
            }),
        );
    }

    fn visible(&self) -> bool {
        match &*self.projection.borrow() {
            NativeDocumentProjection::Display { visible, .. } => *visible,
            NativeDocumentProjection::Editor(_) => true,
        }
    }

    fn sync_projection(&mut self) -> bool {
        if self.role == SurfaceRole::DisplayCanvas {
            return false;
        }
        let Some(output) = self.surface_output.as_ref() else {
            return false;
        };
        let rebuild_started = Instant::now();
        let candidate = {
            let stages = self
                .published_stages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(candidate) = stages.by_output.get(output) else {
                return false;
            };
            let current = self.projection.borrow();
            let NativeDocumentProjection::Editor(current) = &*current else {
                return false;
            };
            if !accepts_stage_projection(current, candidate) || current == candidate {
                return false;
            }
            candidate.clone()
        };
        self.frame_work
            .record("projection_rebuild", rebuild_started.elapsed());
        let previous = self.canvas.presentation();
        let previews_changed = editor_skin_previews_changed(&self.editor_skin_previews, &candidate);
        if !accept_stage_projection_replica(
            self.projection,
            &mut self.document,
            &mut self.editor_skin_previews,
            &self.skin_assets,
            output,
            &candidate,
        ) {
            return false;
        }
        if let Some(canvas) = candidate.selected_canvas.as_ref() {
            self.canvas = crate::config::empty_canvas(
                canvas.id.clone(),
                crate::bridge::data::Backend::Wayland,
                canvas.skin,
            );
            self.canvas.apply_presentation(canvas);
        }
        if previews_changed
            || candidate
                .selected_canvas
                .as_ref()
                .is_some_and(|canvas| editor_skin_presentation_changed(&previous, canvas))
        {
            self.editor_skin_updates.request();
        }
        let interactive = candidate.interactive;
        self.shell.set_input_enabled(interactive);
        self.shell.set_keyboard_enabled(interactive);
        crate::diagnostics::emit(
            "native_editor_projection_received",
            &serde_json::json!({
                "run_id": self.report.borrow().run_id,
                "output": self.surface_output,
                "session_id": match &*self.projection.borrow() { NativeDocumentProjection::Editor(stage) => Some(stage.session_id), NativeDocumentProjection::Display { .. } => None },
                "revision": match &*self.projection.borrow() { NativeDocumentProjection::Editor(stage) => Some(stage.revision), NativeDocumentProjection::Display { .. } => None },
                "canvases": candidate.canvases.iter().map(|canvas| serde_json::json!({
                    "id": canvas.id,
                    "output": canvas.output,
                    "widget_count": canvas.widgets.len(),
                })).collect::<Vec<_>>(),
            }),
        );
        true
    }

    #[allow(clippy::too_many_lines)]
    fn run(&mut self) -> Result<(), String> {
        let _ = self.poll_dioxus();
        while !self.feed_stop.load(std::sync::atomic::Ordering::Acquire)
            && !self
                .external_stop
                .load(std::sync::atomic::Ordering::Acquire)
        {
            let frame_start = self
                .pending_frame_start
                .clone()
                .unwrap_or_else(|| self.frame_work.snapshot());
            self.sync_projection();
            let dispatch_surface_before = self.surface_state;
            let dispatch_renderer_active_before = self.renderer.is_active();
            let events = match self.shell.dispatch(Duration::from_millis(500)) {
                Ok(events) => events,
                Err(_)
                    if self
                        .external_stop
                        .load(std::sync::atomic::Ordering::Acquire) =>
                {
                    return Ok(());
                }
                Err(error) if error == "output_removed" => return Ok(()),
                Err(error) => {
                    if !self.editing() {
                        crate::diagnostics::emit(
                            "native_surface_transition",
                            &serde_json::json!({
                                "run_id": self.report.borrow().run_id,
                                "canvas_id": self.surface_canvas.id,
                                "output": self.surface_output,
                                "visible": self.visible(),
                                "visibility_changed": false,
                                "configured": false,
                                "frame": false,
                                "renderer_active_before": dispatch_renderer_active_before,
                                "renderer_active_after": self.renderer.is_active(),
                                "surface_before": dispatch_surface_before.diagnostic_name(),
                                "surface_after": self.surface_state.diagnostic_name(),
                                "painted": false,
                                "unmapped": false,
                                "status": "error",
                                "boundary": "dispatch",
                                "error_type": canvas_worker_error_type(&error),
                            }),
                        );
                    }
                    return Err(error);
                }
            };
            let mut frame = false;
            let mut configured = false;
            let mut renderer_active_before_configure = None;
            if self.output_descriptions != self.shell.output_descriptions {
                self.output_descriptions
                    .clone_from(&self.shell.output_descriptions);
                let outputs = editor_outputs_from_descriptions(&self.output_descriptions);
                let _ = self.coordinator.send(CoordinatorCommand::EditorInput {
                    input: EditorInput::SetOutputs(outputs),
                    correlation: None,
                });
            }
            for event in events {
                let configuring = matches!(&event, Event::Configure { .. });
                let renderer_active_before_event = self.renderer.is_active();
                let surface_before_event = self.surface_state;
                let outcome = match dispatch_native_event(self, event) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        if configuring && !self.editing() {
                            crate::diagnostics::emit(
                                "native_surface_transition",
                                &serde_json::json!({
                                    "run_id": self.report.borrow().run_id,
                                    "canvas_id": self.surface_canvas.id,
                                    "output": self.surface_output,
                                    "visible": self.visible(),
                                    "visibility_changed": false,
                                    "configured": true,
                                    "frame": false,
                                    "renderer_active_before": renderer_active_before_event,
                                    "renderer_active_after": self.renderer.is_active(),
                                    "surface_before": surface_before_event.diagnostic_name(),
                                    "surface_after": self.surface_state.diagnostic_name(),
                                    "painted": false,
                                    "unmapped": false,
                                    "status": "error",
                                    "boundary": "configure",
                                    "error_type": canvas_worker_error_type(&error),
                                }),
                            );
                        }
                        return Err(error);
                    }
                };
                if outcome.configured {
                    renderer_active_before_configure = Some(renderer_active_before_event);
                }
                if outcome.closed {
                    return Ok(());
                }
                configured |= outcome.configured;
                frame |= outcome.frame;
            }
            let latest = self
                .feed_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let visibility_changed = update_display_visibility(self.projection, latest.screen);
            if let Some(visible) = visibility_changed {
                self.shell.set_input_enabled(visible);
                if visible
                    && !self.editing()
                    && self.surface_state == NativeDisplaySurfaceState::Unmapped
                {
                    self.shell.begin_remap();
                    self.surface_state = NativeDisplaySurfaceState::AwaitingConfigure;
                    configured = false;
                }
                if !visible && !self.editing() {
                    self.next_skin_render = None;
                }
            }
            let visibility_changed = visibility_changed.is_some();
            let visible = self.visible();
            if self.current_state != latest {
                self.current_state = latest.clone();
                if visible {
                    if self.editing() {
                        self.editor_skin_updates.request();
                    } else {
                        self.render_skin(&latest)?;
                    }
                }
            } else if visibility_changed && visible && !self.editing() {
                self.render_skin(&latest)?;
            }
            if visible
                && self
                    .next_skin_render
                    .is_some_and(|deadline| Instant::now() >= deadline)
            {
                if self.editing() {
                    self.editor_skin_updates.request();
                } else {
                    self.render_skin(&latest)?;
                }
            }
            let now = self.started.elapsed();
            if self.editing() {
                let first_paint = self.paint_count == 0;
                let paint_started = Instant::now();
                let editor_state =
                    if self.current_state.system == scorepeek_overlay::LampState::Inactive {
                        scorepeek_overlay::editor_sample_state()
                    } else {
                        self.current_state.clone()
                    };
                let mut presenter = WindowPresenter {
                    shell: &mut self.shell,
                    renderer: &mut self.renderer,
                };
                let result = run_native_editor_stage_turn(
                    &mut self.document,
                    self.projection,
                    &mut self.editor_skin_previews,
                    &self.skin_assets,
                    &self.report,
                    self.surface_output.as_deref().unwrap_or_default(),
                    &editor_state,
                    &mut self.editor_skin_updates,
                    &mut self.skin_runtime_create_count,
                    &mut self.next_skin_render,
                    &mut self.full_layout_pending,
                    &mut self.frame_work,
                    &self.waker,
                    &frame_start,
                    NativeEditorStageTurnInput {
                        frame: if frame {
                            NativeFrameBoundary::Frame
                        } else {
                            NativeFrameBoundary::Deferred
                        },
                        surface: if configured {
                            NativeSurfaceReadiness::Configured
                        } else {
                            NativeSurfaceReadiness::Pending
                        },
                        seconds: now.as_secs_f64(),
                    },
                    &mut presenter,
                )?;
                if !result.text_input_active {
                    self.set_text_composing(false);
                }
                if result.dioxus_changed {
                    let projection = self.projection.borrow();
                    let (session_id, revision) = match &*projection {
                        NativeDocumentProjection::Editor(stage) => {
                            (Some(stage.session_id), Some(stage.revision))
                        }
                        NativeDocumentProjection::Display { .. } => (None, None),
                    };
                    crate::diagnostics::emit(
                        "native_editor_dioxus_rebuilt",
                        &serde_json::json!({"run_id":self.report.borrow().run_id,"output":self.surface_output,"session_id":session_id,"revision":revision}),
                    );
                }
                if result.painted {
                    self.paint_count = self.paint_count.saturating_add(1);
                    self.render_calls = self.render_calls.saturating_add(1);
                    if first_paint {
                        self.report
                            .borrow_mut()
                            .operations
                            .push("dioxus_blitz_initial_paint");
                        crate::diagnostics::emit(
                            "native_startup_timing",
                            &serde_json::json!({"run_id":self.report.borrow().run_id,"canvas_id":self.surface_canvas.id,"phase":"first_paint","paint_us":duration_us(paint_started.elapsed()),"elapsed_us":duration_us(self.startup_started.elapsed()),"status":"success"}),
                        );
                    }
                    let projection = self.projection.borrow();
                    let (session_id, revision) = match &*projection {
                        NativeDocumentProjection::Editor(stage) => {
                            (stage.session_id, stage.revision)
                        }
                        NativeDocumentProjection::Display { .. } => (0, 0),
                    };
                    crate::diagnostics::emit(
                        "native_editor_painted",
                        &serde_json::json!({"run_id":self.report.borrow().run_id,"output":self.surface_output,"session_id":session_id,"revision":revision,"paint_us":duration_us(paint_started.elapsed())}),
                    );
                }
            } else {
                let first_paint = self.paint_count == 0;
                let was_mapped = self.surface_state.is_mapped();
                let surface_before = self.surface_state;
                let renderer_active =
                    renderer_active_before_configure.unwrap_or_else(|| self.renderer.is_active());
                let paint_started = Instant::now();
                let result = {
                    let mut presenter = WindowPresenter {
                        shell: &mut self.shell,
                        renderer: &mut self.renderer,
                    };
                    run_native_display_turn(
                        &mut self.document,
                        &self.skin_assets,
                        &mut self.full_layout_pending,
                        &mut self.surface_state,
                        &mut self.frame_work,
                        &self.waker,
                        &frame_start,
                        NativeDisplayTurnInput {
                            frame: if frame {
                                NativeFrameBoundary::Frame
                            } else {
                                NativeFrameBoundary::Deferred
                            },
                            surface: if configured {
                                NativeSurfaceReadiness::Configured
                            } else {
                                NativeSurfaceReadiness::Pending
                            },
                            visible,
                            live_widgets: u64::try_from(self.canvas.widgets.len())
                                .unwrap_or(u64::MAX),
                            seconds: now.as_secs_f64(),
                        },
                        &mut presenter,
                    )
                };
                let result = match result {
                    Ok(result) => result,
                    Err(error) => {
                        crate::diagnostics::emit(
                            "native_surface_transition",
                            &serde_json::json!({
                                "run_id": self.report.borrow().run_id,
                                "canvas_id": self.surface_canvas.id,
                                "output": self.surface_output,
                                "visible": visible,
                                "visibility_changed": visibility_changed,
                                "configured": configured,
                                "frame": frame,
                                "renderer_active_before": renderer_active,
                                "renderer_active_after": self.renderer.is_active(),
                                "surface_before": surface_before.diagnostic_name(),
                                "surface_after": self.surface_state.diagnostic_name(),
                                "painted": false,
                                "unmapped": false,
                                "status": "error",
                                "boundary": "display_turn",
                                "error_type": canvas_worker_error_type(&error),
                            }),
                        );
                        return Err(error);
                    }
                };
                let renderer_active_after = self.renderer.is_active();
                if visibility_changed
                    || configured
                    || result.unmapped
                    || (result.painted && !was_mapped)
                {
                    crate::diagnostics::emit(
                        "native_surface_transition",
                        &serde_json::json!({
                            "run_id": self.report.borrow().run_id,
                            "canvas_id": self.surface_canvas.id,
                            "output": self.surface_output,
                            "visible": visible,
                            "visibility_changed": visibility_changed,
                            "configured": configured,
                            "frame": frame,
                            "renderer_active_before": renderer_active,
                            "renderer_active_after": renderer_active_after,
                            "surface_before": surface_before.diagnostic_name(),
                            "surface_after": self.surface_state.diagnostic_name(),
                            "painted": result.painted,
                            "unmapped": result.unmapped,
                            "status": "success",
                            "boundary": "display_turn",
                        }),
                    );
                }
                if result.dioxus_changed {
                    self.set_text_composing(false);
                }
                if result.unmapped {
                    crate::diagnostics::emit(
                        "native_surface_visibility",
                        &serde_json::json!({"run_id":self.report.borrow().run_id,"canvas_id":self.surface_canvas.id,"output":self.surface_output,"active":false,"status":"success"}),
                    );
                }
                if result.painted {
                    self.paint_count = self.paint_count.saturating_add(1);
                    self.render_calls = self.render_calls.saturating_add(1);
                    if !was_mapped {
                        crate::diagnostics::emit(
                            "native_surface_visibility",
                            &serde_json::json!({"run_id":self.report.borrow().run_id,"canvas_id":self.surface_canvas.id,"output":self.surface_output,"active":true,"status":"success"}),
                        );
                    }
                    if first_paint {
                        self.report
                            .borrow_mut()
                            .operations
                            .push("dioxus_blitz_initial_paint");
                        crate::diagnostics::emit(
                            "native_startup_timing",
                            &serde_json::json!({"run_id":self.report.borrow().run_id,"canvas_id":self.surface_canvas.id,"phase":"first_paint","paint_us":duration_us(paint_started.elapsed()),"elapsed_us":duration_us(self.startup_started.elapsed()),"status":"success"}),
                        );
                    }
                }
            }
            if frame {
                self.pending_frame_start = None;
            } else if visible && self.pending_frame_start.is_none() {
                self.pending_frame_start = Some(frame_start);
            }
        }
        Ok(())
    }

    fn pointer_button(&mut self, button: u32, pressed: bool, x: f64, y: f64) {
        if button != 0x110 && button != 0x111 {
            return;
        }
        self.pointer
            .dispatch(&mut self.document, [x, y], button, Some(pressed));
    }

    fn poll_dioxus(&mut self) -> bool {
        let changed = poll_native_document_for_frame(
            &mut self.document,
            &self.waker,
            &mut self.full_layout_pending,
            &mut self.frame_work,
        );
        if changed && self.editing() {
            let (session_id, revision) = match &*self.projection.borrow() {
                NativeDocumentProjection::Editor(stage) => {
                    (Some(stage.session_id), Some(stage.revision))
                }
                NativeDocumentProjection::Display { .. } => (None, None),
            };
            crate::diagnostics::emit(
                "native_editor_dioxus_rebuilt",
                &serde_json::json!({"run_id":self.report.borrow().run_id,"output":self.surface_output,"session_id":session_id,"revision":revision}),
            );
        }
        changed
    }

    fn render_skin(&mut self, state: &OverlayState) -> Result<(), String> {
        let display = self
            .display_skin
            .as_mut()
            .ok_or("display skin runtime missing outside display role")?;
        render_native_display_skin(
            &mut self.document,
            &self.canvas,
            display,
            state,
            &mut self.next_skin_render,
        )
    }

    fn configure(
        &mut self,
        logical: [u32; 2],
        physical: [u32; 2],
        scale_120: u32,
    ) -> Result<bool, String> {
        self.surface_logical = logical;
        let [width, height] = physical;
        let geometry_changed = {
            let mut report = self.report.borrow_mut();
            let changed = report.logical_size != Some(logical)
                || report.physical_size != Some(physical)
                || report.scale_120 != scale_120;
            report.logical_size = Some(logical);
            report.physical_size = Some(physical);
            report.scale_120 = scale_120;
            changed
        };
        self.document.inner.borrow_mut().set_viewport(Viewport::new(
            width,
            height,
            f32::from(u16::try_from(scale_120).map_err(|error| error.to_string())?) / 120.0,
            ColorScheme::Dark,
        ));
        if self.renderer.is_active() {
            if geometry_changed {
                self.renderer.set_size(width, height);
            }
        } else {
            self.renderer
                .resume(self.shell.handles(), width, height, || {});
            if !self.renderer.complete_resume() {
                return Err("gpu_adapter".to_owned());
            }
            let info = self
                .renderer
                .current_device_handle()
                .map(|device| device.adapter.get_info())
                .ok_or_else(|| "gpu_adapter".to_owned())?;
            let mut report = self.report.borrow_mut();
            report.gpu_backend = Some(format!("{:?}", info.backend));
            report.gpu_adapter = Some(info.name);
            if report.gpu_backend.as_deref() != Some("Vulkan") {
                return Err("GPU backend is not Vulkan".into());
            }
            report.operations.push("vulkan_surface_configured");
            report.operations.push("system_fonts_enabled");
        }
        crate::diagnostics::emit("surface_configured", &*self.report.borrow());
        Ok(geometry_changed)
    }
}

const fn surface_input_enabled(editing: bool, interactive: bool, visible: bool) -> bool {
    visible && (!editing || interactive)
}
fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn canvas_worker_error_type(error: &str) -> &'static str {
    if error.starts_with("unmap Wayland surface:") {
        "wayland_surface_unmap_failed"
    } else {
        "canvas_worker_failed"
    }
}

fn native_shutdown_result(
    run: Result<(), String>,
    unmap: Result<(), String>,
) -> (Result<(), String>, Option<String>) {
    match unmap {
        Ok(()) => (run, None),
        Err(error) => (Err(format!("unmap Wayland surface: {error}")), run.err()),
    }
}

fn shutdown_native_surface<T, E>(
    target: &mut T,
    unmount: impl FnOnce(&mut T),
    suspend: impl FnOnce(&mut T),
    unmap: impl FnOnce(&mut T) -> Result<(), E>,
) -> Result<(), E> {
    unmount(target);
    suspend(target);
    unmap(target)
}

#[derive(Serialize)]
struct RunReport {
    run_id: String,
    build_revision: &'static str,
    canvas_id: Option<String>,
    output_name: Option<String>,
    logical_size: Option<[u32; 2]>,
    physical_size: Option<[u32; 2]>,
    scale_120: u32,
    gpu_backend: Option<String>,
    gpu_adapter: Option<String>,
    paint_count: u32,
    render_calls: u32,
    editor_skin_update_requests: u64,
    editor_skin_render_count: u64,
    skin_package_open_count: u64,
    skin_package_open_ns: u64,
    skin_package_clone_count: u64,
    skin_package_clone_ns: u64,
    resource_lookup_count: u64,
    resource_lookup_ns: u64,
    skin_runtime_create_count: u64,
    frame_work: FrameWorkProfile,
    elapsed_ms: u64,
    effective_paint_hz: Option<f64>,
    operations: Operations,
    status: &'static str,
    failure_type: Option<&'static str>,
    failure: Option<String>,
    secondary_failure_type: Option<&'static str>,
    secondary_failure: Option<String>,
}

impl RunReport {
    fn new() -> Self {
        static RUN_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        Self {
            run_id: format!(
                "{timestamp}-{}-{}",
                std::process::id(),
                RUN_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ),
            build_revision: option_env!("OVERLAY_BUILD_REVISION").unwrap_or("working-tree"),
            canvas_id: None,
            output_name: None,
            logical_size: None,
            physical_size: None,
            scale_120: 120,
            gpu_backend: None,
            gpu_adapter: None,
            paint_count: 0,
            render_calls: 0,
            editor_skin_update_requests: 0,
            editor_skin_render_count: 0,
            skin_package_open_count: 0,
            skin_package_open_ns: 0,
            skin_package_clone_count: 0,
            skin_package_clone_ns: 0,
            resource_lookup_count: 0,
            resource_lookup_ns: 0,
            skin_runtime_create_count: 0,
            frame_work: FrameWorkProfile::default(),
            elapsed_ms: 0,
            effective_paint_hz: None,
            operations: Operations::default(),
            status: "running",
            failure_type: None,
            failure: None,
            secondary_failure_type: None,
            secondary_failure: None,
        }
    }
}

#[derive(Default, Serialize)]
struct Operations(Vec<&'static str>);
impl Operations {
    fn push(&mut self, name: &'static str) {
        if self.0.len() < 128 {
            self.0.push(name);
        }
    }
}

/// Creates a document using only resources declared by its skin package.
#[must_use]
pub fn document_config() -> DocumentConfig {
    document_config_inner(None)
}

fn document_config_inner(package: Option<crate::skin::Package>) -> DocumentConfig {
    let store = crate::skin::StoreRoot::discover();
    let (cache, fonts) = match package {
        Some(package) => {
            let fonts = package.font_resources().map(<[u8]>::to_vec).collect();
            (
                Arc::new(SkinAssetCache::with_package(store, Arc::new(package))),
                fonts,
            )
        }
        None => (Arc::new(SkinAssetCache::new(store)), Vec::new()),
    };
    document_config_inner_with_handle(cache, fonts).0
}

#[cfg(not(test))]
fn document_config_with_skin_handle(
    package: Arc<crate::skin::Package>,
) -> (DocumentConfig, Arc<SkinAssetCache>) {
    let fonts = package.font_resources().map(<[u8]>::to_vec).collect();
    document_config_inner_with_handle(
        Arc::new(SkinAssetCache::with_package(
            crate::skin::StoreRoot::discover(),
            package,
        )),
        fonts,
    )
}

fn document_config_inner_with_handle(
    cache: Arc<SkinAssetCache>,
    fonts: Vec<Vec<u8>>,
) -> (DocumentConfig, Arc<SkinAssetCache>) {
    let mut font_ctx = blitz_dom::FontContext::default();
    for bytes in fonts {
        font_ctx
            .collection
            .register_fonts(peniko::Blob::new(Arc::new(bytes)), None);
    }
    let config = DocumentConfig {
        font_ctx: Some(font_ctx),
        ua_stylesheets: Some(vec![scorepeek_overlay::HOST_CSS.into()]),
        base_url: Some("http://scorepeek.invalid/".into()),
        net_provider: Some(Arc::new(EmbeddedSkinAssets {
            cache: Arc::clone(&cache),
        })),
        ..DocumentConfig::default()
    };
    (config, cache)
}

mod visual_debug;
pub use visual_debug::{
    VisualDebugAction, VisualDebugButton, VisualDebugScenario, run_visual_debug,
};
#[cfg(test)]
use visual_debug::{VisualDebugSession, prepare_visual_output};

#[cfg(test)]
#[path = "dioxus_dom/tests.rs"]
mod skin_tests;
