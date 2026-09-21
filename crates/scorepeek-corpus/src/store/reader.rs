//! Verified corpus object loading.

use super::{
    CompleteLabel, CorpusError, CorpusGeneration, CorpusStore, ErrorContext, GenerationSource,
    MANIFEST_STAGING_PREFIX, MAX_GENERATION_BYTES, MAX_LABEL_BYTES, MAX_REQUEST_BYTES,
    MAX_SOURCE_OBJECTS, Path, ReplayFrame, ReplayIndex, SourceManifest, canonical_json,
    digest_bytes, fs, read_bounded_regular, resolve_stored_source_path_unverified,
    validate_opaque_id, validate_regular_file, validate_sha256,
};

pub(crate) fn load_source_manifest(
    store: &CorpusStore,
    fixture_id: &str,
    expected_digest: &str,
) -> Result<SourceManifest, CorpusError> {
    let path = store
        .root
        .join("manifests")
        .join(format!("{fixture_id}.json"));
    validate_regular_file(&path, ErrorContext::Replay)?;
    let bytes = read_bounded_regular(&path, MAX_REQUEST_BYTES, ErrorContext::Replay)?;
    if digest_bytes(&bytes) != expected_digest {
        return Err(CorpusError::InvalidReplay(
            "source_manifest_sha256 does not match the stored manifest".to_owned(),
        ));
    }
    let manifest: SourceManifest = serde_json::from_slice(&bytes)?;
    manifest
        .validate()
        .map_err(|_| CorpusError::InvalidReplay("stored source manifest is invalid".to_owned()))?;
    if manifest.fixture_id != fixture_id || canonical_json(&manifest)? != bytes {
        return Err(CorpusError::InvalidReplay(
            "stored source manifest is not canonical or filename-bound".to_owned(),
        ));
    }
    Ok(manifest)
}

pub(crate) fn validate_source_binding(
    store: &CorpusStore,
    index: &ReplayIndex,
) -> Result<(), CorpusError> {
    let manifest_path = store
        .root
        .join("manifests")
        .join(format!("{}.json", index.fixture_id));
    let bytes = read_bounded_regular(&manifest_path, MAX_REQUEST_BYTES, ErrorContext::Replay)?;
    validate_regular_file(&manifest_path, ErrorContext::Replay)?;
    let manifest: SourceManifest = serde_json::from_slice(&bytes)?;
    manifest
        .validate()
        .map_err(|_| CorpusError::InvalidReplay("stored source manifest is invalid".to_owned()))?;
    let canonical = canonical_json(&manifest)?;
    if bytes != canonical {
        return Err(CorpusError::InvalidReplay(
            "stored source manifest is not canonical".to_owned(),
        ));
    }
    if digest_bytes(&canonical) != index.source_manifest_sha256 {
        return Err(CorpusError::InvalidReplay(
            "source_manifest_sha256 does not match the stored manifest".to_owned(),
        ));
    }
    if manifest.fixture_id != index.fixture_id
        || manifest.session_id != index.session_id
        || manifest.capture_profile_id != index.capture_profile_id
        || manifest.source != index.source
    {
        return Err(CorpusError::InvalidReplay(
            "replay index does not match its stored source manifest".to_owned(),
        ));
    }
    store
        .resolve_source_path(&manifest.source)
        .map(|_| ())
        .map_err(|_| CorpusError::InvalidReplay("stored source object is invalid".to_owned()))
}

pub(crate) fn validate_complete_label(
    store: &CorpusStore,
    frame: &ReplayFrame,
) -> Result<(), CorpusError> {
    let label_dir = store.root.join("labels");
    let path = label_dir.join(format!("{}.json", frame.labels_sha256));
    read_complete_label_object(&path, &frame.labels_sha256)?.validate_for(frame)
}

