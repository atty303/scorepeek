//! Wayland ownership boundary: no destroy-capable surface proxies escape this crate.
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle, WindowHandle,
};
use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    delegate_registry,
    output::{OutputHandler, OutputInfo, OutputState},
    reexports::{
        calloop::{EventLoop, ping::PingSource},
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
};
use std::{
    num::NonZeroU32,
    ptr::NonNull,
    sync::Arc,
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_output, wl_surface},
};
use wayland_protocols::{
    wp::fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        wp_fractional_scale_v1::{self, WpFractionalScaleV1},
    },
    wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};

/// Keeps both the private layer surface and its connection alive for GPU borrows.
pub struct SurfaceHandle {
    layer: LayerSurface,
    connection: Connection,
}
impl HasDisplayHandle for SurfaceHandle {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let ptr = NonNull::new(self.connection.backend().display_ptr().cast())
            .ok_or(HandleError::Unavailable)?;
        // The private connection remains alive throughout this borrow.
        Ok(unsafe {
            DisplayHandle::borrow_raw(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(ptr)))
        })
    }
}
impl HasWindowHandle for SurfaceHandle {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        use wayland_client::Proxy as _;
        let ptr = NonNull::new(self.layer.wl_surface().id().as_ptr().cast())
            .ok_or(HandleError::Unavailable)?;
        // Only the private LayerSurface owns destruction authority. Its Arc
        // remains alive throughout this borrow, even after Shell is dropped.
        Ok(unsafe {
            WindowHandle::borrow_raw(RawWindowHandle::Wayland(WaylandWindowHandle::new(ptr)))
        })
    }
}
#[derive(Clone, Copy)]
pub enum Event {
    Configure {
        logical: [u32; 2],
        physical: [u32; 2],
        scale_120: u32,
    },
    Frame,
    Wake,
    PointerMotion {
        x: f64,
        y: f64,
    },
    PointerButton {
        button: u32,
        pressed: bool,
        x: f64,
        y: f64,
    },
    Closed,
}
pub struct Shell {
    owner: Arc<SurfaceHandle>,
    state: Platform,
    event_loop: EventLoop<'static, Platform>,
    started: Instant,
    pub output_name: Option<String>,
    pub available_outputs: Vec<String>,
    pub position: [i32; 2],
    pub output_logical_size: Option<[u32; 2]>,
    pub fractional_scaling: bool,
}
impl Shell {
    /// Creates an interactive layer on an unambiguous output.
    /// # Errors
    /// Returns connection, global binding, output selection or event-loop failures.
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

        let mut app = Platform {
            qh: qh.clone(),
            registry_state: RegistryState::new(&globals),
            output_state: OutputState::new(&globals, &qh),
            owner: None,
            selected_output: None,
            width,
            height,
            scale: 1,
            fractional_scale: None,
            fractional: None,
            viewport: None,
            configured: false,
            needs_configure: false,
            frame_pending: false,
            fallback: [width, height],
            events: Vec::new(),
            failure: None,
            seat: None,
            pointer: None,
            pointer_position: [0.0, 0.0],
        };

        event_queue.roundtrip(&mut app).map_err(|e| e.to_string())?;
        app.seat = globals
            .bind::<wayland_client::protocol::wl_seat::WlSeat, _, _>(&qh, 1..=9, ())
            .ok();
        let selection = select_output(&app.output_state, output)?;
        let available_outputs = app
            .output_state
            .outputs()
            .filter_map(|output| app.output_state.info(&output).and_then(|info| info.name))
            .collect();
        let output_logical_size = selection.info.logical_size.and_then(|(width, height)| {
            Some([u32::try_from(width).ok()?, u32::try_from(height).ok()?])
        });
        app.selected_output = Some(selection.output.clone());

