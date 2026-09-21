//! Private corpus filesystem layout and store entry points.

use super::*;

#[derive(Clone, Debug)]
pub struct CorpusStore {
    pub(crate) root: PathBuf,
}

impl CorpusStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[allow(dead_code)]
    pub(crate) fn register_external_source(
        &self,
        source_path: &Path,
        source: &ContentRef,
    ) -> Result<PathBuf, CorpusError> {
        self.validate_root()?;
        source.validate(ErrorContext::Request)?;
        let canonical_path = fs::canonicalize(source_path)?;
        validate_external_source_file(&canonical_path, source, false)?;
        let path = canonical_path.to_str().ok_or_else(|| {
            CorpusError::InvalidRequest("external source path must be UTF-8".to_owned())
        })?;
        let locator = ExternalSourceLocator {
            schema: EXTERNAL_SOURCE_SCHEMA.to_owned(),
            source: source.clone(),
            path: path.to_owned(),
        };
        let locator_bytes = canonical_json(&locator)?;

        preflight_managed_components(&self.root)?;
        create_private_directory(&self.root)?;
        let lock = open_store_lock(&self.root, true)?;
        lock.lock()?;
        preflight_managed_components(&self.root)?;
        let content_dir = self.root.join("content");
        let manifest_dir = self.root.join("manifests");
        let label_dir = self.root.join("labels");
        create_private_directory(&content_dir)?;
        create_private_directory(&manifest_dir)?;
        create_private_directory(&label_dir)?;
        recover_staging(&content_dir, &manifest_dir)?;

        let destination = content_dir.join(&source.sha256);
        match destination.symlink_metadata() {
            Ok(entry_metadata) => {
                let metadata = destination.metadata()?;
                if !metadata.is_dir() {
                    return Err(CorpusError::InvalidRequest(
                        "content-addressed destination is not a directory".to_owned(),
                    ));
                }
                let stored_media = destination.join(SOURCE_FILE);
                if stored_media.exists() {
                    let resolved = resolve_stored_source_path(&destination, source)?;
                    drop(lock);
                    return Ok(resolved);
                }
                let (existing, existing_path) = read_external_source_locator(&destination, source)?;
                if existing == locator
                    && existing_path == canonical_path
                    && validate_external_source_file(&canonical_path, source, false).is_ok()
                {
                    drop(lock);
                    return Ok(canonical_path);
                }
                if entry_metadata.file_type().is_symlink() {
                    return Err(CorpusError::InvalidRequest(
                        "symlinked content destination cannot be updated".to_owned(),
                    ));
                }
                replace_atomic_file(
                    &destination,
                    &destination.join(EXTERNAL_SOURCE_FILE),
                    &locator_bytes,
                    SOURCE_STAGING_PREFIX,
                )?;
                sync_stored_source_and_parent(&destination, &content_dir)?;
                drop(lock);
                return Ok(canonical_path);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        let staging = Builder::new()
            .prefix(SOURCE_STAGING_PREFIX)
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(&content_dir)?;
        let locator_path = staging.path().join(EXTERNAL_SOURCE_FILE);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&locator_path)?;
        file.write_all(&locator_bytes)?;
        file.flush()?;
        file.sync_all()?;
        File::open(staging.path())?.sync_all()?;
        let staging_path = staging.keep();
        fs::rename(staging_path, &destination)?;
        sync_stored_source_and_parent(&destination, &content_dir)?;
        drop(lock);
        Ok(canonical_path)
    }

    pub(crate) fn resolve_source_path(&self, source: &ContentRef) -> Result<PathBuf, CorpusError> {
        resolve_stored_source_path(&self.root.join("content").join(&source.sha256), source)
    }

    pub(crate) fn open_verified_source(&self, source: &ContentRef) -> Result<File, CorpusError> {
        let path = resolve_stored_source_path_unverified(
            &self.root.join("content").join(&source.sha256),
            source,
        )?;
        let mut file = File::open(path)?;
        verify_open_source(&mut file, source)?;
        Ok(file)
    }

    /// Copies immutable source media into the private content store and binds an opaque
    /// fixture ID to a deterministic manifest.
    ///
    /// Identical content and metadata are idempotent. Reusing a fixture ID for different metadata
    /// fails without changing that binding.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed metadata, non-regular or oversized source media, conflicting
    /// fixture IDs, or failed durable storage operations.
    pub fn ingest(
        &self,
        source_path: impl AsRef<Path>,
        request_path: impl AsRef<Path>,
    ) -> Result<SourceManifest, CorpusError> {
        let request = read_ingest_request(request_path.as_ref())?;
        self.ingest_bound(source_path.as_ref(), None, |_| Ok(request), |_| Ok(()))
            .map(|(manifest, ())| manifest)
    }

    #[allow(dead_code)]
    fn ingest_verified_recording_with<T>(
        &self,
        source_path: &Path,
        capture_profile_id: String,
        expected_source: &ContentRef,
        inspect_staged_source: impl FnOnce(&Path) -> Result<T, CorpusError>,
    ) -> Result<(SourceManifest, T), CorpusError> {
        self.ingest_bound(
            source_path,
            Some(expected_source),
            move |source| {
                Ok(IngestRequest {
                    schema: INGEST_REQUEST_SCHEMA.to_owned(),
                    fixture_id: source.sha256.clone(),
                    session_id: source.sha256.clone(),
                    capture_profile_id,
                })
            },
            inspect_staged_source,
        )
    }

    fn ingest_bound<T>(
        &self,
        source_path: &Path,
        expected_source: Option<&ContentRef>,
        request_for_source: impl FnOnce(&ContentRef) -> Result<IngestRequest, CorpusError>,
        inspect_staged_source: impl FnOnce(&Path) -> Result<T, CorpusError>,
    ) -> Result<(SourceManifest, T), CorpusError> {
        self.validate_root()?;
        validate_source_file(source_path)?;

        preflight_managed_components(&self.root)?;
        create_private_directory(&self.root)?;
        let lock = open_store_lock(&self.root, true)?;
        lock.lock()?;
        preflight_managed_components(&self.root)?;
        let content_dir = self.root.join("content");
        let manifest_dir = self.root.join("manifests");
        let label_dir = self.root.join("labels");
        create_private_directory(&content_dir)?;
        create_private_directory(&manifest_dir)?;
        create_private_directory(&label_dir)?;
        recover_staging(&content_dir, &manifest_dir)?;

        let staging = Builder::new()
            .prefix(SOURCE_STAGING_PREFIX)
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(&content_dir)?;
        let staged_source = staging.path().join(SOURCE_FILE);
        let source = copy_source(source_path, &staged_source, expected_source)?;
        let request = request_for_source(&source)?;
        request.validate()?;
        let staged_inspection = inspect_staged_source(&staged_source)?;
        File::open(&staged_source)?.sync_all()?;
        File::open(staging.path())?.sync_all()?;

        let manifest = SourceManifest {
            schema: SOURCE_MANIFEST_SCHEMA.to_owned(),
            fixture_id: request.fixture_id,
            session_id: request.session_id,
            capture_profile_id: request.capture_profile_id,
            source,
        };
        let manifest_bytes = canonical_json(&manifest)?;
        let manifest_path = manifest_dir.join(format!("{}.json", manifest.fixture_id));
        let manifest_exists = match manifest_path.metadata() {
            Ok(_) => {
                if read_bounded_regular(&manifest_path, MAX_REQUEST_BYTES, ErrorContext::Request)?
                    != manifest_bytes
                {
                    return Err(CorpusError::FixtureConflict);
                }
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };

        let destination = content_dir.join(&manifest.source.sha256);
        let destination_exists = match destination.metadata() {
            Ok(metadata) if metadata.is_dir() => true,
            Ok(_) => {
                return Err(CorpusError::InvalidRequest(
                    "content-addressed destination is not a directory".to_owned(),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        if destination_exists {
            validate_stored_source(&destination, &manifest.source)?;
            sync_stored_source_and_parent(&destination, &content_dir)?;
            staging.close()?;
            File::open(&content_dir)?.sync_all()?;
        } else {
            let staging_path = staging.keep();
            fs::rename(staging_path, &destination)?;
            sync_stored_source_and_parent(&destination, &content_dir)?;
        }

        if !manifest_exists {
            write_atomic_file(
                &manifest_dir,
                &manifest_path,
                &manifest_bytes,
                MANIFEST_STAGING_PREFIX,
            )?;
        }
        sync_file_and_parent(&manifest_path, &manifest_dir)?;
        drop(lock);
        Ok((manifest, staged_inspection))
    }

    /// Seals every source binding currently present in this private store into one immutable
    /// content-addressed corpus generation.
    ///
    /// # Errors
    ///
    /// Returns an error if the generation ID is invalid, the source store is malformed, a source
    /// object no longer matches its canonical manifest, or durable publication fails.
    pub fn seal_generation(&self, generation_id: &str) -> Result<GenerationSummary, CorpusError> {
        self.validate_root()?;
        validate_opaque_id(generation_id, "generation_id", ErrorContext::Request)?;
        preflight_managed_components(&self.root)?;
        validate_directory(&self.root, ErrorContext::Request)?;
        let content_dir = self.root.join("content");
        let manifest_dir = self.root.join("manifests");
        validate_directory(&content_dir, ErrorContext::Request)?;
        validate_directory(&manifest_dir, ErrorContext::Request)?;
        let lock = open_store_lock(&self.root, false)?;
        lock.lock()?;
        preflight_managed_components(&self.root)?;
        recover_staging(&content_dir, &manifest_dir)?;

        let generation_dir = self.root.join("generations");
        create_private_directory(&generation_dir)?;
        recover_generation_staging(&generation_dir)?;
        let sources = read_generation_sources(&manifest_dir, &content_dir)?;
        let generation = CorpusGeneration {
            schema: GENERATION_SCHEMA.to_owned(),
            generation_id: generation_id.to_owned(),
            sources,
        };
        generation
            .validate()
            .map_err(|_| CorpusError::InvalidRequest("generation is invalid".to_owned()))?;
        let bytes = canonical_json(&generation)?;
        if bytes.len() > MAX_GENERATION_BYTES {
            return Err(CorpusError::CapacityExceeded);
        }
        let digest = digest_bytes(&bytes);
        let destination = generation_dir.join(format!("{digest}.json"));
        match destination.symlink_metadata() {
            Ok(_) => {
                if read_bounded_regular(&destination, MAX_GENERATION_BYTES, ErrorContext::Request)?
                    != bytes
                {
                    return Err(CorpusError::InvalidRequest(
                        "generation digest is bound to different bytes".to_owned(),
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                write_atomic_file(
                    &generation_dir,
                    &destination,
                    &bytes,
                    GENERATION_STAGING_PREFIX,
                )?;
            }
            Err(error) => return Err(error.into()),
        }
        sync_file_and_parent(&destination, &generation_dir)?;
        drop(lock);
        Ok(GenerationSummary {
            schema: GENERATION_SUMMARY_SCHEMA.to_owned(),
            generation_id: generation.generation_id,
            corpus_generation_sha256: digest,
            source_count: generation.sources.len() as u64,
        })
    }

    /// Validates and durably publishes one canonical complete-label document in the private
    /// content-addressed label store.
    ///
    /// The returned summary contains only opaque identifiers, a non-personal shape class, and
    /// content evidence. Complete field values are never returned.
    ///
    /// # Errors
    ///
    /// Returns an error if the label is malformed, the private store is unavailable or damaged,
    /// or durable publication fails.
    pub fn author_complete_label(
        &self,
        label_path: impl AsRef<Path>,
    ) -> Result<CompleteLabelSummary, CorpusError> {
        self.validate_root()?;
        let input =
            read_bounded_regular(label_path.as_ref(), MAX_LABEL_BYTES, ErrorContext::Replay)?;
        let label: CompleteLabel = serde_json::from_slice(&input)?;
        label.validate_contents()?;
        let bytes = canonical_json(&label)?;
        if bytes.len() > MAX_LABEL_BYTES {
            return Err(CorpusError::CapacityExceeded);
        }
        let (frame_id, annotation_revision, shape) = label.summary_fields();

        preflight_managed_components(&self.root)?;
        validate_directory(&self.root, ErrorContext::Replay)?;
        let label_dir = self.root.join("labels");
        validate_directory(&label_dir, ErrorContext::Replay)?;
        let lock = open_store_lock(&self.root, false)?;
        lock.lock()?;
        preflight_managed_components(&self.root)?;
        validate_directory(&label_dir, ErrorContext::Replay)?;
        recover_label_staging(&label_dir)?;
        validate_label_store(&label_dir)?;

        let digest = digest_bytes(&bytes);
        let destination = label_dir.join(format!("{digest}.json"));
        match destination.symlink_metadata() {
            Ok(_) => {
                if read_bounded_regular(&destination, MAX_LABEL_BYTES, ErrorContext::Replay)?
                    != bytes
                {
                    return Err(CorpusError::InvalidReplay(
                        "complete-label digest is bound to different bytes".to_owned(),
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                write_atomic_file(&label_dir, &destination, &bytes, LABEL_STAGING_PREFIX)?;
            }
            Err(error) => return Err(error.into()),
        }
        sync_file_and_parent(&destination, &label_dir)?;
        drop(lock);

        Ok(CompleteLabelSummary {
            schema: COMPLETE_LABEL_SUMMARY_SCHEMA.to_owned(),
            frame_id: frame_id.to_owned(),
            annotation_revision: annotation_revision.to_owned(),
            shape,
            labels_sha256: digest,
            label_bytes: bytes.len() as u64,
        })
    }

    /// Generates and durably publishes one canonical replay index from strict frame metadata.
    ///
    /// Episode IDs are the plan's opaque episode-group SHA-256 values. Reusing an episode group
    /// after another group has begun is rejected so an episode always denotes one contiguous
    /// decode interval.
    ///
    /// # Errors
    ///
    /// Returns an error if the plan, selected source manifest, frame labels, decode order, or episode
    /// grouping is invalid, or if bounded durable publication fails.
    pub fn generate_replay_index(
        &self,
        plan_path: impl AsRef<Path>,
    ) -> Result<ReplayIndexSummary, CorpusError> {
        self.validate_root()?;
        let plan = ReplayIndexPlan::read_from(plan_path)?;
        preflight_managed_components(&self.root)?;
        validate_directory(&self.root, ErrorContext::Replay)?;
        let lock = open_store_lock(&self.root, false)?;
        lock.lock()?;
        preflight_managed_components(&self.root)?;
        for name in ["content", "manifests", "labels"] {
            validate_directory(&self.root.join(name), ErrorContext::Replay)?;
        }
        let manifest = load_source_manifest(self, &plan.fixture_id, &plan.source_manifest_sha256)?;
        let index = plan.into_replay_index(manifest);
        index.validate()?;
        for frame in &index.frames {
            validate_complete_label(self, frame)?;
        }

        let bytes = canonical_json(&index)?;
        if bytes.len() > MAX_REPLAY_INDEX_BYTES {
            return Err(CorpusError::CapacityExceeded);
        }
        let digest = digest_bytes(&bytes);
        let index_dir = self.root.join("indexes");
        create_private_directory(&index_dir)?;
        recover_index_staging(&index_dir)?;
        let destination = index_dir.join(format!("{digest}.json"));
        match destination.symlink_metadata() {
            Ok(_) => {
                if read_bounded_regular(&destination, MAX_REPLAY_INDEX_BYTES, ErrorContext::Replay)?
                    != bytes
                {
                    return Err(CorpusError::InvalidReplay(
                        "replay-index digest is bound to different bytes".to_owned(),
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                write_atomic_file(&index_dir, &destination, &bytes, INDEX_STAGING_PREFIX)?;
            }
            Err(error) => return Err(error.into()),
        }
        sync_file_and_parent(&destination, &index_dir)?;
        drop(lock);

        let episodes = index
            .frames
            .iter()
            .map(|frame| frame.episode_id.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        Ok(ReplayIndexSummary {
            schema: INDEX_SUMMARY_SCHEMA.to_owned(),
            fixture_id: index.fixture_id,
            replay_index_sha256: digest,
            frame_count: index.frames.len() as u64,
            episode_count: episodes as u64,
        })
    }

    /// Validates a complete replay suite against the canonical source manifests and immutable
    /// media in this private store.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed suite metadata, missing or mismatched source manifests,
    /// damaged source content, or any grouped split crossing anywhere in the suite.
    pub fn validate_replay_suite(
        &self,
        suite_path: impl AsRef<Path>,
    ) -> Result<ReplaySuiteSummary, CorpusError> {
        self.validate_root()?;
        validate_directory(&self.root, ErrorContext::Replay)?;
        validate_directory(&self.root.join("content"), ErrorContext::Replay)?;
        validate_directory(&self.root.join("manifests"), ErrorContext::Replay)?;
        validate_directory(&self.root.join("generations"), ErrorContext::Replay)?;
        validate_directory(&self.root.join("labels"), ErrorContext::Replay)?;
        validate_label_store(&self.root.join("labels"))?;
        let suite = ReplaySuite::read_from(suite_path)?;
        suite.validate_against(self)
    }

    pub(crate) fn validate_root(&self) -> Result<(), CorpusError> {
        if self.root.as_os_str().is_empty() || !self.root.is_absolute() {
            return Err(CorpusError::InvalidRequest(
                "private store root must be an absolute, non-empty path".to_owned(),
            ));
        }
        Ok(())
    }
}
