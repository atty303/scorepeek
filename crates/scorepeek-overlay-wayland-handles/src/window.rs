use crate::SurfaceHandle;
use smithay_client_toolkit::shell::WaylandSurface as _;

impl SurfaceHandle {
    pub fn set_geometry(&self, x: i32, y: i32, width: u32, height: u32, commit: bool) {
        self.layer.set_margin(y, 0, 0, x);
        self.layer.set_size(width, height);
        if commit {
            self.layer.commit();
        }
    }
}
