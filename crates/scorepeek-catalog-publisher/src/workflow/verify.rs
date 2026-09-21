//! Publisher artifact verification workflow.
use std::path::Path;

/// Verifies a catalog artifact and writes its verification output.
///
/// # Errors
/// Returns an error when the artifact is invalid or output publication fails.
pub fn verify(
    artifact: &Path,
    output: &Path,
) -> Result<scorepeek_core::catalog::artifact::ArtifactManifest, String> {
    scorepeek_core::catalog::artifact::verify_publisher(artifact, output)
        .map_err(|error| error.to_string())
}
