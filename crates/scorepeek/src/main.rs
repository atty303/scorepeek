mod inventory;

use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use scorepeek::catalog::CatalogStore;
use scorepeek::catalog::{CatalogSync, CatalogSyncError};
use scorepeek::recognition::{
    self, CanonicalFrame, DIAGNOSTIC_TITLE_COMPARISON_KEY_ID, DIAGNOSTIC_TITLE_MINIMUM_CONFIDENCE,
};
use serde::Serialize;

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[OsString]) -> Result<(), String> {
    if let Some(result) = try_title_onnx_parity(args) {
        return result;
    }
    if let Some(result) = try_title_dictionary_audit(args) {
        return result;
    }
    match args {
        [command] if command == "doctor" => {
            println!("{}", inventory::collect().to_json());
            Ok(())
        }
        [catalog, sync] if catalog == "catalog" && sync == "sync" => sync_catalog(),
        [
            recognition,
            inspect,
            extraction_flag,
            extraction,
            digest_flag,
            digest,
            frame_flag,
            frame_id,
        ] if recognition == "recognition"
            && inspect == "inspect"
            && extraction_flag == "--extraction"
            && digest_flag == "--extraction-sha256"
            && frame_flag == "--frame-id" =>
        {
            inspect_canonical_frame(extraction, digest, frame_id)
        }
        [
            recognition,
            crop,
            extraction_flag,
            extraction,
            digest_flag,
            digest,
            frame_flag,
            frame_id,
            output_flag,
            output,
        ] if recognition == "recognition"
            && crop == "crop"
            && extraction_flag == "--extraction"
            && digest_flag == "--extraction-sha256"
            && frame_flag == "--frame-id"
            && output_flag == "--output" =>
        {
            crop_canonical_result(extraction, digest, frame_id, output)
        }
        [
            recognition,
            crop,
            extraction_flag,
            extraction,
            digest_flag,
            digest,
            frame_flag,
            frame_id,
            output_flag,
            output,
        ] if recognition == "recognition"
            && crop == "music-select-crop"
            && extraction_flag == "--extraction"
            && digest_flag == "--extraction-sha256"
            && frame_flag == "--frame-id"
            && output_flag == "--output" =>
        {
            crop_canonical_music_select(extraction, digest, frame_id, output)
        }
        [
            recognition,
            title_spike,
            store_flag,
            store,
            text_flag,
            text,
            confidence_flag,
            confidence,
        ] if recognition == "recognition"
            && title_spike == "title-spike"
            && store_flag == "--catalog-store"
            && text_flag == "--ocr-text"
            && confidence_flag == "--ocr-confidence" =>
        {
            diagnostic_title_spike(store, text, confidence)
        }
        [flag] if flag == "--help" || flag == "-h" => {
            print_usage();
            Ok(())
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("scorepeek {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => Err("usage: scorepeek --help".to_owned()),
    }
}

fn try_title_dictionary_audit(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        recognition,
        audit,
        store_flag,
        store,
        dictionary_flag,
        dictionary,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && audit == "title-dictionary-audit"
        && store_flag == "--catalog-store"
        && dictionary_flag == "--dictionary")
        .then(|| title_dictionary_audit(store, dictionary))
}

fn try_title_onnx_parity(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        recognition,
        parity,
        model_flag,
        model,
        reference_flag,
        reference,
        digest_flag,
        digest,
        crop_flag,
        crop,
        store_flag,
        store,
        dictionary_flag,
        dictionary,
        minimum_score_flag,
        minimum_score,
        minimum_margin_flag,
        minimum_margin,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && parity == "title-onnx-parity"
        && model_flag == "--model"
        && reference_flag == "--reference"
        && digest_flag == "--reference-sha256"
        && crop_flag == "--crop-artifact"
        && store_flag == "--catalog-store"
        && dictionary_flag == "--dictionary"
        && minimum_score_flag == "--minimum-log-probability"
        && minimum_margin_flag == "--minimum-runner-up-margin")
        .then(|| {
            title_onnx_parity([
                model,
                reference,
                digest,
                crop,
                store,
                dictionary,
                minimum_score,
                minimum_margin,
            ])
        })
}

