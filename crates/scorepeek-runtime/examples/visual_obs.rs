use scorepeek_overlay_web_host::bridge::data::{Backend, Config};
use scorepeek_runtime::config::control::Controller;
use std::io::Write as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let config_path = args
        .next()
        .ok_or("usage: visual_obs CONFIG.toml [LISTEN] [SOURCE_CONFIG.toml]")?;
    let listen_text = args.next().map_or_else(
        || Ok::<_, Box<dyn std::error::Error>>("127.0.0.1:17384".to_owned()),
        |value| {
            value
                .into_string()
                .map_err(|_| "LISTEN must be UTF-8".into())
        },
    )?;
    let listen: std::net::SocketAddr = listen_text.parse()?;
    if !listen.ip().is_loopback() {
        return Err("LISTEN must use a loopback address".into());
    }
    let source_path = args.next();
    if args.next().is_some() {
        return Err("usage: visual_obs CONFIG.toml [LISTEN] [SOURCE_CONFIG.toml]".into());
    }
    let config_path = std::path::PathBuf::from(config_path);
    let parent = config_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent)?;
    let document = if let Some(source_path) = source_path {
        toml::from_str(&std::fs::read_to_string(source_path)?)?
    } else {
        scorepeek_overlay_web_host::config::OverlayConfig::initial()
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
        backend: Backend::Obs,
        canvases: document
            .canvases
            .into_iter()
            .filter(|canvas| canvas.backend == Backend::Obs)
            .collect(),
        config_path,
        control_socket: controller.path().to_owned(),
        skin_store: scorepeek_overlay_web_host::skin::StoreRoot::discover()
            .path()
            .to_owned(),
        socket: std::env::temp_dir().join(format!(
            "scorepeek-visual-absent-{}.sock",
            std::process::id()
        )),
        invocation: "overlay-visual-debug".into(),
        scores_db: None,
        listen,
        unknown_grace_ms: document.unknown_grace_ms,
        edit_on_start: false,
    };
    eprintln!("Open http://{listen}/overlay and press Enter to stop.");
    scorepeek_overlay_web_host::run(config, std::io::stdin())?;
    drop(controller);
    Ok(())
}
