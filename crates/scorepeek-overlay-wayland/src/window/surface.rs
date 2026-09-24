//! Layer-surface ownership and configure lifecycle.

use std::sync::Arc;
use std::time::{Duration, Instant};

use raw_wayland_handles::SurfaceHandle;
use smithay_client_toolkit::compositor::{CompositorState, Region};
use smithay_client_toolkit::output::OutputState;
use smithay_client_toolkit::reexports::calloop::{EventLoop, ping::PingSource};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::registry::RegistryState;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface,
};
use wayland_client::{Connection, globals::registry_queue_init};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

use crate::host::event_loop::{
    CursorStyle, Event, Platform, ScaleData, fallback_cursor, select_output,
};
use crate::input::keyboard as input;
use crate::window::geometry::upper_right_x;
use crate::window::scale::{OutputDescription, scaled_size};

pub(crate) trait LayerStateTarget {
    fn set_top_left_anchor(&self);
    fn set_position(&self, x: i32, y: i32);
    fn set_non_exclusive(&self);
    fn set_keyboard_enabled(&self, enabled: bool);
    fn set_logical_size(&self, width: u32, height: u32);
}

impl LayerStateTarget for LayerSurface {
    fn set_top_left_anchor(&self) {
        self.set_anchor(Anchor::TOP | Anchor::LEFT);
    }

    fn set_position(&self, x: i32, y: i32) {
        self.set_margin(y, 0, 0, x);
    }

    fn set_non_exclusive(&self) {
        self.set_exclusive_zone(-1);
    }

    fn set_keyboard_enabled(&self, enabled: bool) {
        self.set_keyboard_interactivity(if enabled {
            KeyboardInteractivity::Exclusive
        } else {
            KeyboardInteractivity::None
        });
    }

    fn set_logical_size(&self, width: u32, height: u32) {
        self.set_size(width, height);
    }
}

pub(crate) fn apply_layer_state(
    target: &impl LayerStateTarget,
    position: [i32; 2],
    size: [u32; 2],
    keyboard_enabled: bool,
) {
    target.set_top_left_anchor();
    target.set_position(position[0], position[1]);
    target.set_non_exclusive();
    target.set_keyboard_enabled(keyboard_enabled);
    target.set_logical_size(size[0], size[1]);
}

pub struct Shell {
    pub(crate) owner: Arc<SurfaceHandle>,
    pub(crate) state: Platform,
    event_loop: EventLoop<'static, Platform>,
    configure_phase: ConfigurePhase,
    pub output_name: Option<String>,
    pub available_outputs: Vec<String>,
    pub output_descriptions: Vec<OutputDescription>,
    pub position: [i32; 2],
    pub output_logical_size: Option<[u32; 2]>,
    pub fractional_scaling: bool,
    input_rects: Option<Vec<[i32; 4]>>,
}

#[derive(Clone, Copy, Debug)]
enum ConfigurePhase {
    Awaiting(Instant),
    Ready,
    Unmapped,
}

impl ConfigurePhase {
    fn timed_out(self) -> bool {
        matches!(self, Self::Awaiting(started) if started.elapsed() >= Duration::from_secs(5))
    }
}

