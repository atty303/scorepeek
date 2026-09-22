//! Safe Wayland lifecycle, window, output, and input adapter.
use crate::input::keyboard as input;
pub use crate::input::keyboard::{TextCommand, TextInputState, TextUpdate};
pub use crate::window::scale::OutputDescription;
use raw_wayland_handles::SurfaceHandle;
use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_registry,
    output::{OutputHandler, OutputInfo, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::wlr_layer::{LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
};
use std::{io::Write as _, num::NonZeroU32, os::fd::AsFd as _, sync::Arc, time::Instant};
use wayland_client::{
    Connection, QueueHandle,
    protocol::{wl_buffer, wl_callback, wl_output, wl_shm, wl_shm_pool, wl_surface},
};
use wayland_protocols::{
    wp::cursor_shape::v1::client::{
        wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1},
        wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
    },
    wp::fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        wp_fractional_scale_v1::{self, WpFractionalScaleV1},
    },
    wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorStyle {
    Default,
    Move,
    Grab,
    Grabbing,
    Resize,
}

#[derive(Clone)]
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
    PointerScroll {
        dx: f64,
        dy: f64,
        x: f64,
        y: f64,
    },
    Text(TextCommand),
    Ime(TextUpdate),
    KeyboardFocus(bool),
    Closed,
}
pub(crate) struct Platform {
    pub(crate) qh: QueueHandle<Self>,
    pub(crate) registry_state: RegistryState,
    pub(crate) output_state: OutputState,
    pub(crate) owner: Option<Arc<SurfaceHandle>>,
    pub(crate) overlay_surface: Option<wl_surface::WlSurface>,
    pub(crate) selected_output: Option<wl_output::WlOutput>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) scale: i32,
    pub(crate) fractional_scale: Option<u32>,
    pub(crate) fractional: Option<WpFractionalScaleV1>,
    pub(crate) viewport: Option<WpViewport>,
    pub(crate) configured: bool,
    pub(crate) needs_configure: bool,
    pub(crate) frame_callback: Option<wl_callback::WlCallback>,
    pub(crate) fallback: [u32; 2],
    pub(crate) events: Vec<Event>,
    pub(crate) failure: Option<String>,
    pub(crate) input: input::Input,
    pub(crate) pointer: Option<wayland_client::protocol::wl_pointer::WlPointer>,
    pub(crate) pointer_position: [f64; 2],
    pub(crate) cursor_manager: Option<WpCursorShapeManagerV1>,
    pub(crate) cursor_device: Option<WpCursorShapeDeviceV1>,
    pub(crate) pointer_enter_serial: Option<u32>,
    pub(crate) compositor: CompositorState,
    pub(crate) cursor_surface: Option<wl_surface::WlSurface>,
    pub(crate) cursor_buffer: Option<wl_buffer::WlBuffer>,
    pub(crate) cursor_pool: Option<wl_shm_pool::WlShmPool>,
    pub(crate) cursor_file: Option<std::fs::File>,
}

impl Platform {
    pub(crate) fn is_overlay_surface(&self, surface: &wl_surface::WlSurface) -> bool {
        self.overlay_surface.as_ref() == Some(surface)
    }
}

impl wayland_client::Dispatch<wayland_client::protocol::wl_pointer::WlPointer, ()> for Platform {
    fn event(
        state: &mut Self,
        pointer: &wayland_client::protocol::wl_pointer::WlPointer,
        event: wayland_client::protocol::wl_pointer::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_pointer;
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface_x,
                surface_y,
                ..
            } => {
                state.pointer_enter_serial = Some(serial);
                if let Some(device) = &state.cursor_device {
                    device.set_shape(serial, Shape::Default);
                } else if let Some(surface) = &state.cursor_surface {
                    pointer.set_cursor(serial, Some(surface), 2, 2);
                }
                state.pointer_position = [surface_x, surface_y];
                state.events.push(Event::PointerMotion {
                    x: surface_x,
                    y: surface_y,
                });
            }
            wl_pointer::Event::Motion {
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
            wl_pointer::Event::Leave { .. } => state.pointer_enter_serial = None,
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
            wl_pointer::Event::Axis { axis, value, .. } => {
                if let Ok(axis) = axis.into_result() {
                    let Some([dx, dy]) = pointer_scroll_delta(axis, value) else {
                        return;
                    };
                    state.events.push(Event::PointerScroll {
                        dx,
                        dy,
                        x: state.pointer_position[0],
                        y: state.pointer_position[1],
                    });
                }
            }
            _ => {}
        }
    }
}

