//! Installed overlay skin packages and the versioned core WebAssembly ABI.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read as _,
    path::{Component, Path},
};
use zip::ZipArchive;

pub const API_VERSION: u32 = 2;
pub const MANIFEST_PATH: &str = "skin.toml";
pub const MODULE_PATH: &str = "skin.wasm";
pub const STYLE_PATH: &str = "skin.css";
pub const PREVIEW_PATH: &str = "preview.png";
pub const PREVIEW_VIDEO_PATH: &str = "preview.webm";
pub const MAX_AFTER_MS: u64 = scorepeek_skin_sdk::MAX_AFTER_MS;
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub release: String,
    pub api_version: u32,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub resources: Vec<Resource>,
    #[serde(default)]
    pub widget_defaults: BTreeMap<String, WidgetDefault>,
    #[serde(default)]
    pub canvas_properties: BTreeMap<String, Property>,
    #[serde(default)]
    pub widget_properties: BTreeMap<String, BTreeMap<String, Property>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    pub path: String,
    pub media_type: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetDefault {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Property {
    Boolean {
        default: bool,
    },
    Integer {
        default: i64,
        minimum: i64,
        maximum: i64,
    },
    Number {
        default: f64,
        minimum: f64,
        maximum: f64,
    },
    Color {
        default: String,
    },
    Enum {
        default: String,
        values: Vec<String>,
    },
    String {
        default: String,
        maximum_length: usize,
    },
}

impl Property {
    pub(super) fn kind(&self) -> &'static str {
        match self {
            Self::Boolean { .. } => "boolean",
            Self::Integer { .. } => "integer",
            Self::Number { .. } => "number",
            Self::Color { .. } => "color",
            Self::Enum { .. } => "enum",
            Self::String { .. } => "string",
        }
    }

    fn effective(&self, candidate: Option<&serde_json::Value>) -> serde_json::Value {
        let valid = candidate.filter(|value| match self {
            Self::Boolean { .. } => value.is_boolean(),
            Self::Integer {
                minimum, maximum, ..
            } => value
                .as_i64()
                .is_some_and(|value| *minimum <= value && value <= *maximum),
            Self::Number {
                minimum, maximum, ..
            } => value
                .as_f64()
                .is_some_and(|value| value.is_finite() && *minimum <= value && value <= *maximum),
            Self::Color { .. } => value.as_str().is_some_and(valid_color),
            Self::Enum { values, .. } => value
                .as_str()
                .is_some_and(|value| values.iter().any(|item| item == value)),
            Self::String { maximum_length, .. } => value
                .as_str()
                .is_some_and(|value| value.chars().count() <= *maximum_length),
        });
        valid.cloned().unwrap_or_else(|| match self {
            Self::Boolean { default } => (*default).into(),
            Self::Integer { default, .. } => (*default).into(),
            Self::Number { default, .. } => serde_json::Number::from_f64(*default)
                .map_or(serde_json::Value::Null, serde_json::Value::Number),
            Self::Color { default } | Self::Enum { default, .. } | Self::String { default, .. } => {
                default.clone().into()
            }
        })
    }

    fn validate(&self, scope: &str, key: &str) -> Result<(), String> {
        let invalid = || format!("skin property {scope}.{key} has an invalid default or range");
        match self {
            Self::Boolean { .. } => Ok(()),
            Self::Integer {
                default,
                minimum,
                maximum,
            } => (*minimum <= *default && *default <= *maximum)
                .then_some(())
                .ok_or_else(invalid),
            Self::Number {
                default,
                minimum,
                maximum,
            } => (minimum.is_finite()
                && maximum.is_finite()
                && default.is_finite()
                && *minimum <= *default
                && *default <= *maximum)
                .then_some(())
                .ok_or_else(invalid),
            Self::Color { default } => valid_color(default).then_some(()).ok_or_else(invalid),
            Self::Enum { default, values } => (!values.is_empty()
                && values.iter().all(|value| !value.is_empty())
                && values.iter().collect::<BTreeSet<_>>().len() == values.len()
                && values.contains(default))
            .then_some(())
            .ok_or_else(invalid),
            Self::String {
                default,
                maximum_length,
            } => (default.chars().count() <= *maximum_length)
                .then_some(())
                .ok_or_else(invalid),
        }
    }
}

