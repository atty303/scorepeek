//! Publisher-side ZIP creation and validation.

pub use scorepeek_resources::artifact::{ARTIFACT_SCHEMA, ArtifactManifest};
use scorepeek_resources::artifact::{
    ArtifactError, CATALOG_ENTRY, MANIFEST_ENTRY, NOTICES_ENTRY, extract_client_verified,
};
use scorepeek_resources::validate_publisher_snapshot;
use sha2::{Digest as _, Sha256};
use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::path::Path;
use tempfile::Builder;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const MAX_CATALOG_BYTES: u64 = 128 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 16 * 1024;
const MAX_NOTICES_BYTES: u64 = 1024 * 1024;

/// Packages the exact `SQLite` snapshot and notices into the three-entry distribution ZIP.
///
/// # Errors
/// Returns an error for invalid manifest fields, digest mismatch, oversized inputs, an existing
/// output, or ZIP publication failure.
pub fn build(
    catalog_path: &Path,
    notices_path: &Path,
    output: &Path,
    manifest: &ArtifactManifest,
) -> Result<(), ArtifactError> {
    manifest.validate()?;
    if digest_bounded(catalog_path, MAX_CATALOG_BYTES)? != manifest.sqlite_sha256 {
        return Err(ArtifactError::Invalid(
            "SQLite digest does not match manifest".to_owned(),
        ));
    }
    let notices = read_bounded(notices_path, MAX_NOTICES_BYTES)?;
    std::str::from_utf8(&notices)
        .map_err(|_| ArtifactError::Invalid("notices are not UTF-8".to_owned()))?;
    let manifest_bytes = serde_json::to_vec_pretty(manifest)?;
    if manifest_bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(ArtifactError::Invalid("manifest is too large".to_owned()));
    }
    let parent = output
        .parent()
        .ok_or_else(|| ArtifactError::Invalid("output has no parent".to_owned()))?;
    let mut temporary = Builder::new()
        .prefix(".catalog-artifact-staging-")
        .tempfile_in(parent)?;
    {
        let mut archive = ZipWriter::new(temporary.as_file_mut());
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .unix_permissions(0o644);
        archive.start_file(CATALOG_ENTRY, options)?;
        copy_bounded(catalog_path, &mut archive, MAX_CATALOG_BYTES)?;
        archive.start_file(MANIFEST_ENTRY, options)?;
        archive.write_all(&manifest_bytes)?;
        archive.write_all(b"\n")?;
        archive.start_file(NOTICES_ENTRY, options)?;
        archive.write_all(&notices)?;
        archive.finish()?;
    }
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(output)
        .map_err(|error| error.error)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

/// Reopens an artifact in a fresh directory and applies the complete production catalog loader.
///
/// # Errors
/// Returns client-boundary failures plus `SQLite` or catalog invariant failures.
pub fn verify_publisher(
    artifact: &Path,
    destination: &Path,
) -> Result<ArtifactManifest, ArtifactError> {
    let extracted = extract_client_verified(artifact, destination)?;
    let catalog =
        validate_publisher_snapshot(&extracted.catalog_path, &extracted.manifest.sqlite_sha256)
            .map_err(|error| ArtifactError::Invalid(error.to_string()))?;
    if catalog.semantic_digest() != extracted.manifest.semantic_digest {
        return Err(ArtifactError::Invalid(
            "semantic digest does not match the production-loaded catalog".to_owned(),
        ));
    }
    Ok(extracted.manifest)
}

fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>, ArtifactError> {
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(ArtifactError::Invalid(format!(
            "{} is not a bounded regular file",
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or_default());
    File::open(path)?
        .take(maximum + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(ArtifactError::Invalid(format!(
            "{} exceeds its size limit while reading",
            path.display()
        )));
    }
    Ok(bytes)
}

pub(crate) fn digest_bounded(path: &Path, maximum: u64) -> Result<String, ArtifactError> {
    let bytes = read_bounded(path, maximum)?;
    Ok(hex(&Sha256::digest(bytes)))
}

fn copy_bounded(
    source: &Path,
    output: &mut impl io::Write,
    maximum: u64,
) -> Result<(), ArtifactError> {
    let metadata = source.metadata()?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(ArtifactError::Invalid(format!(
            "{} is not a bounded regular file",
            source.display()
        )));
    }
    let copied = io::copy(&mut File::open(source)?.take(maximum + 1), output)?;
    if copied > maximum {
        return Err(ArtifactError::Invalid(format!(
            "{} exceeds its size limit while reading",
            source.display()
        )));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(value, "{byte:02x}").expect("writing to a String cannot fail");
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn published_zip_is_readable_by_client_verification_and_catalog_reader() {
        use scorepeek_core::catalog::test_support::{SyntheticTachiRecord, catalog_from_tachi};
        use scorepeek_core::catalog::{Chart, ChartKey, Difficulty, DisplayVariantKind, PlayType};

        let root = tempfile::tempdir().unwrap();
        let catalog = catalog_from_tachi(&[SyntheticTachiRecord {
            id: "synthetic-song",
            title: "SYNTHETIC SONG",
            title_kind: DisplayVariantKind::InGameDisplay,
            artist: "SYNTHETIC ARTIST",
            version: "SYNTHETIC",
            charts: vec![Chart {
                key: ChartKey {
                    play_type: PlayType::Single,
                    difficulty: Difficulty::Normal,
                },
                level: 4,
                notes: 400,
            }],
            primary_infinitas: true,
        }]);
        let catalog_path = root.path().join("catalog.sqlite3");
        crate::snapshot::write_snapshot(&catalog_path, &catalog).unwrap();
        let digest = digest_bounded(&catalog_path, MAX_CATALOG_BYTES).unwrap();
        let notices = root.path().join("notices.md");
        fs::write(&notices, b"synthetic notice\n").unwrap();
        let zip = root.path().join("catalog.zip");
        let manifest = ArtifactManifest {
            schema: ARTIFACT_SCHEMA.to_owned(),
            sqlite_sha256: digest.clone(),
            semantic_digest: catalog.semantic_digest(),
            artifact_revision: 1,
            generator_commit: "a".repeat(40),
            workflow_run_url: None,
        };
        build(&catalog_path, &notices, &zip, &manifest).unwrap();
        assert_eq!(
            verify_publisher(&zip, &root.path().join("publisher-verified")).unwrap(),
            manifest
        );
        let extracted =
            extract_client_verified(&zip, &root.path().join("client-verified")).unwrap();
        let loaded =
            validate_publisher_snapshot(&extracted.catalog_path, &manifest.sqlite_sha256).unwrap();
        assert_eq!(loaded, catalog);
    }
}