fn pointer_scroll_delta(
    axis: wayland_client::protocol::wl_pointer::Axis,
    value: f64,
) -> Option<[f64; 2]> {
    use wayland_client::protocol::wl_pointer::Axis;
    match axis {
        Axis::HorizontalScroll => Some([-value, 0.0]),
        Axis::VerticalScroll => Some([0.0, -value]),
        _ => None,
    }
}

impl wayland_client::Dispatch<WpCursorShapeManagerV1, ()> for Platform {
    fn event(
        _: &mut Self,
        _: &WpCursorShapeManagerV1,
        _: <WpCursorShapeManagerV1 as wayland_client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl wayland_client::Dispatch<WpCursorShapeDeviceV1, ()> for Platform {
    fn event(
        _: &mut Self,
        _: &WpCursorShapeDeviceV1,
        _: <WpCursorShapeDeviceV1 as wayland_client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl wayland_client::Dispatch<wl_shm::WlShm, ()> for Platform {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl wayland_client::Dispatch<wl_shm_pool::WlShmPool, ()> for Platform {
    fn event(
        _: &mut Self,
        _: &wl_shm_pool::WlShmPool,
        _: wl_shm_pool::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl wayland_client::Dispatch<wl_buffer::WlBuffer, ()> for Platform {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        _: wl_buffer::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

pub(crate) fn fallback_cursor(
    globals: &wayland_client::globals::GlobalList,
    qh: &QueueHandle<Platform>,
    compositor: &CompositorState,
) -> Option<(
    wl_surface::WlSurface,
    wl_buffer::WlBuffer,
    wl_shm_pool::WlShmPool,
    std::fs::File,
)> {
    const SIZE: usize = 24;
    let shm = globals.bind::<wl_shm::WlShm, _, _>(qh, 1..=1, ()).ok()?;
    let path = std::env::temp_dir().join(format!(
        "scorepeek-cursor-{}-{}",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&path)
        .ok()?;
    let mut pixels = vec![0_u8; SIZE * SIZE * 4];
    for y in 1..21 {
        for x in 1..=y.min(12) {
            let edge = x == 1 || x == y.min(12) || y == 20;
            let color = if edge {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            };
            let offset = (y * SIZE + x) * 4;
            pixels[offset..offset + 4].copy_from_slice(&color);
        }
    }
    if file.write_all(&pixels).and_then(|()| file.flush()).is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
        return None;
    }
    let _ = std::fs::remove_file(&path);
    let pool = shm.create_pool(file.as_fd(), i32::try_from(pixels.len()).ok()?, qh, ());
    let buffer = pool.create_buffer(0, 24, 24, 96, wl_shm::Format::Argb8888, qh, ());
    let surface = compositor.create_surface(qh);
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 24, 24);
    surface.commit();
    Some((surface, buffer, pool, file))
}
impl CompositorHandler for Platform {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        if self.is_overlay_surface(surface) {
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
        let is_overlay = self.is_overlay_surface(surface);
        if !is_overlay {
            return;
        }
        self.frame_callback = None;
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

    registry_handlers![OutputState, smithay_client_toolkit::seat::SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(Platform);

pub(crate) struct ScaleData;

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

pub(crate) struct SelectedOutput {
    pub(crate) output: wl_output::WlOutput,
    pub(crate) info: OutputInfo,
}

pub(crate) fn select_output(
    state: &OutputState,
    requested: Option<&str>,
    required: [u32; 2],
) -> Result<SelectedOutput, String> {
    let mut outputs: Vec<_> = state
        .outputs()
        .filter_map(|output| {
            state
                .info(&output)
                .map(|info| SelectedOutput { output, info })
        })
        .collect();

    let index = choose_output_index(
        requested,
        outputs.iter().map(|candidate| {
            let size = candidate.info.logical_size.and_then(|(width, height)| {
                Some([u32::try_from(width).ok()?, u32::try_from(height).ok()?])
            });
            (candidate.info.name.as_deref(), size)
        }),
        required,
    )?;
    Ok(outputs.swap_remove(index))
}

fn choose_output_index<'a>(
    requested: Option<&str>,
    outputs: impl IntoIterator<Item = (Option<&'a str>, Option<[u32; 2]>)>,
    required: [u32; 2],
) -> Result<usize, String> {
    let outputs = outputs.into_iter().collect::<Vec<_>>();
    if let Some(requested) = requested
        && let Some(index) = outputs
            .iter()
            .position(|(name, _)| *name == Some(requested))
    {
        return Ok(index);
    }
    if let Some((index, _)) = outputs
        .iter()
        .enumerate()
        .filter_map(|(index, (name, size))| Some((index, ((*name)?, (*size)?))))
        .filter(|(_, (_, size))| size[0] >= required[0] && size[1] >= required[1])
        .min_by_key(|(index, (name, _))| (*name, *index))
    {
        return Ok(index);
    }
    outputs
        .iter()
        .enumerate()
        .filter_map(|(index, (name, size))| Some((index, (*name)?, (*size)?)))
        .max_by_key(|(index, name, size)| {
            (
                u64::from(size[0]) * u64::from(size[1]),
                std::cmp::Reverse(*name),
                std::cmp::Reverse(*index),
            )
        })
        .map(|(index, _, _)| index)
        .ok_or_else(|| "no named Wayland output".into())
}

#[cfg(test)]
mod tests {
    use super::{choose_output_index, pointer_scroll_delta};
    use crate::window::surface::{LayerStateTarget, apply_layer_state};
    use std::cell::RefCell;

    #[derive(Debug, Eq, PartialEq)]
    enum LayerRequest {
        AnchorTopLeft,
        Position(i32, i32),
        NonExclusive,
        Keyboard(bool),
        LogicalSize(u32, u32),
    }

    #[derive(Default)]
    struct RecordingLayer {
        requests: RefCell<Vec<LayerRequest>>,
    }

    impl LayerStateTarget for RecordingLayer {
        fn set_top_left_anchor(&self) {
            self.requests.borrow_mut().push(LayerRequest::AnchorTopLeft);
        }

        fn set_position(&self, x: i32, y: i32) {
            self.requests
                .borrow_mut()
                .push(LayerRequest::Position(x, y));
        }

        fn set_non_exclusive(&self) {
            self.requests.borrow_mut().push(LayerRequest::NonExclusive);
        }

        fn set_keyboard_enabled(&self, enabled: bool) {
            self.requests
                .borrow_mut()
                .push(LayerRequest::Keyboard(enabled));
        }

        fn set_logical_size(&self, width: u32, height: u32) {
            self.requests
                .borrow_mut()
                .push(LayerRequest::LogicalSize(width, height));
        }
    }

    #[test]
    fn remap_layer_state_restores_every_double_buffered_request() {
        let layer = RecordingLayer::default();
        apply_layer_state(&layer, [120, 48], [560, 1040], true);
        assert_eq!(
            *layer.requests.borrow(),
            [
                LayerRequest::AnchorTopLeft,
                LayerRequest::Position(120, 48),
                LayerRequest::NonExclusive,
                LayerRequest::Keyboard(true),
                LayerRequest::LogicalSize(560, 1040),
            ]
        );
    }

    #[test]
    fn wayland_axis_uses_the_native_dom_scroll_direction() {
        use wayland_client::protocol::wl_pointer::Axis;
        assert_eq!(
            pointer_scroll_delta(Axis::VerticalScroll, 15.0),
            Some([0.0, -15.0])
        );
        assert_eq!(
            pointer_scroll_delta(Axis::HorizontalScroll, -5.0),
            Some([5.0, 0.0])
        );
    }

    #[test]
    fn unspecified_output_chooses_a_stable_visible_default() {
        assert_eq!(
            choose_output_index(
                None,
                [
                    (Some("HDMI-A-1"), Some([1920, 1080])),
                    (Some("DP-3"), Some([1920, 1080]))
                ],
                [560, 1040]
            ),
            Ok(1)
        );
        assert_eq!(
            choose_output_index(
                Some("HDMI-A-1"),
                [
                    (Some("DP-3"), Some([1920, 1080])),
                    (Some("HDMI-A-1"), Some([1920, 1080]))
                ],
                [560, 1040]
            ),
            Ok(1)
        );
        assert_eq!(
            choose_output_index(
                Some("disconnected"),
                [
                    (Some("HDMI-A-1"), Some([1920, 1080])),
                    (Some("DP-3"), Some([1920, 1080]))
                ],
                [560, 1040]
            ),
            Ok(1)
        );
        assert_eq!(
            choose_output_index(
                None,
                [(None, None), (Some("DP-3"), Some([1920, 1080]))],
                [560, 1040]
            ),
            Ok(1)
        );
        assert_eq!(
            choose_output_index(
                Some("gone"),
                [
                    (Some("small"), Some([1280, 720])),
                    (Some("large"), Some([1920, 1080]))
                ],
                [2000, 1200]
            ),
            Ok(1)
        );
        assert!(choose_output_index(None, [], [560, 1040]).is_err());
        assert!(choose_output_index(None, [(None, None)], [560, 1040]).is_err());
    }
}
