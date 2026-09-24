//! Native Wasmtime execution and Wayland DOM adaptation.

use scorepeek_overlay_runtime::skin::{MODULE_PATH, Package};
use scorepeek_skin_sdk::{Node, Output as RenderOutput};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::{Duration, Instant},
};
use wasmtime::{Engine, Instance, Memory, Module, Store, TypedFunc};

const CALL_TIMEOUT: Duration = Duration::from_secs(2);
const EPOCH_TICK: Duration = Duration::from_millis(10);
const CALL_TIMEOUT_TICKS: u64 = 200;
const COMPILED_MODULE_CACHE_CAPACITY: usize = 16;

struct CompiledModule {
    engine: Engine,
    module: Module,
}

#[derive(Clone, Copy)]
#[allow(
    dead_code,
    reason = "runtime timing is retained for native host profiling"
)]
pub struct RuntimeTiming {
    pub cache_phase: &'static str,
    pub engine_us: Option<u64>,
    pub module_us: Option<u64>,
    pub duration_us: u64,
}

#[derive(Clone, Copy)]
#[allow(
    dead_code,
    reason = "runtime timing is retained for native host profiling"
)]
pub struct RenderTiming {
    pub wasm: Duration,
    pub json_tree: Duration,
}

enum CompiledModuleEntry {
    Compiling,
    Ready(Arc<CompiledModule>),
}

#[derive(Default)]
struct CompiledModuleCache {
    entries: VecDeque<(Vec<u8>, CompiledModuleEntry)>,
}

static COMPILED_MODULES: OnceLock<(Mutex<CompiledModuleCache>, Condvar)> = OnceLock::new();
static SKIN_ENGINE: OnceLock<Result<Engine, String>> = OnceLock::new();

fn engine() -> Result<Engine, String> {
    SKIN_ENGINE
        .get_or_init(|| {
            let mut config = wasmtime::Config::new();
            config.epoch_interruption(true);
            let engine = Engine::new(&config)
                .map_err(|error| format!("initialize skin runtime: {error}"))?;
            let ticker_engine = engine.clone();
            std::thread::Builder::new()
                .name("overlay-skin-epoch".into())
                .spawn(move || {
                    loop {
                        std::thread::sleep(EPOCH_TICK);
                        ticker_engine.increment_epoch();
                    }
                })
                .map_err(|error| format!("start skin timeout clock: {error}"))?;
            Ok(engine)
        })
        .clone()
}

fn compile_module(
    wasm: &[u8],
    requested_at: Instant,
) -> Result<(Arc<CompiledModule>, RuntimeTiming), String> {
    let engine_started = Instant::now();
    let engine = engine()?;
    let engine_us = u64::try_from(engine_started.elapsed().as_micros()).unwrap_or(u64::MAX);
    let module_started = Instant::now();
    let module =
        Module::new(&engine, wasm).map_err(|error| format!("compile skin.wasm: {error:#}"))?;
    let module_us = u64::try_from(module_started.elapsed().as_micros()).unwrap_or(u64::MAX);
    Ok((
        Arc::new(CompiledModule { engine, module }),
        RuntimeTiming {
            cache_phase: "compiled",
            engine_us: Some(engine_us),
            module_us: Some(module_us),
            duration_us: u64::try_from(requested_at.elapsed().as_micros()).unwrap_or(u64::MAX),
        },
    ))
}

fn compiled_module(wasm: &[u8]) -> Result<(Arc<CompiledModule>, RuntimeTiming), String> {
    let requested_at = Instant::now();
    let (cache_mutex, ready) = COMPILED_MODULES
        .get_or_init(|| (Mutex::new(CompiledModuleCache::default()), Condvar::new()));
    let mut cache = cache_mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut waited = false;
    loop {
        match cache
            .entries
            .iter()
            .find(|(candidate, _)| candidate.as_slice() == wasm)
            .map(|(_, entry)| entry)
        {
            Some(CompiledModuleEntry::Ready(compiled)) => {
                return Ok((
                    Arc::clone(compiled),
                    RuntimeTiming {
                        cache_phase: if waited { "waited" } else { "hit" },
                        engine_us: None,
                        module_us: None,
                        duration_us: u64::try_from(requested_at.elapsed().as_micros())
                            .unwrap_or(u64::MAX),
                    },
                ));
            }
            Some(CompiledModuleEntry::Compiling) => {
                waited = true;
                cache = ready
                    .wait(cache)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            None => break,
        }
    }
    cache
        .entries
        .push_back((wasm.to_vec(), CompiledModuleEntry::Compiling));
    drop(cache);

    let compiled = compile_module(wasm, requested_at);

    let mut cache = cache_mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, entry)) = cache
        .entries
        .iter_mut()
        .find(|(candidate, _)| candidate.as_slice() == wasm)
    {
        match &compiled {
            Ok((compiled, _)) => *entry = CompiledModuleEntry::Ready(Arc::clone(compiled)),
            Err(_) => {
                cache
                    .entries
                    .retain(|(candidate, _)| candidate.as_slice() != wasm);
            }
        }
    }
    while cache.entries.len() > COMPILED_MODULE_CACHE_CAPACITY {
        if cache
            .entries
            .front()
            .is_some_and(|(_, entry)| matches!(entry, CompiledModuleEntry::Ready(_)))
        {
            cache.entries.pop_front();
        } else {
            break;
        }
    }
    ready.notify_all();
    compiled
}