fn inspect_canonical_frame(
    extraction: &OsStr,
    extraction_sha256: &OsStr,
    frame_id: &OsStr,
) -> Result<(), String> {
    let extraction_sha256 = extraction_sha256
        .to_str()
        .ok_or_else(|| "canonical extraction SHA-256 must be UTF-8".to_owned())?;
    let frame_id = frame_id
        .to_str()
        .ok_or_else(|| "canonical frame ID must be UTF-8".to_owned())?;
    let frame = CanonicalFrame::read_extraction(extraction, frame_id, extraction_sha256)
        .map_err(|error| error.to_string())?;
    let snapshot = recognition::inspect(&frame).map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&snapshot)
            .map_err(|error| format!("recognition result encoding failed: {error}"))?
    );
    Ok(())
}

fn crop_canonical_result(
    extraction: &OsStr,
    extraction_sha256: &OsStr,
    frame_id: &OsStr,
    output: &OsStr,
) -> Result<(), String> {
    let extraction_sha256 = extraction_sha256
        .to_str()
        .ok_or_else(|| "canonical extraction SHA-256 must be UTF-8".to_owned())?;
    let frame_id = frame_id
        .to_str()
        .ok_or_else(|| "canonical frame ID must be UTF-8".to_owned())?;
    let frame = CanonicalFrame::read_extraction(extraction, frame_id, extraction_sha256)
        .map_err(|error| error.to_string())?;
    let summary = recognition::export_result_crops(&frame, frame_id, output)
        .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("crop export summary encoding failed: {error}"))?
    );
    Ok(())
}

fn crop_canonical_music_select(
    extraction: &OsStr,
    extraction_sha256: &OsStr,
    frame_id: &OsStr,
    output: &OsStr,
) -> Result<(), String> {
    let extraction_sha256 = extraction_sha256
        .to_str()
        .ok_or_else(|| "canonical extraction SHA-256 must be UTF-8".to_owned())?;
    let frame_id = frame_id
        .to_str()
        .ok_or_else(|| "canonical frame ID must be UTF-8".to_owned())?;
    let frame = CanonicalFrame::read_extraction(extraction, frame_id, extraction_sha256)
        .map_err(|error| error.to_string())?;
    let summary = recognition::export_music_select_crops(&frame, frame_id, output)
        .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("crop export summary encoding failed: {error}"))?
    );
    Ok(())
}

#[derive(Serialize)]
struct DiagnosticTitleSpikeSummary {
    schema: &'static str,
    catalog_sha256: String,
    comparison_key_id: &'static str,
    minimum_confidence: f64,
    candidate: recognition::DiagnosticTitleCandidate,
}

fn diagnostic_title_spike(
    catalog_store: &OsStr,
    ocr_text: &OsStr,
    ocr_confidence: &OsStr,
) -> Result<(), String> {
    let catalog_store = absolute_directory(PathBuf::from(catalog_store), "catalog store")?;
    let ocr_text = ocr_text
        .to_str()
        .ok_or_else(|| "diagnostic OCR text must be UTF-8".to_owned())?;
    let ocr_confidence = ocr_confidence
        .to_str()
        .ok_or_else(|| "diagnostic OCR confidence must be UTF-8".to_owned())?
        .parse::<f64>()
        .map_err(|_| "diagnostic OCR confidence must be a decimal number".to_owned())?;
    let active = CatalogStore::new(catalog_store)
        .load_active()
        .map_err(|error| format!("active catalog load failed: {error}"))?
        .ok_or_else(|| "catalog store has no active catalog".to_owned())?;
    let candidate =
        recognition::diagnostic_title_candidate(&active.catalog, ocr_text, ocr_confidence)
            .map_err(|error| error.to_string())?;
    let summary = DiagnosticTitleSpikeSummary {
        schema: "scorepeek-diagnostic-title-spike-v1",
        catalog_sha256: active.digest,
        comparison_key_id: DIAGNOSTIC_TITLE_COMPARISON_KEY_ID,
        minimum_confidence: DIAGNOSTIC_TITLE_MINIMUM_CONFIDENCE,
        candidate,
    };
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("title spike result encoding failed: {error}"))?
    );
    Ok(())
}

#[derive(Serialize)]
struct TitleDictionaryAuditSummary {
    schema: &'static str,
    catalog_sha256: String,
    audit: recognition::CatalogTitleDictionaryAudit,
}

