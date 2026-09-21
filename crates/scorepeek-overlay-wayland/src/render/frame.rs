//! Native frame presentation and surface commit boundary.

use super::dioxus_dom::FrameWorkProfile;
use super::vello::paint_native_scene;
use crate::host::event_loop::Shell;
use anyrender::WindowRenderer as _;
use anyrender_vello::VelloWindowRenderer;
use std::time::{Duration, Instant};

pub(crate) trait NativeFramePresenter {
    fn is_active(&self) -> bool;

    fn set_text_input(&mut self, input: Option<scorepeek_overlay_wayland_handles::TextInputState>);

    fn unmap(&mut self) -> Result<(), String>;

    fn present(
        &mut self,
        document: &mut blitz_dom::BaseDocument,
        scale: f64,
        width: u32,
        height: u32,
        work: &mut FrameWorkProfile,
    ) -> Result<(), String>;
}

pub(crate) struct WindowPresenter<'a> {
    pub(crate) shell: &'a mut Shell,
    pub(crate) renderer: &'a mut VelloWindowRenderer,
}

impl NativeFramePresenter for WindowPresenter<'_> {
    fn is_active(&self) -> bool {
        self.renderer.is_active()
    }

    fn set_text_input(&mut self, input: Option<scorepeek_overlay_wayland_handles::TextInputState>) {
        self.shell.set_text_input(input);
    }

    fn unmap(&mut self) -> Result<(), String> {
        self.renderer.suspend();
        self.shell.unmap()
    }

    fn present(
        &mut self,
        document: &mut blitz_dom::BaseDocument,
        scale: f64,
        width: u32,
        height: u32,
        work: &mut FrameWorkProfile,
    ) -> Result<(), String> {
        self.shell.request_frame();
        let started = Instant::now();
        let mut scene_elapsed = Duration::ZERO;
        self.renderer.render(|scene| {
            let scene_started = Instant::now();
            paint_native_scene(scene, document, scale, width, height);
            scene_elapsed = scene_started.elapsed();
            work.record("scene", scene_elapsed);
        });
        // anyrender-vello exposes scene construction and a combined renderer/present call, but
        // does not expose the wl_surface commit as a separately timed operation. Keep scene time
        // disjoint and mark commit as unavailable instead of publishing a fabricated zero.
        work.record(
            "gpu_present",
            started.elapsed().saturating_sub(scene_elapsed),
        );
        work.unmeasured("surface_commit");
        Ok(())
    }
}