        let resolved_x = if upper_right {
            selection
                .info
                .logical_size
                .and_then(|(output_width, _)| upper_right_x(output_width, width, x))
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
        layer.set_anchor(Anchor::TOP | Anchor::LEFT);
        layer.set_margin(y, 0, 0, resolved_x);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(width, height);
        app.scale = selection.info.scale_factor.max(1);
        layer.commit();

        app.owner = Some(Arc::new(SurfaceHandle {
            layer,
            connection: conn.clone(),
        }));

        let event_loop: EventLoop<Platform> = EventLoop::try_new().map_err(|e| e.to_string())?;
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
            position: [resolved_x, y],
            output_logical_size,
            fractional_scaling: app.viewport.is_some(),
            state: app,
            event_loop,
            started: Instant::now(),
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
        if !self.state.configured && self.started.elapsed() >= Duration::from_secs(5) {
            return Err("configure_timeout".into());
        }
        if self.state.needs_configure && self.state.configured {
            self.state.needs_configure = false;
            let s = &self.state;
            let owner = &self.owner;
            owner.connection.roundtrip().map_err(|e| e.to_string())?;
            let scale_120 = s.fractional_scale.unwrap_or(s.scale.cast_unsigned() * 120);
            owner
                .layer
                .wl_surface()
                .set_buffer_scale(if s.viewport.is_some() { 1 } else { s.scale });
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
    pub fn request_frame(&mut self) {
        if !self.state.frame_pending {
            let surface = self.owner.layer.wl_surface();
            surface.frame(&self.state.qh, FrameCallbackData(surface.clone()));
            self.state.frame_pending = true;
        }
    }
    pub fn set_geometry(&mut self, x: i32, y: i32, width: u32, height: u32) {
        self.state.width = width.max(32);
        self.state.height = height.max(32);
        self.owner.layer.set_margin(y, 0, 0, x);
        self.owner
            .layer
            .set_size(self.state.width, self.state.height);
        self.owner.layer.commit();
    }
}

fn upper_right_x(output_width: i32, surface_width: u32, inset: i32) -> Option<i32> {
    output_width
        .checked_sub(i32::try_from(surface_width).ok()?)?
        .checked_sub(inset.max(0))
}
struct Platform {
    qh: QueueHandle<Self>,
    registry_state: RegistryState,
    output_state: OutputState,
    owner: Option<Arc<SurfaceHandle>>,
    selected_output: Option<wl_output::WlOutput>,
    width: u32,
    height: u32,
    scale: i32,
    fractional_scale: Option<u32>,
    fractional: Option<WpFractionalScaleV1>,
    viewport: Option<WpViewport>,
    configured: bool,
    needs_configure: bool,
    frame_pending: bool,
    fallback: [u32; 2],
    events: Vec<Event>,
    failure: Option<String>,
    seat: Option<wayland_client::protocol::wl_seat::WlSeat>,
    pointer: Option<wayland_client::protocol::wl_pointer::WlPointer>,
    pointer_position: [f64; 2],
}

impl wayland_client::Dispatch<wayland_client::protocol::wl_seat::WlSeat, ()> for Platform {
    fn event(
        state: &mut Self,
        seat: &wayland_client::protocol::wl_seat::WlSeat,
        event: wayland_client::protocol::wl_seat::Event,
        (): &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wayland_client::protocol::wl_seat::Event::Capabilities { capabilities } = event
            && capabilities.into_result().is_ok_and(|caps| {
                caps.contains(wayland_client::protocol::wl_seat::Capability::Pointer)
            })
            && state.pointer.is_none()
        {
            state.pointer = Some(seat.get_pointer(qh, ()));
        }
    }
}

impl wayland_client::Dispatch<wayland_client::protocol::wl_pointer::WlPointer, ()> for Platform {
    fn event(
        state: &mut Self,
        _: &wayland_client::protocol::wl_pointer::WlPointer,
        event: wayland_client::protocol::wl_pointer::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_pointer;
        match event {
            wl_pointer::Event::Enter {
                surface_x,
                surface_y,
                ..
            }
            | wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                state.pointer_position = [surface_x, surface_y];
                state.events.push(Event::PointerMotion {
                    x: surface_x,
                    y: surface_y,
                });
            }
            wl_pointer::Event::Button {
                button,
                state: button_state,
                ..
            } => {
                if let Ok(button_state) = button_state.into_result() {
                    state.events.push(Event::PointerButton {
                        button,
                        pressed: button_state == wl_pointer::ButtonState::Pressed,
                        x: state.pointer_position[0],
                        y: state.pointer_position[1],
                    });
                }
            }
            _ => {}
        }
    }
}
impl CompositorHandler for Platform {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        if self
            .owner
            .as_ref()
            .is_some_and(|layer| layer.layer.wl_surface() == surface)
        {
            self.scale = new_factor.max(1);

            self.needs_configure = true;
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        let is_overlay = self
            .owner
            .as_ref()
            .is_some_and(|layer| layer.layer.wl_surface() == surface);
        if !is_overlay {
            return;
        }
        self.frame_pending = false;
        self.events.push(Event::Frame);
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for Platform {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if self.selected_output.as_ref() == Some(&output) {
            self.failure = Some("output_removed".into());
        }
    }
}

impl LayerShellHandler for Platform {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        self.events.push(Event::Closed);
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let fallback = self.fallback;
        self.width = NonZeroU32::new(configure.new_size.0).map_or(fallback[0], NonZeroU32::get);
        self.height = NonZeroU32::new(configure.new_size.1).map_or(fallback[1], NonZeroU32::get);
        self.configured = true;
        self.needs_configure = true;
    }
}

delegate_registry!(Platform);

impl ProvidesRegistryState for Platform {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState];
}