fn valid_color(value: &str) -> bool {
    matches!(value.len(), 4 | 5 | 7 | 9)
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl Manifest {
    /// Validates the stable package identity and property defaults.
    /// # Errors
    /// Returns a structural manifest error.
    pub fn validate(&self) -> Result<(), String> {
        validate_id(&self.id)?;
        if self.name.is_empty() || self.release.is_empty() {
            return Err("skin name and release must be non-empty".into());
        }
        if self.api_version != API_VERSION {
            return Err(format!("skin api_version must be {API_VERSION}"));
        }
        let mut resource_paths = BTreeSet::new();
        for resource in &self.resources {
            validate_entry_path(&resource.path)?;
            if !resource_paths.insert(resource.path.as_str()) {
                return Err(format!(
                    "skin resource {} is declared more than once",
                    resource.path
                ));
            }
            if resource.media_type.is_empty()
                || !resource.media_type.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'+' | b'.' | b'-')
                })
                || !resource.media_type.contains('/')
            {
                return Err(format!(
                    "skin resource {} has invalid media type",
                    resource.path
                ));
            }
        }
        for (kind, default) in &self.widget_defaults {
            if kind.is_empty() || default.width < 16 || default.height < 16 {
                return Err(format!(
                    "skin widget default {kind:?} has invalid dimensions"
                ));
            }
        }
        let required_widget_defaults = [
            "status",
            "selection",
            "score",
            "history-list",
            "history-graph",
            "empty",
        ];
        if self
            .widget_defaults
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != required_widget_defaults.into_iter().collect()
        {
            return Err(
                "skin widget_defaults must define every supported widget kind exactly once".into(),
            );
        }
        for (key, property) in &self.canvas_properties {
            validate_property_key(key)?;
            property.validate("canvas", key)?;
        }
        for (kind, properties) in &self.widget_properties {
            if kind.is_empty() {
                return Err("skin widget property kind must be non-empty".into());
            }
            for (key, property) in properties {
                validate_property_key(key)?;
                property.validate(kind, key)?;
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn effective_canvas_properties(
        &self,
        stored: &BTreeMap<String, serde_json::Value>,
    ) -> BTreeMap<String, serde_json::Value> {
        self.canvas_properties
            .iter()
            .map(|(key, property)| (key.clone(), property.effective(stored.get(key))))
            .collect()
    }

    #[must_use]
    pub fn effective_widget_properties(
        &self,
        kind: &str,
        stored: &BTreeMap<String, serde_json::Value>,
    ) -> BTreeMap<String, serde_json::Value> {
        self.widget_properties
            .get(kind)
            .or_else(|| self.widget_properties.get("*"))
            .into_iter()
            .flatten()
            .map(|(key, property)| (key.clone(), property.effective(stored.get(key))))
            .collect()
    }
}

fn validate_property_key(key: &str) -> Result<(), String> {
    if !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        Ok(())
    } else {
        Err(format!(
            "skin property key {key:?} must use lowercase ASCII, digits or '-'"
        ))
    }
}

