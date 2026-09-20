use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

pub(crate) fn absolute_asset_directory(configured: PathBuf) -> io::Result<PathBuf> {
    if !configured.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "SCOREPEEK_WEB_ASSET_DIR must be an absolute path",
        ));
    }
    Ok(configured)
}

pub(crate) fn validate_embedded_bundle(assets: &Path, identity: &str) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_files(assets, &mut files)?;
    files.sort();
    let wasm = files
        .iter()
        .filter(|path| path.extension() == Some(OsStr::new("wasm")))
        .map(fs::read)
        .collect::<io::Result<Vec<_>>>()?;
    validate_wasm_identity(&wasm, identity).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("embedded web bundle at {} {error}", assets.display()),
        )
    })?;
    Ok(files)
}

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

fn validate_wasm_identity(wasm: &[Vec<u8>], identity: &str) -> io::Result<()> {
    if identity.len() != 64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "host build identity must contain 64 bytes",
        ));
    }
    if wasm.len() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("must contain exactly one WASM asset, found {}", wasm.len()),
        ));
    }
    let expected = identity.as_bytes();
    if !wasm[0]
        .windows(expected.len())
        .any(|window| window == expected)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("does not match host build {identity}"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{absolute_asset_directory, validate_wasm_identity};
    use std::path::PathBuf;

    const IDENTITY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn asset_directory_must_be_absolute() {
        assert!(absolute_asset_directory(PathBuf::from("relative/public")).is_err());
        assert_eq!(
            absolute_asset_directory(PathBuf::from("/tmp/scorepeek/public")).unwrap(),
            PathBuf::from("/tmp/scorepeek/public")
        );
    }

    #[test]
    fn matching_wasm_passes_and_stale_wasm_fails() {
        let matching = vec![format!("prefix{IDENTITY}suffix").into_bytes()];
        assert!(validate_wasm_identity(&matching, IDENTITY).is_ok());

        let stale = vec![b"stale-browser-build".to_vec()];
        let error = validate_wasm_identity(&stale, IDENTITY).unwrap_err();
        assert!(error.to_string().contains("does not match host build"));
    }

    #[test]
    fn replacement_is_revalidated_and_extra_wasm_fails_closed() {
        let mut wasm = vec![format!("prefix{IDENTITY}suffix").into_bytes()];
        assert!(validate_wasm_identity(&wasm, IDENTITY).is_ok());
        wasm[0] = b"replacement-with-another-build".to_vec();
        assert!(validate_wasm_identity(&wasm, IDENTITY).is_err());

        wasm.push(format!("prefix{IDENTITY}suffix").into_bytes());
        let error = validate_wasm_identity(&wasm, IDENTITY).unwrap_err();
        assert!(error.to_string().contains("exactly one WASM asset"));
    }
}
