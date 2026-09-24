//! Installed skin package store, activation, recovery, and durability.

use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

use super::package::{API_VERSION, Manifest, Package, validate_id};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstallOutcome {
    Installed,
    Replaced { previous_release: String },
    Unchanged,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InstalledSkin {
    pub id: String,
    pub name: String,
    pub release: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct StoreRoot(PathBuf);

impl StoreRoot {
    #[must_use]
    pub fn discover() -> Self {
        let root = std::env::var_os("XDG_DATA_HOME").map_or_else(
            || {
                std::env::var_os("HOME").map_or_else(
                    || PathBuf::from(".local/share"),
                    |home| PathBuf::from(home).join(".local/share"),
                )
            },
            PathBuf::from,
        );
        Self(root.join("scorepeek/skins"))
    }

    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Reports whether an installed package path exists without reopening the ZIP.
    /// Installation validates packages before activation; running processes do not hot-reload them.
    /// # Errors
    /// Returns an identity or metadata error.
    pub fn is_installed(&self, id: &str) -> Result<bool, String> {
        validate_id(id)?;
        match fs::metadata(self.0.join(format!("{id}.zip"))) {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(format!("inspect installed skin {id}: {error}")),
        }
    }

    /// Installs or updates a validated ZIP package atomically.
    /// # Errors
    /// Returns package validation, compatibility, locking, or persistence errors.
    pub fn install_with(
        &self,
        source: &Path,
        validate: impl FnOnce(&Package) -> Result<(), String>,
    ) -> Result<InstallOutcome, String> {
        create_dir_all_durable(&self.0)
            .map_err(|error| format!("create skin store {}: {error}", self.0.display()))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.0.join(".store.lock"))
            .map_err(|error| format!("open skin store lock: {error}"))?;
        lock.lock()
            .map_err(|error| format!("lock skin store: {error}"))?;
        self.recover_install_staging()?;
        let temporary = self.0.join(format!(".install.{}.tmp", std::process::id()));
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| format!("create skin staging file: {error}"))?;
        let result = (|| -> Result<InstallOutcome, String> {
            let mut input = File::open(source)
                .map_err(|error| format!("open skin package {}: {error}", source.display()))?;
            std::io::copy(&mut input, &mut output)
                .map_err(|error| format!("copy skin package into staging: {error}"))?;
            output
                .sync_all()
                .map_err(|error| format!("sync skin staging file: {error}"))?;
            drop(output);

            let package = Package::open(&temporary)?;
            self.recover_staging(&package.manifest.id)?;
            let target = self.0.join(format!("{}.zip", package.manifest.id));
            let prior = if target.exists() {
                match Package::open(&target) {
                    Ok(old) => Some((old.manifest, true)),
                    Err(error) => {
                        let old = Package::read_manifest(&target)?;
                        if old.api_version == API_VERSION {
                            return Err(error);
                        }
                        validate_id(&old.id)?;
                        if old.id != package.manifest.id {
                            return Err(format!(
                                "installed skin package id {} does not match {}",
                                old.id, package.manifest.id
                            ));
                        }
                        if old.release.is_empty() {
                            return Err("installed skin release must be non-empty".into());
                        }
                        Some((old, false))
                    }
                }
            } else {
                None
            };
            if prior
                .as_ref()
                .is_some_and(|(old, current)| *current && old.release == package.manifest.release)
            {
                return Ok(InstallOutcome::Unchanged);
            }
            if let Some((old, _)) = &prior {
                compatible_properties(old, &package.manifest)?;
            }
            validate(&package)?;
            fs::rename(&temporary, &target)
                .map_err(|error| format!("activate skin package: {error}"))?;
            File::open(&self.0)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| format!("sync skin store: {error}"))?;
            Ok(prior.map_or(InstallOutcome::Installed, |(old, _)| {
                InstallOutcome::Replaced {
                    previous_release: old.release,
                }
            }))
        })();
        if temporary.exists() {
            fs::remove_file(&temporary)
                .map_err(|error| format!("remove skin staging file: {error}"))?;
            File::open(&self.0)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| format!("sync skin store after staging cleanup: {error}"))?;
        }
        result
    }

    /// Removes one installed package without rewriting overlay configuration.
    /// # Errors
    /// Returns identity, locking, removal, or durability errors.
    pub fn uninstall(&self, id: &str) -> Result<(), String> {
        validate_id(id)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.0.join(".store.lock"))
            .map_err(|error| format!("open skin store lock: {error}"))?;
        lock.lock()
            .map_err(|error| format!("lock skin store: {error}"))?;
        let target = self.0.join(format!("{id}.zip"));
        fs::remove_file(&target).map_err(|error| format!("uninstall skin {id}: {error}"))?;
        File::open(&self.0)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("sync skin store: {error}"))
    }

    /// Lists validated installed packages in identifier order.
    /// # Errors
    /// Returns store enumeration or package validation errors.
    pub fn list(&self) -> Result<Vec<InstalledSkin>, String> {
        let mut installed = Vec::new();
        let entries = match fs::read_dir(&self.0) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(installed),
            Err(error) => return Err(format!("list skin store: {error}")),
        };
        for entry in entries {
            let path = entry
                .map_err(|error| format!("list skin store entry: {error}"))?
                .path();
            if path.extension().is_some_and(|extension| extension == "zip") {
                let package = Package::open(&path)?;
                installed.push(InstalledSkin {
                    id: package.manifest.id,
                    name: package.manifest.name,
                    release: package.manifest.release,
                    path,
                });
            }
        }
        installed.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(installed)
    }

    /// Opens and validates one installed package by identifier.
    /// # Errors
    /// Returns identity, I/O, or package validation errors.
    pub fn open(&self, id: &str) -> Result<Package, String> {
        validate_id(id)?;
        Package::open(&self.0.join(format!("{id}.zip")))
    }

    fn recover_staging(&self, id: &str) -> Result<(), String> {
        let prefix = format!(".{id}.");
        for entry in fs::read_dir(&self.0).map_err(|error| format!("scan skin staging: {error}"))? {
            let path = entry
                .map_err(|error| format!("scan skin staging entry: {error}"))?
                .path();
            let owned = path
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| {
                    name.starts_with(&prefix)
                        && Path::new(name)
                            .extension()
                            .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
                });
            if owned {
                fs::remove_file(&path)
                    .map_err(|error| format!("recover skin staging {}: {error}", path.display()))?;
            }
        }
        Ok(())
    }

    fn recover_install_staging(&self) -> Result<(), String> {
        for entry in fs::read_dir(&self.0).map_err(|error| format!("scan skin staging: {error}"))? {
            let path = entry
                .map_err(|error| format!("scan skin staging entry: {error}"))?
                .path();
            let owned = path
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| {
                    name.starts_with(".install.")
                        && Path::new(name)
                            .extension()
                            .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
                });
            if owned {
                fs::remove_file(&path)
                    .map_err(|error| format!("recover skin staging {}: {error}", path.display()))?;
            }
        }
        Ok(())
    }
}

