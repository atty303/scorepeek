use super::*;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisualDebugScenario {
    #[serde(default)]
    pub canvases: Option<Vec<scorepeek_overlay::CanvasPresentation>>,
    pub skin: Option<scorepeek_overlay::Skin>,
    #[serde(default = "visual_debug_default_size")]
    pub logical_size: [u32; 2],
    #[serde(default = "visual_debug_default_scale")]
    pub scale: f32,
    pub canvas_id: Option<String>,
    #[serde(default = "visual_debug_default_editing")]
    pub editing: bool,
    #[serde(default)]
    pub selectors: Vec<String>,
    #[serde(default)]
    pub actions: Vec<VisualDebugAction>,
}

const fn visual_debug_default_size() -> [u32; 2] {
    [1920, 1080]
}

const fn visual_debug_default_scale() -> f32 {
    1.0
}

const fn visual_debug_default_editing() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum VisualDebugAction {
    TitleText {
        text: String,
        #[serde(default)]
        composing: bool,
    },
    SetEditing {
        value: bool,
    },
    SetScreen {
        screen: Option<scorepeek_overlay::ScreenKind>,
    },
    Click {
        selector: String,
    },
    Scroll {
        selector: String,
        dx: f64,
        dy: f64,
    },
    Drag {
        from: [f64; 2],
        to: [f64; 2],
        button: VisualDebugButton,
    },
    Capture {
        name: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualDebugButton {
    Left,
    Right,
}

#[derive(Serialize)]
struct VisualDebugManifest {
    schema_version: u32,
    run_id: String,
    resource: VisualDebugResource,
    logical_size: [u32; 2],
    physical_size: Option<[u32; 2]>,
    scale: f32,
    status: &'static str,
    completeness: &'static str,
    operations: Vec<VisualDebugOperation>,
    error: Option<VisualDebugError>,
}

#[derive(Serialize)]
struct VisualDebugResource {
    program: &'static str,
    version: &'static str,
    renderer: &'static str,
}

#[derive(Serialize)]
struct VisualDebugOperation {
    sequence: usize,
    action: String,
    status: &'static str,
    image: String,
    layout: String,
}

#[derive(Serialize)]
struct VisualDebugError {
    operation: String,
    error_type: &'static str,
    message: String,
}

#[derive(Serialize)]
struct VisualDebugLayout {
    schema_version: u32,
    logical_size: [u32; 2],
    physical_size: [u32; 2],
    scale: f32,
    elements: Vec<VisualDebugElement>,
}

#[derive(Serialize)]
struct VisualDebugElement {
    selector: String,
    matches: Vec<[f64; 4]>,
}

pub(super) struct VisualDebugSession {
    pub(super) document: DioxusDocument,
    pub(super) pointer: PointerInput,
    logical_size: [u32; 2],
    physical_size: [u32; 2],
    scale: f32,
    pub(super) projection: Reactive<NativeDocumentProjection>,
    pub(super) authority: NativeEditorAuthority,
    pub(super) commands: std::sync::mpsc::Receiver<CoordinatorCommand>,
    state: OverlayState,
    pub(super) skins: std::collections::BTreeMap<String, EditorSkinPreview>,
    skin_assets: Arc<SkinAssetCache>,
    report: Rc<RefCell<RunReport>>,
    runtime_create_count: u64,
}

impl VisualDebugSession {
    #[allow(clippy::too_many_lines)]
    pub(super) fn new(
        scenario: &VisualDebugScenario,
        physical_size: [u32; 2],
    ) -> Result<Self, String> {
        #[cfg(test)]
        let default_skin = "dev.atty303.scorepeek.skin.cyan-system".parse().ok();
        #[cfg(not(test))]
        let default_skin = SkinAssetCache::new(crate::skin::StoreRoot::discover())
            .installed_editor_skins()?
            .first()
            .map(|skin| skin.id);
        let skin = scenario
            .skin
            .or(default_skin)
            .ok_or("visual debugging requires an installed skin or scenario skin")?;
        let config = crate::config::visual_debug_config(skin);
        let mut canvases = config
            .canvases
            .into_iter()
            .filter(|canvas| canvas.backend == crate::bridge::data::Backend::Wayland)
            .map(|canvas| canvas.presentation())
            .collect::<Vec<_>>();
        if let Some(replacement) = &scenario.canvases {
            canvases.clone_from(replacement);
        }
        if let Some(skin) = scenario.skin {
            for canvas in &mut canvases {
                canvas.skin = skin;
            }
        }
        let selected = scenario
            .canvas_id
            .as_ref()
            .map_or_else(
                || canvases.first().map(|canvas| canvas.id.clone()),
                |id| {
                    canvases
                        .iter()
                        .find(|canvas| &canvas.id == id)
                        .map(|canvas| canvas.id.clone())
                },
            )
            .ok_or_else(|| "canvas_id does not select a Wayland canvas".to_owned())?;
        let canvas = canvases
            .iter()
            .find(|canvas| canvas.id == selected)
            .cloned()
            .ok_or("the initial Wayland workspace is empty")?;
        let output = EditorOutput {
            name: canvas.output.clone().unwrap_or_else(|| "HEADLESS-1".into()),
            model: "scorepeek visual debugger".into(),
            logical_size: Some(scenario.logical_size),
        };
        let mut model = EditorSession::new(canvases, scenario.logical_size, "visual");
        model.set_session_id(1);
        #[cfg(test)]
        model.set_skins(embedded_editor_skins());
        #[cfg(not(test))]
        model.set_skins(
            SkinAssetCache::new(crate::skin::StoreRoot::discover()).installed_editor_skins()?,
        );
        model.set_outputs(vec![output.clone()]);
        model.active_output = Some(output.name.clone());
        model.selected_canvas = Some(selected);
        model.editing = scenario.editing;
        model.readonly = false;
        model.advance_revision();
        let published_stages = Arc::new(std::sync::Mutex::new(PublishedStages::default()));
        let authority = NativeEditorAuthority::new(model, published_stages);
        let initial = if scenario.editing {
            NativeDocumentProjection::Editor(authority.session().stage_projection(&output))
        } else {
            NativeDocumentProjection::Display {
                canvas: canvas.clone(),
                visible: true,
            }
        };
        let published = Rc::new(RefCell::new(None));
        let (sender, commands) = std::sync::mpsc::channel();
        let props = NativeOverlayProps {
            initial,
            published: Rc::clone(&published),
            port: NativeEditorPort {
                coordinator: sender,
                source_output: Some(output.name),
                run_id: "visual-debug".into(),
                sequence: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            },
        };
        #[cfg(test)]
        let (document_config, skin_assets) = {
            let package_root =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/skins");
            let packages = ["cyan-system.zip", "result-aurora.zip", "dj-blackbox.zip"]
                .into_iter()
                .map(|name| crate::skin::Package::open(&package_root.join(name)))
                .collect::<Result<Vec<_>, _>>()?;
            let fonts = packages
                .iter()
                .flat_map(crate::skin::Package::font_resources)
                .map(<[u8]>::to_vec)
                .collect();
            document_config_inner_with_handle(
                Arc::new(SkinAssetCache::with_packages(packages)),
                fonts,
            )
        };
        #[cfg(not(test))]
        let package = {
            let store = crate::skin::StoreRoot::discover();
            let package_path = store.path().join(format!("{}.zip", canvas.skin.name()));
            package_path
                .exists()
                .then(|| store.open(canvas.skin.name()))
                .transpose()?
        };
        #[cfg(not(test))]
        let (document_config, skin_assets) = package.map_or_else(
            || {
                document_config_inner_with_handle(
                    Arc::new(SkinAssetCache::new(crate::skin::StoreRoot::discover())),
                    Vec::new(),
                )
            },
            |package| document_config_with_skin_handle(Arc::new(package)),
        );
        let mut document = DioxusDocument::new(
            VirtualDom::new_with_props(native_overlay, props),
            document_config,
        );
        document.initial_build();
        let projection = published
            .borrow()
            .as_ref()
            .copied()
            .ok_or("native overlay did not publish its projection")?;
        let state = scorepeek_overlay::editor_sample_state();
        let mut skins = std::collections::BTreeMap::new();
        let mounted_canvases = match &*projection.borrow() {
            NativeDocumentProjection::Editor(editor_projection) => {
                editor_projection.canvases.clone()
            }
            NativeDocumentProjection::Display { .. } => vec![canvas.clone()],
        };
        for mounted in mounted_canvases {
            if scenario.editing {
                continue;
            }
            let store = crate::skin::StoreRoot::discover();
            let package_path = store.path().join(format!("{}.zip", mounted.skin.name()));
            if !package_path.exists() {
                continue;
            }
            let package = skin_assets.load(mounted.skin.name())?;
            let root_selector = if scenario.editing {
                format!("#{}", editor_skin_root_id(&mounted.id))
            } else {
                "#scorepeek-skin-root".into()
            };
            let root = document
                .inner
                .borrow()
                .query_selector(&root_selector)
                .map_err(|error| format!("query native skin root: {error:?}"))?
                .ok_or("native skin root is missing")?;
            let css = namespace_skin_css(
                mounted.skin.name(),
                std::str::from_utf8(
                    package
                        .resource(crate::skin::STYLE_PATH)
                        .ok_or("skin.css missing")?,
                )
                .map_err(|error| format!("skin.css is not UTF-8: {error}"))?,
            );
            let mut runtime = crate::skin::Runtime::new(&package)?;
            let input = native_skin_input_presentation(&mounted, &state, &package.manifest);
            let mut initial = runtime.init(&input)?;
            namespace_native_skin_output(&package.manifest.id, &mut initial);
            let mut tree =
                crate::skin::NativeTree::new(&mut document.inner.borrow_mut(), root, &css);
            tree.apply(&mut document.inner.borrow_mut(), &initial);
            skins.insert(
                mounted.id.clone(),
                EditorSkinPreview {
                    canvas: {
                        let mut canvas = crate::config::empty_canvas(
                            mounted.id.clone(),
                            crate::bridge::data::Backend::Wayland,
                            mounted.skin,
                        );
                        canvas.apply_presentation(&mounted);
                        canvas
                    },
                    runtime,
                    tree,
                    package,
                    next_render: skin_deadline(&initial.schedule, false),
                    last_input: input,
                    last_state: state.clone(),
                },
            );
        }
        let report = Rc::new(RefCell::new(RunReport::new()));
        let runtime_create_count = u64::try_from(skins.len()).unwrap_or(u64::MAX);
        let mut session = Self {
            document,
            pointer: PointerInput::default(),
            logical_size: scenario.logical_size,
            physical_size,
            scale: scenario.scale,
            projection,
            authority,
            commands,
            state,
            skins,
            skin_assets,
            report,
            runtime_create_count,
        };
        session.resolve();
        Ok(session)
    }

    fn flush_inputs(&mut self) -> bool {
        let mut changed = false;
        while let Ok(command) = self.commands.try_recv() {
            if let CoordinatorCommand::EditorInput { input, .. } = command {
                let _ = self.authority.dispatch(input);
                changed = true;
            }
        }
        let output = match &*self.projection.borrow() {
            NativeDocumentProjection::Editor(current) => Some(current.output.clone()),
            NativeDocumentProjection::Display { .. } => None,
        };
        if let Some(output) = output {
            let candidate = self.authority.session().stage_projection(&output);
            changed |= accept_stage_projection_replica(
                self.projection,
                &mut self.document,
                &mut self.skins,
                &self.skin_assets,
                &output.name,
                &candidate,
            );
        }
        changed
    }

    pub(super) fn resolve(&mut self) {
        loop {
            let mut changed = false;
            while poll_native_document(&mut self.document, Waker::noop()) {
                changed = true;
            }
            changed |= self.flush_inputs();
            if !changed {
                break;
            }
        }
        let _ = self.render_skin();
        let mut inner = self.document.inner.borrow_mut();
        inner.set_viewport(Viewport::new(
            self.physical_size[0],
            self.physical_size[1],
            self.scale,
            ColorScheme::Dark,
        ));
        inner.resolve(0.0);
        resolve_with_loaded_resources(&mut inner, 1.0);
    }

    #[allow(clippy::too_many_lines)]
    fn render_skin(&mut self) -> Result<(), String> {
        let editor = match &*self.projection.borrow() {
            NativeDocumentProjection::Editor(stage) => {
                Some((stage.canvases.clone(), stage.output.name.clone()))
            }
            NativeDocumentProjection::Display { .. } => None,
        };
        if let Some((canvases, output)) = editor {
            let mut work = FrameWorkProfile::default();
            let _ = reconcile_editor_skin_previews(
                &mut self.document,
                &mut self.skins,
                &canvases,
                &self.skin_assets,
                &self.report,
                Some(&output),
                &self.state,
                &mut self.runtime_create_count,
                &mut work,
            )?;
            return Ok(());
        }
        let canvases = match &*self.projection.borrow() {
            NativeDocumentProjection::Display { canvas, .. } => vec![canvas.clone()],
            NativeDocumentProjection::Editor(_) => unreachable!(),
        };
        let live = canvases
            .iter()
            .map(|canvas| canvas.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        self.skins.retain(|id, _| live.contains(id.as_str()));
        for canvas in canvases {
            let desired_skin = canvas.skin.name();
            if !self.skins.contains_key(&canvas.id) {
                let store = crate::skin::StoreRoot::discover();
                let package_path = store.path().join(format!("{desired_skin}.zip"));
                if !package_path.exists() {
                    continue;
                }
                let package = self.skin_assets.load(desired_skin)?;
                let root_selector = if matches!(
                    &*self.projection.borrow(),
                    NativeDocumentProjection::Editor(_)
                ) {
                    format!("#{}", editor_skin_root_id(&canvas.id))
                } else {
                    "#scorepeek-skin-root".into()
                };
                let Some(root) = self
                    .document
                    .inner
                    .borrow()
                    .query_selector(&root_selector)
                    .map_err(|error| format!("query native skin root: {error:?}"))?
                else {
                    continue;
                };
                let css = namespace_skin_css(
                    desired_skin,
                    std::str::from_utf8(
                        package
                            .resource(crate::skin::STYLE_PATH)
                            .ok_or("skin.css missing")?,
                    )
                    .map_err(|error| format!("skin.css is not UTF-8: {error}"))?,
                );
                let mut runtime = crate::skin::Runtime::new(&package)?;
                let initial_input =
                    native_skin_input_presentation(&canvas, &self.state, &package.manifest);
                let mut output = runtime.init(&initial_input)?;
                namespace_native_skin_output(&package.manifest.id, &mut output);
                let mut tree =
                    crate::skin::NativeTree::new(&mut self.document.inner.borrow_mut(), root, &css);
                tree.apply(&mut self.document.inner.borrow_mut(), &output);
                self.skins.insert(
                    canvas.id.clone(),
                    EditorSkinPreview {
                        canvas: {
                            let mut configured = crate::config::empty_canvas(
                                canvas.id.clone(),
                                crate::bridge::data::Backend::Wayland,
                                canvas.skin,
                            );
                            configured.apply_presentation(&canvas);
                            configured
                        },
                        runtime,
                        tree,
                        package,
                        next_render: skin_deadline(&output.schedule, false),
                        last_input: initial_input,
                        last_state: self.state.clone(),
                    },
                );
            }
            let Some(skin) = self.skins.get_mut(&canvas.id) else {
                continue;
            };
            let changed = skin.package.manifest.id != desired_skin;
            if changed {
                skin.package = self.skin_assets.load(desired_skin)?;
                skin.runtime = crate::skin::Runtime::new(&skin.package)?;
            }
            let input =
                native_skin_input_presentation(&canvas, &self.state, &skin.package.manifest);
            let mut output = if changed {
                skin.runtime.init(&input)?
            } else {
                skin.runtime.render(&input)?
            };
            namespace_native_skin_output(&skin.package.manifest.id, &mut output);
            if changed {
                let css = namespace_skin_css(
                    desired_skin,
                    std::str::from_utf8(
                        skin.package
                            .resource(crate::skin::STYLE_PATH)
                            .ok_or("skin.css missing")?,
                    )
                    .map_err(|error| format!("skin.css is not UTF-8: {error}"))?,
                );
                skin.tree
                    .replace(&mut self.document.inner.borrow_mut(), &css, &output);
            } else {
                skin.tree
                    .apply(&mut self.document.inner.borrow_mut(), &output);
            }
        }
        Ok(())
    }

    fn set_screen(&mut self, screen: Option<scorepeek_overlay::ScreenKind>) {
        self.state.screen.kind = screen;
        self.state.screen.suspended_since_unix_ms = None;
        self.state.screen.revision = self.state.screen.revision.saturating_add(1);
        let canvas = match &*self.projection.borrow() {
            NativeDocumentProjection::Display { canvas, .. } => Some(canvas.clone()),
            NativeDocumentProjection::Editor(_) => None,
        };
        if let Some(canvas) = canvas {
            let visible =
                scorepeek_overlay::canvas_visible(canvas.show_on.as_deref(), self.state.screen);
            self.projection
                .set(NativeDocumentProjection::Display { canvas, visible });
        }
        self.resolve();
    }

    pub(super) fn set_editing(&mut self, editing: bool) {
        let mut model = self.authority.session().clone();
        model.editing = editing;
        model.advance_revision();
        let output = model.outputs.first().cloned();
        self.authority = NativeEditorAuthority::new(
            model,
            Arc::new(std::sync::Mutex::new(PublishedStages::default())),
        );
        // The editor and display projections mount different Dioxus roots. Rebuild the
        // visual harness trees against the new roots just as production surface roles do.
        let previous_output = match &*self.projection.borrow() {
            NativeDocumentProjection::Editor(stage) => Some(stage.output.name.clone()),
            NativeDocumentProjection::Display { .. } => None,
        };
        for (id, mut skin) in std::mem::take(&mut self.skins) {
            skin.tree.unmount(&mut self.document.inner.borrow_mut());
            if let Some(output) = previous_output.as_deref() {
                self.skin_assets.release_editor_owner(&id, output);
            }
        }
        if let Some(output) = output {
            self.projection.set(if editing {
                NativeDocumentProjection::Editor(self.authority.session().stage_projection(&output))
            } else if let Some(canvas) = self.authority.session().current().cloned() {
                NativeDocumentProjection::Display {
                    canvas,
                    visible: true,
                }
            } else {
                return;
            });
        }
        self.resolve();
    }

    fn title_text(&mut self, text: String, composing: bool) -> Result<(), String> {
        if self.authority.session().title.is_none() {
            return Err("title input is not active".into());
        }
        let _ = self
            .authority
            .dispatch(EditorInput::Action(EditorAction::TextComposition {
                field_key: "editor-title-input".into(),
                composing,
            }));
        let _ = self
            .authority
            .dispatch(EditorInput::Action(EditorAction::TitleText(text)));
        self.flush_inputs();
        self.resolve();
        Ok(())
    }

    pub(super) fn click(&mut self, selector: &str) -> Result<(), String> {
        let point = {
            let inner = self.document.inner.borrow();
            let node = inner
                .query_selector(selector)
                .map_err(|_| "invalid selector")?
                .ok_or_else(|| format!("selector did not match: {selector}"))?;
            let rect = inner
                .get_client_bounding_rect(node)
                .ok_or("selector has no layout")?;
            [rect.x + rect.width / 2.0, rect.y + rect.height / 2.0]
        };
        self.pointer.click(&mut self.document, point);
        self.resolve();
        Ok(())
    }

    pub(super) fn scroll(&mut self, selector: &str, dx: f64, dy: f64) -> Result<(), String> {
        let point = {
            let inner = self.document.inner.borrow();
            let node = inner
                .query_selector(selector)
                .map_err(|_| "invalid selector".to_owned())?
                .ok_or_else(|| format!("selector did not match: {selector}"))?;
            let rect = inner
                .get_client_bounding_rect(node)
                .ok_or("selector has no layout")?;
            [rect.x + rect.width / 2.0, rect.y + rect.height / 2.0]
        };
        self.pointer.wheel(&mut self.document, point, [dx, dy]);
        self.resolve();
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn key(&mut self, command: &scorepeek_overlay_wayland_handles::TextCommand) {
        self.key_composing(command, false);
    }

    #[cfg(test)]
    pub(super) fn key_composing(
        &mut self,
        command: &scorepeek_overlay_wayland_handles::TextCommand,
        composing: bool,
    ) {
        text::dispatch_control_key(&mut self.document, command, composing);
        self.resolve();
    }

    #[cfg(test)]
    pub(super) fn ime(&mut self, update: scorepeek_overlay_wayland_handles::TextUpdate) {
        let field_key = text::focused_field_key(&self.document);
        if let Some(composing) = text::dispatch_control_composition(&mut self.document, update)
            && let Some(field_key) = field_key
        {
            let _ = self
                .authority
                .dispatch(EditorInput::Action(EditorAction::TextComposition {
                    field_key,
                    composing,
                }));
        }
        self.resolve();
    }

    #[cfg(test)]
    pub(super) fn focus(&mut self, selector: &str) -> Result<(), String> {
        let node = self
            .document
            .inner
            .borrow()
            .query_selector(selector)
            .map_err(|_| "invalid selector".to_owned())?
            .ok_or_else(|| format!("selector did not match: {selector}"))?;
        self.document.inner.borrow_mut().set_focus_to(node);
        self.resolve();
        Ok(())
    }

    fn drag(
        &mut self,
        from: [f64; 2],
        to: [f64; 2],
        button: VisualDebugButton,
    ) -> Result<(), String> {
        if !matches!(
            &*self.projection.borrow(),
            NativeDocumentProjection::Editor(_)
        ) {
            return Err("drag requires editing".into());
        }
        let button = match button {
            VisualDebugButton::Right => 0x111,
            VisualDebugButton::Left => 0x110,
        };
        self.pointer
            .dispatch(&mut self.document, from, button, None);
        self.pointer
            .dispatch(&mut self.document, from, button, Some(true));
        self.resolve();
        if self.authority.session().drag.is_none() {
            return Err("drag did not reach a Dioxus canvas handler".into());
        }
        self.pointer.dispatch(&mut self.document, to, button, None);
        self.resolve();
        self.pointer
            .dispatch(&mut self.document, to, button, Some(false));
        self.resolve();
        Ok(())
    }

    fn render(
        &mut self,
        renderer: &mut anyrender_vello::VelloImageRenderer,
        path: &std::path::Path,
    ) -> Result<(), String> {
        let mut pixels = Vec::new();
        let mut inner = self.document.inner.borrow_mut();
        resolve_with_loaded_resources(&mut inner, 1.0);
        renderer.render_to_vec(
            |scene| {
                paint_native_scene(
                    scene,
                    &mut inner,
                    f64::from(self.scale),
                    self.physical_size[0],
                    self.physical_size[1],
                );
            },
            &mut pixels,
        );
        image::save_buffer_with_format(
            path,
            &pixels,
            self.physical_size[0],
            self.physical_size[1],
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(|error| error.to_string())
    }

    fn layout(&self, selectors: &[String]) -> Result<VisualDebugLayout, String> {
        let inner = self.document.inner.borrow();
        let elements = selectors
            .iter()
            .map(|selector| {
                let matches = inner
                    .query_selector_all(selector)
                    .map_err(|_| format!("invalid selector: {selector}"))?
                    .into_iter()
                    .filter_map(|node| inner.get_client_bounding_rect(node))
                    .map(|rect| [rect.x, rect.y, rect.width, rect.height])
                    .collect();
                Ok(VisualDebugElement {
                    selector: selector.clone(),
                    matches,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(VisualDebugLayout {
            schema_version: 1,
            logical_size: self.logical_size,
            physical_size: self.physical_size,
            scale: self.scale,
            elements,
        })
    }
}

/// Renders the production Dioxus native DOM through Blitz and Vello without a
/// Wayland compositor, recording every requested interaction and layout.
///
/// # Errors
///
/// Returns an error when the scenario is invalid or an artifact cannot be rendered or written.
#[allow(clippy::cast_possible_truncation, clippy::too_many_lines)]
pub fn run_visual_debug(
    scenario: &VisualDebugScenario,
    output: &std::path::Path,
) -> Result<(), String> {
    std::fs::create_dir(output).map_err(|error| format!("create visual output: {error}"))?;
    let physical_size = if scenario.scale.is_finite() && scenario.scale > 0.0 {
        [
            u32::try_from(
                (f64::from(scenario.logical_size[0]) * f64::from(scenario.scale)).round() as i64,
            )
            .map_err(|error| error.to_string())?,
            u32::try_from(
                (f64::from(scenario.logical_size[1]) * f64::from(scenario.scale)).round() as i64,
            )
            .map_err(|error| error.to_string())?,
        ]
    } else {
        return Err("visual scale must be finite and positive".into());
    };
    let selectors = if scenario.selectors.is_empty() {
        [
            ".canvas-content",
            ".overlay-canvas",
            ".widget-slot",
            ".editor-panel-toggle",
            ".editor-panel",
            ".inspector-scroll",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>()
    } else {
        scenario.selectors.clone()
    };
    let mut manifest = VisualDebugManifest {
        schema_version: 1,
        run_id: format!("visual-{}", std::process::id()),
        resource: VisualDebugResource {
            program: "scorepeek-overlay-native-visual",
            version: env!("CARGO_PKG_VERSION"),
            renderer: "dioxus-native-dom+blitz+vello",
        },
        logical_size: scenario.logical_size,
        physical_size: Some(physical_size),
        scale: scenario.scale,
        status: "running",
        completeness: "partial",
        operations: Vec::new(),
        error: None,
    };
    let result = (|| {
        let mut session = VisualDebugSession::new(scenario, physical_size)?;
        let mut renderer =
            anyrender_vello::VelloImageRenderer::new(physical_size[0], physical_size[1]);
        capture_visual_debug(
            &mut session,
            &mut renderer,
            output,
            &selectors,
            &mut manifest,
            0,
            "initial",
        )?;
        for (index, action) in scenario.actions.iter().enumerate() {
            let name = match action {
                VisualDebugAction::TitleText { text, composing } => {
                    session.title_text(text.clone(), *composing)?;
                    if *composing {
                        "title-preedit".into()
                    } else {
                        "title-commit".into()
                    }
                }
                VisualDebugAction::SetEditing { value } => {
                    session.set_editing(*value);
                    format!("set-editing-{value}")
                }
                VisualDebugAction::SetScreen { screen } => {
                    session.set_screen(*screen);
                    screen.map_or_else(
                        || "set-screen-none".into(),
                        |screen| format!("set-screen-{screen:?}"),
                    )
                }
                VisualDebugAction::Click { selector } => {
                    session.click(selector)?;
                    format!("click-{}", sanitize_artifact_name(selector))
                }
                VisualDebugAction::Scroll { selector, dx, dy } => {
                    session.scroll(selector, *dx, *dy)?;
                    format!("scroll-{}", sanitize_artifact_name(selector))
                }
                VisualDebugAction::Drag { from, to, button } => {
                    session.drag(*from, *to, *button)?;
                    "drag".into()
                }
                VisualDebugAction::Capture { name } => {
                    format!("capture-{}", sanitize_artifact_name(name))
                }
            };
            capture_visual_debug(
                &mut session,
                &mut renderer,
                output,
                &selectors,
                &mut manifest,
                index + 1,
                &name,
            )?;
        }
        Ok::<(), String>(())
    })();
    match &result {
        Ok(()) => {
            manifest.status = "complete";
            manifest.completeness = "complete";
        }
        Err(error) => {
            manifest.status = "failed";
            manifest.error = Some(VisualDebugError {
                operation: "render".into(),
                error_type: "visual_debug_failed",
                message: error.clone(),
            });
        }
    }
    std::fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    result
}

fn capture_visual_debug(
    session: &mut VisualDebugSession,
    renderer: &mut anyrender_vello::VelloImageRenderer,
    output: &std::path::Path,
    selectors: &[String],
    manifest: &mut VisualDebugManifest,
    sequence: usize,
    name: &str,
) -> Result<(), String> {
    let stem = format!("{sequence:03}-{}", sanitize_artifact_name(name));
    let image_name = format!("{stem}.png");
    let layout_name = format!("{stem}.layout.json");
    session.render(renderer, &output.join(&image_name))?;
    let layout = session.layout(selectors)?;
    std::fs::write(
        output.join(&layout_name),
        serde_json::to_vec_pretty(&layout).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    manifest.operations.push(VisualDebugOperation {
        sequence,
        action: name.to_owned(),
        status: "success",
        image: image_name,
        layout: layout_name,
    });
    Ok(())
}

fn sanitize_artifact_name(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "capture".into()
    } else {
        sanitized
    }
}
