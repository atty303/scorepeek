use scorepeek_overlay::{
    control::Controller,
    runtime::{Backend, Config},
};
use std::{io::Write as _, time::Duration};

struct TimedLease(Duration);

impl std::io::Read for TimedLease {
    fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
        std::thread::sleep(self.0);
        Ok(0)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let config_path = args
        .next()
        .ok_or("usage: visual_wayland CONFIG.toml [SECONDS]")?;
    let seconds = args.next().map_or(Ok(30_u64), |value| {
        value
            .to_str()
            .ok_or("SECONDS must be UTF-8")?
            .parse::<u64>()
            .map_err(|error| format!("SECONDS: {error}"))
    })?;
    let source_config = args.next();
    if seconds == 0 || args.next().is_some() {
        return Err("usage: visual_wayland CONFIG.toml [SECONDS>0] [SOURCE_CONFIG.toml]".into());
    }
    let config_path = std::path::PathBuf::from(config_path);
    let parent = config_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent)?;
    let document = if let Some(source_config) = source_config {
        let bytes = std::fs::read(&source_config)?;
        let document: scorepeek_overlay::config::OverlayConfig = toml::from_slice(&bytes)?;
        if document.schema_version != scorepeek_overlay::config::SCHEMA_VERSION {
            return Err("SOURCE_CONFIG.toml must already use the current schema".into());
        }
        document.validated()?;
        document
    } else {
        scorepeek_overlay::config::OverlayConfig::initial()
    };
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&config_path)
        .map_err(|error| format!("CONFIG.toml must not already exist: {error}"))?;
    file.write_all(toml::to_string_pretty(&document)?.as_bytes())?;
    file.sync_all()?;
    let controller = Controller::start(&config_path, document.clone())?;
    let config = Config {
        backend: Backend::Wayland,
        canvases: document
            .canvases
            .iter()
            .filter(|canvas| canvas.backend == Backend::Wayland)
            .cloned()
            .collect(),
        config_path,
        control_socket: controller.path().to_owned(),
        skin_store: scorepeek_overlay::skin::StoreRoot::discover()
            .path()
            .to_owned(),
        socket: std::env::temp_dir().join(format!(
            "scorepeek-visual-absent-{}.sock",
            std::process::id()
        )),
        invocation: "overlay-visual-wayland".into(),
        scores_db: None,
        listen: document.obs_listen.parse()?,
        unknown_grace_ms: document.unknown_grace_ms,
        wayland_refresh_hz: document.wayland_refresh_hz,
        edit_on_start: true,
    };
    eprintln!("Wayland editor will remain open for {seconds} seconds.");
    scorepeek_overlay::native::run(config, TimedLease(Duration::from_secs(seconds)))?;
    drop(controller);
    Ok(())
}
