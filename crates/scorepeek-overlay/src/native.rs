use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
    task::{Context as TaskContext, Wake, Waker},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::runtime::{Config, Feed};
use anyrender::{CompositeAlphaMode, WindowRenderer};
use anyrender_vello::{VelloRendererOptions, VelloWindowRenderer};
use blitz_dom::{Document, DocumentConfig};
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};
use dioxus::prelude::*;
use dioxus_core::{VirtualDom, schedule_update};
use dioxus_native_dom::DioxusDocument;
use scorepeek_overlay_handles::{Event, Shell};
use scorepeek_overlay_ui::{Appearance, OXANIUM, OverlayState, WidgetLayout, overlay_canvas};
use serde::Serialize;
use smithay_client_toolkit::reexports::calloop::ping::{Ping, make_ping};

type NativeUpdate = Rc<RefCell<Option<Arc<dyn Fn() + Send + Sync>>>>;

#[derive(Clone)]
struct NativeOverlayProps {
    appearance: Rc<Cell<Appearance>>,
    widgets: Rc<RefCell<Vec<WidgetLayout>>>,
    editing: Rc<Cell<bool>>,
    selected: Rc<RefCell<Option<String>>>,
    managed: Rc<RefCell<Vec<crate::control::CanvasSummary>>>,
    outputs: Rc<RefCell<Vec<String>>>,
    state: Rc<RefCell<OverlayState>>,
    update: NativeUpdate,
}

fn native_overlay(
    NativeOverlayProps {
        state,
        update,
        appearance,
        widgets,
        editing,
        selected,
        managed,
        outputs,
    }: NativeOverlayProps,
) -> Element {
    *update.borrow_mut() = Some(schedule_update());
    rsx! {
        {overlay_canvas(
            &state.borrow(),
            appearance.get(),
            &widgets.borrow(),
            editing.get(),
            selected.borrow().as_deref(),
        )}
        if editing.get() { div { class:"native-canvas-manager", strong { "WAYLAND CANVASES" } for canvas in managed.borrow().iter() { span { "{canvas.id}" } } b { "ADD EMPTY" } b { "ENABLE / DISABLE" } b { "DELETE CURRENT" } } div { class:"native-output-manager", strong { "OUTPUT" } for output in outputs.borrow().iter() { b { "{output}" } } } }
    }
}

struct CalloopWaker(Ping);

