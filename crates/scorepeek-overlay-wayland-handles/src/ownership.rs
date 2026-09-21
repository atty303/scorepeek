use smithay_client_toolkit::shell::{WaylandSurface as _, wlr_layer::LayerSurface};
use wayland_client::{Connection, Proxy as _};

/// Sole destroy authority for a private layer surface and its connection.
///
/// Raw display and window handles can only borrow through this owner, so they
/// cannot outlive either Wayland object.
pub struct SurfaceHandle {
    pub(crate) layer: LayerSurface,
    pub(crate) connection: Connection,
}

impl SurfaceHandle {
    /// Binds a surface owner only when both objects belong to the same Wayland backend.
    ///
    /// # Errors
    /// Returns an error for a stale surface or a connection mismatch.
    pub fn new(layer: LayerSurface, connection: Connection) -> Result<Self, String> {
        let surface_backend = layer
            .wl_surface()
            .backend()
            .upgrade()
            .ok_or("Wayland surface backend is no longer available")?;
        if surface_backend != connection.backend() {
            return Err("Wayland surface and display connection do not match".into());
        }
        Ok(Self { layer, connection })
    }
}
