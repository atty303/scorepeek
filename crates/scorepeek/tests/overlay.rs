//! Uses only synthetic inputs and an absent, isolated score database.
#![cfg(feature = "embedded-web")]
use scorepeek_overlay::{
    children::Children,
    runtime::{Backend, Config},
};
use std::{
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    thread,
    time::{Duration, Instant},
};

fn get(address: SocketAddr, path: &str) -> std::io::Result<Vec<u8>> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(100))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )?;
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[test]
fn embedded_assets_and_owned_child_shutdown_without_models_or_database() {
    let temporary = tempfile::tempdir().unwrap();
    let address = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let config = Config {
        backend: Backend::Obs,
        canvases: {
            let mut canvas = scorepeek_overlay::config::OverlayConfig::initial()
                .canvases
                .into_iter()
                .find(|canvas| canvas.id == "obs-selection")
                .unwrap();
            canvas.skin = scorepeek_overlay::Skin::ResultAurora;
            vec![canvas]
        },
        config_path: temporary.path().join("overlay.toml"),
        control_socket: temporary.path().join("absent-control.sock"),
        socket: temporary.path().join("absent.sock"),
        invocation: "test".into(),
        scores_db: None,
        listen: address,
        unknown_grace_ms: 1_000,
        settings_revision: 0,
    };
    let executable = std::env::var_os("SCOREPEEK_TEST_BINARY").map_or_else(
        || Path::new(env!("CARGO_BIN_EXE_scorepeek")).to_path_buf(),
        std::path::PathBuf::from,
    );
    let mut children = Children::default();
    children.start(&executable, &config).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let page = loop {
        assert!(children.poll().is_empty(), "overlay exited before serving");
        if let Ok(page) = get(address, "/canvas/obs-selection") {
            break page;
        }
        assert!(Instant::now() < deadline, "overlay never served index");
        thread::sleep(Duration::from_millis(20));
    };
    let page = String::from_utf8(page).unwrap();
    assert!(page.starts_with("HTTP/1.1 200"));
    assert!(page.contains("text/html"));
    assert!(page.contains(".js"));
    assert!(page.contains("result-aurora"));
    assert!(page.contains("scorepeek-canvas"));
    let stage = String::from_utf8(get(address, "/overlay").unwrap()).unwrap();
    assert!(stage.contains("width:100vw;height:100vh"));
    assert!(stage.starts_with("HTTP/1.1 200"));
    assert!(stage.contains("/ws/stage"));
    assert!(stage.contains("obs-selection"));
    let font = get(address, "/fonts/oxanium.ttf").unwrap();
    assert!(font.starts_with(b"HTTP/1.1 200"));
    assert!(font.windows(8).any(|bytes| bytes == b"font/ttf"));
    assert!(font.ends_with(include_bytes!(
        "../../scorepeek-overlay-ui/assets/fonts/Oxanium.ttf"
    )));

    assert!(
        get(address, "/absent-asset")
            .unwrap()
            .starts_with(b"HTTP/1.1 404")
    );
    assert!(
        String::from_utf8(get(address, "/fonts/OFL.txt").unwrap())
            .unwrap()
            .contains("SIL OPEN FONT LICENSE")
    );
    for name in [
        "cyan-system-frame",
        "result-aurora-frame",
        "dj-blackbox-frame",
        "result-aurora-header",
    ] {
        let image = get(address, &format!("/skins/{name}.png")).unwrap();
        assert!(image.starts_with(b"HTTP/1.1 200"));
        assert!(image.windows(9).any(|bytes| bytes == b"image/png"));
        let expected = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../scorepeek-overlay-ui/assets/skins/{name}.png")),
        )
        .unwrap();
        assert!(image.ends_with(&expected));
    }
    assert!(
        get(address, "/skins/missing.png")
            .unwrap()
            .starts_with(b"HTTP/1.1 404")
    );
    // A conflicting OBS child fails independently; the first remains available.
    children.start(&executable, &config).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while children.poll().is_empty() {
        assert!(
            Instant::now() < deadline,
            "conflicting listener did not fail"
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert!(get(address, "/").unwrap().starts_with(b"HTTP/1.1 200"));
    children.shutdown();
    assert!(get(address, "/").is_err());
    assert!(
        children
            .take_observations()
            .iter()
            .any(|record| record["record"]["operation"] == "child_exit")
    );
    assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 0);
}