fn widget_layout(widget: &crate::config::Widget) -> WidgetLayout {
    WidgetLayout {
        id: widget.id.clone(),
        kind: match widget.kind {
            crate::config::WidgetKind::Status => scorepeek_overlay_ui::WidgetKind::Status,
            crate::config::WidgetKind::Selection => scorepeek_overlay_ui::WidgetKind::Selection,
            crate::config::WidgetKind::Score => scorepeek_overlay_ui::WidgetKind::Score,
            crate::config::WidgetKind::HistoryList => scorepeek_overlay_ui::WidgetKind::HistoryList,
            crate::config::WidgetKind::HistoryGraph => {
                scorepeek_overlay_ui::WidgetKind::HistoryGraph
            }
        },
        x: widget.x,
        y: widget.y,
        width: widget.width,
        height: widget.height,
        z: widget.z,
        settings: scorepeek_overlay_ui::WidgetSettings {
            history_count: widget.settings.history_count,
            graph_months: widget.settings.graph_months,
        },
    }
}
#[allow(clippy::cast_possible_truncation)]
fn snap_i32(value: f64) -> i32 {
    ((value / 4.0).round() * 4.0).clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn snap_u32(value: f64) -> u32 {
    ((value.max(32.0) / 4.0).round() * 4.0).min(f64::from(u32::MAX)) as u32
}

impl Wake for CalloopWaker {
    fn wake(self: Arc<Self>) {
        self.0.ping();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.ping();
    }
}

fn watch_parent(
    mut input: impl std::io::Read + Send + 'static,
    stop: Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("overlay-parent".into())
        .spawn(move || {
            let mut byte = [0];
            let _ = input.read(&mut byte);
            stop.store(true, std::sync::atomic::Ordering::Release);
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn recent_same_failure(
    failure: &(Option<String>, u64, Instant),
    canvas: &crate::config::Canvas,
) -> bool {
    failure.0 == canvas.output
        && failure.1 == canvas.revision
        && failure.2.elapsed() < Duration::from_secs(5)
}

/// Runs until the parent's lifetime lease closes.
/// # Errors
/// Returns Wayland, GPU or event-loop failures.
#[allow(clippy::needless_pass_by_value)]
pub fn run(config: Config, input: impl std::io::Read + Send + 'static) -> Result<(), String> {
    use std::collections::BTreeMap;
    struct Worker {
        output: Option<String>,
        stop: Arc<std::sync::atomic::AtomicBool>,
        join: std::thread::JoinHandle<Result<(), String>>,
    }
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    watch_parent(input, Arc::clone(&stop))?;
    let mut desired = config.canvases.clone();
    let mut workers = BTreeMap::<String, Worker>::new();
    let mut failed = BTreeMap::<String, (Option<String>, u64, Instant)>::new();
    while !stop.load(std::sync::atomic::Ordering::Acquire) {
        if let Ok((loaded, _)) = crate::config::load_or_create(&config.config_path) {
            desired = loaded
                .canvases
                .into_iter()
                .filter(|canvas| {
                    canvas.backend == crate::runtime::Backend::Wayland && canvas.enabled
                })
                .collect();
        }
        let desired_ids: std::collections::BTreeSet<_> =
            desired.iter().map(|canvas| canvas.id.clone()).collect();
        let remove: Vec<_> = workers
            .iter()
            .filter(|(id, worker)| {
                !desired_ids.contains(*id)
                    || desired
                        .iter()
                        .find(|canvas| canvas.id == id.as_str())
                        .is_some_and(|canvas| canvas.output != worker.output)
                    || worker.join.is_finished()
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in remove {
            if let Some(worker) = workers.remove(&id) {
                worker
                    .stop
                    .store(true, std::sync::atomic::Ordering::Release);
                match worker.join.join() {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        if let Some(canvas) = desired.iter().find(|canvas| canvas.id == id) {
                            failed.insert(
                                id.clone(),
                                (canvas.output.clone(), canvas.revision, Instant::now()),
                            );
                        }
                        crate::diagnostics::emit(
                            "native_canvas_failed",
                            &serde_json::json!({"canvas_id":id,"error":error}),
                        );
                    }
                    Err(_) => {
                        crate::diagnostics::emit(
                            "native_canvas_failed",
                            &serde_json::json!({"canvas_id":id,"error":"panicked"}),
                        );
                    }
                }
            }
        }
        for canvas in &desired {
            if workers.contains_key(&canvas.id)
                || failed
                    .get(&canvas.id)
                    .is_some_and(|failure| recent_same_failure(failure, canvas))
            {
                continue;
            }
            let mut canvas_config = config.clone();
            canvas_config.canvases = vec![canvas.clone()];
            let canvas_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stopping = Arc::clone(&canvas_stop);
            let join = std::thread::Builder::new()
                .name(format!("overlay-wayland-{}", canvas.id))
                .spawn(move || run_canvas(canvas_config, stopping))
                .map_err(|error| error.to_string())?;
            workers.insert(
                canvas.id.clone(),
                Worker {
                    output: canvas.output.clone(),
                    stop: canvas_stop,
                    join,
                },
            );
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    for (_, worker) in workers {
        worker
            .stop
            .store(true, std::sync::atomic::Ordering::Release);
        let _ = worker.join.join();
    }
    Ok(())
}

fn run_canvas(
    config: Config,
    external_stop: Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), String> {
    let report = Rc::new(RefCell::new(RunReport::new()));
    let ping = make_ping().map_err(|e| e.to_string())?;
    let wake = ping.0.clone();
    let mut canvas = config
        .canvases
        .first()
        .ok_or("Wayland overlay has no enabled canvas")?
        .clone();
    let shell = Shell::open(
        canvas.output.as_deref(),
        canvas.width,
        canvas.height,
        canvas.x,
        canvas.y,
        canvas.initial_placement == Some(crate::config::InitialPlacement::UpperRight),
        ping.1,
    )?;
    canvas.x = shell.position[0];
    canvas.y = shell.position[1];
    let outputs = shell.available_outputs.clone();
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
    let appearance = Appearance { skin: canvas.skin };
    let widgets = canvas.widgets.iter().map(widget_layout).collect();
    let control_socket = config.control_socket.clone();
    let feed = Feed::start(config, Arc::new(move || wake.ping())).map_err(|e| e.to_string())?;
    let mut app = App::new(
        appearance,
        widgets,
        shell,
        Waker::from(Arc::new(CalloopWaker(ping.0))),
        feed,
        external_stop,
        canvas,
        control_socket,
        outputs,
        Rc::clone(&report),
    );
    let result = app.run();
    app.renderer.suspend();
    let mut report = report.borrow_mut();
    report.paint_count = app.paint_count;
    report.render_calls = app.render_calls;
    report.status = if result.is_ok() { "complete" } else { "failed" };
    report.failure = result.as_ref().err().cloned();
    report.operations.push("shutdown");
    crate::diagnostics::emit("native_summary", &*report);
    result
}
struct App {
    // Renderer is dropped before the shell; its own Arc handle also retains ownership.
    renderer: VelloWindowRenderer,
    shell: Shell,
    document: DioxusDocument,
    shared_state: Rc<RefCell<OverlayState>>,
    native_update: NativeUpdate,
    waker: Waker,
    started: Instant,
    animating: bool,
    paint_count: u32,
    render_calls: u32,
    report: Rc<RefCell<RunReport>>,
    feed: Feed,
    external_stop: Arc<std::sync::atomic::AtomicBool>,
    canvas: crate::config::Canvas,
    control_socket: std::path::PathBuf,
    appearance: Rc<Cell<Appearance>>,
    editor_id: String,
    editing: Rc<Cell<bool>>,
    readonly: bool,
    selected: Rc<RefCell<Option<String>>>,
    shared_widgets: Rc<RefCell<Vec<WidgetLayout>>>,
    interaction: Option<NativeInteraction>,
    next_keepalive: Instant,
    managed: Rc<RefCell<Vec<crate::control::CanvasSummary>>>,
    backend_revision: u64,
    outputs: Rc<RefCell<Vec<String>>>,
}

enum NativeInteraction {
    CanvasMove {
        start: [f64; 2],
        origin: [i32; 2],
    },
    CanvasResize {
        start: [f64; 2],
        origin: [u32; 2],
    },
    Widget {
        id: String,
        start: [f64; 2],
        original: WidgetLayout,
        resize: bool,
    },
}
impl App {
    #[allow(clippy::too_many_arguments)]
    fn new(
        appearance: Appearance,
        widgets: Vec<WidgetLayout>,
        shell: Shell,
        waker: Waker,
        feed: Feed,
        external_stop: Arc<std::sync::atomic::AtomicBool>,
        canvas: crate::config::Canvas,
        control_socket: std::path::PathBuf,
        outputs: Vec<String>,
        report: Rc<RefCell<RunReport>>,
    ) -> Self {
        let shared_state = Rc::new(RefCell::new(OverlayState::default()));
        let native_update = Rc::new(RefCell::new(None));
        let shared_widgets = Rc::new(RefCell::new(widgets));
        let editing = Rc::new(Cell::new(false));
        let selected = Rc::new(RefCell::new(None));
        let managed = Rc::new(RefCell::new(Vec::new()));
        let outputs = Rc::new(RefCell::new(outputs));
        let appearance = Rc::new(Cell::new(appearance));
        let vdom = VirtualDom::new_with_props(
            native_overlay,
            NativeOverlayProps {
                appearance: Rc::clone(&appearance),
                widgets: Rc::clone(&shared_widgets),
                editing: Rc::clone(&editing),
                selected: Rc::clone(&selected),
                managed: Rc::clone(&managed),
                outputs: Rc::clone(&outputs),
                state: Rc::clone(&shared_state),
                update: Rc::clone(&native_update),
            },
        );
        let mut document = DioxusDocument::new(vdom, document_config());
        document.initial_build();

        Self {
            renderer: VelloWindowRenderer::with_options(
                VelloRendererOptions::default()
                    .base_color(peniko::Color::TRANSPARENT)
                    .composite_alpha_mode(CompositeAlphaMode::Transparent),
            ),
            shell,
            document,
            shared_state,
            native_update,
            waker,
            started: Instant::now(),
            animating: false,
            paint_count: 0,
            render_calls: 0,
            report,
            feed,
            external_stop,
            canvas,
            control_socket,
            appearance,
            editor_id: format!("wayland-{}", std::process::id()),
            editing,
            readonly: true,
            selected,
            shared_widgets,
            interaction: None,
            next_keepalive: Instant::now(),
            managed,
            backend_revision: 0,
            outputs,
        }
    }
    fn run(&mut self) -> Result<(), String> {
        let _ = self.poll_dioxus();
        while !self.feed.stop.load(std::sync::atomic::Ordering::Acquire)
            && !self
                .external_stop
                .load(std::sync::atomic::Ordering::Acquire)
        {
            if self.editing.get() && Instant::now() >= self.next_keepalive {
                let _ = self.request(crate::control::Request::KeepAlive {
                    canvas_id: self.canvas.id.clone(),
                    editor_id: self.editor_id.clone(),
                });
                self.next_keepalive = Instant::now() + Duration::from_secs(5);
            }
            let events = self.shell.dispatch(Duration::from_millis(500))?;
            let mut wake = false;
            let mut frame = false;
            if *self.outputs.borrow() != self.shell.available_outputs {
                self.outputs
                    .borrow_mut()
                    .clone_from(&self.shell.available_outputs);
                if let Some(update) = self.native_update.borrow().as_ref() {
                    update();
                }
                wake = true;
            }
            for event in events {
                match event {
                    Event::Configure {
                        logical,
                        physical,
                        scale_120,
                    } => self.configure(logical, physical, scale_120)?,
                    Event::Wake => wake = true,
                    Event::PointerMotion { x, y } => {
                        self.pointer_motion(x, y);
                        wake = true;
                    }
                    Event::PointerButton {
                        button,
                        pressed,
                        x,
                        y,
                    } => {
                        self.pointer_button(button, pressed, x, y);
                        wake = true;
                    }
                    Event::Frame => frame = true,
                    Event::Closed => return Ok(()),
                }
            }
            let latest = self
                .feed
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if *self.shared_state.borrow() != latest {
                *self.shared_state.borrow_mut() = latest;
                if let Some(update) = self.native_update.borrow().as_ref() {
                    update();
                }
                wake = true;
            }
            let changed = wake && self.poll_dioxus();
            if self.renderer.is_active() && (changed || (frame && self.animating)) {
                self.paint()?;
                if changed {
                    crate::diagnostics::emit(
                        "native_state_paint",
                        &serde_json::json!({"paint_count":self.paint_count,"render_calls":self.render_calls}),
                    );
                }
            }
        }
        Ok(())
    }
    #[allow(clippy::needless_pass_by_value)]
    fn request(&mut self, request: crate::control::Request) -> Option<crate::control::Response> {
        let updates_readonly = control_updates_readonly(&request);
        let response = crate::control::request(&self.control_socket, &request).ok()?;
        if updates_readonly {
            self.readonly = response.readonly || !response.ok;
        }
        if let Some(presentation) = &response.canvas {
            self.canvas.revision = presentation.revision;
        }
        if let Some(revision) = response.backend_revision {
            self.backend_revision = revision;
            self.managed.borrow_mut().clone_from(&response.canvases);
        }
        Some(response)
    }
    fn acquire(&mut self) {
        let _ = self.request(crate::control::Request::Acquire {
            canvas_id: self.canvas.id.clone(),
            editor_id: self.editor_id.clone(),
        });
    }
    fn set_editing(&mut self, value: bool) {
        if value {
            self.acquire();
            let _ = self.request(crate::control::Request::ListCanvases {
                backend: crate::runtime::Backend::Wayland,
            });
            self.next_keepalive = Instant::now() + Duration::from_secs(5);
        } else {
            let _ = self.request(crate::control::Request::Release {
                canvas_id: self.canvas.id.clone(),
                editor_id: self.editor_id.clone(),
            });
        }
        self.editing.set(value);
        self.interaction = None;
        if !value {
            self.selected.borrow_mut().take();
        }
        if let Some(update) = self.native_update.borrow().as_ref() {
            update();
        }
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::too_many_lines
    )]
    fn pointer_button(&mut self, button: u32, pressed: bool, x: f64, y: f64) {
        if button == 0x111 && pressed {
            if !self.editing.get() {
                self.set_editing(true);
            }
            return;
        }
        if button != 0x110 {
            return;
        }
        if pressed {
            if !self.editing.get() {
                self.acquire();
                if self.readonly {
                    return;
                }
                self.interaction = Some(NativeInteraction::CanvasMove {
                    start: [x, y],
                    origin: [self.canvas.x, self.canvas.y],
                });
                return;
            }
            if x >= f64::from(self.canvas.width.saturating_sub(80)) && y < 52.0 {
                self.set_editing(false);
                return;
            }
            if self.readonly {
                return;
            }
            if x < 150.0 && y >= 250.0 {
                let list_rows =
                    f64::from(u32::try_from(self.managed.borrow().len()).unwrap_or(u32::MAX))
                        * 22.0;
                let actions_y = 278.0 + list_rows;
                if (actions_y..actions_y + 28.0).contains(&y) {
                    let suffix = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis();
                    let _ = self.request(crate::control::Request::AddCanvas {
                        backend: crate::runtime::Backend::Wayland,
                        expected_revision: self.backend_revision,
                        canvas_id: format!("wayland-{suffix}"),
                    });
                    return;
                }
                if (actions_y + 28.0..actions_y + 56.0).contains(&y) {
                    let enabled = self
                        .managed
                        .borrow()
                        .iter()
                        .find(|canvas| canvas.id == self.canvas.id)
                        .is_none_or(|canvas| canvas.enabled);
                    let _ = self.request(crate::control::Request::SetCanvasEnabled {
                        backend: crate::runtime::Backend::Wayland,
                        expected_revision: self.backend_revision,
                        canvas_id: self.canvas.id.clone(),
                        enabled: !enabled,
                    });
                    return;
                }
                if (actions_y + 56.0..actions_y + 84.0).contains(&y) {
                    let _ = self.request(crate::control::Request::DeleteCanvas {
                        backend: crate::runtime::Backend::Wayland,
                        expected_revision: self.backend_revision,
                        canvas_id: self.canvas.id.clone(),
                    });
                    return;
                }
            }
            if y < 52.0 {
                let skin = if (105.0..177.0).contains(&x) {
                    Some(scorepeek_overlay_ui::Skin::CyanSystem)
                } else if (177.0..249.0).contains(&x) {
                    Some(scorepeek_overlay_ui::Skin::ResultAurora)
                } else if (249.0..337.0).contains(&x) {
                    Some(scorepeek_overlay_ui::Skin::DjBlackbox)
                } else {
                    None
                };
                if let Some(skin) = skin {
                    self.appearance.set(Appearance { skin });
                    self.canvas.skin = skin;
                    self.persist_canvas();
                    return;
                }
            }
            if x < 150.0 && (58.0..198.0).contains(&y) {
                let index = ((y - 58.0) / 28.0).floor() as usize;
                if let Some(kind) = [
                    scorepeek_overlay_ui::WidgetKind::Status,
                    scorepeek_overlay_ui::WidgetKind::Selection,
                    scorepeek_overlay_ui::WidgetKind::Score,
                    scorepeek_overlay_ui::WidgetKind::HistoryList,
                    scorepeek_overlay_ui::WidgetKind::HistoryGraph,
                ]
                .get(index)
                .copied()
                {
                    self.add_widget(kind);
                    return;
                }
            }
            let selected_id = self.selected.borrow().clone();
            if x >= f64::from(self.canvas.width.saturating_sub(188)) {
                if y >= 220.0 {
                    let index = usize::try_from(((y - 248.0).max(0.0) / 28.0).floor() as u64)
                        .unwrap_or(usize::MAX);
                    let output = self.outputs.borrow().get(index).cloned();
                    if let Some(output) = output {
                        let response = self.request(crate::control::Request::SetOutput {
                            canvas_id: self.canvas.id.clone(),
                            editor_id: self.editor_id.clone(),
                            expected_revision: self.canvas.revision,
                            output: output.clone(),
                        });
                        if response.as_ref().is_some_and(|response| response.ok) {
                            self.canvas.output = Some(output);
                        }
                        return;
                    }
                }
                if (90.0..122.0).contains(&y)
                    && let Some(id) = selected_id.clone()
                {
                    self.shared_widgets
                        .borrow_mut()
                        .retain(|widget| widget.id != id);
                    self.selected.borrow_mut().take();
                    self.persist_canvas();
                    return;
                }
                if (128.0..162.0).contains(&y)
                    && let Some(id) = selected_id.clone()
                {
                    let index = (((x - f64::from(self.canvas.width.saturating_sub(180))) / 42.0)
                        .floor() as usize)
                        .min(3);
                    if let Some(widget) = self
                        .shared_widgets
                        .borrow_mut()
                        .iter_mut()
                        .find(|widget| widget.id == id)
                    {
                        match widget.kind {
                            scorepeek_overlay_ui::WidgetKind::HistoryList => {
                                widget.settings.history_count = [5, 10, 20, 50][index];
                            }
                            scorepeek_overlay_ui::WidgetKind::HistoryGraph => {
                                widget.settings.graph_months = [1, 3, 6, 12][index];
                            }
                            _ => return,
                        }
                    }
                    self.persist_canvas();
                    return;
                }
                if (168.0..202.0).contains(&y)
                    && let Some(id) = selected_id
                {
                    if let Some(widget) = self
                        .shared_widgets
                        .borrow_mut()
                        .iter_mut()
                        .find(|widget| widget.id == id)
                    {
                        widget.x = widget.x.clamp(
                            0,
                            i32::try_from(self.canvas.width.saturating_sub(32)).unwrap_or(i32::MAX),
                        );
                        widget.y = widget.y.clamp(
                            0,
                            i32::try_from(self.canvas.height.saturating_sub(32))
                                .unwrap_or(i32::MAX),
                        );
                    }
                    self.persist_canvas();
                    return;
                }
            }
            if x >= f64::from(self.canvas.width.saturating_sub(24))
                && y >= f64::from(self.canvas.height.saturating_sub(24))
            {
                self.interaction = Some(NativeInteraction::CanvasResize {
                    start: [x, y],
                    origin: [self.canvas.width, self.canvas.height],
                });
                return;
            }
            let hit = self
                .shared_widgets
                .borrow()
                .iter()
                .filter(|widget| {
                    x >= f64::from(widget.x)
                        && y >= f64::from(widget.y)
                        && x < f64::from(widget.x) + f64::from(widget.width)
                        && y < f64::from(widget.y) + f64::from(widget.height)
                })
                .max_by_key(|widget| widget.z)
                .cloned();
            if let Some(original) = hit {
                let resize = x
                    >= f64::from(original.x) + f64::from(original.width.saturating_sub(18))
                    && y >= f64::from(original.y) + f64::from(original.height.saturating_sub(18));
                *self.selected.borrow_mut() = Some(original.id.clone());
                let top = self
                    .shared_widgets
                    .borrow()
                    .iter()
                    .map(|widget| widget.z)
                    .max()
                    .unwrap_or(0)
                    .saturating_add(1);
                if let Some(widget) = self
                    .shared_widgets
                    .borrow_mut()
                    .iter_mut()
                    .find(|widget| widget.id == original.id)
                {
                    widget.z = top;
                }
                self.interaction = Some(NativeInteraction::Widget {
                    id: original.id.clone(),
                    start: [x, y],
                    original,
                    resize,
                });
            } else {
                self.interaction = Some(NativeInteraction::CanvasMove {
                    start: [x, y],
                    origin: [self.canvas.x, self.canvas.y],
                });
            }
        } else {
            self.persist_interaction();
        }
    }
    fn pointer_motion(&mut self, x: f64, y: f64) {
        let Some(interaction) = &self.interaction else {
            return;
        };
        match interaction {
            NativeInteraction::CanvasMove { start, origin } => {
                self.move_canvas(*start, *origin, x, y);
            }
            NativeInteraction::CanvasResize { start, origin } => {
                self.resize_canvas(*start, *origin, x, y);
            }
            NativeInteraction::Widget {
                id,
                start,
                original,
                resize,
            } => {
                let horizontal_edges: Vec<_> = self
                    .shared_widgets
                    .borrow()
                    .iter()
                    .filter(|widget| &widget.id != id)
                    .flat_map(|widget| {
                        [
                            widget.x,
                            widget
                                .x
                                .saturating_add(i32::try_from(widget.width).unwrap_or(i32::MAX)),
                        ]
                    })
                    .collect();
                let vertical_edges: Vec<_> = self
                    .shared_widgets
                    .borrow()
                    .iter()
                    .filter(|widget| &widget.id != id)
                    .flat_map(|widget| {
                        [
                            widget.y,
                            widget
                                .y
                                .saturating_add(i32::try_from(widget.height).unwrap_or(i32::MAX)),
                        ]
                    })
                    .collect();
                if let Some(widget) = self
                    .shared_widgets
                    .borrow_mut()
                    .iter_mut()
                    .find(|widget| &widget.id == id)
                {
                    if *resize {
                        widget.width = snap_u32(f64::from(original.width) + x - start[0]);
                        widget.height = snap_u32(f64::from(original.height) + y - start[1]);
                    } else {
                        widget.x = magnetic_snap(
                            snap_i32(f64::from(original.x) + x - start[0]),
                            widget.width,
                            self.canvas.width,
                            horizontal_edges.into_iter(),
                        );
                        widget.y = magnetic_snap(
                            snap_i32(f64::from(original.y) + y - start[1]),
                            widget.height,
                            self.canvas.height,
                            vertical_edges.into_iter(),
                        );
                    }
                }
            }
        }
        if let Some(update) = self.native_update.borrow().as_ref() {
            update();
        }
    }

    fn move_canvas(&mut self, start: [f64; 2], origin: [i32; 2], x: f64, y: f64) {
        self.canvas.x = snap_i32(f64::from(origin[0]) + x - start[0]);
        self.canvas.y = snap_i32(f64::from(origin[1]) + y - start[1]);
        if let Some([output_width, output_height]) = self.shell.output_logical_size {
            self.canvas.x = self.canvas.x.clamp(
                -i32::try_from(self.canvas.width.saturating_sub(32)).unwrap_or(i32::MAX),
                i32::try_from(output_width.saturating_sub(32)).unwrap_or(i32::MAX),
            );
            self.canvas.y = self.canvas.y.clamp(
                -i32::try_from(self.canvas.height.saturating_sub(32)).unwrap_or(i32::MAX),
                i32::try_from(output_height.saturating_sub(32)).unwrap_or(i32::MAX),
            );
        }
        self.shell.set_geometry(
            self.canvas.x,
            self.canvas.y,
            self.canvas.width,
            self.canvas.height,
        );
    }

    fn resize_canvas(&mut self, start: [f64; 2], origin: [u32; 2], x: f64, y: f64) {
        self.canvas.width = snap_u32(f64::from(origin[0]) + x - start[0]);
        self.canvas.height = snap_u32(f64::from(origin[1]) + y - start[1]);
        if let Some([output_width, output_height]) = self.shell.output_logical_size {
            self.canvas.width = self.canvas.width.min(output_width);
            self.canvas.height = self.canvas.height.min(output_height);
        }
        self.shell.set_geometry(
            self.canvas.x,
            self.canvas.y,
            self.canvas.width,
            self.canvas.height,
        );
    }
    fn persist_interaction(&mut self) {
        let Some(interaction) = self.interaction.take() else {
            return;
        };
        let request = match interaction {
            NativeInteraction::CanvasMove { .. } | NativeInteraction::CanvasResize { .. } => {
                crate::control::Request::SetGeometry {
                    canvas_id: self.canvas.id.clone(),
                    editor_id: self.editor_id.clone(),
                    expected_revision: self.canvas.revision,
                    x: self.canvas.x,
                    y: self.canvas.y,
                    width: self.canvas.width,
                    height: self.canvas.height,
                }
            }
            NativeInteraction::Widget { .. } => {
                let mut presentation = self.canvas.presentation();
                presentation
                    .widgets
                    .clone_from(&self.shared_widgets.borrow());
                crate::control::Request::ReplaceCanvas {
                    canvas_id: self.canvas.id.clone(),
                    editor_id: self.editor_id.clone(),
                    expected_revision: self.canvas.revision,
                    presentation,
                }
            }
        };
        let _ = self.request(request);
        if !self.editing.get() {
            let _ = self.request(crate::control::Request::Release {
                canvas_id: self.canvas.id.clone(),
                editor_id: self.editor_id.clone(),
            });
        }
    }
    fn persist_canvas(&mut self) {
        let mut presentation = self.canvas.presentation();
        presentation.skin = self.appearance.get().skin;
        presentation
            .widgets
            .clone_from(&self.shared_widgets.borrow());
        let _ = self.request(crate::control::Request::ReplaceCanvas {
            canvas_id: self.canvas.id.clone(),
            editor_id: self.editor_id.clone(),
            expected_revision: self.canvas.revision,
            presentation,
        });
        if let Some(update) = self.native_update.borrow().as_ref() {
            update();
        }
    }
    fn add_widget(&mut self, kind: scorepeek_overlay_ui::WidgetKind) {
        let id = scorepeek_overlay_ui::next_widget_id(kind, &self.shared_widgets.borrow());
        let (width, height) = scorepeek_overlay_ui::default_widget_size(kind);
        let z = self
            .shared_widgets
            .borrow()
            .iter()
            .map(|widget| widget.z)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        self.shared_widgets.borrow_mut().push(WidgetLayout {
            id: id.clone(),
            kind,
            x: 160,
            y: 64,
            width,
            height,
            z,
            settings: scorepeek_overlay_ui::WidgetSettings::default(),
        });
        *self.selected.borrow_mut() = Some(id);
        self.persist_canvas();
    }
    fn poll_dioxus(&mut self) -> bool {
        let mut changed = false;
        while self
            .document
            .poll(Some(TaskContext::from_waker(&self.waker)))
        {
            changed = true;
        }
        changed
    }
    fn configure(
        &mut self,
        logical: [u32; 2],
        physical: [u32; 2],
        scale_120: u32,
    ) -> Result<(), String> {
        let [width, height] = physical;
        {
            let mut report = self.report.borrow_mut();
            report.logical_size = Some(logical);
            report.physical_size = Some(physical);
            report.scale_120 = scale_120;
        }
        self.document.inner.borrow_mut().set_viewport(Viewport::new(
            width,
            height,
            f32::from(u16::try_from(scale_120).map_err(|e| e.to_string())?) / 120.0,
            ColorScheme::Dark,
        ));
        if self.renderer.is_active() {
            self.renderer.set_size(width, height);
        } else {
            self.renderer
                .resume(self.shell.handles(), width, height, || {});
            if !self.renderer.complete_resume() {
                return Err("gpu_adapter".into());
            }
            let device = self.renderer.current_device_handle().ok_or("gpu_adapter")?;
            let info = device.adapter.get_info();
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
        self.paint()
    }
    fn paint(&mut self) -> Result<(), String> {
        if !self.renderer.is_active() {
            return Err("native renderer inactive".into());
        }
        let mut inner = self.document.inner.borrow_mut();
        inner.resolve(self.started.elapsed().as_secs_f64());
        // Embedded images complete synchronously during resolve; ingest them before painting.
        inner.handle_messages();
        self.animating = inner.is_animating();
        if self.animating {
            self.shell.request_frame();
        }
        let (width, height) = inner.viewport().window_size;
        let scale = inner.viewport().scale_f64();
        self.renderer.render(|scene| {
            paint_scene(scene, &mut inner, scale, width, height, 0, 0);
        });
        self.paint_count += 1;
        self.render_calls += 1;
        if self.paint_count == 1 {
            self.report
                .borrow_mut()
                .operations
                .push("dioxus_blitz_initial_paint");
        }
        Ok(())
    }
}
fn magnetic_snap(
    candidate: i32,
    size: u32,
    canvas_size: u32,
    other_edges: impl Iterator<Item = i32>,
) -> i32 {
    let size = i32::try_from(size).unwrap_or(i32::MAX);
    let canvas = i32::try_from(canvas_size).unwrap_or(i32::MAX);
    let mut anchors = vec![0, canvas / 2, canvas];
    anchors.extend(other_edges);
    let points = [
        candidate,
        candidate.saturating_add(size / 2),
        candidate.saturating_add(size),
    ];
    let mut best = (7, candidate);
    for point in points {
        for anchor in &anchors {
            let distance = point.abs_diff(*anchor);
            if distance < best.0 {
                best = (distance, candidate.saturating_add(*anchor - point));
            }
        }
    }
    best.1
}
const fn control_updates_readonly(request: &crate::control::Request) -> bool {
    matches!(
        request,
        crate::control::Request::Acquire { .. }
            | crate::control::Request::KeepAlive { .. }
            | crate::control::Request::Release { .. }
            | crate::control::Request::ReplaceCanvas { .. }
            | crate::control::Request::SetGeometry { .. }
            | crate::control::Request::SetOutput { .. }
    )
}

#[derive(Serialize)]
struct RunReport {
    run_id: String,
    build_revision: &'static str,
    output_name: Option<String>,
    logical_size: Option<[u32; 2]>,
    physical_size: Option<[u32; 2]>,
    scale_120: u32,
    gpu_backend: Option<String>,
    gpu_adapter: Option<String>,
    paint_count: u32,
    render_calls: u32,
    operations: Operations,
    status: &'static str,
    failure: Option<String>,
}

impl RunReport {
    fn new() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        Self {
            run_id: format!("{timestamp}-{}", std::process::id()),
            build_revision: option_env!("OVERLAY_BUILD_REVISION").unwrap_or("working-tree"),
            output_name: None,
            logical_size: None,
            physical_size: None,
            scale_120: 120,
            gpu_backend: None,
            gpu_adapter: None,
            paint_count: 0,
            render_calls: 0,
            operations: Operations::default(),
            status: "running",
            failure: None,
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

struct EmbeddedSkinAssets;

impl blitz_traits::net::NetProvider for EmbeddedSkinAssets {
    fn fetch(
        &self,
        _doc_id: usize,
        request: blitz_traits::net::Request,
        handler: Box<dyn blitz_traits::net::NetHandler>,
    ) {
        let bytes = scorepeek_overlay_ui::skin_asset(request.url.path()).unwrap_or_default();
        handler.bytes(
            request.url.to_string(),
            blitz_traits::net::Bytes::from_static(bytes),
        );
    }
}

/// Registers embedded artwork and the Latin font, preserving Japanese system fallbacks.
#[must_use]
pub fn document_config() -> DocumentConfig {
    let mut font_ctx = blitz_dom::FontContext::default();
    font_ctx
        .collection
        .register_fonts(peniko::Blob::new(Arc::new(OXANIUM)), None);
    DocumentConfig {
        font_ctx: Some(font_ctx),
        base_url: Some("http://scorepeek.invalid/".into()),
        net_provider: Some(Arc::new(EmbeddedSkinAssets)),
        ..DocumentConfig::default()
    }
}

#[cfg(test)]
mod skin_tests {
    use super::*;
    use scorepeek_overlay_ui::Skin;

    #[test]
    fn embedded_artwork_decodes_with_the_native_png_feature() {
        for (path, bytes) in scorepeek_overlay_ui::SKIN_ASSETS {
            let image = image::load_from_memory(bytes).expect(path);
            assert!(image.width() >= 1024 && image.height() >= 768, "{path}");
        }
    }

    #[test]
    fn every_skin_lays_out_all_initial_widgets_and_stops_animating() {
        for skin in [Skin::CyanSystem, Skin::ResultAurora, Skin::DjBlackbox] {
            let mut document = DioxusDocument::new(
                VirtualDom::new_with_props(
                    native_overlay,
                    NativeOverlayProps {
                        appearance: Rc::new(Cell::new(Appearance { skin })),
                        widgets: Rc::new(RefCell::new(scorepeek_overlay_ui::default_widgets())),
                        editing: Rc::new(Cell::new(false)),
                        selected: Rc::new(RefCell::new(None)),
                        managed: Rc::new(RefCell::new(Vec::new())),
                        outputs: Rc::new(RefCell::new(Vec::new())),
                        state: Rc::new(RefCell::new(OverlayState::default())),
                        update: Rc::new(RefCell::new(None)),
                    },
                ),
                document_config(),
            );
            document.initial_build();
            let mut inner = document.inner.borrow_mut();
            inner.set_viewport(Viewport::new(560, 1040, 1.25, ColorScheme::Dark));
            inner.resolve(0.0);
            inner.resolve(1.0);
            assert!(!inner.is_animating(), "{skin:?}");
            for selector in [
                ".status-widget",
                ".selection-widget",
                ".score-widget",
                ".history-list-widget",
                ".history-graph-widget",
            ] {
                let id = inner.query_selector(selector).unwrap().unwrap();
                let rect = inner.get_client_bounding_rect(id).unwrap();
                assert!(rect.width > 0.0 && rect.height > 0.0, "{skin:?} {selector}");
            }
        }
    }

    #[test]
    fn canvas_list_does_not_replace_the_lease_state() {
        assert!(control_updates_readonly(
            &crate::control::Request::KeepAlive {
                canvas_id: "wayland-main".into(),
                editor_id: "editor".into(),
            }
        ));
        assert!(!control_updates_readonly(
            &crate::control::Request::ListCanvases {
                backend: crate::runtime::Backend::Wayland,
            }
        ));
    }
}
