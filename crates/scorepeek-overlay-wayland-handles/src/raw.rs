//! The only unsafe boundary for borrowed raw Wayland handles.

use crate::SurfaceHandle;
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle, WindowHandle,
};
use smithay_client_toolkit::shell::WaylandSurface as _;
use std::ptr::NonNull;

impl HasDisplayHandle for SurfaceHandle {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let pointer = NonNull::new(self.connection.backend().display_ptr().cast())
            .ok_or(HandleError::Unavailable)?;
        // SurfaceHandle keeps the private connection alive for this borrow.
        Ok(unsafe {
            DisplayHandle::borrow_raw(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
                pointer,
            )))
        })
    }
}

impl HasWindowHandle for SurfaceHandle {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        use wayland_client::Proxy as _;

        let pointer = NonNull::new(self.layer.wl_surface().id().as_ptr().cast())
            .ok_or(HandleError::Unavailable)?;
        // SurfaceHandle is the sole destroy authority and outlives this borrow.
        Ok(unsafe {
            WindowHandle::borrow_raw(RawWindowHandle::Wayland(WaylandWindowHandle::new(pointer)))
        })
    }
}
