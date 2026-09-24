//! Typed action bridge to the runtime-owned overlay configuration authority.

use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

pub use scorepeek_overlay::editor::protocol::{
    CONTROL_MESSAGE_MAX_BYTES, Request, Response, decode_message, encode_message,
};

#[cfg(not(test))]
const CONTROL_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(test)]
const CONTROL_TIMEOUT: Duration = Duration::from_millis(100);

/// Sends one typed request to the parent-owned configuration writer.
/// # Errors
/// Returns transport or response decoding errors.
pub fn request(path: &Path, request: &Request) -> Result<Response, String> {
    let mut stream = UnixStream::connect(path).map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(CONTROL_TIMEOUT))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(CONTROL_TIMEOUT))
        .map_err(|error| error.to_string())?;
    let bytes = encode_message(request)?;
    stream
        .write_all(&bytes)
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    let read_limit = u64::try_from(CONTROL_MESSAGE_MAX_BYTES + 1)
        .map_err(|_| "control message limit exceeds u64".to_owned())?;
    BufReader::new(stream)
        .take(read_limit)
        .read_until(b'\n', &mut bytes)
        .map_err(|error| error.to_string())?;
    decode_message(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn stalled_authority_returns_a_bounded_transport_error() {
        let root = std::env::temp_dir().join(format!(
            "scorepeek-control-timeout-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let socket = root.join("control.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let worker = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(250));
        });
        let started = std::time::Instant::now();
        let error = request(
            &socket,
            &Request::GetBackend {
                backend: scorepeek_overlay::Backend::Wayland,
            },
        )
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!error.is_empty());
        worker.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_authority_response_is_rejected() {
        let root = std::env::temp_dir().join(format!(
            "scorepeek-control-size-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let socket = root.join("control.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(&stream).read_line(&mut request).unwrap();
            stream
                .write_all(&vec![b'x'; CONTROL_MESSAGE_MAX_BYTES + 1])
                .unwrap();
        });
        let error = request(
            &socket,
            &Request::GetBackend {
                backend: scorepeek_overlay::Backend::Wayland,
            },
        )
        .unwrap_err();
        assert!(error.contains("maximum size"), "{error}");
        worker.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
