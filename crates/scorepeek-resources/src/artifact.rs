use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tempfile::Builder;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::store::validate_publisher_snapshot;

pub const ARTIFACT_SCHEMA: &str = "scorepeek-catalog-artifact-v1";
pub const CATALOG_ENTRY: &str = "catalog.sqlite3";
pub const MANIFEST_ENTRY: &str = "manifest.json";
pub const NOTICES_ENTRY: &str = "THIRD_PARTY_NOTICES.md";
pub const MAX_ARTIFACT_BYTES: u64 = 160 * 1024 * 1024;
const MAX_CATALOG_BYTES: u64 = 128 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 16 * 1024;
const MAX_NOTICES_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactManifest {
    pub schema: String,
    pub sqlite_sha256: String,
    pub semantic_digest: String,
    pub artifact_revision: u64,
    pub generator_commit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_run_url: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractedArtifact {
    pub manifest: ArtifactManifest,
    pub catalog_path: PathBuf,
    pub notices_path: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    Candidate,
    Current,
}

#[derive(Debug)]
pub enum ArtifactError {
    Io(io::Error),
    Zip(zip::result::ZipError),
    Json(serde_json::Error),
    Invalid(String),
}

impl fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "catalog artifact I/O failed: {error}"),
            Self::Zip(error) => write!(formatter, "catalog artifact ZIP failed: {error}"),
            Self::Json(error) => write!(formatter, "catalog artifact manifest failed: {error}"),
            Self::Invalid(detail) => write!(formatter, "invalid catalog artifact: {detail}"),
        }
    }
}

impl Error for ArtifactError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Zip(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

impl From<io::Error> for ArtifactError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<zip::result::ZipError> for ArtifactError {
    fn from(error: zip::result::ZipError) -> Self {
        Self::Zip(error)
    }
}

impl From<serde_json::Error> for ArtifactError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl ArtifactManifest {
    /// Validates the schema and bounded identity fields shared by official and custom artifacts.
    ///
    /// # Errors
    /// Returns a stable structural validation failure.
    pub fn validate(&self) -> Result<(), ArtifactError> {
        if self.schema != ARTIFACT_SCHEMA {
            return Err(ArtifactError::Invalid(format!(
                "schema is {:?}, expected {ARTIFACT_SCHEMA:?}",
                self.schema
            )));
        }
        for (name, value, length) in [
            ("sqlite_sha256", self.sqlite_sha256.as_str(), 64),
            ("semantic_digest", self.semantic_digest.as_str(), 64),
            ("generator_commit", self.generator_commit.as_str(), 40),
        ] {
            if !is_lower_hex(value, length) {
                return Err(ArtifactError::Invalid(format!(
                    "{name} must be {length} lowercase hexadecimal characters"
                )));
            }
        }
        if self.artifact_revision == 0 {
            return Err(ArtifactError::Invalid(
                "artifact_revision must be positive".to_owned(),
            ));
        }
        if self
            .workflow_run_url
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 2048)
        {
            return Err(ArtifactError::Invalid(
                "workflow_run_url is empty or too long".to_owned(),
            ));
        }
        Ok(())
    }
}

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