fn title_dictionary_audit(catalog_store: &OsStr, dictionary: &OsStr) -> Result<(), String> {
    let catalog_store = absolute_directory(PathBuf::from(catalog_store), "catalog store")?;
    let active = CatalogStore::new(catalog_store)
        .load_active()
        .map_err(|error| format!("active catalog load failed: {error}"))?
        .ok_or_else(|| "catalog store has no active catalog".to_owned())?;
    let audit = recognition::audit_catalog_title_dictionary(&active.catalog, Path::new(dictionary))
        .map_err(|error| error.to_string())?;
    let summary = TitleDictionaryAuditSummary {
        schema: "scorepeek-catalog-title-dictionary-audit-v1",
        catalog_sha256: active.digest,
        audit,
    };
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("title dictionary audit encoding failed: {error}"))?
    );
    Ok(())
}

fn title_onnx_parity(arguments: [&OsStr; 8]) -> Result<(), String> {
    let [
        model,
        reference,
        reference_sha256,
        crop,
        catalog_store,
        dictionary,
        minimum_log_probability,
        minimum_runner_up_margin,
    ] = arguments;
    let reference_sha256 = reference_sha256
        .to_str()
        .ok_or_else(|| "parity reference SHA-256 must be UTF-8".to_owned())?;
    let catalog_store = absolute_directory(PathBuf::from(catalog_store), "catalog store")?;
    let active = CatalogStore::new(catalog_store)
        .load_active()
        .map_err(|error| format!("active catalog load failed: {error}"))?
        .ok_or_else(|| "catalog store has no active catalog".to_owned())?;
    let minimum_log_probability =
        parse_f64(minimum_log_probability, "minimum title log probability")?;
    let minimum_runner_up_margin =
        parse_f64(minimum_runner_up_margin, "minimum title runner-up margin")?;
    let thresholds = recognition::DiagnosticTitleThresholds {
        minimum_log_probability,
        minimum_runner_up_margin,
    };
    let request = recognition::OnnxTitleDiagnosticRequest {
        model_path: Path::new(model),
        reference_directory: Path::new(reference),
        reference_sha256,
        crop_directory: Path::new(crop),
        catalog_sha256: &active.digest,
        inference_yml: Path::new(dictionary),
    };
    let summary = recognition::compare_paddle_onnx(request, &active.catalog, thresholds)
        .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("ONNX parity summary encoding failed: {error}"))?
    );
    Ok(())
}

fn parse_f64(value: &OsStr, label: &str) -> Result<f64, String> {
    value
        .to_str()
        .ok_or_else(|| format!("{label} must be UTF-8"))?
        .parse::<f64>()
        .map_err(|_| format!("{label} must be a decimal number"))
}

fn sync_catalog() -> Result<(), String> {
    let xdg_data_home = env::var_os("XDG_DATA_HOME");
    let xdg_cache_home = env::var_os("XDG_CACHE_HOME");
    let home = env::var_os("HOME");
    let (store_root, cache_root) = catalog_paths(
        xdg_data_home.as_deref(),
        xdg_cache_home.as_deref(),
        home.as_deref(),
    )?;
    let result = CatalogSync::new(store_root, cache_root)
        .sync()
        .map_err(|error| catalog_sync_error(&error))?;
    println!(
        "{}",
        serde_json::to_string(&result.into_summary())
            .map_err(|error| format!("catalog sync result encoding failed: {error}"))?
    );
    Ok(())
}

fn catalog_sync_error(error: &CatalogSyncError) -> String {
    format!(
        "scorepeek catalog sync failed: {}",
        error.redacted_message()
    )
}

fn catalog_paths(
    xdg_data_home: Option<&OsStr>,
    xdg_cache_home: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Result<(PathBuf, PathBuf), String> {
    let data = xdg_base_directory(xdg_data_home, home, ".local/share")?;
    let cache = xdg_base_directory(xdg_cache_home, home, ".cache")?;
    Ok((
        data.join("scorepeek/catalog"),
        cache.join("scorepeek/catalog/sources"),
    ))
}

fn xdg_base_directory(
    configured: Option<&OsStr>,
    home: Option<&OsStr>,
    fallback: &str,
) -> Result<PathBuf, String> {
    if let Some(configured) = configured {
        let path = PathBuf::from(configured);
        return absolute_directory(path, "XDG base directory");
    }
    let home =
        home.ok_or_else(|| "HOME is required when an XDG base directory is unset".to_owned())?;
    let home = absolute_directory(PathBuf::from(home), "HOME")?;
    Ok(home.join(fallback))
}

fn absolute_directory(path: PathBuf, name: &str) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() || !Path::new(&path).is_absolute() {
        return Err(format!("{name} must be an absolute, non-empty path"));
    }
    Ok(path)
}

