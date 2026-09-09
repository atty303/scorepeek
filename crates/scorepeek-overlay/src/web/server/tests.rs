use super::*;
use crate::{config::OverlayConfig, control::Controller, runtime::Backend};
use serde_json::{Value, json};
use std::{
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpStream},
    path::PathBuf,
};

struct Client(TcpStream);
impl Client {
    fn connect(address: SocketAddr, version: Option<&str>) -> Self {
        let query = version.map_or_else(String::new, |value| format!("?asset_version={value}"));
        Self::connect_path(address, &format!("/ws/stage{query}"))
    }
    fn connect_path(address: SocketAddr, path: &str) -> Self {
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write!(stream, "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            header.push(byte[0]);
            assert!(header.len() < 4096);
        }
        assert!(header.starts_with(b"HTTP/1.1 101 "));
        Self(stream)
    }
    fn send(&mut self, request: Value, version: Option<&str>) {
        let mut envelope = json!({"request_id": 1});
        envelope["request"] = request;
        if let Some(version) = version {
            envelope["asset_version"] = version.into();
        }
        let payload = serde_json::to_vec(&envelope).unwrap();
        let length = u16::try_from(payload.len()).unwrap();
        let mut frame = vec![0x81, 0xfe];
        frame.extend_from_slice(&length.to_be_bytes());
        frame.extend_from_slice(&[0; 4]);
        frame.extend_from_slice(&payload);
        self.0.write_all(&frame).unwrap();
    }
    fn receive(&mut self) -> Value {
        let mut header = [0; 2];
        self.0.read_exact(&mut header).unwrap();
        assert_eq!(header[0], 0x81);
        assert_eq!(header[1] & 0x80, 0);
        let length = match header[1] & 0x7f {
            126 => {
                let mut bytes = [0; 2];
                self.0.read_exact(&mut bytes).unwrap();
                usize::from(u16::from_be_bytes(bytes))
            }
            127 => {
                let mut bytes = [0; 8];
                self.0.read_exact(&mut bytes).unwrap();
                usize::try_from(u64::from_be_bytes(bytes)).unwrap()
            }
            value => usize::from(value),
        };
        assert!(length < 1_000_000);
        let mut bytes = vec![0; length];
        self.0.read_exact(&mut bytes).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
    fn control(&mut self, request: Value) -> Value {
        self.send(request, Some(ASSET_VERSION));
        loop {
            let message = self.receive();
            if message["type"] == "control" {
                return message["response"].clone();
            }
        }
    }
}

struct TestDirectory(PathBuf);
impl Drop for TestDirectory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

struct Fixture {
    directory: TestDirectory,
    address: SocketAddr,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
    controller: Option<Controller>,
}
impl Fixture {
    async fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let directory = std::env::temp_dir().join(format!(
            "scorepeek-stage-session-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let directory = TestDirectory(directory);
        let skins = crate::skin::StoreRoot::new(directory.0.join("skins"));
        let mut document = OverlayConfig::initial();
        document
            .canvases
            .push(crate::config::empty_canvas("obs-test".into(), Backend::Obs));
        let path = directory.0.join("overlay.toml");
        let controller = Controller::start(&path, document.clone()).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let changed = Arc::new(Notify::new());
        let config = Config {
            backend: Backend::Obs,
            canvases: document.canvases.clone(),
            config_path: path,
            control_socket: controller.path().to_owned(),
            skin_store: skins.path().to_owned(),
            socket: directory.0.join("absent.sock"),
            invocation: "stage-session-test".into(),
            scores_db: None,
            listen: address,
            unknown_grace_ms: document.unknown_grace_ms,
            wayland_refresh_hz: document.wayland_refresh_hz,
            edit_on_start: false,
        };
        let shared = Arc::new(Shared {
            canvases: Mutex::new(
                document
                    .canvases
                    .into_iter()
                    .filter(|canvas| canvas.backend == Backend::Obs)
                    .collect(),
            ),
            control_socket: controller.path().to_owned(),
            feed: Feed::start(config, Arc::new(|| {})).unwrap(),
            changed,
            skins,
        });
        let app = Router::new()
            .route("/ws/stage", get(stage_socket))
            .route("/ws/{id}", get(socket))
            .with_state(Arc::clone(&shared));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            directory,
            address,
            shared,
            task,
            controller: Some(controller),
        }
    }
    fn client(&self) -> Client {
        let mut client = Client::connect(self.address, Some(ASSET_VERSION));
        let message = client.receive();
        assert_eq!(message["type"], "stage");
        assert_eq!(message["asset_version"], ASSET_VERSION);
        client
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.shared.feed.stop.store(true, Ordering::Release);
        self.task.abort();
        drop(self.controller.take());
    }
}

fn acquire() -> Value {
    json!({"command":"acquire_backend", "backend":"obs", "editor_id":"same-client-supplied-id"})
}
fn acquire_after_disconnect(client: &mut Client) -> Value {
    let started = std::time::Instant::now();
    loop {
        let reply = client.control(acquire());
        if reply["readonly"] == false {
            return reply;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "disconnected owner lease was not released promptly"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn assert_version_rejections(fixture: &Fixture) {
    for version in [None, Some("stale-assets")] {
        let mut client = Client::connect(fixture.address, version);
        let message = client.receive();
        assert_eq!(message["type"], "version_mismatch");
        assert_eq!(message["asset_version"], ASSET_VERSION);
        let mut client = fixture.client();
        client.send(acquire(), version);
        assert_eq!(client.receive()["type"], "version_mismatch");
    }
}
fn assert_disconnect_discards_draft(fixture: &Fixture) {
    let mut owner = fixture.client();
    let acquired = owner.control(acquire());
    assert_eq!(acquired["readonly"], false);
    let saved = acquired["canvases"].clone();
    let mut draft = saved.clone();
    draft[0]["width"] = 500.into();
    let updated = owner.control(json!({"command":"update_backend_draft", "backend":"obs", "editor_id":"ignored", "canvases":draft}));
    assert_eq!(updated["ok"], true, "{updated}");
    assert_eq!(updated["dirty"], true);
    let mut observer = fixture.client();
    assert_eq!(observer.control(acquire())["readonly"], true);
    assert_eq!(fixture.shared.canvases.lock().unwrap()[0].width, 500);
    drop(observer);
    assert_eq!(owner.control(acquire())["dirty"], true);
    drop(owner);
    let mut successor = fixture.client();
    let restored = acquire_after_disconnect(&mut successor);
    assert_eq!(restored["canvases"], saved);
    assert_eq!(restored["dirty"], false);
    successor.send(json!({"command":"commit_backend", "backend":"obs", "editor_id":"ignored", "canvases":draft}), Some("stale-assets"));
    loop {
        if successor.receive()["type"] == "version_mismatch" {
            break;
        }
    }
    assert!(
        !fixture.directory.0.join("overlay.toml").exists(),
        "mismatched save reached persistent storage"
    );
    drop(successor);
    let mut fresh = fixture.client();
    assert_eq!(acquire_after_disconnect(&mut fresh)["canvases"], saved);
    assert_eq!(
        fresh.control(json!({"command":"release_backend", "backend":"obs", "editor_id":"ignored"}))
            ["ok"],
        true
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stage_versions_gate_edits_and_disconnect_releases_only_its_lease() {
    let fixture = Fixture::new().await;
    assert_version_rejections(&fixture);
    assert_disconnect_discards_draft(&fixture);

    let id = fixture.shared.canvases.lock().unwrap()[0].id.clone();
    let mut client = Client::connect_path(fixture.address, &format!("/ws/{id}?sample=1"));
    let message = client.receive();
    assert_eq!(message["type"], "state");
    assert_eq!(message["state"]["chart"]["title"], "NEON CIRCUIT");
    assert_eq!(message["state"]["best"]["score"], "2846");
    assert!(
        message["state"]["history"]["plays"]
            .as_array()
            .unwrap()
            .len()
            >= 5
    );
}

#[test]
fn sample_display_state_only_replaces_an_inactive_live_state() {
    let inactive = scorepeek_overlay_ui::OverlayState::default();
    let sample = display_state(inactive.clone(), true);
    assert_ne!(sample, inactive);
    assert!(sample.chart.is_some());

    let mut active = inactive;
    active.system = scorepeek_overlay_ui::LampState::Active;
    assert_eq!(display_state(active.clone(), true), active);
    assert_eq!(display_state(active.clone(), false), active);
}