pub struct Runtime {
    _compiled: Arc<CompiledModule>,
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
        Self::new_measured(package).map(|(runtime, _)| runtime)
    }

    /// # Errors
    /// Returns compile, import, or export ABI errors.
    pub fn new_measured(package: &Package) -> Result<(Self, RuntimeTiming), String> {
        let (compiled, timing) = compiled_module(
            package
                .resource(MODULE_PATH)
                .ok_or_else(|| format!("skin ZIP requires {MODULE_PATH} at its root"))?,
        )?;
        let engine = compiled.engine.clone();
        let module = compiled.module.clone();
        if module.imports().next().is_some() {
            return Err("skin.wasm must not import host or WASI functions".into());
        }
        let mut store = Store::new(&engine, ());
        store.set_epoch_deadline(CALL_TIMEOUT_TICKS);
        let instance = run_with_timeout("instantiate", || {
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
        Ok((
            Self {
                _compiled: compiled,
                store,
                memory,
                alloc,
                dealloc,
                init,
                render,
                _instance: instance,
            },
            timing,
        ))
    }

    /// Initializes this canvas instance and returns its first tree.
    /// # Errors
    /// Returns an ABI, trap, timeout, output decoding, or tree validation error.
    pub fn init(&mut self, input: &serde_json::Value) -> Result<RenderOutput, String> {
        self.call(input, true).map(|(output, _)| output)
    }

    /// # Errors
    /// Returns execution, timeout, or output validation errors.
    pub fn init_measured(
        &mut self,
        input: &serde_json::Value,
    ) -> Result<(RenderOutput, RenderTiming), String> {
        self.call(input, true)
    }

    /// Updates this canvas instance and returns its next complete tree.
    /// # Errors
    /// Returns an ABI, trap, timeout, output decoding, or tree validation error.
    pub fn render(&mut self, input: &serde_json::Value) -> Result<RenderOutput, String> {
        self.call(input, false).map(|(output, _)| output)
    }

    /// # Errors
    /// Returns execution, timeout, or output validation errors.
    pub fn render_measured(
        &mut self,
        input: &serde_json::Value,
    ) -> Result<(RenderOutput, RenderTiming), String> {
        self.call(input, false)
    }

    fn call(
        &mut self,
        input: &serde_json::Value,
        initialize: bool,
    ) -> Result<(RenderOutput, RenderTiming), String> {
        let bytes =
            serde_json::to_vec(input).map_err(|error| format!("serialize skin input: {error}"))?;
        let length =
            i32::try_from(bytes.len()).map_err(|_| "skin input exceeds ABI address space")?;
        self.store.set_epoch_deadline(CALL_TIMEOUT_TICKS);
        let phase = if initialize { "init" } else { "render" };
        let wasm_started = Instant::now();
        let packed = run_with_timeout(phase, || {
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
        let wasm = wasm_started.elapsed();
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
        let json_started = Instant::now();
        let output: RenderOutput =
            serde_json::from_slice(output).map_err(|error| format!("skin output JSON: {error}"))?;
        output.validate()?;
        Ok((
            output,
            RenderTiming {
                wasm,
                json_tree: json_started.elapsed(),
            },
        ))
    }
}

fn run_with_timeout<T>(
    phase: &str,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let started = Instant::now();
    let result = operation();
    result.map_err(|error| {
        if started.elapsed() >= CALL_TIMEOUT {
            format!("skin {phase} hard timeout")
        } else {
            error
        }
    })
}

/// Stateful keyed adapter from the shared JSON tree to Blitz's ordinary DOM.
pub struct NativeTree {
    root: blitz_dom::NodeId,
    style: blitz_dom::NodeId,
    marker: String,
    mounted: Option<Mounted>,
}

struct Mounted {
    key: String,
    kind: MountedKind,
    node: blitz_dom::NodeId,
    attributes: BTreeMap<String, String>,
    text: Option<String>,
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
        static NEXT_MARKER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let marker = NEXT_MARKER
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .to_string();
        let mut mutator = document.mutate();
        let style = mutator.create_element(html_name("style"), Vec::new());
        mutator.set_attribute(style, attribute_name("data-scorepeek-tree"), &marker);
        let text = mutator.create_text_node(css);
        mutator.append_children(style, &[text]);
        mutator.append_children(root, &[style]);
        drop(mutator);
        Self {
            root,
            style,
            marker,
            mounted: None,
        }
    }

    #[must_use]
    pub fn is_attached(&self, document: &blitz_dom::BaseDocument) -> bool {
        document
            .query_selector(&format!("[data-scorepeek-tree-root='{}']", self.marker))
            .ok()
            .flatten()
            .is_some()
    }

    /// Reconciles a validated full tree by stable key.
    pub fn apply(&mut self, document: &mut blitz_dom::BaseDocument, output: &RenderOutput) -> bool {
        let old = self.mounted.take();
        let old_root = old.as_ref().map(|mounted| mounted.node);
        let mut mutator = document.mutate();
        let (mounted, changed) = reconcile(&mut mutator, old, &output.tree, false);
        if old_root != Some(mounted.node) {
            mutator.set_attribute(
                mounted.node,
                attribute_name("data-scorepeek-tree-root"),
                &self.marker,
            );
            mutator.append_children(self.root, &[mounted.node]);
        }
        self.mounted = Some(mounted);
        changed
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
        mutator.set_attribute(
            self.style,
            attribute_name("data-scorepeek-tree"),
            &self.marker,
        );
        let text = mutator.create_text_node(css);
        mutator.append_children(self.style, &[text]);
        drop(mutator);
        self.apply(document, output);
    }

    pub fn set_css(&mut self, document: &mut blitz_dom::BaseDocument, css: &str) {
        let mut mutator = document.mutate();
        mutator.remove_and_drop_node(self.style);
        self.style = mutator.create_element(html_name("style"), Vec::new());
        mutator.set_attribute(
            self.style,
            attribute_name("data-scorepeek-tree"),
            &self.marker,
        );
        let text = mutator.create_text_node(css);
        mutator.append_children(self.style, &[text]);
        mutator.append_children(self.root, &[self.style]);
    }

    /// Removes every package-owned node while leaving the host-owned root intact.
    pub fn unmount(&mut self, document: &mut blitz_dom::BaseDocument) {
        let mut mutator = document.mutate();
        if let Some(mounted) = self.mounted.take() {
            mutator.remove_and_drop_node(mounted.node);
        }
        mutator.remove_and_drop_node(self.style);
    }
}

fn reconcile(
    mutator: &mut blitz_dom::DocumentMutator<'_>,
    old: Option<Mounted>,
    next: &Node,
    parent_svg: bool,
) -> (Mounted, bool) {
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
                let changed = mounted.text.as_deref() != Some(text);
                if changed {
                    mutator.set_node_text(mounted.node, text);
                    mounted.text = Some(text.clone());
                }
                mounted.children.clear();
                mounted.attributes.clear();
                (mounted, changed)
            } else {
                (
                    Mounted {
                        key: key.clone(),
                        kind: MountedKind::Text,
                        node: mutator.create_text_node(text),
                        attributes: BTreeMap::new(),
                        text: Some(text.clone()),
                        children: Vec::new(),
                    },
                    true,
                )
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
            let reused = reusable.is_some();
            let mut mounted = reusable.unwrap_or_else(|| Mounted {
                key: key.clone(),
                kind: MountedKind::Element(tag.clone()),
                node: mutator.create_element(element_name(tag, svg), Vec::new()),
                attributes: BTreeMap::new(),
                text: None,
                children: Vec::new(),
            });
            let mut changed = !reused;
            for removed in mounted
                .attributes
                .keys()
                .filter(|name| !attributes.contains_key(*name))
                .cloned()
                .collect::<Vec<_>>()
            {
                mutator.clear_attribute(mounted.node, attribute_name(&removed));
                changed = true;
            }
            for (name, value) in attributes {
                if mounted.attributes.get(name) != Some(value) {
                    mutator.set_attribute(mounted.node, attribute_name(name), value);
                    changed = true;
                }
            }
            let (reconciled, children_changed) = reconcile_children(
                mutator,
                mounted.node,
                std::mem::take(&mut mounted.children),
                children,
                children_are_svg,
            );
            changed |= children_changed;
            mounted.attributes.clone_from(attributes);
            mounted.children = reconciled;
            (mounted, changed)
        }
    }
}

