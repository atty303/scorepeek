mod inventory;

use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use scorepeek::catalog::{CatalogSync, CatalogSyncError};

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
    match args {
        [command] if command == "doctor" => {
            println!("{}", inventory::collect().to_json());
            Ok(())
        }
        [catalog, sync] if catalog == "catalog" && sync == "sync" => sync_catalog(),
        [flag] if flag == "--help" || flag == "-h" => {
            print_usage();
            Ok(())
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("scorepeek {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => Err("usage: scorepeek <doctor|catalog sync>".to_owned()),
    }
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
        "scorepeek {}\n\nUsage:\n  scorepeek doctor\n  scorepeek catalog sync",
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
