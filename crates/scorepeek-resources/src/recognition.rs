//! Registered catalog and OCR bundle loading for one recognition session.

use std::path::Path;

use scorepeek_core::catalog::Catalog;
use scorepeek_core::recognition::registered_field::RegisteredTextBundleBytes;
use scorepeek_core::recognition::title::{LIVE_MODEL_SHA256, LIVE_RUNTIME_SHA256, OnnxParityError};
use serde::Serialize;

use crate::model::load_registered_text_bundle;
use crate::{CatalogStore, CatalogStoreError};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisteredResourceLoadErrorType {
    InvalidLocation,
    ModelBindingMismatch,
    RuntimeBindingMismatch,
    CatalogUnavailable,
    CatalogBindingMismatch,
    CatalogLoadFailed,
    ModelBundleInvalid,
}

#[derive(Debug)]
pub enum RegisteredResourceLoadError {
    InvalidLocation {
        role: &'static str,
        source: Option<std::io::Error>,
    },
    ModelBindingMismatch,
    RuntimeBindingMismatch,
    CatalogUnavailable,
    CatalogBindingMismatch,
    Catalog(CatalogStoreError),
    Runtime(OnnxParityError),
}

impl RegisteredResourceLoadError {
    #[must_use]
    pub const fn error_type(&self) -> RegisteredResourceLoadErrorType {
        match self {
            Self::InvalidLocation { .. } => RegisteredResourceLoadErrorType::InvalidLocation,
            Self::ModelBindingMismatch => RegisteredResourceLoadErrorType::ModelBindingMismatch,
            Self::RuntimeBindingMismatch => RegisteredResourceLoadErrorType::RuntimeBindingMismatch,
            Self::CatalogUnavailable => RegisteredResourceLoadErrorType::CatalogUnavailable,
            Self::CatalogBindingMismatch => RegisteredResourceLoadErrorType::CatalogBindingMismatch,
            Self::Catalog(_) => RegisteredResourceLoadErrorType::CatalogLoadFailed,
            Self::Runtime(_) => RegisteredResourceLoadErrorType::ModelBundleInvalid,
        }
    }
}

impl std::fmt::Display for RegisteredResourceLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLocation { role, source } => {
                if let Some(source) = source {
                    write!(formatter, "registered {role} metadata failed: {source}")
                } else {
                    write!(formatter, "registered {role} must be an absolute directory")
                }
            }
            Self::ModelBindingMismatch => {
                formatter.write_str("recognition binding does not select the registered model")
            }
            Self::RuntimeBindingMismatch => {
                formatter.write_str("recognition binding does not select the registered runtime")
            }
            Self::CatalogUnavailable => formatter.write_str("active catalog is unavailable"),
            Self::CatalogBindingMismatch => {
                formatter.write_str("active catalog does not match the recognition binding")
            }
            Self::Catalog(error) => write!(formatter, "active catalog load failed: {error}"),
            Self::Runtime(error) => {
                write!(formatter, "registered text runtime load failed: {error}")
            }
        }
    }
}

impl std::error::Error for RegisteredResourceLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidLocation {
                source: Some(error),
                ..
            } => Some(error),
            Self::Catalog(error) => Some(error),
            Self::Runtime(error) => Some(error),
            Self::InvalidLocation { source: None, .. }
            | Self::ModelBindingMismatch
            | Self::RuntimeBindingMismatch
            | Self::CatalogUnavailable
            | Self::CatalogBindingMismatch => None,
        }
    }
}

/// Exact catalog and verified text-model bytes retained for one immutable recognition run.
pub struct RegisteredRecognitionResources {
    catalog_digest: String,
    catalog: Catalog,
    text_bundle: RegisteredTextBundleBytes,
}

impl RegisteredRecognitionResources {
    /// Loads and digest-checks the active catalog and registered model bundle.
    ///
    /// # Errors
    /// Returns a stable typed failure for location, binding, catalog, or bundle errors.
    /// No download, fallback, or active-state mutation is attempted.
    pub fn load(
        catalog_root: &Path,
        bundle_root: &Path,
        expected_catalog_sha256: &str,
        expected_model_sha256: &str,
        expected_runtime_sha256: &str,
    ) -> Result<Self, RegisteredResourceLoadError> {
        if expected_model_sha256 != LIVE_MODEL_SHA256 {
            return Err(RegisteredResourceLoadError::ModelBindingMismatch);
        }
        if expected_runtime_sha256 != LIVE_RUNTIME_SHA256 {
            return Err(RegisteredResourceLoadError::RuntimeBindingMismatch);
        }
        validate_registered_resource_directory(catalog_root, "catalog store")?;
        validate_registered_resource_directory(bundle_root, "model bundle")?;
        let active = match CatalogStore::new(catalog_root)
            .load_generation_for_run(expected_catalog_sha256)
        {
            Ok(active) => active,
            Err(CatalogStoreError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(RegisteredResourceLoadError::CatalogUnavailable);
            }
            Err(error) => return Err(RegisteredResourceLoadError::Catalog(error)),
        };
        let text_bundle = load_registered_text_bundle(bundle_root)
            .map_err(RegisteredResourceLoadError::Runtime)?;
        Ok(Self {
            catalog_digest: active.digest,
            catalog: active.catalog,
            text_bundle,
        })
    }