pub(crate) fn validate_label_store(label_dir: &Path) -> Result<(), CorpusError> {
    for entry in fs::read_dir(label_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(CorpusError::InvalidReplay(
                "complete-label store contains a non-UTF-8 entry".to_owned(),
            ));
        };
        let digest = name.strip_suffix(".json").ok_or_else(|| {
            CorpusError::InvalidReplay(
                "complete-label store contains an unrecognized entry".to_owned(),
            )
        })?;
        validate_sha256(digest, "complete-label filename", ErrorContext::Replay)?;
        let metadata = entry.path().metadata()?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_LABEL_BYTES as u64 {
            return Err(CorpusError::InvalidReplay(
                "complete-label store contains an invalid object".to_owned(),
            ));
        }
        read_complete_label_object(&entry.path(), digest)?;
    }
    Ok(())
}

pub(crate) fn read_complete_label_object(
    path: &Path,
    expected_digest: &str,
) -> Result<CompleteLabel, CorpusError> {
    validate_regular_file(path, ErrorContext::Replay)?;
    let bytes = read_bounded_regular(path, MAX_LABEL_BYTES, ErrorContext::Replay)?;
    if digest_bytes(&bytes) != expected_digest {
        return Err(CorpusError::InvalidReplay(
            "complete-label digest does not match its path".to_owned(),
        ));
    }
    let label: CompleteLabel = serde_json::from_slice(&bytes)?;
    label.validate_contents()?;
    if canonical_json(&label)? != bytes {
        return Err(CorpusError::InvalidReplay(
            "complete-label document is not canonical".to_owned(),
        ));
    }
    Ok(label)
}

pub(crate) fn read_generation_sources(
    manifest_dir: &Path,
    content_dir: &Path,
) -> Result<Vec<GenerationSource>, CorpusError> {
    let mut entries = fs::read_dir(manifest_dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    let mut sources = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(CorpusError::InvalidRequest(
                "manifest store contains a non-UTF-8 entry".to_owned(),
            ));
        };
        if name.starts_with(MANIFEST_STAGING_PREFIX) {
            continue;
        }
        let fixture_id = name.strip_suffix(".json").ok_or_else(|| {
            CorpusError::InvalidRequest("manifest store contains an unrecognized entry".to_owned())
        })?;
        validate_opaque_id(fixture_id, "stored fixture ID", ErrorContext::Request)?;
        validate_regular_file(&entry.path(), ErrorContext::Request)?;
        let bytes = read_bounded_regular(&entry.path(), MAX_REQUEST_BYTES, ErrorContext::Request)?;
        let manifest: SourceManifest = serde_json::from_slice(&bytes)?;
        manifest.validate()?;
        if manifest.fixture_id != fixture_id || canonical_json(&manifest)? != bytes {
            return Err(CorpusError::InvalidRequest(
                "stored source manifest is not canonical or filename-bound".to_owned(),
            ));
        }
        resolve_stored_source_path_unverified(
            &content_dir.join(&manifest.source.sha256),
            &manifest.source,
        )?;
        sources.push(GenerationSource {
            fixture_id: manifest.fixture_id,
            source_manifest_sha256: digest_bytes(&bytes),
        });
    }
    if sources.is_empty() || sources.len() > MAX_SOURCE_OBJECTS {
        return Err(CorpusError::InvalidRequest(
            "source manifest count is outside the admitted range".to_owned(),
        ));
    }
    Ok(sources)
}

pub(crate) fn load_generation(
    store: &CorpusStore,
    expected_digest: &str,
) -> Result<CorpusGeneration, CorpusError> {
    let path = store
        .root
        .join("generations")
        .join(format!("{expected_digest}.json"));
    validate_regular_file(&path, ErrorContext::Replay)?;
    let bytes = read_bounded_regular(&path, MAX_GENERATION_BYTES, ErrorContext::Replay)?;
    if digest_bytes(&bytes) != expected_digest {
        return Err(CorpusError::InvalidReplay(
            "corpus generation digest does not match its path".to_owned(),
        ));
    }
    let generation: CorpusGeneration = serde_json::from_slice(&bytes)?;
    generation.validate()?;
    if canonical_json(&generation)? != bytes {
        return Err(CorpusError::InvalidReplay(
            "corpus generation is not canonical".to_owned(),
        ));
    }
    Ok(generation)
}