fn print_usage() {
    println!(
        "scorepeek {}\n\nUsage:\n  scorepeek doctor\n  scorepeek catalog sync\n  scorepeek recognition inspect --extraction DIRECTORY --extraction-sha256 SHA256 --frame-id FRAME_ID\n  scorepeek recognition crop --extraction DIRECTORY --extraction-sha256 SHA256 --frame-id FRAME_ID --output DIRECTORY\n  scorepeek recognition music-select-crop --extraction DIRECTORY --extraction-sha256 SHA256 --frame-id FRAME_ID --output DIRECTORY\n  scorepeek recognition title-dictionary-audit --catalog-store DIRECTORY --dictionary FILE\n  scorepeek recognition title-spike --catalog-store DIRECTORY --ocr-text TEXT --ocr-confidence SCORE\n  scorepeek recognition title-onnx-parity --model FILE --reference DIRECTORY --reference-sha256 SHA256 --crop-artifact DIRECTORY --catalog-store DIRECTORY --dictionary FILE --minimum-log-probability SCORE --minimum-runner-up-margin SCORE",
        env!("CARGO_PKG_VERSION")
    );
}

#[cfg(test)]
mod tests {
    use super::{catalog_paths, catalog_sync_error};
    use scorepeek::catalog::{
        AdapterError, CatalogStoreError, CatalogSyncError, DqnAcquisitionError,
        TachiAcquisitionError, TachiResource, TextageAcquisitionError, TextageResource,
    };
    use std::ffi::OsStr;
    use std::path::PathBuf;

    #[test]
    fn catalog_paths_use_absolute_xdg_directories() {
        let paths =
            catalog_paths(Some(OsStr::new("/data")), Some(OsStr::new("/cache")), None).unwrap();
        assert_eq!(paths.0, PathBuf::from("/data/scorepeek/catalog"));
        assert_eq!(paths.1, PathBuf::from("/cache/scorepeek/catalog/sources"));
    }

    #[test]
    fn catalog_paths_fall_back_to_home_and_reject_relative_values() {
        let paths = catalog_paths(None, None, Some(OsStr::new("/home/test"))).unwrap();
        assert_eq!(
            paths.0,
            PathBuf::from("/home/test/.local/share/scorepeek/catalog")
        );
        assert_eq!(
            paths.1,
            PathBuf::from("/home/test/.cache/scorepeek/catalog/sources")
        );
        assert!(
            catalog_paths(
                Some(OsStr::new("relative")),
                Some(OsStr::new("/cache")),
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn catalog_sync_errors_do_not_expose_source_values() {
        let adapter = catalog_sync_error(&CatalogSyncError::DqnAcquisition(
            DqnAcquisitionError::Adapter(AdapterError::DuplicateRecord(
                "PRIVATE SOURCE SENTINEL".to_owned(),
            )),
        ));
        assert!(!adapter.contains("PRIVATE SOURCE SENTINEL"));
        assert!(adapter.contains("response validation failed"));

        let tachi = catalog_sync_error(&CatalogSyncError::TachiAcquisition(
            TachiAcquisitionError::Transport(
                TachiResource::Songs,
                "PRIVATE TRANSPORT SENTINEL".to_owned(),
            ),
        ));
        assert!(!tachi.contains("PRIVATE TRANSPORT SENTINEL"));
        assert!(tachi.contains("Tachi songs seed transport failed"));

        let textage = catalog_sync_error(&CatalogSyncError::TextageAcquisition(
            TextageAcquisitionError::Transport(
                TextageResource::Title,
                "PRIVATE TEXTAGE SENTINEL".to_owned(),
            ),
        ));
        assert!(!textage.contains("PRIVATE TEXTAGE SENTINEL"));
        assert!(textage.contains("Textage title table transport failed"));

        let store = catalog_sync_error(&CatalogSyncError::Store(
            CatalogStoreError::InvalidSnapshot("PRIVATE SNAPSHOT SENTINEL".to_owned()),
        ));
        assert!(!store.contains("PRIVATE SNAPSHOT SENTINEL"));
        assert!(store.contains("store operation failed"));
    }
}
