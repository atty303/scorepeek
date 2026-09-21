mod dispatch;
mod role;

pub use dispatch::dispatch;
pub(crate) use role::{WAYLAND_ENTRYPOINT, WEB_ENTRYPOINT};
