use std::env;
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

const MANIFEST_NAME: &str = "VkLayer_SCOREPEEK_capture.json";
const LIBRARY_NAME: &str = "libscorepeek_vulkan_capture.so";

fn main() {
    let target = env::var("TARGET").expect("Cargo must provide TARGET");
    assert_eq!(
        target, "x86_64-unknown-linux-gnu",
        "scorepeek currently supports only x86_64-unknown-linux-gnu"
    );

    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo must provide CARGO_MANIFEST_DIR"),
    );
    let repository = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("scorepeek crate must be inside the workspace");
    let native = repository.join("native/vulkan-capture");
    emit_rerun_directives(&native);

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"));
    let prefix = out_dir.join("vulkan-layer");
    let output = Command::new("zig")
        .current_dir(repository)
        .args([
            "build",
            "--build-file",
            "native/vulkan-capture/build.zig",
            "--cache-dir",
        ])
        .arg(out_dir.join("zig-cache"))
        .arg("--global-cache-dir")
        .arg(out_dir.join("zig-global-cache"))
        .args([
            "--release=fast",
            "-Dtarget=x86_64-linux-gnu",
            "-Dcpu=baseline",
            "--prefix",
        ])
        .arg(&prefix)
        .output()
        .expect("failed to start pinned Zig; build scorepeek through mise");
    assert!(
        output.status.success(),
        "Scorepeek Vulkan layer build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let manifest = prefix
        .join("share/vulkan/explicit_layer.d")
        .join(MANIFEST_NAME);
    let library = prefix
        .join("share/scorepeek/vulkan-layer")
        .join(LIBRARY_NAME);
    write_payload_archive(
        &out_dir.join("scorepeek-vulkan-layer.zip"),
        &manifest,
        &library,
    );
}

fn emit_rerun_directives(native: &Path) {
    println!("cargo::rerun-if-changed={}", native.display());
    println!("cargo::rerun-if-env-changed=PATH");
}

fn write_payload_archive(output: &Path, manifest: &Path, library: &Path) {
    let file = File::create(output).expect("failed to create embedded Vulkan layer archive");
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(9))
        .last_modified_time(DateTime::default())
        .unix_permissions(0o644);

    for (name, path) in [(MANIFEST_NAME, manifest), (LIBRARY_NAME, library)] {
        let bytes = fs::read(path).unwrap_or_else(|error| {
            panic!(
                "failed to read Vulkan layer payload {}: {error}",
                path.display()
            )
        });
        archive
            .start_file(name, options)
            .unwrap_or_else(|error| panic!("failed to start {name} in layer archive: {error}"));
        archive
            .write_all(&bytes)
            .unwrap_or_else(|error| panic!("failed to write {name} to layer archive: {error}"));
    }
    archive
        .finish()
        .expect("failed to finish embedded Vulkan layer archive");
}
