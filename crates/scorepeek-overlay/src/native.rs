use std::{
    cell::RefCell,
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
use dioxus_core::{Element, VirtualDom, schedule_update};
use dioxus_native_dom::DioxusDocument;
use scorepeek_overlay_handles::{Event, Shell};
use scorepeek_overlay_ui::{Appearance, OXANIUM, OverlayState, overlay_panel};
use serde::Serialize;
use smithay_client_toolkit::reexports::calloop::ping::{Ping, make_ping};

type NativeUpdate = Rc<RefCell<Option<Arc<dyn Fn() + Send + Sync>>>>;

#[derive(Clone)]
struct NativeOverlayProps {
    appearance: Appearance,
    state: Rc<RefCell<OverlayState>>,
    update: NativeUpdate,
}

fn native_overlay(
    NativeOverlayProps {
        state,
        update,
        appearance,
    }: NativeOverlayProps,
) -> Element {
    *update.borrow_mut() = Some(schedule_update());
    overlay_panel(&state.borrow(), appearance)
}

struct CalloopWaker(Ping);

impl Wake for CalloopWaker {
    fn wake(self: Arc<Self>) {
        self.0.ping();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.ping();
    }
}

/// Runs until the parent's lifetime lease closes.
/// # Errors
/// Returns Wayland, GPU or event-loop failures.
pub fn run(config: Config, mut input: impl std::io::Read + Send + 'static) -> Result<(), String> {
    let report = Rc::new(RefCell::new(RunReport::new()));
    let ping = make_ping().map_err(|e| e.to_string())?;
    let wake = ping.0.clone();
    let shell = Shell::open(config.output.as_deref(), ping.1)?;
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
    let appearance = config.appearance;
    let feed = Feed::start(config, Arc::new(move || wake.ping())).map_err(|e| e.to_string())?;
    let stop = Arc::clone(&feed.stop);
    std::thread::Builder::new()
        .name("overlay-parent".into())
        .spawn(move || {
            let mut byte = [0];
            let _ = input.read(&mut byte);
            stop.store(true, std::sync::atomic::Ordering::Release);
        })
        .map_err(|e| e.to_string())?;
    let mut app = App::new(
        appearance,
        shell,
        Waker::from(Arc::new(CalloopWaker(ping.0))),
        feed,
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
}
impl App {
    fn new(
        appearance: Appearance,
        shell: Shell,
        waker: Waker,
        feed: Feed,
        report: Rc<RefCell<RunReport>>,
    ) -> Self {
        let shared_state = Rc::new(RefCell::new(OverlayState::default()));
        let native_update = Rc::new(RefCell::new(None));
        let vdom = VirtualDom::new_with_props(
            native_overlay,
            NativeOverlayProps {
                appearance,
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
        }
    }
    fn run(&mut self) -> Result<(), String> {
        let _ = self.poll_dioxus();
        while !self.feed.stop.load(std::sync::atomic::Ordering::Acquire) {
            let events = self.shell.dispatch(Duration::from_millis(500))?;
            let mut wake = false;
            let mut frame = false;
            for event in events {
                match event {
                    Event::Configure {
                        logical,
                        physical,
                        scale_120,
                    } => self.configure(logical, physical, scale_120)?,
                    Event::Wake => wake = true,
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

/// Registers the embedded Latin font without disabling Japanese system fallbacks.
#[must_use]
pub fn document_config() -> DocumentConfig {
    let mut font_ctx = blitz_dom::FontContext::default();
    font_ctx
        .collection
        .register_fonts(peniko::Blob::new(Arc::new(OXANIUM)), None);
    DocumentConfig {
        font_ctx: Some(font_ctx),
        ..DocumentConfig::default()
    }
}

#[cfg(test)]
mod skin_tests {
    use super::*;
    use scorepeek_overlay_ui::{Layout, Skin};

    #[test]
    fn skins_keep_all_sections_in_view_and_settle_at_integer_and_fractional_scale() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/skin-preview.json")).unwrap();
        let original: OverlayState = serde_json::from_value(fixture["state"].clone()).unwrap();
        for skin in [Skin::CyanSystem, Skin::ResultAurora, Skin::DjBlackbox] {
            for layout in [Layout::Compact, Layout::Sidebar] {
                for scale in [1.0, 1.25] {
                    for scenario in 0..5 {
                        let mut state = original.clone();
                        match scenario {
                            1 => {
                                state.connected = false;
                            }
                            2 => {
                                state.result = None;
                                state.selecting = true;
                            }
                            3 => {
                                state.history = scorepeek_overlay_ui::History::default();
                                state.result.as_mut().unwrap().fields.clear();
                            }
                            4 => {
                                state = OverlayState::default();
                            }
                            _ => {}
                        }
                        let mut document = DioxusDocument::new(
                            VirtualDom::new_with_props(
                                native_overlay,
                                NativeOverlayProps {
                                    appearance: Appearance { skin, layout },
                                    state: Rc::new(RefCell::new(state)),
                                    update: Rc::new(RefCell::new(None)),
                                },
                            ),
                            document_config(),
                        );
                        document.initial_build();
                        let mut inner = document.inner.borrow_mut();
                        inner.set_viewport(Viewport::new(750, 1350, scale, ColorScheme::Dark));
                        inner.resolve(0.0);
                        inner.resolve(1.0);
                        assert!(!inner.is_animating(), "{skin:?} {layout:?} {scenario}");
                        for selector in [".live", ".confirmation", ".history", ".overlay-footer"] {
                            let id = inner.query_selector(selector).unwrap().unwrap();
                            let rect = inner.get_client_bounding_rect(id).unwrap();
                            assert!(rect.width > 0.0 && rect.height > 0.0);
                            assert!(
                                rect.x >= 0.0
                                    && rect.x + rect.width <= 750.0 / f64::from(scale) + 1.0,
                                "horizontal overflow: {skin:?} {layout:?} {selector}"
                            );
                            assert!(
                                rect.y >= 0.0
                                    && rect.y + rect.height <= 1350.0 / f64::from(scale) + 1.0,
                                "vertical overflow: {skin:?} {layout:?} {selector}"
                            );
                        }
                    }
                }
            }
        }
    }
}