impl Shell {
    /// Creates an interactive layer on the requested or deterministic default output.
    /// # Errors
    /// Returns connection, global binding, output selection or event-loop failures.
    #[allow(clippy::too_many_lines)]
    pub fn open(
        output: Option<&str>,
        width: u32,
        height: u32,
        x: i32,
        y: i32,
        upper_right: bool,
        ping: PingSource,
    ) -> Result<Self, String> {
        let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;

        let (globals, mut event_queue) = registry_queue_init(&conn).map_err(|e| e.to_string())?;
        let qh = event_queue.handle();
        let compositor = CompositorState::bind(&globals, &qh).map_err(|e| e.to_string())?;
        let layer_shell = LayerShell::bind(&globals, &qh).map_err(|e| e.to_string())?;

        let event_loop: EventLoop<Platform> = EventLoop::try_new().map_err(|e| e.to_string())?;
        let mut app = Platform {
            qh: qh.clone(),
            registry_state: RegistryState::new(&globals),
            output_state: OutputState::new(&globals, &qh),
            owner: None,
            overlay_surface: None,
            selected_output: None,
            width,
            height,
            scale: 1,
            fractional_scale: None,
            fractional: None,
            viewport: None,
            configured: false,
            needs_configure: false,
            frame_callback: None,
            fallback: [width, height],
            events: Vec::new(),
            failure: None,
            input: input::Input::new(&globals, &qh, event_loop.handle()),
            pointer: None,
            pointer_position: [0.0, 0.0],
            cursor_manager: globals
                .bind::<WpCursorShapeManagerV1, _, _>(&qh, 1..=2, ())
                .ok(),
            cursor_device: None,
            pointer_enter_serial: None,
            compositor: compositor.clone(),
            cursor_surface: None,
            cursor_buffer: None,
            cursor_pool: None,
            cursor_file: None,
        };

        if app.cursor_manager.is_none() {
            let (cursor_surface, cursor_buffer, cursor_pool, cursor_file) =
                fallback_cursor(&globals, &qh, &compositor)
                    .ok_or("Wayland cursor shape and fallback cursor are unavailable")?;
            app.cursor_surface = Some(cursor_surface);
            app.cursor_buffer = Some(cursor_buffer);
            app.cursor_pool = Some(cursor_pool);
            app.cursor_file = Some(cursor_file);
        }

        event_queue.roundtrip(&mut app).map_err(|e| e.to_string())?;
        let selection = select_output(&app.output_state, output, [width, height])?;
        let available_outputs = app
            .output_state
            .outputs()
            .filter_map(|output| app.output_state.info(&output).and_then(|info| info.name))
            .collect();
        let output_descriptions = app
            .output_state
            .outputs()
            .filter_map(|output| {
                let info = app.output_state.info(&output)?;
                Some(OutputDescription {
                    name: info.name?,
                    model: info.model,
                    logical_size: info.logical_size.and_then(|(width, height)| {
                        Some([u32::try_from(width).ok()?, u32::try_from(height).ok()?])
                    }),
                })
            })
            .collect();
        let output_logical_size = selection.info.logical_size.and_then(|(width, height)| {
            Some([u32::try_from(width).ok()?, u32::try_from(height).ok()?])
        });
        let surface_width = output_logical_size.map_or(width, |size| width.min(size[0]));
        let surface_height = output_logical_size.map_or(height, |size| height.min(size[1]));
        app.width = surface_width;
        app.height = surface_height;
        app.fallback = [surface_width, surface_height];
        app.selected_output = Some(selection.output.clone());

        let resolved_x = if upper_right {
            selection
                .info
                .logical_size
                .and_then(|(output_width, _)| upper_right_x(output_width, surface_width, x))
                .unwrap_or(x)
        } else {
            x
        };
        let surface = compositor.create_surface(&qh);
        if let (Ok(manager), Ok(viewporter)) = (
            globals.bind::<WpFractionalScaleManagerV1, _, _>(&qh, 1..=1, ScaleData),
            globals.bind::<WpViewporter, _, _>(&qh, 1..=1, ScaleData),
        ) {
            app.fractional = Some(manager.get_fractional_scale(&surface, &qh, ScaleData));
            app.viewport = Some(viewporter.get_viewport(&surface, &qh, ScaleData));
            manager.destroy();
            viewporter.destroy();
        }
        let layer = layer_shell.create_layer_surface(
            &qh,
            surface.clone(),
            Layer::Overlay,
            Some("scorepeek-overlay"),
            Some(&selection.output),
        );
        apply_layer_state(
            &layer,
            [resolved_x, y],
            [surface_width, surface_height],
            false,
        );
        app.scale = selection.info.scale_factor.max(1);
        layer.commit();

        app.owner = Some(Arc::new(SurfaceHandle::new(layer, conn.clone())?));
        app.overlay_surface = Some(surface);

        WaylandSource::new(conn, event_queue)
            .insert(event_loop.handle())
            .map_err(|e| e.to_string())?;
        event_loop
            .handle()
            .insert_source(ping, |(), &mut (), app| {
                app.events.push(Event::Wake);
            })
            .map_err(|e| e.to_string())?;

        let owner = Arc::clone(app.owner.as_ref().ok_or("surface creation failed")?);
        Ok(Self {
            owner,
            output_name: selection.info.name,
            available_outputs,
            output_descriptions,
            position: [resolved_x, y],
            output_logical_size,
            fractional_scaling: app.viewport.is_some(),
            state: app,
            event_loop,
            configure_phase: ConfigurePhase::Awaiting(Instant::now()),
            input_rects: None,
        })
    }
    #[must_use]
    pub fn handles(&self) -> Arc<SurfaceHandle> {
        Arc::clone(&self.owner)
    }
    /// Dispatches protocol events without exposing destroy-capable proxies.
    /// # Errors
    /// Returns dispatch, configure timeout, invalid size or output removal failures.
    pub fn dispatch(&mut self, timeout: Duration) -> Result<Vec<Event>, String> {
        self.event_loop
            .dispatch(timeout, &mut self.state)
            .map_err(|e| e.to_string())?;
        self.refresh_output_snapshot();
        if let Some(error) = self.state.failure.take() {
            return Err(error);
        }
        if self.state.configured {
            self.configure_phase = ConfigurePhase::Ready;
        }
        if self.configure_phase.timed_out() {
            return Err("configure_timeout".into());
        }
        if self.state.needs_configure && self.state.configured {
            self.state.needs_configure = false;
            let s = &self.state;
            let owner = &self.owner;
            owner.roundtrip()?;
            let scale_120 = s.fractional_scale.unwrap_or(s.scale.cast_unsigned() * 120);
            owner.set_buffer_scale(if s.viewport.is_some() { 1 } else { s.scale });
            if let Some(viewport) = &s.viewport {
                viewport.set_destination(
                    i32::try_from(s.width).map_err(|e| e.to_string())?,
                    i32::try_from(s.height).map_err(|e| e.to_string())?,
                );
            }
            self.state.events.push(Event::Configure {
                logical: [s.width, s.height],
                physical: [
                    scaled_size(s.width, scale_120),
                    scaled_size(s.height, scale_120),
                ],
                scale_120,
            });
        }
        Ok(std::mem::take(&mut self.state.events))
    }

