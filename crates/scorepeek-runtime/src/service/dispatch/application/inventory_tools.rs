use super::*;
use scorepeek_frontend_api::{
    CatalogReport, CatalogStatus, CatalogUpdateErrorType, CatalogUpdateFailure, DoctorReport,
    NumericModelReport,
};

pub(super) fn collect_doctor_report() -> DoctorReport {
    let target_inventory = inventory::collect().into_report();
    let numeric_model =
        match scorepeek_core::recognition::result::numeric::RegisteredNumericRuntime::load_embedded(
        ) {
            Ok(runtime) => NumericModelReport::Active {
                model_id: runtime.contract().model_id.clone(),
                model_sha256: runtime.contract().model_sha256.clone(),
                manifest_sha256:
                    scorepeek_core::recognition::result::numeric::NUMERIC_MODEL_MANIFEST_SHA256
                        .to_owned(),
                preprocessor_id: runtime.contract().preprocessor_id.clone(),
            },
            Err(error) => NumericModelReport::Unavailable {
                reason: error.to_string(),
                registered_manifest_sha256:
                    scorepeek_core::recognition::result::numeric::NUMERIC_MODEL_MANIFEST_SHA256
                        .to_owned(),
            },
        };
    let catalog = catalog_paths(
        env::var_os("XDG_DATA_HOME").as_deref(),
        env::var_os("XDG_CACHE_HOME").as_deref(),
        env::var_os("HOME").as_deref(),
    )
    .and_then(|(store_root, _)| {
        let active = CatalogStore::new(&store_root)
            .load_active_for_run()
            .map_err(|error| error.to_string())?;
        let state = crate::resources::catalog::cache::load_state(&store_root)
            .map_err(|error| error.to_string())?;
        let last_failure = state.last_failure.map(|failure| CatalogUpdateFailure {
            source_url_sha256: failure.source_url_sha256,
            failed_unix_seconds: failure.failed_unix_seconds,
            error_type: match failure.error_type {
                crate::resources::catalog::acquire::UpdateErrorType::ConfigurationInvalid => {
                    CatalogUpdateErrorType::ConfigurationInvalid
                }
                crate::resources::catalog::acquire::UpdateErrorType::TransportFailed => {
                    CatalogUpdateErrorType::TransportFailed
                }
                crate::resources::catalog::acquire::UpdateErrorType::HttpStatus => {
                    CatalogUpdateErrorType::HttpStatus
                }
                crate::resources::catalog::acquire::UpdateErrorType::ArtifactInvalid => {
                    CatalogUpdateErrorType::ArtifactInvalid
                }
                crate::resources::catalog::acquire::UpdateErrorType::ActivationFailed => {
                    CatalogUpdateErrorType::ActivationFailed
                }
                crate::resources::catalog::acquire::UpdateErrorType::StateFailed => {
                    CatalogUpdateErrorType::StateFailed
                }
            },
        });
        Ok(CatalogReport {
            status: if active.is_some() {
                CatalogStatus::Active
            } else {
                CatalogStatus::Unavailable
            },
            reason: None,
            active_catalog_sha256: Some(active.map(|catalog| catalog.digest)),
            source_url_sha256: Some(state.source_url_sha256),
            last_success_unix_seconds: Some(state.last_success_unix_seconds),
            etag_present: Some(state.etag.is_some()),
            last_modified_present: Some(state.last_modified.is_some()),
            last_failure: Some(last_failure),
        })
    })
    .unwrap_or_else(|error| CatalogReport {
        status: CatalogStatus::Unavailable,
        reason: Some(error),
        active_catalog_sha256: None,
        source_url_sha256: None,
        last_success_unix_seconds: None,
        etag_present: None,
        last_modified_present: None,
        last_failure: None,
    });
    DoctorReport {
        schema: "scorepeek-doctor-v5".to_owned(),
        target_inventory,
        numeric_model,
        catalog,
        vulkan_layer: vulkan_layer::inspect(),
    }
}

pub(super) fn collect_frontend_doctor() -> scorepeek_frontend_api::CommandResult {
    scorepeek_frontend_api::CommandResult::Doctor {
        report: Box::new(collect_doctor_report()),
    }
}
