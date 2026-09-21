use std::{env, ffi::OsStr, fmt::Write as _, fs, io, path::Path};

use sha2::{Digest, Sha256};

#[path = "overlay-build-assets.rs"]
mod overlay_build_assets;

fn main() -> io::Result<()> {
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory");
    let root = Path::new(&manifest_dir)
        .parent()
        .and_then(Path::parent)
        .expect("overlay crate in workspace/crates");
    let mut digest = Sha256::new();
    digest.update(b"scorepeek-overlay-build-v1\0");
    for input in [
        "Cargo.toml",
        "Cargo.lock",
        "mise.toml",
        "scripts/overlay-build-assets.rs",
        "scripts/overlay-build.rs",
        "crates/scorepeek-overlay/Cargo.toml",
        "crates/scorepeek-overlay/src",
        "crates/scorepeek-overlay-web/Cargo.toml",
        "crates/scorepeek-overlay-web/src",
        "crates/scorepeek-overlay-ui/Cargo.toml",
        "crates/scorepeek-overlay-ui/src",
        "crates/scorepeek-overlay-ui/styles",
        "crates/scorepeek-overlay-handles/Cargo.toml",
        "crates/scorepeek-overlay-handles/src",
    ] {
        hash_input(root, Path::new(input), &mut digest)?;
    }
    let mut identity = String::with_capacity(64);
    for byte in digest.finalize() {
        write!(identity, "{byte:02x}").expect("write identity to string");
    }
    if env::var_os("CARGO_PKG_NAME").as_deref() == Some(OsStr::new("scorepeek-overlay")) {
        validate_embedded_bundle(&identity)?;
    }
    println!("cargo:rustc-env=SCOREPEEK_OVERLAY_BUILD_ID={identity}");
    Ok(())
}

fn validate_embedded_bundle(identity: &str) -> io::Result<()> {
    println!("cargo:rerun-if-env-changed=SCOREPEEK_WEB_ASSET_DIR");
    let configured = env::var_os("SCOREPEEK_WEB_ASSET_DIR").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "SCOREPEEK_WEB_ASSET_DIR is required for the embedded web build",
        )
    })?;
    let assets = overlay_build_assets::absolute_asset_directory(configured.into())?;
    println!("cargo:rerun-if-changed={}", assets.display());
    let complete = assets
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "web asset path has no parent"))?
        .join(".scorepeek-bundle-complete");
    println!("cargo:rerun-if-changed={}", complete.display());
    if !complete.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "embedded web bundle is incomplete at {}; run `mise run overlay:web:bundle`",
                assets.display()
            ),
        ));
    }

    let files =
        overlay_build_assets::validate_embedded_bundle(&assets, identity).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("{error}; run `mise run overlay:web:bundle`"),
            )
        })?;
    for path in files {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    Ok(())
}

fn hash_input(root: &Path, relative: &Path, digest: &mut Sha256) -> io::Result<()> {
    let path = root.join(relative);
    println!("cargo:rerun-if-changed={}", path.display());
    if path.is_dir() {
        let mut entries = fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<io::Result<Vec<_>>>()?;
        entries.sort();
        for entry in entries {
            hash_input(root, &relative.join(entry), digest)?;
        }
    } else {
        let name = relative.to_str().expect("UTF-8 overlay source path");
        let bytes = fs::read(path)?;
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    Ok(())
}