/// Validates the reverse-domain skin identifier.
/// # Errors
/// Returns an identity syntax error.
pub fn validate_id(id: &str) -> Result<(), String> {
    let segments = id.split('.').collect::<Vec<_>>();
    if segments.len() < 2
        || segments.iter().any(|segment| {
            segment.is_empty()
                || !segment.as_bytes()[0].is_ascii_lowercase()
                    && !segment.as_bytes()[0].is_ascii_digit()
                || !segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
    {
        return Err("skin id must be a lowercase ASCII reverse-domain name".into());
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct Package {
    pub manifest: Manifest,
    entries: BTreeMap<String, Vec<u8>>,
}

impl Package {
    #[doc(hidden)]
    #[must_use]
    pub fn test_with_entries(manifest: Manifest, entries: BTreeMap<String, Vec<u8>>) -> Self {
        Self { manifest, entries }
    }

    /// Reads and structurally validates one self-contained skin ZIP.
    /// # Errors
    /// Returns ZIP, path, manifest, module, CSS or preview errors.
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path)
            .map_err(|error| format!("open skin package {}: {error}", path.display()))?;
        let mut archive =
            ZipArchive::new(file).map_err(|error| format!("read skin ZIP: {error}"))?;
        let mut entries = BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|error| format!("read skin ZIP entry: {error}"))?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().to_owned();
            validate_entry_path(&name)?;
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|error| format!("read skin ZIP entry {name}: {error}"))?;
            if entries.insert(name.clone(), bytes).is_some() {
                return Err(format!("skin ZIP contains duplicate entry {name}"));
            }
        }
        let manifest_bytes = required(&entries, MANIFEST_PATH)?;
        let manifest_text = std::str::from_utf8(manifest_bytes)
            .map_err(|error| format!("skin.toml is not UTF-8: {error}"))?;
        let manifest: Manifest =
            toml::from_str(manifest_text).map_err(|error| format!("skin.toml: {error}"))?;
        manifest.validate()?;
        let declared = manifest
            .resources
            .iter()
            .map(|resource| resource.path.as_str())
            .collect::<BTreeSet<_>>();
        let packaged = entries
            .keys()
            .map(String::as_str)
            .filter(|path| {
                !matches!(
                    *path,
                    MANIFEST_PATH | MODULE_PATH | STYLE_PATH | PREVIEW_PATH | PREVIEW_VIDEO_PATH
                )
            })
            .collect::<BTreeSet<_>>();
        if declared != packaged {
            let missing = declared.difference(&packaged).copied().collect::<Vec<_>>();
            let undeclared = packaged.difference(&declared).copied().collect::<Vec<_>>();
            return Err(format!(
                "skin resource inventory mismatch: missing={missing:?} undeclared={undeclared:?}"
            ));
        }
        std::str::from_utf8(required(&entries, STYLE_PATH)?)
            .map_err(|error| format!("skin.css is not UTF-8: {error}"))?;
        required(&entries, MODULE_PATH)?;
        image::load_from_memory_with_format(
            required(&entries, PREVIEW_PATH)?,
            image::ImageFormat::Png,
        )
        .map_err(|error| format!("preview.png: {error}"))?;
        Ok(Self { manifest, entries })
    }

    pub(super) fn read_manifest(path: &Path) -> Result<Manifest, String> {
        let file = File::open(path)
            .map_err(|error| format!("open skin package {}: {error}", path.display()))?;
        let mut archive =
            ZipArchive::new(file).map_err(|error| format!("read skin ZIP: {error}"))?;
        let mut entry = archive
            .by_name(MANIFEST_PATH)
            .map_err(|error| format!("read skin ZIP entry {MANIFEST_PATH}: {error}"))?;
        let mut manifest_text = String::new();
        entry
            .read_to_string(&mut manifest_text)
            .map_err(|error| format!("skin.toml is not UTF-8: {error}"))?;
        toml::from_str(&manifest_text).map_err(|error| format!("skin.toml: {error}"))
    }

    #[must_use]
    pub fn resource(&self, path: &str) -> Option<&[u8]> {
        self.entries.get(path).map(Vec::as_slice)
    }

    #[must_use]
    pub fn resource_media_type(&self, path: &str) -> Option<&str> {
        self.manifest
            .resources
            .iter()
            .find(|resource| resource.path == path)
            .map(|resource| resource.media_type.as_str())
    }

    pub fn font_resources(&self) -> impl Iterator<Item = &[u8]> {
        self.manifest
            .resources
            .iter()
            .filter(|resource| resource.media_type.starts_with("font/"))
            .filter_map(|resource| self.resource(&resource.path))
    }
}

fn required<'a>(entries: &'a BTreeMap<String, Vec<u8>>, path: &str) -> Result<&'a [u8], String> {
    entries
        .get(path)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("skin ZIP requires {path} at its root"))
}

