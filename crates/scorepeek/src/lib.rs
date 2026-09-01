extern crate self as scorepeek;

pub mod capture;
pub mod catalog;
#[allow(dead_code)]
pub mod diagnostic_control;
#[allow(dead_code)]
pub mod diagnostic_live;
#[allow(dead_code)]
pub mod diagnostic_recording;
#[allow(dead_code)]
pub mod diagnostic_worker;
pub mod model_cache;
pub mod numeric_model_store;
pub mod recognition;
#[allow(dead_code)]
mod recognition_artifact;
pub mod recognition_cadence;
#[allow(dead_code)]
pub mod recognition_live;
pub mod screen_episode;
pub mod song_context;
pub mod temporal_recognition;

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
