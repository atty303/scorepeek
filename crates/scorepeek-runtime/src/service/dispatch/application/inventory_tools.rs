use super::*;

pub(super) fn collect_doctor_report() -> Result<serde_json::Value, String> {
    let target_inventory: serde_json::Value = serde_json::from_str(&inventory::collect().to_json())
        .map_err(|error| format!("doctor report serialization failed: {error}"))?;
    let numeric_model =
        match scorepeek_core::recognition::result::numeric::RegisteredNumericRuntime::load_embedded(
        ) {
            Ok(runtime) => serde_json::json!({
                "status": "active",
                "model_id": runtime.contract().model_id,
                "model_sha256": runtime.contract().model_sha256,
                "manifest_sha256": scorepeek_core::recognition::result::numeric::NUMERIC_MODEL_MANIFEST_SHA256,
                "preprocessor_id": runtime.contract().preprocessor_id,
            }),
            Err(error) => serde_json::json!({
                "status": "unavailable",
                "reason": error.to_string(),
                "registered_manifest_sha256": scorepeek_core::recognition::result::numeric::NUMERIC_MODEL_MANIFEST_SHA256,
            }),
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
        Ok(serde_json::json!({
            "status": if active.is_some() { "active" } else { "unavailable" },
            "active_catalog_sha256": active.as_ref().map(|catalog| catalog.digest.as_str()),
            "source_url_sha256": state.source_url_sha256,
            "last_success_unix_seconds": state.last_success_unix_seconds,
            "etag_present": state.etag.is_some(),
            "last_modified_present": state.last_modified.is_some(),
            "last_failure": state.last_failure,
        }))
    })
    .unwrap_or_else(|error| serde_json::json!({"status": "unavailable", "reason": error}));
    let vulkan_layer = serde_json::to_value(vulkan_layer::inspect())
        .map_err(|error| format!("doctor report serialization failed: {error}"))?;
    Ok(serde_json::json!({
        "schema": "scorepeek-doctor-v5",
        "target_inventory": target_inventory,
        "numeric_model": numeric_model,
        "catalog": catalog,
        "vulkan_layer": vulkan_layer,
    }))
}

pub(super) fn print_doctor(format: OutputFormat) -> Result<(), String> {
    let report = collect_doctor_report()?;
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string(&report)
                .map_err(|error| format!("doctor report serialization failed: {error}"))?
        ),
        OutputFormat::Human => {
            println!("scorepeek doctor");
            println!(
                "  numeric model: {} ({})",
                report["numeric_model"]["status"]
                    .as_str()
                    .unwrap_or("unknown"),
                report["numeric_model"]["model_id"]
                    .as_str()
                    .unwrap_or("not available")
            );
            println!(
                "  catalog: {}",
                report["catalog"]["status"].as_str().unwrap_or("unknown")
            );
            println!(
                "  capture inventory: {}",
                if report["target_inventory"].is_object() {
                    "available"
                } else {
                    "unavailable"
                }
            );
            println!(
                "  Vulkan layer: {}",
                report["vulkan_layer"]["status"]
                    .as_str()
                    .unwrap_or("unknown")
            );
        }
    }
    Ok(())
}

pub(super) fn collect_frontend_doctor(
    format: OutputFormat,
) -> Result<scorepeek_frontend_api::CommandResult, String> {
    let report = collect_doctor_report()?;
    let field = |name: &str| {
        report
            .get(name)
            .cloned()
            .ok_or_else(|| format!("doctor report is missing {name}"))
    };
    Ok(scorepeek_frontend_api::CommandResult::Doctor {
        format: frontend_api_output_format(format),
        report: scorepeek_frontend_api::DoctorReport {
            schema: report["schema"].as_str().unwrap_or_default().to_owned(),
            target_inventory: field("target_inventory")?,
            numeric_model: field("numeric_model")?,
            catalog: field("catalog")?,
            vulkan_layer: field("vulkan_layer")?,
        },
    })
}
