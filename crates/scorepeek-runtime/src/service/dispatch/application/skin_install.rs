use std::io::{self, Read as _};
use std::path::Path;
use std::time::Duration;

use scorepeek_overlay_runtime::skin::{InstallOutcome, StoreRoot};
use tempfile::{Builder, NamedTempFile};
use url::Url;

const MAX_DOWNLOAD_BYTES: u64 = 128 * 1024 * 1024;
const MAX_REDIRECTS: u32 = 10;
const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);

enum Source<'a> {
    Local(&'a Path),
    Remote(Url),
}

fn source(package: &str) -> Result<Source<'_>, String> {
    if !package.starts_with("https:") && !package.contains("://") {
        return Ok(Source::Local(Path::new(package)));
    }
    let url = Url::parse(package).map_err(|_| "skin URL is invalid".to_owned())?;
    if url.scheme() != "https" || url.host_str().is_none() {
        return Err("skin URL must use HTTPS".into());
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err("skin URL must not contain credentials or a fragment".into());
    }
    Ok(Source::Remote(url))
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .https_only(true)
        .max_redirects(MAX_REDIRECTS)
        .timeout_global(Some(REQUEST_TIMEOUT))
        .user_agent(format!(
            "scorepeek/{} (+https://github.com/atty303/scorepeek)",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .new_agent()
}

fn download(
    agent: &ureq::Agent,
    url: &Url,
    temporary_directory: Option<&Path>,
) -> Result<NamedTempFile, String> {
    let mut response = agent.get(url.as_str()).call().map_err(|error| {
        if matches!(error, ureq::Error::Timeout(_)) {
            "skin download timed out".to_owned()
        } else {
            "skin download request failed".to_owned()
        }
    })?;
    if response.status().as_u16() != 200 {
        return Err(format!("skin download returned HTTP {}", response.status()));
    }
    if response
        .body()
        .content_length()
        .is_some_and(|length| length > MAX_DOWNLOAD_BYTES)
    {
        return Err("skin download exceeds the ZIP size limit".into());
    }
    let builder = Builder::new();
    let mut temporary = match temporary_directory {
        Some(directory) => builder.tempfile_in(directory),
        None => builder.tempfile(),
    }
    .map_err(|error| format!("create skin download temporary file: {error}"))?;
    let mut reader = response.body_mut().as_reader().take(MAX_DOWNLOAD_BYTES + 1);
    let bytes = io::copy(&mut reader, temporary.as_file_mut())
        .map_err(|error| format!("read skin download: {error}"))?;
    if bytes > MAX_DOWNLOAD_BYTES {
        return Err("skin download exceeds the ZIP size limit".into());
    }
    Ok(temporary)
}

fn install_remote(
    store: &StoreRoot,
    agent: &ureq::Agent,
    url: &Url,
    force: bool,
    temporary_directory: Option<&Path>,
) -> Result<InstallOutcome, String> {
    let temporary = download(agent, url, temporary_directory)?;
    store.install(temporary.path(), force)
}

pub(super) fn install(
    store: &StoreRoot,
    package: &str,
    force: bool,
) -> Result<InstallOutcome, String> {
    match source(package)? {
        Source::Local(path) => store.install(path, force),
        Source::Remote(url) => install_remote(store, &agent(), &url, force, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::thread;

    const MANIFEST: &str = r#"
id = "dev.example.skin"
name = "Example"
release = "2026-09-27"
api_version = 2
[widget_defaults.status]
width = 16
height = 16
[widget_defaults.selection]
width = 16
height = 16
[widget_defaults.score]
width = 16
height = 16
[widget_defaults.history-list]
width = 16
height = 16
[widget_defaults.history-graph]
width = 16
height = 16
[widget_defaults.empty]
width = 16
height = 16
"#;

    fn package_zip(path: &Path) -> Vec<u8> {
        let mut archive = zip::ZipWriter::new(File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, bytes) in [
            ("skin.toml", MANIFEST.as_bytes()),
            ("skin.wasm", b"wasm".as_slice()),
            ("skin.css", b".scorepeek-skin-scope {}".as_slice()),
            (
                "preview.png",
                include_bytes!("../../../../../../skins/infinitas/preview.png").as_slice(),
            ),
        ] {
            archive.start_file(name, options).unwrap();
            archive.write_all(bytes).unwrap();
        }
        archive.finish().unwrap();
        fs::read(path).unwrap()
    }

    fn http_server(responses: Vec<(u16, Vec<u8>)>) -> (Url, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 1024];
                let _ = stream.read(&mut request).unwrap();
                if status == 302 {
                    write!(
                        stream,
                        "HTTP/1.1 302 Found\r\nLocation: http://{address}/asset\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .unwrap();
                } else {
                    write!(
                        stream,
                        "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .unwrap();
                    stream.write_all(&body).unwrap();
                }
            }
        });
        (
            Url::parse(&format!("http://{address}/skin.zip")).unwrap(),
            handle,
        )
    }

    fn test_agent() -> ureq::Agent {
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(MAX_REDIRECTS)
            .proxy(None)
            .build()
            .new_agent()
    }

    #[test]
    fn source_distinguishes_local_paths_and_https_urls() {
        assert!(matches!(
            source("relative/skin.zip").unwrap(),
            Source::Local(_)
        ));
        assert!(matches!(source("/tmp/skin.zip").unwrap(), Source::Local(_)));
        assert!(matches!(
            source("https://example.test/skin.zip").unwrap(),
            Source::Remote(_)
        ));
        assert!(source("http://example.test/skin.zip").is_err());
        assert!(source("https://user:secret@example.test/skin.zip").is_err());
        assert!(source("https://example.test/skin.zip#fragment").is_err());
    }

    #[test]
    fn redirected_download_installs_the_exact_zip_and_cleans_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        let bytes = package_zip(&root.path().join("source.zip"));
        let (url, server) = http_server(vec![(302, Vec::new()), (200, bytes.clone())]);
        let temporary = root.path().join("temporary");
        fs::create_dir(&temporary).unwrap();
        let store = StoreRoot::new(root.path().join("skins"));
        assert_eq!(
            install_remote(&store, &test_agent(), &url, false, Some(&temporary)).unwrap(),
            InstallOutcome::Installed
        );
        server.join().unwrap();
        assert_eq!(
            fs::read(store.path().join("dev.example.skin.zip")).unwrap(),
            bytes
        );
        assert_eq!(fs::read_dir(&temporary).unwrap().count(), 0);
    }

    #[test]
    fn failed_download_preserves_installed_skin_and_cleans_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.zip");
        let bytes = package_zip(&source);
        let store = StoreRoot::new(root.path().join("skins"));
        store.install(&source, false).unwrap();
        let temporary = root.path().join("temporary");
        fs::create_dir(&temporary).unwrap();
        let (url, server) = http_server(vec![(200, b"not a ZIP".to_vec())]);
        assert!(install_remote(&store, &test_agent(), &url, false, Some(&temporary)).is_err());
        server.join().unwrap();
        assert_eq!(
            fs::read(store.path().join("dev.example.skin.zip")).unwrap(),
            bytes
        );
        assert_eq!(fs::read_dir(&temporary).unwrap().count(), 0);
    }
}
