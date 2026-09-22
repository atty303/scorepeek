//! Registered model acquisition and transport.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest as _, Sha256};

use scorepeek_core::model::manifest::{RegisteredLiveModelFile, registered_live_model_files};
use scorepeek_core::model::registry::LIVE_MODEL_BUNDLE_MANIFEST_SHA256;
use scorepeek_resources::model::verify_registered_live_model_bundle;

use super::cache::{
    ModelCacheError, ModelCacheEvent, default_model_store, ensure_model_with,
    validate_absolute_directory,
};

const MAX_REDIRECTS: u32 = 10;
const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);

#[derive(Clone, Debug)]
pub(super) struct ModelHttpResponse {
    pub(super) status: u16,
    pub(super) content_length: Option<u64>,
    pub(super) body: Vec<u8>,
}

pub(super) trait ModelTransport {
    fn get(&self, file: &RegisteredLiveModelFile) -> Result<ModelHttpResponse, ModelCacheError>;
}

struct UreqModelTransport {
    agent: ureq::Agent,
}

impl UreqModelTransport {
    fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .https_only(true)
            .max_redirects(MAX_REDIRECTS)
            .timeout_global(Some(REQUEST_TIMEOUT))
            .user_agent(format!(
                "scorepeek/{} (+https://github.com/atty303/scorepeek)",
                env!("CARGO_PKG_VERSION")
            ))
            .build();
        Self {
            agent: config.new_agent(),
        }
    }
}

impl ModelTransport for UreqModelTransport {
    fn get(&self, file: &RegisteredLiveModelFile) -> Result<ModelHttpResponse, ModelCacheError> {
        let mut response = self.agent.get(&file.source_url).call().map_err(|error| {
            if matches!(error, ureq::Error::Timeout(_)) {
                ModelCacheError::Timeout {
                    filename: file.filename.clone(),
                }
            } else {
                ModelCacheError::Transport {
                    filename: file.filename.clone(),
                    detail: error.to_string(),
                }
            }
        })?;
        let status = response.status().as_u16();
        let content_length = response.body().content_length();
        if content_length.is_some_and(|declared| declared != file.bytes) {
            return Err(ModelCacheError::DeclaredSize {
                filename: file.filename.clone(),
                declared: content_length.expect("checked content length"),
                expected: file.bytes,
            });
        }
        let mut body = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(file.bytes + 1)
            .read_to_end(&mut body)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::TimedOut {
                    ModelCacheError::Timeout {
                        filename: file.filename.clone(),
                    }
                } else {
                    ModelCacheError::Transport {
                        filename: file.filename.clone(),
                        detail: "response body read failed".to_owned(),
                    }
                }
            })?;
        Ok(ModelHttpResponse {
            status,
            content_length,
            body,
        })
    }
}

/// Ensures the registered live model exists, or verifies the explicit development override.
///
/// # Errors
/// Returns before command dispatch when location, download, verification, locking, or publication
/// fails. An existing completed cache is returned without network access or mutation.
pub fn ensure_small_model(
    override_bundle: Option<&Path>,
    observer: impl FnMut(ModelCacheEvent),
) -> Result<PathBuf, ModelCacheError> {
    if let Some(bundle) = override_bundle {
        validate_absolute_directory(bundle)?;
        verify_registered_live_model_bundle(bundle)
            .map_err(|error| ModelCacheError::InvalidBundle(error.to_string()))?;
        return Ok(bundle.to_path_buf());
    }
    let store = default_model_store()?;
    ensure_small_model_with(&store, &UreqModelTransport::new(), observer)
}

fn ensure_small_model_with(
    store: &Path,
    transport: &impl ModelTransport,
    observer: impl FnMut(ModelCacheEvent),
) -> Result<PathBuf, ModelCacheError> {
    let files = registered_live_model_files()
        .map_err(|error| ModelCacheError::Registration(error.to_string()))?;
    ensure_model_with(
        store,
        LIVE_MODEL_BUNDLE_MANIFEST_SHA256,
        &files,
        |file| download(transport, file),
        |path| {
            verify_registered_live_model_bundle(path)
                .map_err(|error| ModelCacheError::InvalidBundle(error.to_string()))
        },
        observer,
    )
}

pub(super) fn download(
    transport: &impl ModelTransport,
    file: &RegisteredLiveModelFile,
) -> Result<Vec<u8>, ModelCacheError> {
    let response = transport.get(file)?;
    verify_response(file, &response)?;
    Ok(response.body)
}

fn verify_response(
    file: &RegisteredLiveModelFile,
    response: &ModelHttpResponse,
) -> Result<(), ModelCacheError> {
    if response.status != 200 {
        return Err(ModelCacheError::Http {
            filename: file.filename.clone(),
            status: response.status,
        });
    }
    if response
        .content_length
        .is_some_and(|declared| declared != file.bytes)
    {
        return Err(ModelCacheError::DeclaredSize {
            filename: file.filename.clone(),
            declared: response.content_length.expect("checked content length"),
            expected: file.bytes,
        });
    }
    if response.body.len() as u64 != file.bytes {
        return Err(ModelCacheError::Size {
            filename: file.filename.clone(),
            actual: response.body.len(),
            expected: file.bytes,
        });
    }
    if sha256_hex(&response.body) != file.sha256 {
        return Err(ModelCacheError::Digest {
            filename: file.filename.clone(),
        });
    }
    Ok(())
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}