fn create_dir_all_durable(path: &Path) -> std::io::Result<()> {
    let mut missing = Vec::new();
    let mut cursor = path;
    while !cursor.exists() {
        missing.push(cursor.to_path_buf());
        cursor = cursor.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "directory has no parent")
        })?;
    }
    for directory in missing.into_iter().rev() {
        match fs::create_dir(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        File::open(&directory)?.sync_all()?;
        if let Some(parent) = directory.parent() {
            File::open(parent)?.sync_all()?;
        }
    }
    Ok(())
}

pub(super) fn compatible_properties(old: &Manifest, new: &Manifest) -> Result<(), String> {
    let properties = |manifest: &Manifest| {
        let mut values: BTreeMap<(String, String), &'static str> = BTreeMap::new();
        for (key, property) in &manifest.canvas_properties {
            values.insert(("canvas".into(), key.clone()), property.kind());
        }
        for (widget, fields) in &manifest.widget_properties {
            for (key, property) in fields {
                values.insert((format!("widget:{widget}"), key.clone()), property.kind());
            }
        }
        values
    };
    let old = properties(old);
    let new = properties(new);
    for key in old.keys().filter(|key| new.contains_key(*key)) {
        if old.get(key) != new.get(key) {
            return Err(format!(
                "skin update changes property type for {}.{}",
                key.0, key.1
            ));
        }
    }
    Ok(())
}
