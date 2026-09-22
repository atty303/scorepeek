use super::*;

pub(super) struct EditorSkinPreview {
    pub(super) canvas: crate::config::Canvas,
    pub(super) package: Arc<crate::skin::Package>,
    pub(super) runtime: crate::skin::Runtime,
    pub(super) tree: crate::skin::NativeTree,
    pub(super) next_render: Option<Instant>,
    pub(super) last_input: serde_json::Value,
    pub(super) last_state: OverlayState,
}

pub(super) struct NativeDisplaySkin {
    pub(super) runtime: crate::skin::Runtime,
    pub(super) tree: crate::skin::NativeTree,
    pub(super) release: String,
    pub(super) manifest: crate::skin::Manifest,
}

pub(super) fn render_native_display_skin(
    document: &mut DioxusDocument,
    canvas: &crate::config::Canvas,
    display: &mut NativeDisplaySkin,
    state: &OverlayState,
    next_skin_render: &mut Option<Instant>,
) -> Result<(), String> {
    let started = Instant::now();
    let mut output =
        display
            .runtime
            .render(&native_skin_input(canvas, state, &display.manifest))?;
    namespace_native_skin_output(&display.manifest.id, &mut output);
    display
        .tree
        .apply(&mut document.inner.borrow_mut(), &output);
    *next_skin_render = skin_deadline(&output.schedule, false);
    crate::diagnostics::emit(
        "skin_render",
        &serde_json::json!({"skin_id":canvas.skin.name(),"release":display.release,"canvas_id":canvas.id,"backend":"native","phase":"render","status":"success","duration_us":duration_us(started.elapsed()),"next_tick":format!("{:?}",output.schedule),"tree_applied":true}),
    );
    Ok(())
}

pub(super) fn create_native_display_skin(
    document: &mut DioxusDocument,
    canvas: &crate::config::Canvas,
    package: &Arc<crate::skin::Package>,
    report: &Rc<RefCell<RunReport>>,
    output: Option<&str>,
    state: &OverlayState,
) -> Result<(NativeDisplaySkin, Option<Instant>), String> {
    let mut runtime = new_native_skin_runtime(package, report, &canvas.id, output, &[])?;
    let skin_input = native_skin_input(canvas, state, &package.manifest);
    let started = Instant::now();
    let mut initial = runtime.init(&skin_input).inspect_err(|error| {
        crate::diagnostics::emit(
            "skin_render",
            &serde_json::json!({"skin_id":package.manifest.id,"release":package.manifest.release,"canvas_id":canvas.id,"backend":"native","phase":"init","status":"failed","error_type":skin_error_type(error)}),
        );
    })?;
    namespace_native_skin_output(&package.manifest.id, &mut initial);
    let root = document
        .inner
        .borrow()
        .query_selector("#scorepeek-skin-root")
        .map_err(|error| format!("query native skin root: {error:?}"))?
        .ok_or("native skin root is missing")?;
    let css = namespace_skin_css(
        canvas.skin.name(),
        std::str::from_utf8(
            package
                .resource(crate::skin::STYLE_PATH)
                .ok_or("skin.css missing")?,
        )
        .map_err(|error| format!("skin.css is not UTF-8: {error}"))?,
    );
    let mut tree = crate::skin::NativeTree::new(&mut document.inner.borrow_mut(), root, &css);
    tree.apply(&mut document.inner.borrow_mut(), &initial);
    crate::diagnostics::emit(
        "skin_render",
        &serde_json::json!({"skin_id":package.manifest.id,"release":package.manifest.release,"canvas_id":canvas.id,"backend":"native","phase":"init","status":"success","duration_us":u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),"tree_applied":true}),
    );
    let next = skin_deadline(&initial.schedule, false);
    Ok((
        NativeDisplaySkin {
            runtime,
            tree,
            release: package.manifest.release.clone(),
            manifest: package.manifest.clone(),
        },
        next,
    ))
}

#[derive(Default)]
pub(super) struct EditorSkinReconciliation {
    pub(super) retry_owner: bool,
    pub(super) wasm_calls: u64,
    pub(super) tree_updates: u64,
    pub(super) input_generations: u64,
}