fn validate_entry_path(path: &str) -> Result<(), String> {
    let candidate = Path::new(path);
    if path.contains('\\')
        || candidate.is_absolute()
        || candidate
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!(
            "skin ZIP entry path is not relative and normalized: {path}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::resources::compatible_properties;
    use super::*;
    use scorepeek_skin_sdk::{Node, Output as RenderOutput, Schedule};
    use std::fs;
    use std::io::Write as _;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn reverse_domain_ids_are_strict() {
        for valid in ["dev.atty303.skin", "io.7skin.a-b"] {
            validate_id(valid).unwrap();
        }
        for invalid in [
            "skin",
            "Dev.skin",
            "dev._skin",
            "dev.skin_1",
            ".skin",
            "dev..skin",
        ] {
            assert!(validate_id(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn tree_requires_globally_unique_stable_keys() {
        let output = RenderOutput {
            schedule: Schedule::Idle,
            tree: Node::Element {
                key: "root".into(),
                tag: "main".into(),
                attributes: BTreeMap::new(),
                children: vec![Node::Text {
                    key: "root".into(),
                    text: "x".into(),
                }],
            },
        };
        assert!(output.validate().is_err());
    }

    #[test]
    fn output_defaults_element_collections_and_bounds_browser_timers() {
        let output: RenderOutput = serde_json::from_value(serde_json::json!({
            "schedule": {"kind": "after-ms", "milliseconds": MAX_AFTER_MS},
            "tree": {"kind": "element", "key": "root", "tag": "main"}
        }))
        .unwrap();
        output.validate().unwrap();
        assert_eq!(
            serde_json::from_value::<RenderOutput>(serde_json::json!({
                "schedule": {"kind": "after-ms", "milliseconds": MAX_AFTER_MS + 1},
                "tree": {"kind": "text", "key": "root", "text": "x"}
            }))
            .unwrap()
            .validate()
            .unwrap_err(),
            format!("skin schedule milliseconds must be at most {MAX_AFTER_MS}")
        );
    }

    #[test]
    fn repository_manifests_are_valid_package_sources() {
        for manifest in [
            include_str!("../../../../skins/cyan-system/skin.toml"),
            include_str!("../../../../skins/result-aurora/skin.toml"),
            include_str!("../../../../skins/dj-blackbox/skin.toml"),
        ] {
            toml::from_str::<Manifest>(manifest)
                .unwrap()
                .validate()
                .unwrap();
        }
    }

    #[test]
    fn legacy_manifest_remains_readable_for_explicit_replacement() {
        let mut manifest: Manifest =
            toml::from_str(include_str!("../../../../skins/cyan-system/skin.toml")).unwrap();
        manifest.api_version = 1;
        manifest.release = "legacy".into();
        let path = std::env::temp_dir().join(format!(
            "scorepeek-legacy-skin-{}-{}.zip",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let file = File::create(&path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file(MANIFEST_PATH, zip::write::SimpleFileOptions::default())
            .unwrap();
        archive
            .write_all(toml::to_string(&manifest).unwrap().as_bytes())
            .unwrap();
        archive.finish().unwrap();

        assert!(Package::open(&path).is_err());
        let prior = Package::read_manifest(&path).unwrap();
        assert_eq!(prior.id, manifest.id);
        assert_eq!(prior.release, "legacy");
        assert_eq!(prior.api_version, 1);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn updates_preserve_only_surviving_scope_and_type_pairs() {
        let old: Manifest =
            toml::from_str(include_str!("../../../../skins/cyan-system/skin.toml")).unwrap();
        let mut added = old.clone();
        added
            .widget_properties
            .entry("*".into())
            .or_default()
            .insert(
                "background".into(),
                Property::String {
                    default: String::new(),
                    maximum_length: 20,
                },
            );
        compatible_properties(&old, &added).unwrap();

        let mut changed = old.clone();
        changed.canvas_properties.insert(
            "background".into(),
            Property::String {
                default: String::new(),
                maximum_length: 20,
            },
        );
        assert!(compatible_properties(&old, &changed).is_err());
    }
}
