//! Atomic corpus publication, staging recovery, and writer locking.

use super::*;

pub(crate) fn write_atomic_file(
    directory: &Path,
    path: &Path,
    bytes: &[u8],
    staging_prefix: &str,
) -> Result<(), CorpusError> {
    let mut temporary = Builder::new()
        .prefix(staging_prefix)
        .tempfile_in(directory)?;
    temporary.write_all(bytes)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    File::open(directory)?.sync_all()?;
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn replace_atomic_file(
    directory: &Path,
    path: &Path,
    bytes: &[u8],
    staging_prefix: &str,
) -> Result<(), CorpusError> {
    let mut temporary = Builder::new()
        .prefix(staging_prefix)
        .permissions(fs::Permissions::from_mode(0o600))
        .tempfile_in(directory)?;
    temporary.write_all(bytes)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    File::open(directory)?.sync_all()?;
    Ok(())
}

pub(crate) fn sync_stored_source_and_parent(
    directory: &Path,
    content_dir: &Path,
) -> io::Result<()> {
    let source = directory.join(SOURCE_FILE);
    match source.metadata() {
        Ok(metadata) if metadata.is_file() => File::open(source)?.sync_all()?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            File::open(directory.join(EXTERNAL_SOURCE_FILE))?.sync_all()?;
        }
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "stored source entry is not a regular file",
            ));
        }
        Err(error) => return Err(error),
    }
    File::open(directory)?.sync_all()?;
    File::open(content_dir)?.sync_all()
}

pub(crate) fn sync_file_and_parent(path: &Path, directory: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()?;
    File::open(directory)?.sync_all()
}

pub(crate) fn recover_staging(content_dir: &Path, manifest_dir: &Path) -> Result<(), CorpusError> {
    let mut changed_content = false;
    for entry in fs::read_dir(content_dir)? {
        let entry = entry?;
        let is_staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(SOURCE_STAGING_PREFIX));
        if !is_staging {
            continue;
        }
        if !entry.path().symlink_metadata()?.is_dir() {
            return Err(CorpusError::InvalidRequest(
                "source staging entry is not a directory".to_owned(),
            ));
        }
        fs::remove_dir_all(entry.path())?;
        changed_content = true;
    }
    if changed_content {
        File::open(content_dir)?.sync_all()?;
    }

    let mut changed_manifests = false;
    for entry in fs::read_dir(manifest_dir)? {
        let entry = entry?;
        let is_staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(MANIFEST_STAGING_PREFIX));
        if !is_staging {
            continue;
        }
        if !entry.path().symlink_metadata()?.is_file() {
            return Err(CorpusError::InvalidRequest(
                "manifest staging entry is not a file".to_owned(),
            ));
        }
        fs::remove_file(entry.path())?;
        changed_manifests = true;
    }
    if changed_manifests {
        File::open(manifest_dir)?.sync_all()?;
    }
    Ok(())
}

pub(crate) fn recover_generation_staging(generation_dir: &Path) -> Result<(), CorpusError> {
    let mut changed = false;
    for entry in fs::read_dir(generation_dir)? {
        let entry = entry?;
        let is_staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(GENERATION_STAGING_PREFIX));
        if !is_staging {
            continue;
        }
        if !entry.path().symlink_metadata()?.is_file() {
            return Err(CorpusError::InvalidRequest(
                "generation staging entry is not a file".to_owned(),
            ));
        }
        fs::remove_file(entry.path())?;
        changed = true;
    }
    if changed {
        File::open(generation_dir)?.sync_all()?;
    }
    Ok(())
}

pub(crate) fn recover_label_staging(label_dir: &Path) -> Result<(), CorpusError> {
    let mut changed = false;
    for entry in fs::read_dir(label_dir)? {
        let entry = entry?;
        let is_staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(LABEL_STAGING_PREFIX));
        if !is_staging {
            continue;
        }
        if !entry.path().symlink_metadata()?.is_file() {
            return Err(CorpusError::InvalidReplay(
                "complete-label staging entry is not a file".to_owned(),
            ));
        }
        fs::remove_file(entry.path())?;
        changed = true;
    }
    if changed {
        File::open(label_dir)?.sync_all()?;
    }
    Ok(())
}

pub(crate) fn recover_index_staging(index_dir: &Path) -> Result<(), CorpusError> {
    let mut changed = false;
    for entry in fs::read_dir(index_dir)? {
        let entry = entry?;
        let is_staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(INDEX_STAGING_PREFIX));
        if !is_staging {
            continue;
        }
        if !entry.path().symlink_metadata()?.is_file() {
            return Err(CorpusError::InvalidReplay(
                "replay-index staging entry is not a file".to_owned(),
            ));
        }
        fs::remove_file(entry.path())?;
        changed = true;
    }
    if changed {
        File::open(index_dir)?.sync_all()?;
    }
    Ok(())
}

pub(crate) fn preflight_managed_components(root: &Path) -> Result<(), CorpusError> {
    match root.metadata() {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(CorpusError::InvalidRequest(
                "private store root is not a directory".to_owned(),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }

    for name in [
        "content",
        "manifests",
        "generations",
        "labels",
        "indexes",
        "profiles",
        "probes",
        "recordings",
        "dataset-generations",
    ] {
        match root.join(name).metadata() {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(CorpusError::InvalidRequest(format!(
                    "managed store component {name:?} is not a directory"
                )));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    match root.join("corpus-ingest.lock").symlink_metadata() {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(CorpusError::InvalidRequest(
            "corpus writer lock is not a regular file".to_owned(),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn create_private_directory(path: &Path) -> io::Result<()> {
    let mut missing = Vec::new();
    let mut candidate = path;
    loop {
        match candidate.metadata() {
            Ok(metadata) if metadata.is_dir() => break,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "corpus ancestor is not a directory",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(candidate.to_owned());
                candidate = candidate.parent().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "directory has no existing ancestor",
                    )
                })?;
            }
            Err(error) => return Err(error),
        }
    }
    for directory in missing.into_iter().rev() {
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        sync_directory_and_parent(&directory)?;
    }
    sync_directory_and_parent(path)
}

pub(crate) fn open_store_lock(root: &Path, create: bool) -> Result<File, CorpusError> {
    let path = root.join("corpus-ingest.lock");
    let exists = match path.symlink_metadata() {
        Ok(metadata) if metadata.is_file() => true,
        Ok(_) => {
            return Err(CorpusError::InvalidRequest(
                "corpus writer lock is not a regular file".to_owned(),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && create => false,
        Err(error) => return Err(error.into()),
    };
    let lock = if exists {
        OpenOptions::new().read(true).write(true).open(&path)?
    } else {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?
    };
    if !path.symlink_metadata()?.is_file() || !lock.metadata()?.is_file() {
        return Err(CorpusError::InvalidRequest(
            "corpus writer lock changed while opening".to_owned(),
        ));
    }
    lock.sync_all()?;
    File::open(root)?.sync_all()?;
    Ok(lock)
}

pub(crate) fn sync_directory_and_parent(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}
