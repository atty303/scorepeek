use std::{env, fmt::Write as _, fs, io, path::Path};

use sha2::{Digest, Sha256};

fn main() -> io::Result<()> {
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory");
    if env::var_os("CARGO_PKG_NAME").as_deref() == Some("scorepeek-overlay-web-host".as_ref()) {
        let asset_dir = env::var_os("SCOREPEEK_WEB_ASSET_DIR").expect("web asset directory");
        // The mise dependency creates this input; Cargo only invalidates the embedding crate.
        println!("cargo:rerun-if-env-changed=SCOREPEEK_WEB_ASSET_DIR");
        println!("cargo:rerun-if-changed={}", Path::new(&asset_dir).display());
    }
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
        "scripts/overlay-build.rs",
        "crates/scorepeek-overlay-web-client/Cargo.toml",
        "crates/scorepeek-overlay-web-client/src",
        "crates/scorepeek-overlay/Cargo.toml",
        "crates/scorepeek-overlay/src",
        "crates/scorepeek-overlay-runtime/styles",
        "crates/scorepeek-overlay-web-host/Cargo.toml",
        "crates/scorepeek-overlay-web-host/src",
        "crates/scorepeek-overlay-wayland/Cargo.toml",
        "crates/scorepeek-overlay-wayland/src",
        "crates/scorepeek-overlay-wayland-handles/Cargo.toml",
        "crates/scorepeek-overlay-wayland-handles/src",
    ] {
        hash_input(root, Path::new(input), &mut digest)?;
    }
    let mut identity = String::with_capacity(64);
    for byte in digest.finalize() {
        write!(identity, "{byte:02x}").expect("write identity to string");
    }
    println!("cargo:rustc-env=SCOREPEEK_OVERLAY_BUILD_ID={identity}");
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