/// Extracts and verifies the client-side trust boundary: inventory, sizes, ZIP/CRC and `SQLite`
/// digest. It intentionally does not adjudicate catalog records or source evidence.
///
/// # Errors
/// Returns an error for any malformed or oversized package, manifest, notice, or digest mismatch.
pub fn extract_client_verified(
    artifact: &Path,
    destination: &Path,
) -> Result<ExtractedArtifact, ArtifactError> {
    let metadata = artifact.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_ARTIFACT_BYTES {
        return Err(ArtifactError::Invalid(
            "ZIP is not a bounded regular file".to_owned(),
        ));
    }
    fs::create_dir(destination)?;
    let mut archive = ZipArchive::new(File::open(artifact)?)?;
    if archive.len() != 3 {
        return Err(ArtifactError::Invalid(format!(
            "ZIP contains {} entries, expected 3",
            archive.len()
        )));
    }
    let expected = BTreeSet::from([
        CATALOG_ENTRY.to_owned(),
        MANIFEST_ENTRY.to_owned(),
        NOTICES_ENTRY.to_owned(),
    ]);
    let mut observed = BTreeSet::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_owned();
        if entry.is_dir() || !expected.contains(&name) || !observed.insert(name.clone()) {
            return Err(ArtifactError::Invalid(format!(
                "unexpected or duplicate ZIP entry {name:?}"
            )));
        }
        let limit = entry_limit(&name);
        if entry.size() > limit {
            return Err(ArtifactError::Invalid(format!(
                "ZIP entry {name:?} exceeds its size limit"
            )));
        }
        let path = destination.join(&name);
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let copied = io::copy(&mut entry.by_ref().take(limit + 1), &mut output)?;
        if copied > limit {
            return Err(ArtifactError::Invalid(format!(
                "ZIP entry {name:?} exceeds its size limit while extracting"
            )));
        }
        output.sync_all()?;
    }
    if observed != expected {
        return Err(ArtifactError::Invalid(
            "ZIP entry inventory is incomplete".to_owned(),
        ));
    }
    File::open(destination)?.sync_all()?;
    let manifest_bytes = read_bounded(&destination.join(MANIFEST_ENTRY), MAX_MANIFEST_BYTES)?;
    let manifest: ArtifactManifest = serde_json::from_slice(&manifest_bytes)?;
    manifest.validate()?;
    let notices_path = destination.join(NOTICES_ENTRY);
    let notices = read_bounded(&notices_path, MAX_NOTICES_BYTES)?;
    std::str::from_utf8(&notices)
        .map_err(|_| ArtifactError::Invalid("notices are not UTF-8".to_owned()))?;
    let catalog_path = destination.join(CATALOG_ENTRY);
    if digest_bounded(&catalog_path, MAX_CATALOG_BYTES)? != manifest.sqlite_sha256 {
        return Err(ArtifactError::Invalid(
            "SQLite digest does not match manifest".to_owned(),
        ));
    }
    Ok(ExtractedArtifact {
        manifest,
        catalog_path,
        notices_path,
    })
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

/// Selects the current bytes for a semantic/artifact/notices no-op, otherwise the candidate.
///
/// # Errors
/// Returns an error if either artifact is invalid or output publication fails.
pub fn select(
    candidate_path: &Path,
    current_path: &Path,
    output: &Path,
    scratch: &Path,
) -> Result<Selection, ArtifactError> {
    let candidate_dir = scratch.join("candidate");
    let current_dir = scratch.join("current");
    let candidate = extract_client_verified(candidate_path, &candidate_dir)?;
    let current = extract_client_verified(current_path, &current_dir)?;
    let unchanged = candidate.manifest.semantic_digest == current.manifest.semantic_digest
        && candidate.manifest.artifact_revision == current.manifest.artifact_revision
        && fs::read(&candidate.notices_path)? == fs::read(&current.notices_path)?;
    let (source, selection) = if unchanged {
        (current_path, Selection::Current)
    } else {
        (candidate_path, Selection::Candidate)
    };
    copy_noclobber(source, output)?;
    Ok(selection)
}

fn copy_noclobber(source: &Path, output: &Path) -> Result<(), ArtifactError> {
    let mut input = File::open(source)?;
    let mut target = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(output)?;
    io::copy(&mut input, &mut target)?;
    target.sync_all()?;
    Ok(())
}