pub(super) fn create_editor_skin_preview(
    document: &mut DioxusDocument,
    presentation: &scorepeek_overlay::CanvasPresentation,
    skin_assets: &Arc<SkinAssetCache>,
    report: &Rc<RefCell<RunReport>>,
    output: Option<&str>,
    state: &OverlayState,
    work: &mut FrameWorkProfile,
) -> Result<EditorSkinPreview, String> {
    let canvas = work.measure("canvas_config", || {
        let mut canvas = crate::config::empty_canvas(
            presentation.id.clone(),
            crate::bridge::data::Backend::Wayland,
            presentation.skin,
        );
        canvas.apply_presentation(presentation);
        canvas
    });
    let package = skin_assets.load_profiled(canvas.skin.name(), work)?;
    let mut runtime = work.measure("wasm_runtime_create", || {
        new_native_skin_runtime(&package, report, &canvas.id, output, &[])
    })?;
    let input = work.measure("skin_input", || {
        native_skin_input(&canvas, state, &package.manifest)
    });
    let (mut rendered, timing) = runtime.init_measured(&input)?;
    work.record("wasm_render", timing.wasm);
    work.record("json_tree", timing.json_tree);
    namespace_native_skin_output(&package.manifest.id, &mut rendered);
    let root_id = editor_skin_root_id(&canvas.id);
    let root = document
        .inner
        .borrow()
        .query_selector(&format!("#{root_id}"))
        .map_err(|error| format!("query editor skin root: {error:?}"))?
        .ok_or_else(|| format!("editor skin root is missing for {}", canvas.id))?;
    let css = namespace_skin_css(
        canvas.skin.name(),
        std::str::from_utf8(
            package
                .resource(crate::skin::STYLE_PATH)
                .ok_or("skin.css missing")?,
        )
        .map_err(|error| format!("skin.css is not UTF-8: {error}"))?,
    );
    let mut tree = crate::skin::NativeTree::new(&mut document.inner.borrow_mut(), root, &css);
    work.measure("tree_reconciliation", || {
        tree.apply(&mut document.inner.borrow_mut(), &rendered);
    });
    Ok(EditorSkinPreview {
        canvas,
        package,
        runtime,
        tree,
        next_render: skin_deadline(&rendered.schedule, false),
        last_input: input,
        last_state: state.clone(),
    })
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) fn reconcile_editor_skin_previews(
    document: &mut DioxusDocument,
    previews: &mut std::collections::BTreeMap<String, EditorSkinPreview>,
    presentations: &[scorepeek_overlay::CanvasPresentation],
    skin_assets: &Arc<SkinAssetCache>,
    report: &Rc<RefCell<RunReport>>,
    output: Option<&str>,
    state: &OverlayState,
    runtime_create_count: &mut u64,
    work: &mut FrameWorkProfile,
) -> Result<EditorSkinReconciliation, String> {
    let reconcile_started = Instant::now();
    let output = output.ok_or("editor skin preview requires an output owner")?;
    let mut reconciliation = EditorSkinReconciliation::default();
    let live = presentations
        .iter()
        .map(|presentation| presentation.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let removed = previews
        .keys()
        .filter(|id| !live.contains(id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    for id in removed {
        if let Some(mut preview) = previews.remove(&id) {
            preview.tree.unmount(&mut document.inner.borrow_mut());
            skin_assets.release_editor_owner(&id, output);
        }
    }

    let mut retry_owner = false;
    for presentation in presentations {
        if !previews.contains_key(&presentation.id) {
            if !skin_assets.acquire_editor_owner(&presentation.id, output) {
                retry_owner = true;
                continue;
            }
            let preview = match create_editor_skin_preview(
                document,
                presentation,
                skin_assets,
                report,
                Some(output),
                state,
                work,
            ) {
                Ok(preview) => preview,
                Err(error) => {
                    skin_assets.release_editor_owner(&presentation.id, output);
                    return Err(error);
                }
            };
            previews.insert(presentation.id.clone(), preview);
            *runtime_create_count = runtime_create_count.saturating_add(1);
            reconciliation.wasm_calls = reconciliation.wasm_calls.saturating_add(1);
            reconciliation.tree_updates = reconciliation.tree_updates.saturating_add(1);
            reconciliation.input_generations = reconciliation.input_generations.saturating_add(1);
            continue;
        }
        let preview = previews
            .get_mut(&presentation.id)
            .expect("checked editor preview");
        let presentation_changed = preview.canvas.presentation() != *presentation;
        if presentation_changed {
            work.measure("canvas_config", || {
                preview.canvas.apply_presentation(presentation);
            });
        }
        if !preview.tree.is_attached(&document.inner.borrow()) {
            let root_id = editor_skin_root_id(&presentation.id);
            let root = document
                .inner
                .borrow()
                .query_selector(&format!("#{root_id}"))
                .map_err(|error| format!("query editor skin root: {error:?}"))?
                .ok_or_else(|| format!("editor skin root is missing for {}", presentation.id))?;
            let input = work.measure("skin_input", || {
                native_skin_input(&preview.canvas, state, &preview.package.manifest)
            });
            let (mut rendered, timing) = preview.runtime.render_measured(&input)?;
            work.record("wasm_render", timing.wasm);
            work.record("json_tree", timing.json_tree);
            namespace_native_skin_output(&preview.package.manifest.id, &mut rendered);
            let css = namespace_skin_css(
                &preview.package.manifest.id,
                std::str::from_utf8(
                    preview
                        .package
                        .resource(crate::skin::STYLE_PATH)
                        .ok_or("skin.css missing")?,
                )
                .map_err(|error| format!("skin.css is not UTF-8: {error}"))?,
            );
            let mut tree =
                crate::skin::NativeTree::new(&mut document.inner.borrow_mut(), root, &css);
            tree.apply(&mut document.inner.borrow_mut(), &rendered);
            preview.tree = tree;
            preview.next_render = skin_deadline(&rendered.schedule, false);
            preview.last_input = input;
            preview.last_state = state.clone();
            reconciliation.wasm_calls = reconciliation.wasm_calls.saturating_add(1);
            reconciliation.tree_updates = reconciliation.tree_updates.saturating_add(1);
            reconciliation.input_generations = reconciliation.input_generations.saturating_add(1);
            continue;
        }
        let desired_skin = preview.canvas.skin.name();
        if preview.package.manifest.id != desired_skin {
            let next_package = skin_assets.load_profiled(desired_skin, work)?;
            let mut next_runtime = work.measure("wasm_runtime_create", || {
                new_native_skin_runtime(
                    &next_package,
                    report,
                    &preview.canvas.id,
                    Some(output),
                    &[],
                )
            })?;
            let next_input = work.measure("skin_input", || {
                native_skin_input(&preview.canvas, state, &next_package.manifest)
            });
            let (mut rendered, timing) = next_runtime.init_measured(&next_input)?;
            work.record("wasm_render", timing.wasm);
            work.record("json_tree", timing.json_tree);
            namespace_native_skin_output(&next_package.manifest.id, &mut rendered);
            let css = namespace_skin_css(
                desired_skin,
                std::str::from_utf8(
                    next_package
                        .resource(crate::skin::STYLE_PATH)
                        .ok_or("skin.css missing")?,
                )
                .map_err(|error| format!("skin.css is not UTF-8: {error}"))?,
            );
            work.measure("tree_reconciliation", || {
                preview
                    .tree
                    .replace(&mut document.inner.borrow_mut(), &css, &rendered);
            });
            preview.package = next_package;
            preview.runtime = next_runtime;
            preview.next_render = skin_deadline(&rendered.schedule, false);
            preview.last_input = next_input;
            preview.last_state = state.clone();
            *runtime_create_count = runtime_create_count.saturating_add(1);
            reconciliation.wasm_calls = reconciliation.wasm_calls.saturating_add(1);
            reconciliation.tree_updates = reconciliation.tree_updates.saturating_add(1);
            reconciliation.input_generations = reconciliation.input_generations.saturating_add(1);
            continue;
        }
        let due = preview
            .next_render
            .is_some_and(|deadline| deadline <= Instant::now());
        if !due && !presentation_changed && preview.last_state == *state {
            continue;
        }
        let input = work.measure("skin_input", || {
            native_skin_input(&preview.canvas, state, &preview.package.manifest)
        });
        reconciliation.input_generations = reconciliation.input_generations.saturating_add(1);
        if !due && input == preview.last_input {
            preview.last_state = state.clone();
            continue;
        }
        let (mut rendered, timing) = preview.runtime.render_measured(&input)?;
        work.record("wasm_render", timing.wasm);
        work.record("json_tree", timing.json_tree);
        namespace_native_skin_output(&preview.package.manifest.id, &mut rendered);
        work.measure("tree_reconciliation", || {
            preview
                .tree
                .apply(&mut document.inner.borrow_mut(), &rendered);
        });
        preview.next_render = skin_deadline(&rendered.schedule, false);
        preview.last_input = input;
        preview.last_state = state.clone();
        reconciliation.wasm_calls = reconciliation.wasm_calls.saturating_add(1);
        reconciliation.tree_updates = reconciliation.tree_updates.saturating_add(1);
    }
    reconciliation.retry_owner = retry_owner;
    work.record("skin_reconciliation", reconcile_started.elapsed());
    Ok(reconciliation)
}
