use crate::SurfaceHandle;
use smithay_client_toolkit::shell::{
    WaylandSurface as _,
    wlr_layer::{Anchor, KeyboardInteractivity},
};

impl SurfaceHandle {
    /// Detaches the current buffer and flushes the Wayland connection.
    ///
    /// # Errors
    /// Returns the Wayland connection error when the flush fails.
    pub fn unmap(&self) -> Result<(), String> {
        let surface = self.layer.wl_surface();
        surface.attach(None, 0, 0);
        surface.commit();
        self.connection.flush().map_err(|error| error.to_string())
    }

    pub fn set_keyboard_enabled(&self, enabled: bool) {
        self.layer.set_keyboard_interactivity(if enabled {
            KeyboardInteractivity::Exclusive
        } else {
            KeyboardInteractivity::None
        });
        self.layer.commit();
    }

    pub fn set_buffer_scale(&self, scale: i32) {
        self.layer.wl_surface().set_buffer_scale(scale);
    }

    pub fn begin_remap(&self, position: [i32; 2], size: [u32; 2], keyboard: bool) {
        self.layer.set_anchor(Anchor::TOP | Anchor::LEFT);
        self.layer.set_margin(position[1], 0, 0, position[0]);
        self.layer.set_exclusive_zone(-1);
        self.layer.set_keyboard_interactivity(if keyboard {
            KeyboardInteractivity::Exclusive
        } else {
            KeyboardInteractivity::None
        });
        self.layer.set_size(size[0], size[1]);
        self.layer.commit();
    }
}
