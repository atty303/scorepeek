//! Installed overlay skin packages and the versioned core WebAssembly ABI.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::Read as _,
    path::{Component, Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};
use wasmtime::{Engine, Instance, Memory, Module, Store, TypedFunc};
use zip::ZipArchive;

pub const API_VERSION: u32 = 1;
pub const MANIFEST_PATH: &str = "skin.toml";
pub const MODULE_PATH: &str = "skin.wasm";
pub const STYLE_PATH: &str = "skin.css";
pub const PREVIEW_PATH: &str = "preview.png";
pub const PREVIEW_VIDEO_PATH: &str = "preview.webm";
pub const MAX_AFTER_MS: u64 = 2_147_483_647;
const CALL_TIMEOUT: Duration = Duration::from_secs(2);

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
    pub canvas_properties: BTreeMap<String, Property>,
    #[serde(default)]
    pub widget_properties: BTreeMap<String, BTreeMap<String, Property>>,
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
    fn kind(&self) -> &'static str {
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
        std::str::from_utf8(required(&entries, STYLE_PATH)?)
            .map_err(|error| format!("skin.css is not UTF-8: {error}"))?;
        let wasm = required(&entries, MODULE_PATH)?;
        let engine = engine()?;
        Module::validate(&engine, wasm).map_err(|error| format!("skin.wasm: {error}"))?;
        image::load_from_memory_with_format(
            required(&entries, PREVIEW_PATH)?,
            image::ImageFormat::Png,
        )
        .map_err(|error| format!("preview.png: {error}"))?;
        Ok(Self { manifest, entries })
    }

    #[must_use]
    pub fn resource(&self, path: &str) -> Option<&[u8]> {
        self.entries.get(path).map(Vec::as_slice)
    }

    /// Runs native and browser-equivalent initialization smoke calls.
    /// # Errors
    /// Returns an ABI, trap, timeout, JSON tree or stable-key error.
    pub fn smoke_test(&self) -> Result<(), String> {
        for backend in ["native", "obs"] {
            let mut runtime = Runtime::new(self)?;
            let input = serde_json::json!({"schema":"scorepeek-skin-input-v1","backend":backend,"canvas":{"id":"install-smoke","skin":self.manifest.id,"width":1920,"height":1080,"properties":{}},"widgets":[],"state":{"screen":"unknown"}});
            runtime.init(&input)?;
            runtime.render(&input)?;
        }
        Ok(())
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

fn engine() -> Result<Engine, String> {
    let mut config = wasmtime::Config::new();
    config.epoch_interruption(true);
    Engine::new(&config).map_err(|error| format!("initialize skin runtime: {error}"))
}

pub struct Runtime {
    engine: Engine,
    store: Store<()>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    dealloc: TypedFunc<(i32, i32), ()>,
    init: TypedFunc<(i32, i32), i64>,
    render: TypedFunc<(i32, i32), i64>,
    _instance: Instance,
}

impl Runtime {
    /// Creates one isolated plugin instance for one canvas.
    /// # Errors
    /// Returns compile, import or export ABI errors.
    pub fn new(package: &Package) -> Result<Self, String> {
        let engine = engine()?;
        let module = Module::new(&engine, required(&package.entries, MODULE_PATH)?)
            .map_err(|error| format!("compile skin.wasm: {error}"))?;
        if module.imports().next().is_some() {
            return Err("skin.wasm must not import host or WASI functions".into());
        }
        let mut store = Store::new(&engine, ());
        store.set_epoch_deadline(1);
        let instance = run_with_timeout(&engine, "instantiate", || {
            Instance::new(&mut store, &module, &[])
                .map_err(|error| format!("instantiate skin.wasm: {error}"))
        })?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or("skin.wasm must export memory")?;
        let alloc = instance
            .get_typed_func(&mut store, "scorepeek_alloc")
            .map_err(|error| format!("skin.wasm scorepeek_alloc ABI: {error}"))?;
        let dealloc = instance
            .get_typed_func(&mut store, "scorepeek_dealloc")
            .map_err(|error| format!("skin.wasm scorepeek_dealloc ABI: {error}"))?;
        let init = instance
            .get_typed_func(&mut store, "scorepeek_init")
            .map_err(|error| format!("skin.wasm scorepeek_init ABI: {error}"))?;
        let render = instance
            .get_typed_func(&mut store, "scorepeek_render")
            .map_err(|error| format!("skin.wasm scorepeek_render ABI: {error}"))?;
        Ok(Self {
            engine,
            store,
            memory,
            alloc,
            dealloc,
            init,
            render,
            _instance: instance,
        })
    }

    /// Initializes this canvas instance and returns its first tree.
    /// # Errors
    /// Returns an ABI, trap, timeout, output decoding, or tree validation error.
    pub fn init(&mut self, input: &serde_json::Value) -> Result<RenderOutput, String> {
        self.call(input, true)
    }

    /// Updates this canvas instance and returns its next complete tree.
    /// # Errors
    /// Returns an ABI, trap, timeout, output decoding, or tree validation error.
    pub fn render(&mut self, input: &serde_json::Value) -> Result<RenderOutput, String> {
        self.call(input, false)
    }

    fn call(
        &mut self,
        input: &serde_json::Value,
        initialize: bool,
    ) -> Result<RenderOutput, String> {
        let bytes =
            serde_json::to_vec(input).map_err(|error| format!("serialize skin input: {error}"))?;
        let length =
            i32::try_from(bytes.len()).map_err(|_| "skin input exceeds ABI address space")?;
        self.store.set_epoch_deadline(1);
        let phase = if initialize { "init" } else { "render" };
        let engine = self.engine.clone();
        let packed = run_with_timeout(&engine, phase, || {
            let pointer = self
                .alloc
                .call(&mut self.store, length)
                .map_err(|error| format!("skin allocation trapped: {error}"))?;
            let offset = usize::try_from(pointer)
                .map_err(|_| "skin allocator returned a negative pointer".to_owned())?;
            self.memory
                .write(&mut self.store, offset, &bytes)
                .map_err(|error| format!("write skin input: {error}"))?;
            let packed = if initialize {
                self.init.call(&mut self.store, (pointer, length))
            } else {
                self.render.call(&mut self.store, (pointer, length))
            }
            .map_err(|error| format!("skin {phase} trapped: {error}"))?;
            self.dealloc
                .call(&mut self.store, (pointer, length))
                .map_err(|error| format!("skin deallocation trapped: {error}"))?;
            Ok(packed)
        })?;
        let packed = packed.cast_unsigned();
        let output_pointer = usize::try_from(
            u32::try_from(packed >> 32).map_err(|_| "skin output pointer is invalid")?,
        )
        .map_err(|_| "skin output pointer is invalid")?;
        let output_length = usize::try_from(
            u32::try_from(packed & u64::from(u32::MAX))
                .map_err(|_| "skin output length is invalid")?,
        )
        .map_err(|_| "skin output length is invalid")?;
        let range = output_pointer
            ..output_pointer
                .checked_add(output_length)
                .ok_or("skin output range overflow")?;
        let output = self
            .memory
            .data(&self.store)
            .get(range)
            .ok_or("skin output is outside memory")?;
        let output: RenderOutput =
            serde_json::from_slice(output).map_err(|error| format!("skin output JSON: {error}"))?;
        output.validate()?;
        Ok(output)
    }
}

fn run_with_timeout<T>(
    engine: &Engine,
    phase: &str,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let (cancel_tx, cancel_rx) = mpsc::channel();
    let timer_engine = engine.clone();
    let timer = std::thread::spawn(move || {
        if cancel_rx.recv_timeout(CALL_TIMEOUT).is_err() {
            timer_engine.increment_epoch();
        }
    });
    let started = Instant::now();
    let result = operation();
    let _ = cancel_tx.send(());
    let _ = timer.join();
    result.map_err(|error| {
        if started.elapsed() >= CALL_TIMEOUT {
            format!("skin {phase} hard timeout")
        } else {
            error
        }
    })
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RenderOutput {
    pub schedule: Schedule,
    pub tree: Node,
}

impl RenderOutput {
    fn validate(&self) -> Result<(), String> {
        if matches!(
            self.schedule,
            Schedule::AfterMs { milliseconds } if milliseconds > MAX_AFTER_MS
        ) {
            return Err(format!(
                "skin schedule milliseconds must be at most {MAX_AFTER_MS}"
            ));
        }
        let mut keys = BTreeSet::new();
        self.tree.validate(&mut keys)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Schedule {
    Idle,
    NextFrame,
    AfterMs { milliseconds: u64 },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Node {
    Element {
        key: String,
        tag: String,
        #[serde(default)]
        attributes: BTreeMap<String, String>,
        #[serde(default)]
        children: Vec<Node>,
    },
    Text {
        key: String,
        text: String,
    },
}

impl Node {
    fn key(&self) -> &str {
        match self {
            Self::Element { key, .. } | Self::Text { key, .. } => key,
        }
    }

    fn validate(&self, keys: &mut BTreeSet<String>) -> Result<(), String> {
        let (key, children) = match self {
            Self::Element {
                key, tag, children, ..
            } => {
                if tag.is_empty() {
                    return Err("skin tree element tag must be non-empty".into());
                }
                (key, children.as_slice())
            }
            Self::Text { key, .. } => (key, &[][..]),
        };
        if key.is_empty() || !keys.insert(key.clone()) {
            return Err("skin tree keys must be non-empty and unique".into());
        }
        for child in children {
            child.validate(keys)?;
        }
        Ok(())
    }
}

/// Stateful keyed adapter from the shared JSON tree to Blitz's ordinary DOM.
pub struct NativeTree {
    root: blitz_dom::NodeId,
    style: blitz_dom::NodeId,
    mounted: Option<Mounted>,
}

struct Mounted {
    key: String,
    kind: MountedKind,
    node: blitz_dom::NodeId,
    attributes: BTreeMap<String, String>,
    children: Vec<Mounted>,
}

#[derive(Eq, PartialEq)]
enum MountedKind {
    Element(String),
    Text,
}

impl NativeTree {
    /// Creates a package-owned subtree below a host-owned canvas root.
    pub fn new(document: &mut blitz_dom::BaseDocument, root: blitz_dom::NodeId, css: &str) -> Self {
        let mut mutator = document.mutate();
        let style = mutator.create_element(html_name("style"), Vec::new());
        let text = mutator.create_text_node(css);
        mutator.append_children(style, &[text]);
        mutator.append_children(root, &[style]);
        drop(mutator);
        Self {
            root,
            style,
            mounted: None,
        }
    }

    /// Reconciles a validated full tree by stable key.
    pub fn apply(&mut self, document: &mut blitz_dom::BaseDocument, output: &RenderOutput) {
        let old = self.mounted.take();
        let mut mutator = document.mutate();
        let mounted = reconcile(&mut mutator, old, &output.tree, false);
        mutator.append_children(self.root, &[self.style, mounted.node]);
        self.mounted = Some(mounted);
    }

    pub fn replace(
        &mut self,
        document: &mut blitz_dom::BaseDocument,
        css: &str,
        output: &RenderOutput,
    ) {
        let mut mutator = document.mutate();
        if let Some(mounted) = self.mounted.take() {
            mutator.remove_and_drop_node(mounted.node);
        }
        mutator.remove_and_drop_node(self.style);
        self.style = mutator.create_element(html_name("style"), Vec::new());
        let text = mutator.create_text_node(css);
        mutator.append_children(self.style, &[text]);
        drop(mutator);
        self.apply(document, output);
    }

    pub fn set_css(&mut self, document: &mut blitz_dom::BaseDocument, css: &str) {
        let mut mutator = document.mutate();
        mutator.remove_and_drop_node(self.style);
        self.style = mutator.create_element(html_name("style"), Vec::new());
        let text = mutator.create_text_node(css);
        mutator.append_children(self.style, &[text]);
        mutator.append_children(self.root, &[self.style]);
    }
}

fn reconcile(
    mutator: &mut blitz_dom::DocumentMutator<'_>,
    old: Option<Mounted>,
    next: &Node,
    parent_svg: bool,
) -> Mounted {
    let expected = match next {
        Node::Element { tag, .. } => MountedKind::Element(tag.clone()),
        Node::Text { .. } => MountedKind::Text,
    };
    let reusable = match old {
        Some(mounted) if mounted.key == next.key() && mounted.kind == expected => Some(mounted),
        Some(mounted) => {
            mutator.remove_and_drop_node(mounted.node);
            None
        }
        None => None,
    };
    match next {
        Node::Text { key, text } => {
            if let Some(mut mounted) = reusable {
                mutator.set_node_text(mounted.node, text);
                mounted.children.clear();
                mounted.attributes.clear();
                mounted
            } else {
                Mounted {
                    key: key.clone(),
                    kind: MountedKind::Text,
                    node: mutator.create_text_node(text),
                    attributes: BTreeMap::new(),
                    children: Vec::new(),
                }
            }
        }
        Node::Element {
            key,
            tag,
            attributes,
            children,
        } => {
            let svg = parent_svg || tag == "svg";
            let children_are_svg = svg && tag != "foreignObject";
            let mut mounted = reusable.unwrap_or_else(|| Mounted {
                key: key.clone(),
                kind: MountedKind::Element(tag.clone()),
                node: mutator.create_element(element_name(tag, svg), Vec::new()),
                attributes: BTreeMap::new(),
                children: Vec::new(),
            });
            for removed in mounted
                .attributes
                .keys()
                .filter(|name| !attributes.contains_key(*name))
                .cloned()
                .collect::<Vec<_>>()
            {
                mutator.clear_attribute(mounted.node, attribute_name(&removed));
            }
            for (name, value) in attributes {
                if mounted.attributes.get(name) != Some(value) {
                    mutator.set_attribute(mounted.node, attribute_name(name), value);
                }
            }
            let mut prior = std::mem::take(&mut mounted.children)
                .into_iter()
                .map(|child| (child.key.clone(), child))
                .collect::<BTreeMap<_, _>>();
            let mut reconciled = Vec::with_capacity(children.len());
            for child in children {
                reconciled.push(reconcile(
                    mutator,
                    prior.remove(child.key()),
                    child,
                    children_are_svg,
                ));
            }
            for removed in prior.into_values() {
                mutator.remove_and_drop_node(removed.node);
            }
            let ids = reconciled
                .iter()
                .map(|child| child.node)
                .collect::<Vec<_>>();
            mutator.append_children(mounted.node, &ids);
            mounted.attributes.clone_from(attributes);
            mounted.children = reconciled;
            mounted
        }
    }
}

fn html_name(tag: &str) -> blitz_dom::QualName {
    blitz_dom::QualName {
        prefix: None,
        ns: blitz_dom::ns!(html),
        local: blitz_dom::LocalName::from(tag),
    }
}

fn element_name(tag: &str, svg: bool) -> blitz_dom::QualName {
    if svg {
        blitz_dom::QualName {
            prefix: None,
            ns: blitz_dom::ns!(svg),
            local: blitz_dom::LocalName::from(tag),
        }
    } else {
        html_name(tag)
    }
}

fn attribute_name(name: &str) -> blitz_dom::QualName {
    blitz_dom::QualName {
        prefix: None,
        ns: blitz_dom::ns!(),
        local: blitz_dom::LocalName::from(name),
    }
}

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
    pub fn install(&self, source: &Path) -> Result<InstallOutcome, String> {
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
                Some(Package::open(&target)?)
            } else {
                None
            };
            if prior
                .as_ref()
                .is_some_and(|old| old.manifest.release == package.manifest.release)
            {
                return Ok(InstallOutcome::Unchanged);
            }
            if let Some(old) = &prior {
                compatible_properties(&old.manifest, &package.manifest)?;
            }
            package.smoke_test()?;
            fs::rename(&temporary, &target)
                .map_err(|error| format!("activate skin package: {error}"))?;
            File::open(&self.0)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| format!("sync skin store: {error}"))?;
            Ok(
                prior.map_or(InstallOutcome::Installed, |old| InstallOutcome::Replaced {
                    previous_release: old.manifest.release,
                }),
            )
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

fn compatible_properties(old: &Manifest, new: &Manifest) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let browser = include_str!("skin_browser.js");
        assert!(browser.contains("Number.isSafeInteger"));
        assert!(browser.contains("value.attributes === undefined"));
        assert!(browser.contains("value.children === undefined"));
    }

    #[test]
    fn repository_manifests_are_valid_package_sources() {
        for manifest in [
            include_str!("../../../skins/cyan-system/skin.toml"),
            include_str!("../../../skins/result-aurora/skin.toml"),
            include_str!("../../../skins/dj-blackbox/skin.toml"),
        ] {
            toml::from_str::<Manifest>(manifest)
                .unwrap()
                .validate()
                .unwrap();
        }
    }

    #[test]
    fn updates_preserve_only_surviving_scope_and_type_pairs() {
        let old: Manifest =
            toml::from_str(include_str!("../../../skins/cyan-system/skin.toml")).unwrap();
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

    #[test]
    fn wasm_start_function_is_covered_by_the_hard_timeout() {
        let manifest: Manifest =
            toml::from_str(include_str!("../../../skins/cyan-system/skin.toml")).unwrap();
        let looping_start = vec![
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x08, 0x01, 0x00, 0x0a, 0x09, 0x01, 0x07, 0x00, 0x03, 0x40,
            0x0c, 0x00, 0x0b, 0x0b,
        ];
        let package = Package {
            manifest,
            entries: BTreeMap::from([(MODULE_PATH.into(), looping_start)]),
        };
        let started = Instant::now();
        let error = Runtime::new(&package).err().unwrap();
        assert!(error.contains("hard timeout"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(4));
    }
}