    #[must_use]
    pub fn catalog_sha256(&self) -> &str {
        &self.catalog_digest
    }

    #[must_use]
    pub const fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    #[must_use]
    pub fn into_catalog_and_text_bundle(self) -> (Catalog, RegisteredTextBundleBytes) {
        (self.catalog, self.text_bundle)
    }
}

fn validate_registered_resource_directory(
    path: &Path,
    role: &'static str,
) -> Result<(), RegisteredResourceLoadError> {
    if !path.is_absolute() {
        return Err(RegisteredResourceLoadError::InvalidLocation { role, source: None });
    }
    let metadata =
        path.metadata()
            .map_err(|source| RegisteredResourceLoadError::InvalidLocation {
                role,
                source: Some(source),
            })?;
    if !metadata.is_dir() {
        return Err(RegisteredResourceLoadError::InvalidLocation { role, source: None });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_error(
        result: Result<RegisteredRecognitionResources, RegisteredResourceLoadError>,
    ) -> RegisteredResourceLoadError {
        match result {
            Ok(_) => panic!("resource load unexpectedly succeeded"),
            Err(error) => error,
        }
    }

    #[test]
    fn registered_resources_reject_binding_and_catalog_failures_before_runtime_loading() {
        let missing = tempfile::tempdir().unwrap();
        let bundle = tempfile::tempdir().unwrap();
        let model_mismatch = load_error(RegisteredRecognitionResources::load(
            missing.path(),
            bundle.path(),
            &"1".repeat(64),
            &"2".repeat(64),
            LIVE_RUNTIME_SHA256,
        ));
        assert_eq!(
            model_mismatch.error_type(),
            RegisteredResourceLoadErrorType::ModelBindingMismatch
        );
        let runtime_mismatch = load_error(RegisteredRecognitionResources::load(
            missing.path(),
            bundle.path(),
            &"1".repeat(64),
            LIVE_MODEL_SHA256,
            &"2".repeat(64),
        ));
        assert_eq!(
            runtime_mismatch.error_type(),
            RegisteredResourceLoadErrorType::RuntimeBindingMismatch
        );
        let unavailable = load_error(RegisteredRecognitionResources::load(
            missing.path(),
            bundle.path(),
            &"1".repeat(64),
            LIVE_MODEL_SHA256,
            LIVE_RUNTIME_SHA256,
        ));
        assert!(matches!(
            unavailable,
            RegisteredResourceLoadError::CatalogUnavailable
        ));

        let catalog_root = tempfile::tempdir().unwrap();
        let active = CatalogStore::new(catalog_root.path())
            .begin_update()
            .unwrap()
            .publish(&Catalog::default())
            .unwrap();
        let unavailable_generation = load_error(RegisteredRecognitionResources::load(
            catalog_root.path(),
            bundle.path(),
            &"3".repeat(64),
            LIVE_MODEL_SHA256,
            LIVE_RUNTIME_SHA256,
        ));
        assert_ne!(active.digest, "3".repeat(64));
        assert_eq!(
            unavailable_generation.error_type(),
            RegisteredResourceLoadErrorType::CatalogUnavailable
        );
    }

    #[test]
    fn registered_resource_location_errors_retain_role_and_io_source() {
        let root = tempfile::tempdir().unwrap();
        let bundle = tempfile::tempdir().unwrap();
        let missing_catalog = root.path().join("missing-catalog");
        let catalog_error = load_error(RegisteredRecognitionResources::load(
            &missing_catalog,
            bundle.path(),
            &"1".repeat(64),
            LIVE_MODEL_SHA256,
            LIVE_RUNTIME_SHA256,
        ));
        assert_eq!(
            catalog_error.error_type(),
            RegisteredResourceLoadErrorType::InvalidLocation
        );
        assert!(
            catalog_error
                .to_string()
                .contains("catalog store metadata failed")
        );
        assert!(std::error::Error::source(&catalog_error).is_some());

        let missing_bundle = root.path().join("missing-bundle");
        let bundle_error = load_error(RegisteredRecognitionResources::load(
            root.path(),
            &missing_bundle,
            &"1".repeat(64),
            LIVE_MODEL_SHA256,
            LIVE_RUNTIME_SHA256,
        ));
        assert!(
            bundle_error
                .to_string()
                .contains("model bundle metadata failed")
        );
        assert!(std::error::Error::source(&bundle_error).is_some());
    }
}
