//! Registered text-model bundle manifests, identities, shapes, and file validation.

use std::fmt::Write as _;

use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use super::registry::LIVE_MODEL_BUNDLE_MANIFEST_SHA256;

const LIVE_MODEL_ID: &str = "pp-ocrv6-small-rec-onnx-v1";
const INPUT_HEIGHT: usize = 48;

#[derive(Debug)]
pub enum ManifestError {
    Json(serde_json::Error),
    InvalidArtifact,
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "ONNX parity JSON failed: {error}"),
            Self::InvalidArtifact => formatter.write_str("ONNX parity artifact is invalid"),
        }
    }
}

impl std::error::Error for ManifestError {}

impl From<serde_json::Error> for ManifestError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DynamicBundleManifest {
    schema: String,
    model_id: String,
    model_name: String,
    source_repository: String,
    source_revision: String,
    license_id: String,
    license_url: String,
    native_contract: DynamicNativeContract,
    files: Vec<DynamicBundleFile>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DynamicNativeContract {
    input_layout: String,
    input_color_order: String,
    input_channels: usize,
    input_height: usize,
    preprocessor_minimum_width: usize,
    preprocessor_maximum_width: usize,
    output_classes: usize,
    ctc_blank_token: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DynamicBundleFile {
    filename: String,
    source_url: String,
    sha256: String,
    bytes: u64,
}

/// One immutable file registered for the live PP-OCRv6-small bundle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredLiveModelFile {
    pub filename: String,
    pub source_url: String,
    pub sha256: String,
    pub bytes: u64,
}

/// Returns the verified download contract embedded for the live PP-OCRv6-small bundle.
///
/// # Errors
/// Returns an error if the embedded manifest no longer matches the compiled registration.
pub fn registered_live_model_files() -> Result<Vec<RegisteredLiveModelFile>, ManifestError> {
    let manifest = DynamicBundleManifest::load_registered(LIVE_MODEL_ID)?;
    Ok(manifest
        .files
        .into_iter()
        .map(|file| RegisteredLiveModelFile {
            filename: file.filename,
            source_url: file.source_url,
            sha256: file.sha256,
            bytes: file.bytes,
        })
        .collect())
}

/// Verifies the complete registered live bundle without constructing an ONNX session.
///
/// # Errors
/// Returns an error for missing, changed, non-regular, or malformed bundle files.
pub fn verify_registered_live_model_bundle_bytes(
    files: &[(&str, &[u8])],
) -> Result<(), ManifestError> {
    DynamicBundleManifest::load_registered(LIVE_MODEL_ID)?
        .verified_model_bytes(files)
        .map(|_| ())
}

struct BundleRegistration {
    manifest: &'static [u8],
    manifest_sha256: &'static str,
    model_name: &'static str,
    repository: &'static str,
    revision: &'static str,
    output_classes: usize,
    files: &'static [(&'static str, &'static str, u64)],
}

const SMALL_FILES: &[(&str, &str, u64)] = &[
    (
        "inference.onnx",
        "5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634",
        21_159_378,
    ),
    (
        "inference.json",
        "f0bf53c853937a917affdd74467472167727f8ab0f0f7bded01c4a16c27e46e6",
        208_004,
    ),
    (
        "inference.yml",
        "ab078671bb49f06228eadccd34f1bb501e157f7a047095ffb943ba81512c77d1",
        150_579,
    ),
];
const TINY_FILES: &[(&str, &str, u64)] = &[
    (
        "inference.onnx",
        "9ef676d6ed3c88256a2d92c640c44f25b0c40947e111b14b8be8f594091563e6",
        4_462_639,
    ),
    (
        "inference.json",
        "b5b14770c7dcf092781e92f4278a2ae5f95048f08b4b8a04140e88cb2745f147",
        108_959,
    ),
    (
        "inference.yml",
        "66170210bad538e83fff3c4a3867e547d6bf20b50d64b20347c4b913f3034ea1",
        55_571,
    ),
];
const MEDIUM_FILES: &[(&str, &str, u64)] = &[
    (
        "inference.onnx",
        "9c09abf0957f7968c7586464b7397b84ad2387a0497a351af40e9acc71b673ba",
        76_554_979,
    ),
    (
        "inference.json",
        "0b2e25e990bd072f1bf77d59d67d508bce6c4bd44af6624e0fb27d6da2cd00e8",
        221_814,
    ),
    (
        "inference.yml",
        "991b700facf5b50a7de193468207d5f4255b538dde0d312ae3b7c7a9b6873129",
        150_580,
    ),
];
const V5_MOBILE_FILES: &[(&str, &str, u64)] = &[
    (
        "inference.onnx",
        "da72dc72ca4dc220df0dfde68c1dedc31c58d3e76a25871122e5056227d50092",
        16_534_782,
    ),
    (
        "inference.yml",
        "5dfeb2777f6d0db8177d8128a8acfcf6e6276dc4ac73ea3bf0dc06d6a5e85d8e",
        148_345,
    ),
];
const V5_SERVER_FILES: &[(&str, &str, u64)] = &[
    (
        "inference.onnx",
        "d9dc333c9c7b042c6dffb8e33d72b6f65c9c1d463d0a3c2f78174fea55e94752",
        84_503_027,
    ),
    (
        "inference.yml",
        "2c719dba044c4e2228aef8ff92f5f575394d75d24c16de096a33b7cfd902f66d",
        148_345,
    ),
];

fn registration(model_id: &str) -> Result<BundleRegistration, ManifestError> {
    match model_id {
        LIVE_MODEL_ID => Ok(BundleRegistration {
            manifest: include_bytes!(
                "../../../../models/manifests/pp-ocrv6-small-rec-onnx-bundle-v1.json"
            ),
            manifest_sha256: LIVE_MODEL_BUNDLE_MANIFEST_SHA256,
            model_name: "PP-OCRv6_small_rec",
            repository: "PaddlePaddle/PP-OCRv6_small_rec_onnx",
            revision: "b8f84f0b80c529de40b4fbb3544b84fa7233a513",
            output_classes: 18_710,
            files: SMALL_FILES,
        }),
        "pp-ocrv6-tiny-rec-onnx-v1" => Ok(BundleRegistration {
            manifest: include_bytes!(
                "../../../../models/manifests/pp-ocrv6-tiny-rec-onnx-bundle-v1.json"
            ),
            manifest_sha256: "d24f1ec10098065efd24216b23b405bb2af5feabbb815bc499ba0a5735b8bfd0",
            model_name: "PP-OCRv6_tiny_rec",
            repository: "PaddlePaddle/PP-OCRv6_tiny_rec_onnx",
            revision: "2612ab37152ae0a677521bae4e1e3d4fb4cf7c30",
            output_classes: 6_906,
            files: TINY_FILES,
        }),
        "pp-ocrv6-medium-rec-onnx-v1" => Ok(BundleRegistration {
            manifest: include_bytes!(
                "../../../../models/manifests/pp-ocrv6-medium-rec-onnx-bundle-v1.json"
            ),
            manifest_sha256: "f794d77fb6d9860e2aadedd1ef575bd67c044b83fe2821243867b66c9a7c5abe",
            model_name: "PP-OCRv6_medium_rec",
            repository: "PaddlePaddle/PP-OCRv6_medium_rec_onnx",
            revision: "50c7eacafc52fa7bcf4194e8cd08e46f8558504b",
            output_classes: 18_710,
            files: MEDIUM_FILES,
        }),
        "pp-ocrv5-mobile-rec-onnx-v1" => Ok(BundleRegistration {
            manifest: include_bytes!(
                "../../../../models/manifests/pp-ocrv5-mobile-rec-onnx-bundle-v1.json"
            ),
            manifest_sha256: "ebbd34d2c0e360b1cf55199fc1400886e7bfbb4d6917c7d86a994b79c2256971",
            model_name: "PP-OCRv5_mobile_rec",
            repository: "PaddlePaddle/PP-OCRv5_mobile_rec_onnx",
            revision: "ed152b8b495f84de93cda5709d768548a9127622",
            output_classes: 18_385,
            files: V5_MOBILE_FILES,
        }),
        "pp-ocrv5-server-rec-onnx-v1" => Ok(BundleRegistration {
            manifest: include_bytes!(
                "../../../../models/manifests/pp-ocrv5-server-rec-onnx-bundle-v1.json"
            ),
            manifest_sha256: "4fe22f41508ed31b86e86caa88d433a20702d0a6e95cea07bcaca577441594fe",
            model_name: "PP-OCRv5_server_rec",
            repository: "PaddlePaddle/PP-OCRv5_server_rec_onnx",
            revision: "b70df217f4fd99d14f970bad092cebe7d74cc4d1",
            output_classes: 18_385,
            files: V5_SERVER_FILES,
        }),
        _ => Err(ManifestError::InvalidArtifact),
    }
}

impl DynamicBundleManifest {
    pub(crate) fn load_registered(model_id: &str) -> Result<Self, ManifestError> {
        let registered = registration(model_id)?;
        if sha256(registered.manifest) != registered.manifest_sha256 {
            return Err(ManifestError::InvalidArtifact);
        }
        let manifest: Self = serde_json::from_slice(registered.manifest)?;
        if manifest.schema != "scorepeek-ocr-onnx-model-bundle-v1"
            || manifest.model_id != model_id
            || manifest.model_name != registered.model_name
            || manifest.source_repository != registered.repository
            || manifest.source_revision != registered.revision
            || manifest.license_id != "Apache-2.0"
            || manifest.license_url
                != format!(
                    "https://huggingface.co/{}/blob/{}/README.md",
                    registered.repository, registered.revision
                )
            || manifest.native_contract.input_layout != "NCHW"
            || manifest.native_contract.input_color_order != "BGR"
            || manifest.native_contract.input_channels != 3
            || manifest.native_contract.input_height != INPUT_HEIGHT
            || manifest.native_contract.preprocessor_minimum_width != 320
            || manifest.native_contract.preprocessor_maximum_width != 3_200
            || manifest.native_contract.output_classes != registered.output_classes
            || manifest.native_contract.ctc_blank_token != 0
            || manifest.files.len() != registered.files.len()
        {
            return Err(ManifestError::InvalidArtifact);
        }
        for (file, (filename, digest, bytes)) in
            manifest.files.iter().zip(registered.files.iter().copied())
        {
            if file.filename != filename
                || file.sha256 != digest
                || file.bytes != bytes
                || file.source_url
                    != format!(
                        "https://huggingface.co/{}/resolve/{}/{filename}",
                        registered.repository, registered.revision
                    )
            {
                return Err(ManifestError::InvalidArtifact);
            }
        }
        Ok(manifest)
    }