    fn refresh_output_snapshot(&mut self) {
        let outputs: Vec<_> = self
            .state
            .output_state
            .outputs()
            .filter_map(|output| self.state.output_state.info(&output))
            .collect();
        self.available_outputs = outputs
            .iter()
            .filter_map(|info| info.name.clone())
            .collect();
        self.output_descriptions = outputs
            .iter()
            .filter_map(|info| {
                Some(OutputDescription {
                    name: info.name.clone()?,
                    model: info.model.clone(),
                    logical_size: info.logical_size.and_then(|(width, height)| {
                        Some([u32::try_from(width).ok()?, u32::try_from(height).ok()?])
                    }),
                })
            })
            .collect();
        if let Some(name) = self.output_name.as_deref()
            && let Some(info) = outputs
                .iter()
                .find(|info| info.name.as_deref() == Some(name))
        {
            self.output_logical_size = info.logical_size.and_then(|(width, height)| {
                Some([u32::try_from(width).ok()?, u32::try_from(height).ok()?])
            });
        }
    }
    /// Requests at most one frame callback for the next renderer commit.
    ///
    /// # Panics
    /// Panics when called after the configured shell has lost its owned overlay surface.
    pub fn request_frame(&mut self) {
        if self.state.frame_callback.is_none() {
            let surface = self
                .state
                .overlay_surface
                .as_ref()
                .expect("configured shell owns an overlay surface");
            self.state.frame_callback = Some(surface.frame(
                &self.state.qh,
                smithay_client_toolkit::compositor::FrameCallbackData(surface.clone()),
            ));
        }
    }

    pub fn set_geometry(&mut self, x: i32, y: i32, width: u32, height: u32) {
        let width = width.max(32);
        let height = height.max(32);
        if self.position == [x, y] && self.state.width == width && self.state.height == height {
            return;
        }
        self.position = [x, y];
        self.state.width = width;
        self.state.height = height;
        self.owner
            .set_geometry(x, y, width, height, self.state.configured);
    }

    /// Detaches the current buffer and resets the layer-surface configure handshake.
    /// # Errors
    /// Returns a Wayland connection flush failure.
    pub fn unmap(&mut self) -> Result<(), String> {
        self.owner.unmap()?;
        self.state.configured = false;
        self.state.needs_configure = false;
        self.configure_phase = ConfigurePhase::Unmapped;
        Ok(())
    }

    /// Starts the layer-shell remap handshake with a bufferless commit.
    pub fn begin_remap(&mut self) {
        self.state.configured = false;
        self.state.needs_configure = false;
        self.configure_phase = ConfigurePhase::Awaiting(Instant::now());
        self.owner.begin_remap(
            self.position,
            [self.state.width, self.state.height],
            self.state.input.keyboard_enabled(),
        );
    }

    /// Stages the whole surface input region or replaces it with an empty one.
    ///
    /// # Panics
    /// Panics when called after the configured shell has lost its owned overlay surface.
    pub fn set_input_enabled(&mut self, enabled: bool) {
        let _ = self.set_input_rects(if enabled { None } else { Some(Vec::new()) });
    }

    /// Sets the union of logical surface rectangles that accept pointer input.
    ///
    /// # Panics
    /// Panics when called after the configured shell has lost its owned overlay surface.
    pub fn set_input_rects(&mut self, rects: Option<Vec<[i32; 4]>>) -> bool {
        if self.input_rects == rects {
            return false;
        }
        let surface = self
            .state
            .overlay_surface
            .as_ref()
            .expect("configured shell owns an overlay surface");
        if rects.is_none() {
            surface.set_input_region(None);
        } else if let Ok(region) = Region::new(&self.state.compositor) {
            for [x, y, width, height] in rects.as_ref().into_iter().flatten() {
                if *width > 0 && *height > 0 {
                    region.add(*x, *y, *width, *height);
                }
            }
            surface.set_input_region(Some(region.wl_region()));
        } else {
            return false;
        }
        self.input_rects = rects;
        if self.state.configured {
            surface.commit();
        }
        true
    }

    /// Selects a compositor-provided cursor shape while the pointer is over this surface.
    pub fn set_cursor(&self, style: CursorStyle) {
        let Some(serial) = self.state.pointer_enter_serial else {
            return;
        };
        let Some(device) = &self.state.cursor_device else {
            return;
        };
        let shape = match style {
            CursorStyle::Default => Shape::Default,
            CursorStyle::Move => Shape::Move,
            CursorStyle::Grab => Shape::Grab,
            CursorStyle::Grabbing => Shape::Grabbing,
            CursorStyle::Resize => Shape::NwseResize,
        };
        device.set_shape(serial, shape);
    }
}