smithay_client_toolkit::delegate_dispatch2!(Platform);

struct ScaleData;

impl Dispatch2<WpFractionalScaleV1, Platform> for ScaleData {
    fn event(
        &self,
        state: &mut Platform,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &Connection,
        _: &QueueHandle<Platform>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.fractional_scale = Some(scale.max(1));
            state.needs_configure = state.configured;
        }
    }
}

macro_rules! no_scale_events {
    ($($proxy:ty),+) => { $(
        impl Dispatch2<$proxy, Platform> for ScaleData {
            fn event(&self, _: &mut Platform, _: &$proxy, _: <$proxy as wayland_client::Proxy>::Event, _: &Connection, _: &QueueHandle<Platform>) {}
        }
    )+ };
}
no_scale_events!(WpFractionalScaleManagerV1, WpViewporter, WpViewport);

fn scaled_size(logical: u32, scale_120: u32) -> u32 {
    u32::try_from((u64::from(logical) * u64::from(scale_120)).div_ceil(120)).unwrap_or(u32::MAX)
}

struct SelectedOutput {
    output: wl_output::WlOutput,
    info: OutputInfo,
}

fn select_output(state: &OutputState, requested: Option<&str>) -> Result<SelectedOutput, String> {
    let outputs: Vec<_> = state
        .outputs()
        .filter_map(|output| {
            state
                .info(&output)
                .map(|info| SelectedOutput { output, info })
        })
        .collect();

    match requested {
        Some(name) => outputs
            .into_iter()
            .find(|candidate| candidate.info.name.as_deref() == Some(name))
            .ok_or_else(|| format!("Wayland output not found: {name}")),
        None if outputs.len() == 1 => Ok(outputs.into_iter().next().expect("length checked")),
        None if outputs.is_empty() => Err("no Wayland output".into()),
        None => Err("multiple Wayland outputs; set canvas.output in overlay TOML".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{scaled_size, upper_right_x};
    #[test]
    fn integer_and_fractional_buffers_round_up() {
        assert_eq!(scaled_size(1920, 120), 1920);
        assert_eq!(scaled_size(960, 240), 1920);
        assert_eq!(scaled_size(1001, 150), 1252);
    }

    #[test]
    fn upper_right_position_uses_logical_output_width_and_inset() {
        assert_eq!(upper_right_x(1920, 560, 20), Some(1340));
        assert_eq!(upper_right_x(1920, 560, -20), Some(1360));
        assert_eq!(upper_right_x(100, 560, 20), Some(-480));
    }
}