fn entry_limit(name: &str) -> u64 {
    match name {
        CATALOG_ENTRY => MAX_CATALOG_BYTES,
        MANIFEST_ENTRY => MAX_MANIFEST_BYTES,
        NOTICES_ENTRY => MAX_NOTICES_BYTES,
        _ => 0,
    }
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

fn digest_bounded(path: &Path, maximum: u64) -> Result<String, ArtifactError> {
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

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
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

    #[test]
    fn manifest_rejects_zero_revision_and_missing_custom_workflow_is_valid() {
        let mut manifest = ArtifactManifest {
            schema: ARTIFACT_SCHEMA.to_owned(),
            sqlite_sha256: "a".repeat(64),
            semantic_digest: "b".repeat(64),
            artifact_revision: 1,
            generator_commit: "c".repeat(40),
            workflow_run_url: None,
        };
        manifest.validate().unwrap();
        manifest.artifact_revision = 0;
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn no_op_selection_preserves_the_current_zip_bytes() {
        let root = tempfile::tempdir().unwrap();
        let current = test_package(root.path(), "current", "a".repeat(40), 1, b"notice\n");
        let candidate = test_package(root.path(), "candidate", "b".repeat(40), 1, b"notice\n");
        let output = root.path().join("selected.zip");
        let scratch = root.path().join("scratch");
        fs::create_dir(&scratch).unwrap();
        assert_eq!(
            select(&candidate, &current, &output, &scratch).unwrap(),
            Selection::Current
        );
        assert_eq!(fs::read(output).unwrap(), fs::read(current).unwrap());
    }

    #[test]
    fn artifact_revision_or_notices_change_selects_the_candidate() {
        let root = tempfile::tempdir().unwrap();
        let current = test_package(root.path(), "current", "a".repeat(40), 1, b"notice\n");
        let candidate = test_package(
            root.path(),
            "candidate",
            "b".repeat(40),
            2,
            b"updated notice\n",
        );
        let output = root.path().join("selected.zip");
        let scratch = root.path().join("scratch");
        fs::create_dir(&scratch).unwrap();
        assert_eq!(
            select(&candidate, &current, &output, &scratch).unwrap(),
            Selection::Candidate
        );
        assert_eq!(fs::read(output).unwrap(), fs::read(candidate).unwrap());
    }

    #[test]
    fn oversized_zip_is_rejected_before_opening() {
        let root = tempfile::tempdir().unwrap();
        let artifact = root.path().join("oversized.zip");
        File::create(&artifact)
            .unwrap()
            .set_len(MAX_ARTIFACT_BYTES + 1)
            .unwrap();
        assert!(
            extract_client_verified(&artifact, &root.path().join("extract"))
                .unwrap_err()
                .to_string()
                .contains("bounded regular file")
        );
    }

    #[test]
    fn sqlite_digest_mismatch_is_rejected_after_crc_verification() {
        let root = tempfile::tempdir().unwrap();
        let artifact = root.path().join("mismatch.zip");
        let manifest = ArtifactManifest {
            schema: ARTIFACT_SCHEMA.to_owned(),
            sqlite_sha256: hex(&Sha256::digest(b"expected bytes")),
            semantic_digest: "c".repeat(64),
            artifact_revision: 1,
            generator_commit: "d".repeat(40),
            workflow_run_url: None,
        };
        let mut archive = ZipWriter::new(File::create(&artifact).unwrap());
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        archive.start_file(CATALOG_ENTRY, options).unwrap();
        archive.write_all(b"different but CRC-valid bytes").unwrap();
        archive.start_file(MANIFEST_ENTRY, options).unwrap();
        serde_json::to_writer(&mut archive, &manifest).unwrap();
        archive.start_file(NOTICES_ENTRY, options).unwrap();
        archive.write_all(b"notice\n").unwrap();
        archive.finish().unwrap();

        assert!(
            extract_client_verified(&artifact, &root.path().join("extract"))
                .unwrap_err()
                .to_string()
                .contains("digest does not match")
        );
    }

    #[test]
    fn corrupt_entry_crc_is_rejected_before_activation() {
        let root = tempfile::tempdir().unwrap();
        let artifact = root.path().join("corrupt-crc.zip");
        let marker = b"unique stored catalog payload 12345";
        let manifest = ArtifactManifest {
            schema: ARTIFACT_SCHEMA.to_owned(),
            sqlite_sha256: hex(&Sha256::digest(marker)),
            semantic_digest: "c".repeat(64),
            artifact_revision: 1,
            generator_commit: "d".repeat(40),
            workflow_run_url: None,
        };
        let mut archive = ZipWriter::new(File::create(&artifact).unwrap());
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        archive.start_file(CATALOG_ENTRY, options).unwrap();
        archive.write_all(marker).unwrap();
        archive.start_file(MANIFEST_ENTRY, options).unwrap();
        serde_json::to_writer(&mut archive, &manifest).unwrap();
        archive.start_file(NOTICES_ENTRY, options).unwrap();
        archive.write_all(b"notice\n").unwrap();
        archive.finish().unwrap();

        let mut bytes = fs::read(&artifact).unwrap();
        let offset = bytes
            .windows(marker.len())
            .position(|window| window == marker)
            .unwrap();
        bytes[offset] ^= 0x01;
        fs::write(&artifact, bytes).unwrap();

        assert!(extract_client_verified(&artifact, &root.path().join("extract")).is_err());
    }

    fn test_package(
        root: &Path,
        name: &str,
        generator_commit: String,
        artifact_revision: u64,
        notices: &[u8],
    ) -> PathBuf {
        let catalog = root.join(format!("{name}.sqlite3"));
        fs::write(&catalog, b"synthetic sqlite bytes").unwrap();
        let notices_path = root.join(format!("{name}-notices.md"));
        fs::write(&notices_path, notices).unwrap();
        let output = root.join(format!("{name}.zip"));
        build(
            &catalog,
            &notices_path,
            &output,
            &ArtifactManifest {
                schema: ARTIFACT_SCHEMA.to_owned(),
                sqlite_sha256: digest_bounded(&catalog, MAX_CATALOG_BYTES).unwrap(),
                semantic_digest: "c".repeat(64),
                artifact_revision,
                generator_commit,
                workflow_run_url: None,
            },
        )
        .unwrap();
        output
    }
}
