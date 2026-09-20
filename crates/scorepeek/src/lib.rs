extern crate self as scorepeek;

mod application;
mod canonical_recording;
mod canonical_source;
#[allow(dead_code)]
pub mod capture;
mod capture_live;
#[allow(dead_code)]
pub mod catalog;
#[allow(dead_code)]
pub mod diagnostic_live;
#[allow(dead_code)]
pub mod diagnostic_recording;
#[allow(dead_code)]
pub mod diagnostic_stream;
#[allow(dead_code)]
pub mod diagnostic_worker;
pub mod game_version;
mod inventory;
mod live_control;
#[allow(dead_code)]
mod local_profiles;
pub mod model_cache;
#[allow(dead_code)]
pub mod play_attempt;
pub mod recognition;
#[allow(dead_code)]
mod recognition_artifact;
pub mod recognition_cadence;
#[allow(dead_code)]
pub mod recognition_live;
mod recording_simulation;
#[allow(
    clippy::missing_errors_doc,
    dead_code,
    reason = "offline canonical replay shares the binary's internal run-event reducer"
)]
pub mod routine_output;
mod routine_watcher;
pub mod screen_episode;
pub mod song_context;
pub mod temporal_recognition;
pub mod timeline_driver;
mod vulkan_layer;

pub use application::{dev_main, public_main};

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
