//! Raw Wayland handle and surface-lifetime boundary.

mod display;
mod ownership;
mod raw;
mod surface;
mod window;

pub use ownership::SurfaceHandle;
