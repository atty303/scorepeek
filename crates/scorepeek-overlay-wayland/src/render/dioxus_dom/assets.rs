#![allow(
    clippy::wildcard_imports,
    reason = "this file is an implementation partition of the parent Dioxus DOM renderer"
)]

use super::*;

pub(super) struct SkinAssetCache {
    pub(super) store: crate::skin::StoreRoot,
    pub(super) packages:
        std::sync::Mutex<std::collections::BTreeMap<String, Arc<crate::skin::Package>>>,
    pub(super) editor_owners: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
    pub(super) open_count: std::sync::atomic::AtomicU64,
    pub(super) open_ns: std::sync::atomic::AtomicU64,
    pub(super) clone_count: std::sync::atomic::AtomicU64,
    pub(super) clone_ns: std::sync::atomic::AtomicU64,
    pub(super) resource_lookup_count: std::sync::atomic::AtomicU64,
    pub(super) resource_lookup_ns: std::sync::atomic::AtomicU64,
}

impl SkinAssetCache {
    pub(super) fn new(store: crate::skin::StoreRoot) -> Self {
        Self {
            store,
            packages: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            editor_owners: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            open_count: std::sync::atomic::AtomicU64::new(0),
            open_ns: std::sync::atomic::AtomicU64::new(0),
            clone_count: std::sync::atomic::AtomicU64::new(0),
            clone_ns: std::sync::atomic::AtomicU64::new(0),
            resource_lookup_count: std::sync::atomic::AtomicU64::new(0),
            resource_lookup_ns: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub(super) fn with_package(
        store: crate::skin::StoreRoot,
        package: Arc<crate::skin::Package>,
    ) -> Self {
        let id = package.manifest.id.clone();
        Self {
            store,
            packages: std::sync::Mutex::new(std::collections::BTreeMap::from([(id, package)])),
            editor_owners: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            open_count: std::sync::atomic::AtomicU64::new(1),
            open_ns: std::sync::atomic::AtomicU64::new(0),
            clone_count: std::sync::atomic::AtomicU64::new(0),
            clone_ns: std::sync::atomic::AtomicU64::new(0),
            resource_lookup_count: std::sync::atomic::AtomicU64::new(0),
            resource_lookup_ns: std::sync::atomic::AtomicU64::new(0),
        }
    }

    #[cfg(test)]
    pub(super) fn with_packages(packages: Vec<crate::skin::Package>) -> Self {
        let packages = packages
            .into_iter()
            .map(|package| (package.manifest.id.clone(), Arc::new(package)))
            .collect();
        Self {
            store: crate::skin::StoreRoot::discover(),
            packages: std::sync::Mutex::new(packages),
            editor_owners: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            open_count: std::sync::atomic::AtomicU64::new(0),
            open_ns: std::sync::atomic::AtomicU64::new(0),
            clone_count: std::sync::atomic::AtomicU64::new(0),
            clone_ns: std::sync::atomic::AtomicU64::new(0),
            resource_lookup_count: std::sync::atomic::AtomicU64::new(0),
            resource_lookup_ns: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub(super) fn acquire_editor_owner(&self, canvas: &str, output: &str) -> bool {
        let mut owners = self
            .editor_owners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(owner) = owners.get(canvas) {
            owner == output
        } else {
            owners.insert(canvas.to_owned(), output.to_owned());
            true
        }
    }

    pub(super) fn release_editor_owner(&self, canvas: &str, output: &str) {
        let mut owners = self
            .editor_owners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if owners.get(canvas).is_some_and(|owner| owner == output) {
            owners.remove(canvas);
        }
    }

    pub(super) fn load(&self, id: &str) -> Result<Arc<crate::skin::Package>, String> {
        let clone_started = Instant::now();
        let mut packages = self
            .packages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(package) = packages.get(id).cloned() {
            self.clone_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.clone_ns.fetch_add(
                duration_ns(clone_started.elapsed()),
                std::sync::atomic::Ordering::Relaxed,
            );
            return Ok(package);
        }
        let open_started = Instant::now();
        let package = Arc::new(self.store.open(id)?);
        self.open_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.open_ns.fetch_add(
            duration_ns(open_started.elapsed()),
            std::sync::atomic::Ordering::Relaxed,
        );
        let clone_started = Instant::now();
        let package = Arc::clone(packages.entry(id.to_owned()).or_insert(package));
        self.clone_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.clone_ns.fetch_add(
            duration_ns(clone_started.elapsed()),
            std::sync::atomic::Ordering::Relaxed,
        );
        Ok(package)
    }

    pub(super) fn load_profiled(
        &self,
        id: &str,
        work: &mut FrameWorkProfile,
    ) -> Result<Arc<crate::skin::Package>, String> {
        let open_before = WorkStat {
            calls: self.open_count.load(std::sync::atomic::Ordering::Relaxed),
            total_ns: self.open_ns.load(std::sync::atomic::Ordering::Relaxed),
        };
        let clone_before = WorkStat {
            calls: self.clone_count.load(std::sync::atomic::Ordering::Relaxed),
            total_ns: self.clone_ns.load(std::sync::atomic::Ordering::Relaxed),
        };
        let result = self.load(id);
        let open_after = WorkStat {
            calls: self.open_count.load(std::sync::atomic::Ordering::Relaxed),
            total_ns: self.open_ns.load(std::sync::atomic::Ordering::Relaxed),
        };
        let clone_after = WorkStat {
            calls: self.clone_count.load(std::sync::atomic::Ordering::Relaxed),
            total_ns: self.clone_ns.load(std::sync::atomic::Ordering::Relaxed),
        };
        work.record_stat(
            "package_open",
            WorkStat {
                calls: open_after.calls.saturating_sub(open_before.calls),
                total_ns: open_after.total_ns.saturating_sub(open_before.total_ns),
            },
        );
        work.record_stat(
            "package_clone",
            WorkStat {
                calls: clone_after.calls.saturating_sub(clone_before.calls),
                total_ns: clone_after.total_ns.saturating_sub(clone_before.total_ns),
            },
        );
        result
    }

    pub(super) fn installed_editor_skins(
        &self,
    ) -> Result<Vec<scorepeek_overlay::editor::EditorSkin>, String> {
        let entries = match std::fs::read_dir(self.store.path()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("list skin store: {error}")),
        };
        let mut packages = self
            .packages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in entries {
            let path = entry
                .map_err(|error| format!("list skin store entry: {error}"))?
                .path();
            if path.extension().is_none_or(|extension| extension != "zip") {
                continue;
            }
            let open_started = Instant::now();
            let package = Arc::new(crate::skin::Package::open(&path)?);
            let id = package.manifest.id.clone();
            if packages.insert(id.clone(), package).is_some() {
                return Err(format!("skin store contains duplicate package id {id}"));
            }
            self.open_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.open_ns.fetch_add(
                duration_ns(open_started.elapsed()),
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        packages
            .values()
            .map(|package| {
                let manifest = &package.manifest;
                Ok(scorepeek_overlay::editor::EditorSkin {
                    id: manifest.id.parse()?,
                    name: manifest.name.clone(),
                    release: manifest.release.clone(),
                    preview: format!("/skin/{}/{}", manifest.id, crate::skin::PREVIEW_PATH),
                    preview_video: None,
                    widget_defaults: serde_json::from_value(
                        serde_json::to_value(&manifest.widget_defaults)
                            .map_err(|error| error.to_string())?,
                    )
                    .map_err(|error| error.to_string())?,
                    canvas_properties: serde_json::from_value(
                        serde_json::to_value(&manifest.canvas_properties)
                            .map_err(|error| error.to_string())?,
                    )
                    .map_err(|error| error.to_string())?,
                    widget_properties: serde_json::from_value(
                        serde_json::to_value(&manifest.widget_properties)
                            .map_err(|error| error.to_string())?,
                    )
                    .map_err(|error| error.to_string())?,
                })
            })
            .collect()
    }
}

pub(super) fn namespace_skin_css(id: &str, css: &str) -> String {
    let mut output = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(offset) = rest.find("url(") {
        let (before, value) = rest.split_at(offset + 4);
        output.push_str(before);
        let Some(end) = value.find(')') else {
            output.push_str(value);
            return output;
        };
        let (raw, after) = value.split_at(end);
        let trimmed = raw.trim();
        let quote = trimmed
            .as_bytes()
            .first()
            .copied()
            .filter(|byte| matches!(byte, b'\'' | b'"'));
        let path = quote.map_or(trimmed, |_| &trimmed[1..trimmed.len().saturating_sub(1)]);
        if path.starts_with('/') || path.starts_with('#') || has_uri_scheme(path) {
            output.push_str(raw);
        } else {
            let quote = quote.map_or('"', char::from);
            output.push(quote);
            output.push_str("/skin/");
            output.push_str(id);
            output.push('/');
            output.push_str(path);
            output.push(quote);
        }
        output.push(')');
        rest = &after[1..];
    }
    output.push_str(rest);
    output
}

fn namespace_skin_resource(id: &str, value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.starts_with('#')
        || has_uri_scheme(trimmed)
    {
        value.to_owned()
    } else {
        format!("/skin/{id}/{value}")
    }
}

fn has_uri_scheme(value: &str) -> bool {
    let Some((scheme, _)) = value.split_once(':') else {
        return false;
    };
    let mut characters = scheme.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && characters
            .all(|character| character.is_ascii_alphanumeric() || "+-.".contains(character))
}

pub(super) fn namespace_native_skin_output(id: &str, output: &mut crate::skin::RenderOutput) {
    fn visit(id: &str, node: &mut crate::skin::Node) {
        let crate::skin::Node::Element {
            attributes,
            children,
            ..
        } = node
        else {
            return;
        };
        if let Some(style) = attributes.get_mut("style") {
            *style = namespace_skin_css(id, style);
        }
        for name in ["src", "poster"] {
            if let Some(value) = attributes.get_mut(name) {
                *value = namespace_skin_resource(id, value);
            }
        }
        for child in children {
            visit(id, child);
        }
    }
    visit(id, &mut output.tree);
}

pub(super) struct EmbeddedSkinAssets {
    pub(super) cache: Arc<SkinAssetCache>,
}

impl blitz_traits::net::NetProvider for EmbeddedSkinAssets {
    fn fetch(
        &self,
        _doc_id: usize,
        request: blitz_traits::net::Request,
        handler: Box<dyn blitz_traits::net::NetHandler>,
    ) {
        let started = Instant::now();
        let path = request.url.path().trim_start_matches('/');
        let bytes = if let Some(rest) = path.strip_prefix("skin/")
            && let Some((id, resource)) = rest.split_once('/')
            && crate::skin::validate_id(id).is_ok()
            && let Ok(package) = self.cache.load(id)
            && let Some(bytes) = package.resource(resource)
        {
            blitz_traits::net::Bytes::copy_from_slice(bytes)
        } else {
            blitz_traits::net::Bytes::new()
        };
        self.cache
            .resource_lookup_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.cache.resource_lookup_ns.fetch_add(
            duration_ns(started.elapsed()),
            std::sync::atomic::Ordering::Relaxed,
        );
        handler.bytes(request.url.to_string(), bytes);
    }
}