fn reconcile_children(
    mutator: &mut blitz_dom::DocumentMutator<'_>,
    parent: blitz_dom::NodeId,
    previous: Vec<Mounted>,
    next: &[Node],
    svg: bool,
) -> (Vec<Mounted>, bool) {
    let mut prior = previous
        .into_iter()
        .map(|child| (child.key.clone(), child))
        .collect::<BTreeMap<_, _>>();
    let mut reconciled = Vec::with_capacity(next.len());
    let mut changed = false;
    for child in next {
        let (child, child_changed) = reconcile(mutator, prior.remove(child.key()), child, svg);
        changed |= child_changed;
        reconciled.push(child);
    }
    for removed in prior.into_values() {
        mutator.remove_and_drop_node(removed.node);
        changed = true;
    }
    let mut current = mutator.child_ids(parent).into_iter().collect::<Vec<_>>();
    for (index, child) in reconciled.iter().enumerate() {
        if current.get(index) == Some(&child.node) {
            continue;
        }
        if let Some(previous_index) = current.iter().position(|id| *id == child.node) {
            current.remove(previous_index);
        }
        if let Some(anchor) = current.get(index) {
            mutator.insert_nodes_before(*anchor, &[child.node]);
        } else {
            mutator.append_children(parent, &[child.node]);
        }
        current.insert(index, child.node);
        changed = true;
    }
    (reconciled, changed)
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

#[cfg(test)]
mod tests {
    use super::*;
    use scorepeek_overlay_runtime::skin::Manifest;
    #[test]
    fn compiled_modules_are_reused_by_exact_bytes() {
        let wasm = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x08, 0x01, 0x00, 0x0a, 0x09, 0x01, 0x07, 0x00, 0x03, 0x40,
            0x0c, 0x00, 0x0b, 0x0b,
        ];
        let (first, _) = compiled_module(&wasm).unwrap();
        let (second, timing) = compiled_module(&wasm).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(timing.cache_phase, "hit");
    }

    #[test]
    fn one_runtime_timeout_does_not_expire_another_runtime_deadline() {
        let wasm = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x60, 0x00, 0x01,
            0x7f, 0x60, 0x00, 0x00, 0x03, 0x03, 0x02, 0x00, 0x01, 0x07, 0x0f, 0x02, 0x04, 0x66,
            0x61, 0x73, 0x74, 0x00, 0x00, 0x04, 0x6c, 0x6f, 0x6f, 0x70, 0x00, 0x01, 0x0a, 0x0e,
            0x02, 0x04, 0x00, 0x41, 0x07, 0x0b, 0x07, 0x00, 0x03, 0x40, 0x0c, 0x00, 0x0b, 0x0b,
        ];
        let (compiled, _) = compiled_module(&wasm).unwrap();
        let mut looping_store = Store::new(&compiled.engine, ());
        looping_store.set_epoch_deadline(CALL_TIMEOUT_TICKS);
        let looping_instance = Instance::new(&mut looping_store, &compiled.module, &[]).unwrap();
        let looping = looping_instance
            .get_typed_func::<(), ()>(&mut looping_store, "loop")
            .unwrap();
        let keep_compiled = Arc::clone(&compiled);
        let timeout = std::thread::spawn(move || {
            let result = looping.call(&mut looping_store, ());
            drop(keep_compiled);
            result
        });

        std::thread::sleep(Duration::from_secs(1));
        let mut fast_store = Store::new(&compiled.engine, ());
        fast_store.set_epoch_deadline(CALL_TIMEOUT_TICKS);
        let fast_instance = Instance::new(&mut fast_store, &compiled.module, &[]).unwrap();
        let fast = fast_instance
            .get_typed_func::<(), i32>(&mut fast_store, "fast")
            .unwrap();
        assert!(timeout.join().unwrap().is_err());
        assert_eq!(fast.call(&mut fast_store, ()).unwrap(), 7);
    }

    #[test]
    fn wasm_start_function_is_covered_by_the_hard_timeout() {
        let manifest: Manifest =
            toml::from_str(include_str!("../../../../skins/cyan-system/skin.toml")).unwrap();
        let looping_start = vec![
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x08, 0x01, 0x00, 0x0a, 0x09, 0x01, 0x07, 0x00, 0x03, 0x40,
            0x0c, 0x00, 0x0b, 0x0b,
        ];
        let package = Package::test_with_entries(
            manifest,
            BTreeMap::from([(MODULE_PATH.into(), looping_start)]),
        );
        let started = Instant::now();
        let error = Runtime::new(&package).err().unwrap();
        assert!(error.contains("hard timeout"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(4));
    }
}
