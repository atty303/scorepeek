//! Corpus filesystem and content-integrity validation.

use super::*;

pub(crate) fn validate_directory(path: &Path, context: ErrorContext) -> Result<(), CorpusError> {
    let metadata = path.metadata()?;
    if !metadata.is_dir() {
        return Err(context.error("private store path must be a directory"));
    }
    Ok(())
}

pub(crate) fn validate_regular_file(path: &Path, context: ErrorContext) -> Result<(), CorpusError> {
    let metadata = path.metadata()?;
    if !metadata.is_file() {
        return Err(context.error("private store path must be a regular file"));
    }
    Ok(())
}

pub(crate) fn read_bounded_regular(
    path: &Path,
    maximum: usize,
    context: ErrorContext,
) -> Result<Vec<u8>, CorpusError> {
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() > maximum as u64 {
        return Err(context.error("metadata input is not a bounded regular file"));
    }
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| context.error("metadata input size is not representable"))?;
    let mut bytes = Vec::with_capacity(capacity);
    File::open(path)?
        .take((maximum + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(context.error("metadata input exceeds the size limit while reading"));
    }
    Ok(bytes)
}

pub(crate) fn digest_regular_file(path: &Path, maximum: u64) -> Result<String, CorpusError> {
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum {
        return Err(CorpusError::InvalidRequest(
            "stored object is not a bounded regular file".to_owned(),
        ));
    }
    let mut file = File::open(path)?;
    Ok(digest_open_file(&mut file, maximum)?.0)
}

pub(crate) fn digest_open_file(
    file: &mut File,
    maximum: u64,
) -> Result<(String, u64), CorpusError> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum {
        return Err(CorpusError::InvalidRequest(
            "opened object is not a bounded regular file".to_owned(),
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or(CorpusError::CapacityExceeded)?;
        if bytes > maximum {
            return Err(CorpusError::InvalidRequest(
                "stored object exceeds the size limit while reading".to_owned(),
            ));
        }
        hasher.update(&buffer[..read]);
    }
    if bytes != metadata.len() {
        return Err(CorpusError::InvalidRequest(
            "opened object size changed while hashing".to_owned(),
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    Ok((encode_digest(hasher.finalize()), bytes))
}

pub(crate) fn canonical_json(value: &impl Serialize) -> Result<Vec<u8>, CorpusError> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub(crate) fn digest_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    encode_digest(hasher.finalize())
}

pub(crate) fn encode_digest(digest: impl IntoIterator<Item = u8>) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

pub(crate) fn validate_opaque_id(
    value: &str,
    name: &str,
    context: ErrorContext,
) -> Result<(), CorpusError> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        });
    if !valid {
        return Err(context.error(format!(
            "{name} must be a 1-64 byte lowercase opaque identifier"
        )));
    }
    Ok(())
}

pub(crate) fn validate_token(
    value: &str,
    name: &str,
    context: ErrorContext,
) -> Result<(), CorpusError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'+'));
    if !valid {
        return Err(context.error(format!("{name} is not a bounded portable token")));
    }
    Ok(())
}

pub(crate) fn validate_sha256(
    value: &str,
    name: &str,
    context: ErrorContext,
) -> Result<(), CorpusError> {
    if !is_sha256(value) {
        return Err(context.error(format!(
            "{name} must be 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

pub(crate) fn valid_label_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.chars().any(char::is_control)
        && value.trim() == value
}

pub(crate) fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
