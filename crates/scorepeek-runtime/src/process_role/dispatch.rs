use super::role::{WAYLAND_ENTRYPOINT, WEB_ENTRYPOINT};
use std::{ffi::OsString, process::ExitCode};

pub fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    let role = match arguments {
        [_, role] => role.to_str()?,
        _ => return None,
    };
    let result = match role {
        WAYLAND_ENTRYPOINT => scorepeek_overlay_wayland::diagnostics::run(),
        WEB_ENTRYPOINT => scorepeek_overlay_web_host::diagnostics::run(),
        _ => return None,
    };
    Some(crate::service::dispatch::exit_for_result(result))
}