    pub(crate) const fn output_classes(&self) -> usize {
        self.native_contract.output_classes
    }

    pub(crate) fn file(&self, filename: &str) -> Option<&DynamicBundleFile> {
        self.files.iter().find(|file| file.filename == filename)
    }

    pub(crate) fn verified_model_bytes<'a>(
        &self,
        files: &'a [(&str, &[u8])],
    ) -> Result<&'a [u8], ManifestError> {
        if files.len() != self.files.len() {
            return Err(ManifestError::InvalidArtifact);
        }
        let mut model = None;
        for file in &self.files {
            let bytes = files
                .iter()
                .find_map(|(name, bytes)| (*name == file.filename).then_some(*bytes))
                .ok_or(ManifestError::InvalidArtifact)?;
            if bytes.len() as u64 != file.bytes || sha256(bytes) != file.sha256 {
                return Err(ManifestError::InvalidArtifact);
            }
            if file.filename == "inference.onnx" {
                model = Some(bytes);
            }
        }
        model.ok_or(ManifestError::InvalidArtifact)
    }
}

impl DynamicBundleFile {
    pub(crate) fn sha256(&self) -> &str {
        &self.sha256
    }
}

fn sha256(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::DynamicBundleManifest;

    #[test]
    fn registered_tiny_bundle_manifest_is_exact() {
        let manifest = DynamicBundleManifest::load_registered("pp-ocrv6-tiny-rec-onnx-v1").unwrap();
        assert_eq!(manifest.model_id, "pp-ocrv6-tiny-rec-onnx-v1");
        assert_eq!(manifest.native_contract.output_classes, 6_906);
        assert_eq!(manifest.files.len(), 3);
    }

    #[test]
    fn registered_small_bundle_manifest_is_exact() {
        let manifest =
            DynamicBundleManifest::load_registered("pp-ocrv6-small-rec-onnx-v1").unwrap();
        assert_eq!(manifest.model_id, "pp-ocrv6-small-rec-onnx-v1");
        assert_eq!(manifest.native_contract.output_classes, 18_710);
        assert_eq!(manifest.files.len(), 3);
    }

    #[test]
    fn registered_medium_bundle_manifest_is_exact() {
        let manifest =
            DynamicBundleManifest::load_registered("pp-ocrv6-medium-rec-onnx-v1").unwrap();
        assert_eq!(manifest.model_id, "pp-ocrv6-medium-rec-onnx-v1");
        assert_eq!(manifest.native_contract.output_classes, 18_710);
        assert_eq!(manifest.files.len(), 3);
    }

    #[test]
    fn registered_v5_mobile_bundle_manifest_is_exact() {
        let manifest =
            DynamicBundleManifest::load_registered("pp-ocrv5-mobile-rec-onnx-v1").unwrap();
        assert_eq!(manifest.model_id, "pp-ocrv5-mobile-rec-onnx-v1");
        assert_eq!(manifest.native_contract.output_classes, 18_385);
        assert_eq!(manifest.files.len(), 2);
    }

    #[test]
    fn registered_v5_server_bundle_manifest_is_exact() {
        let manifest =
            DynamicBundleManifest::load_registered("pp-ocrv5-server-rec-onnx-v1").unwrap();
        assert_eq!(manifest.model_id, "pp-ocrv5-server-rec-onnx-v1");
        assert_eq!(manifest.native_contract.output_classes, 18_385);
        assert_eq!(manifest.files.len(), 2);
    }
}
