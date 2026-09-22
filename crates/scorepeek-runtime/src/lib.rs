extern crate self as scorepeek;

#[allow(dead_code)]
pub mod capture;
pub mod config;
#[allow(dead_code)]
pub mod diagnostics;
pub mod events;
mod inventory;
pub mod overlay;
pub mod platform;
mod process_role;
pub mod recording;
pub mod resources;
pub mod scores;
#[allow(
    clippy::missing_errors_doc,
    dead_code,
    reason = "offline canonical replay shares the binary's internal run-event reducer"
)]
mod service;
pub(crate) use capture::live as capture_live;
pub(crate) use recording::artifact as recognition_artifact;
pub(crate) use recording::source as canonical_source;
pub use service::ServiceHandle;

#[must_use]
pub fn dispatch_private_role(arguments: &[std::ffi::OsString]) -> Option<std::process::ExitCode> {
    process_role::dispatch(arguments)
}

use std::fs;
use std::io::Write as _;
use std::path::Path;

fn publish_private_file(output: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = output.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "output has no parent")
    })?;
    let mut staging = tempfile::Builder::new()
        .prefix(".scorepeek-private-staging-")
        .tempfile_in(parent)?;
    staging.as_file_mut().write_all(bytes)?;
    staging.as_file_mut().sync_all()?;
    let staging_path = staging.path().to_owned();
    fs::hard_link(&staging_path, output)?;
    if let Err(error) = fs::remove_file(&staging_path) {
        let _ = fs::remove_file(output);
        return Err(error);
    }
    fs::File::open(parent)?.sync_all()
}
