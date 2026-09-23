//! Repository-registered replay resources acquired into an isolated temporary store.

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use scorepeek_core::model::registry::{LIVE_MODEL_SHA256, LIVE_RUNTIME_SHA256};
use scorepeek_core::recognition::title::registered_live_model_files;
use scorepeek_resources::CatalogStore;
use scorepeek_resources::artifact::{MAX_ARTIFACT_BYTES, extract_client_verified};
use scorepeek_resources::model::verify_registered_live_model_bundle;
use scorepeek_resources::recognition::RegisteredRecognitionResources;
use sha2::{Digest as _, Sha256};

const CATALOG_URL: &str = include_str!("../../../registration/catalog-url.txt");
const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .https_only(true)
        .max_redirects(10)
        .timeout_global(Some(REQUEST_TIMEOUT))
        .user_agent(format!("scorepeek-corpus/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

fn download(agent: &ureq::Agent, url: &str, bound: u64) -> Result<Vec<u8>, String> {
    let mut response = agent.get(url).call().map_err(|error| error.to_string())?;
    if response.status() != 200 {
        return Err(format!(
            "registered resource returned HTTP {}",
            response.status()
        ));
    }
    if response
        .body()
        .content_length()
        .is_some_and(|size| size > bound)
    {
        return Err("registered resource declared size exceeds bound".into());
    }
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(bound + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > bound {
        return Err("registered resource exceeds bound".into());
    }
    Ok(bytes)
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

/// Repository-selected bytes shared by independent offline OCR workers.
#[derive(Clone)]
pub(crate) struct PreparedRegisteredResources {
    catalog_root: PathBuf,
    model_root: PathBuf,
    catalog_sha256: String,
}

impl PreparedRegisteredResources {
    pub(crate) fn load_observer_resources(&self) -> Result<RegisteredRecognitionResources, String> {
        RegisteredRecognitionResources::load(
            &self.catalog_root,
            &self.model_root,
            &self.catalog_sha256,
            LIVE_MODEL_SHA256,
            LIVE_RUNTIME_SHA256,
        )
        .map_err(|error| error.to_string())
    }
}

/// Acquires only the catalog URL and OCR model files registered in this source tree. The caller
/// owns the temporary root until all recognition workers have stopped.
///
/// # Errors
/// Returns the failing download, integrity, catalog activation, or registered resource error.
pub(crate) fn prepare_registered(root: &Path) -> Result<PreparedRegisteredResources, String> {
    let agent = agent();
    let artifact = root.join("catalog.zip");
    let catalog_bytes = download(&agent, CATALOG_URL, MAX_ARTIFACT_BYTES)?;
    fs::write(&artifact, catalog_bytes).map_err(|error| error.to_string())?;
    let extracted = extract_client_verified(&artifact, &root.join("catalog-extracted"))
        .map_err(|error| error.to_string())?;
    let catalog_root = root.join("catalog-store");
    fs::create_dir(&catalog_root).map_err(|error| error.to_string())?;
    CatalogStore::new(&catalog_root)
        .install_verified_snapshot(&extracted.catalog_path, &extracted.manifest.sqlite_sha256)
        .map_err(|error| error.to_string())?;

    let model_root = root.join("model-bundle");
    fs::create_dir(&model_root).map_err(|error| error.to_string())?;
    for file in registered_live_model_files().map_err(|error| error.to_string())? {
        let bytes = download(&agent, &file.source_url, file.bytes)?;
        if bytes.len() as u64 != file.bytes || sha256(&bytes) != file.sha256 {
            return Err(format!("registered model file differs: {}", file.filename));
        }
        fs::write(model_root.join(&file.filename), bytes).map_err(|error| error.to_string())?;
    }
    verify_registered_live_model_bundle(&model_root).map_err(|error| error.to_string())?;
    Ok(PreparedRegisteredResources {
        catalog_root,
        model_root,
        catalog_sha256: extracted.manifest.sqlite_sha256,
    })
}
