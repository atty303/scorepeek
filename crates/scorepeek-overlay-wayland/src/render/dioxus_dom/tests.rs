use super::*;
use crate::render::blitz::NativeEventOutcome;
use crate::render::vello::retain_native_image_atlas;
use blitz_dom::Document as _;

fn test_skin(id: &str) -> scorepeek_overlay::Skin {
    id.parse().unwrap()
}

fn cyan_skin() -> scorepeek_overlay::Skin {
    test_skin("dev.atty303.scorepeek.skin.cyan-system")
}

fn result_skin() -> scorepeek_overlay::Skin {
    test_skin("dev.atty303.scorepeek.skin.result-aurora")
}

#[test]
fn configure_event_is_a_surface_boundary_even_when_geometry_is_unchanged() {
    #[derive(Default)]
    struct ConfigureConsumer {
        calls: usize,
    }

    impl NativeEventConsumer for ConfigureConsumer {
        fn configure_event(
            &mut self,
            _logical: [u32; 2],
            _physical: [u32; 2],
            _scale_120: u32,
        ) -> Result<(), String> {
            self.calls += 1;
            Ok(())
        }

        fn pointer_motion_event(&mut self, _point: [f64; 2]) {}
        fn pointer_button_event(&mut self, _button: u32, _pressed: bool, _point: [f64; 2]) {}
        fn pointer_scroll_event(&mut self, _delta: [f64; 2], _point: [f64; 2]) {}
        fn text_event(&mut self, _command: &scorepeek_overlay_wayland_handles::TextCommand) {}
        fn ime_event(&mut self, _update: scorepeek_overlay_wayland_handles::TextUpdate) {}
        fn keyboard_focus_event(&mut self, _focused: bool) {}
    }

    let event = Event::Configure {
        logical: [1280, 136],
        physical: [1920, 204],
        scale_120: 180,
    };
    let mut consumer = ConfigureConsumer::default();

    let initial = dispatch_native_event(&mut consumer, event.clone()).unwrap();
    let unchanged_remap = dispatch_native_event(&mut consumer, event).unwrap();

    assert!(initial.configured);
    assert!(unchanged_remap.configured);
    assert_eq!(consumer.calls, 2);
}

#[test]
fn display_visibility_update_releases_the_signal_read_before_writing() {
    let mut canvas = crate::config::empty_canvas(
        "screen-filtered".into(),
        crate::bridge::data::Backend::Wayland,
        cyan_skin(),
    );
    canvas.show_on = Some(vec![scorepeek_overlay::ScreenKind::MusicSelect]);
    let published = Rc::new(RefCell::new(None));
    let (coordinator, _commands) = std::sync::mpsc::channel();
    let props = NativeOverlayProps {
        initial: NativeDocumentProjection::Display {
            canvas: canvas.presentation(),
            visible: false,
        },
        published: Rc::clone(&published),
        port: NativeEditorPort {
            coordinator,
            source_output: Some("WL-1".into()),
            run_id: "visibility-test".into(),
            sequence: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        },
    };
    let mut document = DioxusDocument::new(
        VirtualDom::new_with_props(native_overlay, props),
        document_config(),
    );
    document.initial_build();
    let projection = published.borrow().as_ref().copied().unwrap();

    assert_eq!(
        update_display_visibility(
            projection,
            scorepeek_overlay::ScreenView {
                kind: Some(scorepeek_overlay::ScreenKind::MusicSelect),
                ..scorepeek_overlay::ScreenView::default()
            }
        ),
        Some(true)
    );
    assert!(matches!(
        &*projection.borrow(),
        NativeDocumentProjection::Display { visible: true, .. }
    ));
    assert_eq!(
        update_display_visibility(
            projection,
            scorepeek_overlay::ScreenView {
                kind: Some(scorepeek_overlay::ScreenKind::MusicSelect),
                ..scorepeek_overlay::ScreenView::default()
            }
        ),
        None
    );
}

#[test]
fn canvas_worker_panic_retains_its_payload_and_error_type() {
    let result = std::panic::catch_unwind(|| -> Result<(), String> {
        panic!("already borrowed: BorrowMutError")
    });

    assert_eq!(
        classify_native_worker_exit(result),
        NativeWorkerExit::Failed {
            error_type: "panic",
            error: "already borrowed: BorrowMutError".into(),
        }
    );
}

#[test]
fn shutdown_reap_propagates_a_late_canvas_worker_panic() {
    let worker = NativeWorker {
        output: Some("WL-1".into()),
        stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        join: std::thread::spawn(|| -> Result<(), String> { panic!("late shutdown panic") }),
    };
    let workers = std::collections::BTreeMap::from([("canvas-1".into(), worker)]);

    let error = reap_native_workers_on_shutdown(workers).unwrap_err();

    assert!(error.contains("canvas-1"));
    assert!(error.contains("panic"));
    assert!(error.contains("late shutdown panic"));
}

#[test]
fn stage_replica_rejects_stale_revisions_and_accepts_a_new_session() {
    let mut session = EditorSession::new(Vec::new(), [1920, 1080], "test");
    session.set_session_id(7);
    let output = EditorOutput {
        name: "DP-1".into(),
        model: "test".into(),
        logical_size: Some([1920, 1080]),
    };
    session.set_outputs(vec![output.clone()]);
    session.advance_revision();
    let current = session.stage_projection(&output);
    let mut stale = current.clone();
    stale.revision = current.revision.saturating_sub(1);
    assert!(!accepts_stage_projection(&current, &stale));
    let mut next = current.clone();
    next.revision += 1;
    assert!(accepts_stage_projection(&current, &next));
    let mut replacement = stale;
    replacement.session_id += 1;
    assert!(accepts_stage_projection(&current, &replacement));
}

#[test]
fn passive_pointer_motion_does_not_publish_a_native_stage_replica() {
    let mut session = EditorSession::new(Vec::new(), [1920, 1080], "test");
    session.set_outputs(vec![EditorOutput {
        name: "DP-1".into(),
        model: "test".into(),
        logical_size: Some([1920, 1080]),
    }]);
    session.editing = true;
    session.advance_revision();
    let published = Arc::new(std::sync::Mutex::new(PublishedStages::default()));
    let mut authority = NativeEditorAuthority::new(session, Arc::clone(&published));
    let before = published.lock().unwrap().publications;

    let effects = authority.dispatch(EditorInput::Surface(
        scorepeek_overlay::editor_surface::SurfaceAction::Move([320, 180]),
    ));

    assert!(effects.is_empty());
    assert_eq!(published.lock().unwrap().publications, before);
}

#[test]
fn native_run_ids_are_unique_across_parallel_surfaces() {
    let ids = (0..32)
        .map(|_| std::thread::spawn(|| RunReport::new().run_id))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 32);
}

#[test]
fn editor_skin_updates_coalesce_while_dragging_and_flush_on_frame_or_release() {
    let mut updates = EditorSkinUpdates::default();
    for _ in 0..100 {
        updates.request();
        assert!(!updates.take_if_ready(false, true));
    }
    assert!(updates.take_if_ready(true, true));
    assert!(!updates.take_if_ready(true, true));

    updates.request();
    assert!(updates.take_if_ready(false, false));
    assert_eq!(updates.requests, 101);
}

#[test]
fn native_animation_work_does_not_copy_skin_archives_per_frame() {
    let manifest: crate::skin::Manifest =
        toml::from_str(include_str!("../../../../../skins/cyan-system/skin.toml")).unwrap();
    let package = crate::skin::Package::test_with_entries(
        manifest,
        std::collections::BTreeMap::from([("artwork.bin".into(), vec![0; 8 * 1024 * 1024])]),
    );
    let package = Arc::new(package);
    let cache =
        SkinAssetCache::with_package(crate::skin::StoreRoot::discover(), Arc::clone(&package));

    // Four live canvases at 120 Hz model one second of production editor animation.
    for _ in 0..(4 * 120) {
        let resolved = cache.load(&package.manifest.id).unwrap();
        assert!(Arc::ptr_eq(&resolved, &package));
    }

    assert_eq!(cache.packages.lock().unwrap().len(), 1);
    assert_eq!(
        cache.open_count.load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[test]
fn editor_canvas_owner_prevents_reverse_delivery_from_mounting_two_runtimes() {
    let cache = SkinAssetCache::new(crate::skin::StoreRoot::discover());
    assert!(cache.acquire_editor_owner("canvas-1", "WL-1"));
    assert!(!cache.acquire_editor_owner("canvas-1", "WL-2"));
    cache.release_editor_owner("canvas-1", "WL-2");
    assert!(!cache.acquire_editor_owner("canvas-1", "WL-2"));
    cache.release_editor_owner("canvas-1", "WL-1");
    assert!(cache.acquire_editor_owner("canvas-1", "WL-2"));
    assert_eq!(
        cache.editor_owners.lock().unwrap().get("canvas-1"),
        Some(&"WL-2".to_owned())
    );
}

#[test]
fn native_skin_resources_are_namespaced_by_immutable_skin_identity() {
    let css = namespace_skin_css(
        "dev.atty303.skin",
        "a{src:url('font.ttf')}b{background:url(\"/shared.png\")}c{mask:url(data:image/png;base64,abc)}d{mask:url(https://example.test/shared.svg)}e{mask:url(https:shared.svg)}f{src:url(urn:scorepeek:asset)}",
    );
    assert!(
        css.contains("url('/skin/dev.atty303.skin/font.ttf')"),
        "{css}"
    );
    assert!(css.contains("url(\"/shared.png\")"), "{css}");
    assert!(css.contains("url(data:image/png;base64,abc)"), "{css}");
    assert!(
        css.contains("url(https://example.test/shared.svg)"),
        "{css}"
    );
    assert!(css.contains("url(https:shared.svg)"), "{css}");
    assert!(css.contains("url(urn:scorepeek:asset)"), "{css}");
}

#[test]
fn native_skin_inline_resources_use_the_package_namespace() {
    let mut output = crate::skin::RenderOutput {
        schedule: crate::skin::Schedule::Idle,
        tree: crate::skin::Node::Element {
            key: "background".into(),
            tag: "div".into(),
            attributes: std::collections::BTreeMap::from([
                (
                    "style".into(),
                    "background-image:url('background.png');mask:url(\"mask.svg\")".into(),
                ),
                ("src".into(), "preview.png".into()),
                ("poster".into(), "file:preview.webm".into()),
            ]),
            children: Vec::new(),
        },
    };

    namespace_native_skin_output("dev.atty303.skin", &mut output);

    let crate::skin::Node::Element { attributes, .. } = output.tree else {
        panic!("probe must remain an element");
    };
    assert_eq!(
        attributes.get("style").map(String::as_str),
        Some(
            "background-image:url('/skin/dev.atty303.skin/background.png');mask:url(\"/skin/dev.atty303.skin/mask.svg\")"
        )
    );
    assert_eq!(
        attributes.get("src").map(String::as_str),
        Some("/skin/dev.atty303.skin/preview.png")
    );
    assert_eq!(
        attributes.get("poster").map(String::as_str),
        Some("file:preview.webm")
    );
}

#[test]
fn native_skin_input_uses_the_manifest_property_authority() {
    let mut canvas = crate::config::empty_canvas(
        "background-probe".into(),
        crate::bridge::data::Backend::Wayland,
        cyan_skin(),
    );
    canvas
        .skin_properties
        .insert("background".into(), serde_json::json!("static"));
    let manifest: crate::skin::Manifest =
        toml::from_str(include_str!("../../../../../skins/cyan-system/skin.toml")).unwrap();

    let input = native_skin_input(&canvas, &OverlayState::default(), &manifest);

    assert_eq!(input["canvas"]["properties"]["background"], "static");
}

#[test]
fn unchanged_native_frames_do_not_rebuild_surface_projection() {
    let config = crate::config::visual_debug_config(cyan_skin());
    let draft = config
        .canvases
        .iter()
        .filter(|canvas| canvas.backend == crate::bridge::data::Backend::Wayland)
        .map(crate::config::Canvas::presentation)
        .collect::<Vec<_>>();
    let mut session = EditorSession::new(draft, [1920, 1080], "fake-wayland");
    session.set_session_id(7);
    session.set_outputs(vec![EditorOutput {
        name: "WL-1".into(),
        model: "fake output".into(),
        logical_size: Some([1920, 1080]),
    }]);
    session.editing = true;
    session.advance_revision();
    let mut cache = NativeProjectionCache::default();

    for _ in 0..120 {
        assert_eq!(
            cache
                .resolve(Some(cyan_skin()), &session, &[])
                .unwrap()
                .len(),
            1
        );
    }

    assert_eq!(cache.rebuilds, 1);
    session.advance_revision();
    let _ = cache.resolve(Some(cyan_skin()), &session, &[]).unwrap();
    assert_eq!(cache.rebuilds, 2);
}

#[test]
#[allow(clippy::too_many_lines)]
fn fake_wayland_axis_scrolls_ancestor_beneath_nested_editor_rows() {
    struct FakeProtocolTarget<'a>(&'a mut VisualDebugSession);
    impl NativeEventConsumer for FakeProtocolTarget<'_> {
        fn configure_event(
            &mut self,
            _logical: [u32; 2],
            physical: [u32; 2],
            scale_120: u32,
        ) -> Result<(), String> {
            self.0
                .document
                .inner
                .borrow_mut()
                .set_viewport(Viewport::new(
                    physical[0],
                    physical[1],
                    f32::from(u16::try_from(scale_120).map_err(|error| error.to_string())?) / 120.0,
                    ColorScheme::Dark,
                ));
            Ok(())
        }
        fn pointer_motion_event(&mut self, point: [f64; 2]) {
            self.0
                .pointer
                .dispatch(&mut self.0.document, point, 0x110, None);
        }
        fn pointer_button_event(&mut self, button: u32, pressed: bool, point: [f64; 2]) {
            self.0
                .pointer
                .dispatch(&mut self.0.document, point, button, Some(pressed));
        }
        fn pointer_scroll_event(&mut self, delta: [f64; 2], point: [f64; 2]) {
            self.0.pointer.wheel(&mut self.0.document, point, delta);
        }
        fn text_event(&mut self, command: &scorepeek_overlay_wayland_handles::TextCommand) {
            text::dispatch_control_key(&mut self.0.document, command, false);
        }
        fn ime_event(&mut self, update: scorepeek_overlay_wayland_handles::TextUpdate) {
            let _ = text::dispatch_control_composition(&mut self.0.document, update);
        }
        fn keyboard_focus_event(&mut self, _focused: bool) {}
    }
    let axis = |session: &mut VisualDebugSession, point: [f64; 2], delta: [f64; 2]| {
        let outcome = dispatch_native_event(
            &mut FakeProtocolTarget(session),
            Event::PointerScroll {
                dx: delta[0],
                dy: delta[1],
                x: point[0],
                y: point[1],
            },
        )
        .unwrap();
        assert!(outcome.input_damage);
    };
    let canvases = crate::config::visual_debug_config(cyan_skin())
        .canvases
        .into_iter()
        .filter(|canvas| canvas.backend == crate::bridge::data::Backend::Wayland)
        .map(|mut canvas| {
            canvas.output = "WL-1".into();
            canvas.presentation()
        })
        .collect();
    let scenario = VisualDebugScenario {
        canvases: Some(canvases),
        skin: None,
        logical_size: [1280, 1080],
        scale: 1.0,
        canvas_id: Some("wayland-status".into()),
        editing: true,
        selectors: Vec::new(),
        actions: Vec::new(),
    };
    let offset = |session: &VisualDebugSession| {
        let inner = session.document.inner.borrow();
        let node = inner.query_selector(".navigator-scroll").unwrap().unwrap();
        inner.get_node(node).unwrap().scroll_offset().y
    };

    let point_in_navigator = |session: &VisualDebugSession, selector: &str| {
        let inner = session.document.inner.borrow();
        let target = inner
            .get_client_bounding_rect(inner.query_selector(selector).unwrap().unwrap())
            .unwrap();
        let viewport = inner
            .get_client_bounding_rect(inner.query_selector(".navigator-scroll").unwrap().unwrap())
            .unwrap();
        [
            target.x + target.width / 2.0,
            target
                .y
                .max(viewport.y + 2.0)
                .min(viewport.y + viewport.height - 2.0),
        ]
    };
    for (selector, message, delta) in [
        (
            ".workspace-output-option .navigator-item-select",
            "output row",
            -80.0,
        ),
        (
            ".workspace-output-option .tree-disclosure",
            "disclosure child",
            -80.0,
        ),
    ] {
        let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
        let prior = offset(&session);
        let point = point_in_navigator(&session, selector);
        axis(&mut session, point, [0.0, delta]);
        session.resolve();
        assert!(
            (offset(&session) - prior).abs() > f64::EPSILON,
            "axis over an {message} must reach navigator scroll"
        );
    }
    let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
    let before = offset(&session);
    let canvas_point = {
        let inner = session.document.inner.borrow();
        let node = inner.query_selector(".canvas-select").unwrap().unwrap();
        let target = inner.get_client_bounding_rect(node).unwrap();
        let viewport = inner
            .get_client_bounding_rect(inner.query_selector(".navigator-scroll").unwrap().unwrap())
            .unwrap();
        [
            target.x + target.width / 2.0,
            target.y.max(viewport.y) + 4.0,
        ]
    };
    session
        .pointer
        .dispatch(&mut session.document, [1000.0, 500.0], 0x110, None);
    let raw_point =
        dioxus::html::geometry::ClientPoint::new(canvas_point[0], canvas_point[1]).to_f32();
    session
        .document
        .handle_ui_event(blitz_traits::events::UiEvent::Wheel(
            blitz_traits::events::BlitzWheelEvent {
                delta: blitz_traits::events::BlitzWheelDelta::Pixels(0.0, -120.0),
                coords: blitz_traits::events::PointerCoords {
                    page_x: raw_point.x,
                    page_y: raw_point.y,
                    screen_x: raw_point.x,
                    screen_y: raw_point.y,
                    client_x: raw_point.x,
                    client_y: raw_point.y,
                },
                buttons: session.pointer.buttons(),
                mods: dioxus::html::Modifiers::default(),
                element: blitz_traits::events::Point::default(),
            },
        ));
    session.resolve();
    assert!(
        (offset(&session) - before).abs() <= f64::EPSILON,
        "unadapted Blitz routes wheel through stale hover instead of event coordinates"
    );
    axis(&mut session, canvas_point, [0.0, -120.0]);
    session.resolve();
    let after_canvas = offset(&session);
    assert!(
        (after_canvas - before).abs() > f64::EPSILON,
        "axis over a canvas row must reach navigator scroll"
    );

    let widget_point = {
        let inner = session.document.inner.borrow();
        let viewport = inner
            .get_client_bounding_rect(inner.query_selector(".navigator-scroll").unwrap().unwrap())
            .unwrap();
        let target = inner
            .get_client_bounding_rect(inner.query_selector(".widget-row").unwrap().unwrap())
            .unwrap();
        [
            target.x + target.width / 2.0,
            target
                .y
                .max(viewport.y + 2.0)
                .min(viewport.y + viewport.height - 2.0),
        ]
    };
    axis(&mut session, widget_point, [0.0, 240.0]);
    session.resolve();
    let after_widget = offset(&session);
    assert!(
        (after_widget - after_canvas).abs() > f64::EPSILON,
        "axis over a widget row must reach navigator scroll"
    );

    let inspector_offset = |session: &VisualDebugSession| {
        let inner = session.document.inner.borrow();
        let node = inner.query_selector(".inspector-scroll").unwrap().unwrap();
        inner.get_node(node).unwrap().scroll_offset().y
    };
    let inspector_point = {
        let inner = session.document.inner.borrow();
        let target = inner
            .get_client_bounding_rect(
                inner
                    .query_selector(".editor-number-field")
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
        [
            target.x + target.width / 2.0,
            target.y + target.height / 2.0,
        ]
    };
    let inspector_before = inspector_offset(&session);
    axis(&mut session, inspector_point, [0.0, -240.0]);
    session.resolve();
    assert!(
        (inspector_offset(&session) - inspector_before).abs() > f64::EPSILON,
        "axis over an Inspector field must reach its scroll ancestor"
    );
}

#[test]
fn blitz_adapter_preserves_browser_interaction_identity_across_empty_vdom_diff() {
    let scenario = VisualDebugScenario {
        canvases: None,
        skin: None,
        logical_size: [1280, 720],
        scale: 1.0,
        canvas_id: Some("wayland-status".into()),
        editing: true,
        selectors: Vec::new(),
        actions: Vec::new(),
    };
    let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
    session.click(".editor-number-field").unwrap();
    let focus_before = session
        .document
        .inner
        .borrow()
        .get_focussed_node_id()
        .expect("number field must be focused");
    let point = {
        let inner = session.document.inner.borrow();
        let target = inner
            .get_client_bounding_rect(inner.query_selector(".widget-row").unwrap().unwrap())
            .unwrap();
        [
            target.x + target.width / 2.0,
            target.y + target.height / 2.0,
        ]
    };

    session
        .pointer
        .dispatch(&mut session.document, point, 0x110, None);
    let hover_before = session
        .document
        .inner
        .borrow()
        .get_hover_node_id()
        .expect("pointer move must establish hover");
    session.resolve();
    let inner = session.document.inner.borrow();

    assert_eq!(
        inner.get_hover_node_id(),
        Some(hover_before),
        "browser keeps hover when a Dioxus render produces no semantic DOM replacement"
    );
    assert_eq!(
        inner.get_focussed_node_id(),
        Some(focus_before),
        "browser keeps keyboard focus when pointer motion does not replace the focused DOM node"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn projection_model_describes_wayland_lifecycle_without_resource_churn() {
    #[derive(Default)]
    struct FakeWayland {
        projection_cache: NativeProjectionCache,
        surfaces: std::collections::BTreeMap<String, String>,
        trees: std::collections::BTreeSet<(String, String)>,
        runtimes: std::collections::BTreeMap<(String, String), scorepeek_overlay::Skin>,
        revisions: std::collections::BTreeMap<String, (u64, u64)>,
        creates: u64,
        unmaps: u64,
        runtime_creates: u64,
        runtime_drops: u64,
    }
    impl FakeWayland {
        #[allow(clippy::too_many_lines)]
        fn apply(&mut self, session: &EditorSession) {
            let display = session
                .draft
                .iter()
                .map(|presentation| {
                    let mut canvas = crate::config::empty_canvas(
                        presentation.id.clone(),
                        crate::bridge::data::Backend::Wayland,
                        presentation.skin,
                    );
                    canvas.apply_presentation(presentation);
                    canvas
                })
                .collect::<Vec<_>>();
            let projected = self
                .projection_cache
                .resolve(Some(cyan_skin()), session, &display)
                .unwrap();
            let lifecycle = reconcile_worker_lifecycle(
                self.surfaces
                    .iter()
                    .map(|(id, output)| (id.as_str(), Some(output.as_str()), false)),
                projected,
            );
            for id in lifecycle.stop_join {
                let output = self.surfaces.remove(&id).unwrap();
                self.unmaps += 1;
                self.revisions.remove(&output);
                let dropped = self
                    .runtimes
                    .keys()
                    .filter(|(owner, _)| owner == &output)
                    .cloned()
                    .collect::<Vec<_>>();
                self.runtime_drops += u64::try_from(dropped.len()).unwrap();
                for key in dropped {
                    self.runtimes.remove(&key);
                    self.trees.remove(&key);
                }
            }
            for id in lifecycle.start {
                let canvas = projected
                    .iter()
                    .find(|canvas| canvas.id == id)
                    .expect("fake adapter receives production lifecycle IDs only");
                self.surfaces.insert(id, canvas.output.clone());
                self.creates += 1;
            }
            let outputs = if session.editing {
                session.outputs.clone()
            } else {
                session
                    .draft
                    .iter()
                    .filter_map(|canvas| canvas.output.as_ref())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .map(|name| EditorOutput {
                        name: name.clone(),
                        model: "fake".into(),
                        logical_size: None,
                    })
                    .collect()
            };
            for output in outputs {
                let revision = (session.session_id, session.revision);
                if self
                    .revisions
                    .get(&output.name)
                    .is_some_and(|old| *old >= revision)
                {
                    continue;
                }
                self.revisions.insert(output.name.clone(), revision);
                let live = session
                    .stage_projection(&output)
                    .canvases
                    .into_iter()
                    .map(|canvas| ((output.name.clone(), canvas.id), canvas.skin))
                    .collect::<std::collections::BTreeMap<_, _>>();
                let removed = self
                    .runtimes
                    .keys()
                    .filter(|key| key.0 == output.name && !live.contains_key(*key))
                    .cloned()
                    .collect::<Vec<_>>();
                for key in removed {
                    self.runtimes.remove(&key);
                    self.trees.remove(&key);
                    self.runtime_drops += 1;
                }
                for (key, skin) in live {
                    match self.runtimes.insert(key.clone(), skin) {
                        None => self.runtime_creates += 1,
                        Some(previous) if previous != skin => {
                            self.runtime_drops += 1;
                            self.runtime_creates += 1;
                        }
                        Some(_) => {}
                    }
                    self.trees.insert(key);
                }
            }
        }
    }

    let mut canvases = crate::config::visual_debug_config(cyan_skin())
        .canvases
        .into_iter()
        .filter(|canvas| canvas.backend == crate::bridge::data::Backend::Wayland)
        .take(2)
        .map(|canvas| canvas.presentation())
        .collect::<Vec<_>>();
    canvases[0].output = Some("WL-1".into());
    canvases[1].output = Some("WL-2".into());
    let mut session = EditorSession::new(canvases, [1920, 1080], "fake-wayland");
    session.set_session_id(11);
    session.set_outputs(vec![
        EditorOutput {
            name: "WL-1".into(),
            model: "fake one".into(),
            logical_size: Some([1920, 1080]),
        },
        EditorOutput {
            name: "WL-2".into(),
            model: "fake two".into(),
            logical_size: Some([1280, 720]),
        },
    ]);
    session.editing = true;
    session.readonly = false;
    session.selected_canvas = Some(session.draft[0].id.clone());
    session.advance_revision();
    let mut fake = FakeWayland::default();
    fake.apply(&session);
    assert_eq!(
        (fake.surfaces.len(), fake.runtimes.len(), fake.trees.len()),
        (2, 2, 2)
    );
    let steady = (
        fake.projection_cache.rebuilds,
        fake.creates,
        fake.unmaps,
        fake.runtime_creates,
        fake.runtime_drops,
    );
    for _ in 0..120 {
        fake.apply(&session);
    }
    assert_eq!(
        (
            fake.projection_cache.rebuilds,
            fake.creates,
            fake.unmaps,
            fake.runtime_creates,
            fake.runtime_drops,
        ),
        steady,
        "120 steady frames must not rebuild projections, surfaces, or heavyweight runtimes"
    );

    session.draft[1].show_on = Some(vec![scorepeek_overlay::ScreenKind::Result]);
    session.advance_revision();
    fake.apply(&session);
    assert_eq!(fake.runtimes.len(), 1, "hidden canvases must be unmounted");
    session.reduce(EditorInput::Action(EditorAction::PreviewScreen(
        scorepeek_overlay::ScreenKind::Result,
    )));
    fake.apply(&session);
    assert_eq!(fake.runtimes.len(), 2, "visible canvases must be remounted");

    session.reduce(EditorInput::Action(EditorAction::Output("WL-2".into())));
    fake.apply(&session);
    assert_eq!(
        fake.surfaces.len(),
        2,
        "moving a canvas must not duplicate editor stages"
    );
    assert!(fake.runtimes.keys().all(|(output, _)| output == "WL-2"));

    let creates = fake.runtime_creates;
    let drops = fake.runtime_drops;
    session.draft[0].skin = result_skin();
    session.advance_revision();
    fake.apply(&session);
    assert_eq!(fake.runtime_creates, creates + 1);
    assert_eq!(fake.runtime_drops, drops + 1);
    assert_eq!(
        fake.runtimes.len(),
        2,
        "a skin switch replaces only its owner"
    );

    session.reduce(EditorInput::Action(EditorAction::DeleteCanvas));
    fake.apply(&session);
    assert_eq!(fake.runtimes.len(), 1);
    assert_eq!(fake.trees, fake.runtimes.keys().cloned().collect(),);

    session.reduce(EditorInput::SetOutputs(vec![EditorOutput {
        name: "WL-2".into(),
        model: "fake two".into(),
        logical_size: Some([1280, 720]),
    }]));
    fake.apply(&session);
    assert_eq!(
        fake.surfaces.values().collect::<Vec<_>>(),
        vec![&"WL-2".to_owned()]
    );
    assert!(fake.unmaps >= 1);

    session.editing = false;
    session.advance_revision();
    fake.apply(&session);
    assert!(
        fake.surfaces
            .keys()
            .all(|id| !id.starts_with("__scorepeek-editor-stage"))
    );
    assert_eq!(fake.trees, fake.runtimes.keys().cloned().collect());
    assert_eq!(
        fake.runtime_creates - fake.runtime_drops,
        fake.runtimes.len() as u64
    );

    let display_creates = fake.creates;
    session.editing = true;
    session.advance_revision();
    fake.apply(&session);
    assert_eq!(
        fake.surfaces.len(),
        session.outputs.len(),
        "reopening creates exactly one editor stage for each current output"
    );
    assert!(fake.creates > display_creates);
    assert_eq!(fake.trees, fake.runtimes.keys().cloned().collect());
}

#[test]
#[allow(clippy::items_after_statements, clippy::too_many_lines)]
fn fake_wayland_adapter_drives_production_stage_and_skin_lifecycle() {
    struct TestSkinStore(std::path::PathBuf);
    impl Drop for TestSkinStore {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    static NEXT_STORE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    #[allow(clippy::struct_excessive_bools)]
    struct FakeStage {
        output: String,
        projection: Reactive<NativeDocumentProjection>,
        document: DioxusDocument,
        coordinator: std::sync::mpsc::Sender<CoordinatorCommand>,
        commands: std::sync::mpsc::Receiver<CoordinatorCommand>,
        previews: std::collections::BTreeMap<String, EditorSkinPreview>,
        assets: Arc<SkinAssetCache>,
        report: Rc<RefCell<RunReport>>,
        skin_updates: EditorSkinUpdates,
        runtime_creates: u64,
        projection_accepts: u64,
        dioxus_polls: u64,
        reconciliations: u64,
        input_generations: u64,
        wasm_calls: u64,
        tree_updates: u64,
        work: FrameWorkProfile,
        layouts: u64,
        resource_resolves: u64,
        scenes: u64,
        presents: u64,
        commits: u64,
        motion_seconds: f64,
        elapsed: Duration,
        full_layout_pending: bool,
        paint_count: u64,
        physical_size: [u32; 2],
        scale: f32,
        pointer: PointerInput,
        text_composing: bool,
        renderer: anyrender_vello::VelloImageRenderer,
        pending_frame_start: Option<FrameWorkSample>,
        next_skin_render: Option<Instant>,
    }
    impl FakeStage {
        fn new(stage: StageProjection, assets: Arc<SkinAssetCache>) -> Result<Self, String> {
            let output = stage.output.name.clone();
            let initial = NativeDocumentProjection::Editor(stage);
            let published = Rc::new(RefCell::new(None));
            let (sender, commands) = std::sync::mpsc::channel();
            let props = NativeOverlayProps {
                initial,
                published: Rc::clone(&published),
                port: NativeEditorPort {
                    coordinator: sender.clone(),
                    source_output: Some(output.clone()),
                    run_id: "fake-wayland-stage".into(),
                    sequence: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                },
            };
            let mut document = DioxusDocument::new(
                VirtualDom::new_with_props(native_overlay, props),
                document_config_inner_with_handle(Arc::clone(&assets), Vec::new()).0,
            );
            document.initial_build();
            let projection = published
                .borrow()
                .as_ref()
                .copied()
                .ok_or("fake stage did not publish its projection")?;
            let mut skin_updates = EditorSkinUpdates::default();
            skin_updates.request();
            let stage = Self {
                output,
                projection,
                document,
                coordinator: sender,
                commands,
                previews: std::collections::BTreeMap::new(),
                assets,
                report: Rc::new(RefCell::new(RunReport::new())),
                skin_updates,
                runtime_creates: 0,
                projection_accepts: 0,
                dioxus_polls: 0,
                reconciliations: 0,
                input_generations: 0,
                wasm_calls: 0,
                tree_updates: 0,
                work: FrameWorkProfile::default(),
                layouts: 0,
                resource_resolves: 0,
                scenes: 0,
                presents: 0,
                commits: 0,
                motion_seconds: 0.0,
                elapsed: Duration::ZERO,
                full_layout_pending: true,
                paint_count: 0,
                physical_size: [1, 1],
                scale: 1.0,
                pointer: PointerInput::default(),
                text_composing: false,
                renderer: anyrender_vello::VelloImageRenderer::new(1920, 1080),
                pending_frame_start: None,
                next_skin_render: None,
            };
            Ok(stage)
        }

        fn accept(&mut self, next: &StageProjection) {
            let frame_start = self.work.snapshot();
            let rebuild_started = Instant::now();
            let previews_changed = editor_skin_previews_changed(&self.previews, next);
            let accepted = accept_stage_projection_replica(
                self.projection,
                &mut self.document,
                &mut self.previews,
                &self.assets,
                &self.output,
                next,
            );
            if accepted {
                if self.pending_frame_start.is_none() {
                    self.pending_frame_start = Some(frame_start);
                }
                self.work
                    .record("projection_rebuild", rebuild_started.elapsed());
                if previews_changed {
                    self.skin_updates.request();
                }
                self.projection_accepts += 1;
            }
        }

        fn frame(&mut self) -> Result<(), String> {
            self.turn(
                NativeEventOutcome {
                    frame: true,
                    ..NativeEventOutcome::default()
                },
                120,
            )
        }

        fn turn(&mut self, outcome: NativeEventOutcome, hz: u32) -> Result<(), String> {
            let seconds = 1.0 / f64::from(hz);
            self.elapsed += Duration::from_secs_f64(seconds);
            if outcome.frame {
                self.dioxus_polls += 1;
                self.motion_seconds += seconds;
            }
            struct FakePresenter<'a> {
                scenes: &'a mut u64,
                presents: &'a mut u64,
                commits: &'a mut u64,
                text_input_active: &'a mut bool,
            }
            impl NativeFramePresenter for FakePresenter<'_> {
                fn is_active(&self) -> bool {
                    true
                }

                fn set_text_input(
                    &mut self,
                    input: Option<scorepeek_overlay_wayland_handles::TextInputState>,
                ) {
                    *self.text_input_active = input.is_some();
                }

                fn unmap(&mut self) -> Result<(), String> {
                    Ok(())
                }

                fn present(
                    &mut self,
                    document: &mut blitz_dom::BaseDocument,
                    scale: f64,
                    width: u32,
                    height: u32,
                    work: &mut FrameWorkProfile,
                ) -> Result<(), String> {
                    let mut scene = anyrender::Scene::new();
                    work.measure("scene", || {
                        paint_native_scene(&mut scene, document, scale, width, height);
                    });
                    work.measure("gpu_present", || {
                        *self.presents = self.presents.saturating_add(1);
                    });
                    work.measure("surface_commit", || {
                        *self.commits = self.commits.saturating_add(1);
                    });
                    *self.scenes = self.scenes.saturating_add(1);
                    Ok(())
                }
            }
            let frame_start = if outcome.frame {
                self.pending_frame_start
                    .take()
                    .unwrap_or_else(|| self.work.snapshot())
            } else {
                self.work.snapshot()
            };
            let mut text_input_active = false;
            let mut presenter = FakePresenter {
                scenes: &mut self.scenes,
                presents: &mut self.presents,
                commits: &mut self.commits,
                text_input_active: &mut text_input_active,
            };
            let result = run_native_editor_stage_turn(
                &mut self.document,
                self.projection,
                &mut self.previews,
                &self.assets,
                &self.report,
                &self.output,
                &scorepeek_overlay::editor_sample_state(),
                &mut self.skin_updates,
                &mut self.runtime_creates,
                &mut self.next_skin_render,
                &mut self.full_layout_pending,
                &mut self.work,
                Waker::noop(),
                &frame_start,
                NativeEditorStageTurnInput {
                    frame: if outcome.frame {
                        NativeFrameBoundary::Frame
                    } else {
                        NativeFrameBoundary::Deferred
                    },
                    surface: if outcome.configured {
                        NativeSurfaceReadiness::Configured
                    } else {
                        NativeSurfaceReadiness::Pending
                    },
                    seconds: self.motion_seconds,
                },
                &mut presenter,
            )?;
            if result.reconciled {
                self.reconciliations = self.reconciliations.saturating_add(1);
            }
            self.input_generations = self
                .input_generations
                .saturating_add(result.reconciliation.input_generations);
            self.wasm_calls = self
                .wasm_calls
                .saturating_add(result.reconciliation.wasm_calls);
            self.tree_updates = self
                .tree_updates
                .saturating_add(result.reconciliation.tree_updates);
            if result.painted {
                self.paint_count = self.paint_count.saturating_add(1);
                self.layouts = self.layouts.saturating_add(1);
                self.resource_resolves = self.resource_resolves.saturating_add(1);
            }
            Ok(())
        }

        fn drain_commands(&mut self) -> Vec<CoordinatorCommand> {
            std::iter::from_fn(|| self.commands.try_recv().ok()).collect()
        }

        fn render_pixels(&mut self) -> Vec<u8> {
            let mut pixels = Vec::new();
            let mut inner = self.document.inner.borrow_mut();
            resolve_with_loaded_resources(&mut inner, self.motion_seconds);
            self.renderer.render_to_vec(
                |scene| paint_native_scene(scene, &mut inner, 1.0, 1920, 1080),
                &mut pixels,
            );
            pixels
        }

        fn shutdown(mut self, operations: &mut Vec<String>) {
            let phases = RefCell::new(Vec::new());
            shutdown_native_surface(
                &mut self,
                |stage| {
                    for (id, mut preview) in std::mem::take(&mut stage.previews) {
                        preview.tree.unmount(&mut stage.document.inner.borrow_mut());
                        stage.assets.release_editor_owner(&id, &stage.output);
                        phases
                            .borrow_mut()
                            .push(format!("runtime-drop:{id}:{}", stage.output));
                    }
                },
                |stage| {
                    phases
                        .borrow_mut()
                        .push(format!("suspend:{}", stage.output));
                },
                |stage| {
                    phases.borrow_mut().push(format!("unmap:{}", stage.output));
                    Ok::<(), ()>(())
                },
            )
            .unwrap();
            operations.extend(phases.into_inner());
            operations.push(format!("join:{}", self.output));
        }
    }

    impl NativeEventConsumer for FakeStage {
        fn configure_event(
            &mut self,
            logical: [u32; 2],
            physical: [u32; 2],
            scale_120: u32,
        ) -> Result<(), String> {
            let scale =
                f32::from(u16::try_from(scale_120).map_err(|error| error.to_string())?) / 120.0;
            self.document.inner.borrow_mut().set_viewport(Viewport::new(
                physical[0],
                physical[1],
                scale,
                ColorScheme::Dark,
            ));
            self.physical_size = physical;
            self.scale = scale;
            let _ = self.coordinator.send(CoordinatorCommand::EditorInput {
                input: EditorInput::Resize {
                    output: self.output.clone(),
                    logical_size: logical,
                },
                correlation: None,
            });
            Ok(())
        }

        fn pointer_motion_event(&mut self, point: [f64; 2]) {
            self.pointer
                .dispatch(&mut self.document, point, 0x110, None);
        }

        fn pointer_button_event(&mut self, button: u32, pressed: bool, point: [f64; 2]) {
            self.pointer
                .dispatch(&mut self.document, point, button, Some(pressed));
        }

        fn pointer_scroll_event(&mut self, delta: [f64; 2], point: [f64; 2]) {
            self.pointer.wheel(&mut self.document, point, delta);
        }

        fn text_event(&mut self, command: &scorepeek_overlay_wayland_handles::TextCommand) {
            text::dispatch_control_key(&mut self.document, command, self.text_composing);
        }

        fn ime_event(&mut self, update: scorepeek_overlay_wayland_handles::TextUpdate) {
            if let Some(composing) = text::dispatch_control_composition(&mut self.document, update)
            {
                self.text_composing = composing;
            }
        }

        fn keyboard_focus_event(&mut self, focused: bool) {
            if !focused {
                self.text_composing = false;
            }
        }
    }

    #[allow(clippy::struct_excessive_bools)]
    struct FakeDisplay {
        canvas: crate::config::Canvas,
        live_widgets: u64,
        document: DioxusDocument,
        commands: std::sync::mpsc::Receiver<CoordinatorCommand>,
        pointer: PointerInput,
        text_composing: bool,
        skin: NativeDisplaySkin,
        assets: Arc<SkinAssetCache>,
        work: FrameWorkProfile,
        full_layout_pending: bool,
        surface_state: NativeDisplaySurfaceState,
        elapsed: Duration,
        motion_seconds: f64,
        renderer: anyrender_vello::VelloImageRenderer,
        presents: u64,
        commits: u64,
        unmaps: u64,
        paint_count: u64,
    }

    impl FakeDisplay {
        fn new(
            canvas: &crate::config::Canvas,
            assets: Arc<SkinAssetCache>,
        ) -> Result<Self, String> {
            let published = Rc::new(RefCell::new(None));
            let (sender, commands) = std::sync::mpsc::channel();
            let props = NativeOverlayProps {
                initial: NativeDocumentProjection::Display {
                    canvas: canvas.presentation(),
                    visible: true,
                },
                published,
                port: NativeEditorPort {
                    coordinator: sender,
                    source_output: Some(canvas.output.clone()),
                    run_id: "fake-wayland-display".into(),
                    sequence: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                },
            };
            let mut document = DioxusDocument::new(
                VirtualDom::new_with_props(native_overlay, props),
                document_config_inner_with_handle(Arc::clone(&assets), Vec::new()).0,
            );
            document.initial_build();
            while poll_native_document(&mut document, Waker::noop()) {}
            let package = assets.load(canvas.skin.name())?;
            let report = Rc::new(RefCell::new(RunReport::new()));
            let (skin, _) = create_native_display_skin(
                &mut document,
                canvas,
                &package,
                &report,
                Some(&canvas.output),
                &scorepeek_overlay::editor_sample_state(),
            )?;
            Ok(Self {
                canvas: canvas.clone(),
                live_widgets: u64::try_from(canvas.widgets.len()).unwrap_or(u64::MAX),
                document,
                commands,
                pointer: PointerInput::default(),
                text_composing: false,
                skin,
                assets,
                work: FrameWorkProfile::default(),
                full_layout_pending: true,
                surface_state: NativeDisplaySurfaceState::AwaitingConfigure,
                elapsed: Duration::ZERO,
                motion_seconds: 0.0,
                renderer: anyrender_vello::VelloImageRenderer::new(canvas.width, canvas.height),
                presents: 0,
                commits: 0,
                unmaps: 0,
                paint_count: 0,
            })
        }

        fn turn(&mut self, outcome: NativeEventOutcome, hz: u32) -> Result<(), String> {
            self.turn_visibility(outcome, hz, true)
        }

        fn turn_visibility(
            &mut self,
            outcome: NativeEventOutcome,
            hz: u32,
            visible: bool,
        ) -> Result<(), String> {
            let seconds = 1.0 / f64::from(hz);
            self.elapsed += Duration::from_secs_f64(seconds);
            if outcome.frame {
                self.motion_seconds += seconds;
            }
            if visible && self.surface_state == NativeDisplaySurfaceState::Unmapped {
                self.surface_state = NativeDisplaySurfaceState::AwaitingConfigure;
            }
            struct FakeDisplayPresenter<'a> {
                renderer: &'a mut anyrender_vello::VelloImageRenderer,
                presents: &'a mut u64,
                commits: &'a mut u64,
                unmaps: &'a mut u64,
            }
            impl NativeFramePresenter for FakeDisplayPresenter<'_> {
                fn is_active(&self) -> bool {
                    true
                }

                fn set_text_input(
                    &mut self,
                    input: Option<scorepeek_overlay_wayland_handles::TextInputState>,
                ) {
                    assert!(
                        input.is_none(),
                        "display surfaces never activate text input"
                    );
                }

                fn unmap(&mut self) -> Result<(), String> {
                    *self.unmaps = self.unmaps.saturating_add(1);
                    Ok(())
                }

                fn present(
                    &mut self,
                    document: &mut blitz_dom::BaseDocument,
                    scale: f64,
                    width: u32,
                    height: u32,
                    work: &mut FrameWorkProfile,
                ) -> Result<(), String> {
                    let mut pixels = Vec::new();
                    let present_started = Instant::now();
                    let mut scene_elapsed = Duration::ZERO;
                    self.renderer.render_to_vec(
                        |scene| {
                            let scene_started = Instant::now();
                            paint_native_scene(scene, document, scale, width, height);
                            scene_elapsed += scene_started.elapsed();
                        },
                        &mut pixels,
                    );
                    work.record("scene", scene_elapsed);
                    work.record(
                        "gpu_present",
                        present_started.elapsed().saturating_sub(scene_elapsed),
                    );
                    if pixels.is_empty() {
                        return Err("fake display paint produced no pixels".into());
                    }
                    *self.presents = self.presents.saturating_add(1);
                    work.measure("surface_commit", || {
                        *self.commits = self.commits.saturating_add(1);
                    });
                    Ok(())
                }
            }
            let frame_start = self.work.snapshot();
            let mut presenter = FakeDisplayPresenter {
                renderer: &mut self.renderer,
                presents: &mut self.presents,
                commits: &mut self.commits,
                unmaps: &mut self.unmaps,
            };
            let result = run_native_display_turn(
                &mut self.document,
                &self.assets,
                &mut self.full_layout_pending,
                &mut self.surface_state,
                &mut self.work,
                Waker::noop(),
                &frame_start,
                NativeDisplayTurnInput {
                    frame: if outcome.frame {
                        NativeFrameBoundary::Frame
                    } else {
                        NativeFrameBoundary::Deferred
                    },
                    surface: if outcome.configured {
                        NativeSurfaceReadiness::Configured
                    } else {
                        NativeSurfaceReadiness::Pending
                    },
                    visible,
                    live_widgets: self.live_widgets,
                    seconds: self.motion_seconds,
                },
                &mut presenter,
            )?;
            if result.painted {
                self.paint_count = self.paint_count.saturating_add(1);
            }
            Ok(())
        }

        fn drain_commands(&mut self) -> Vec<CoordinatorCommand> {
            std::iter::from_fn(|| self.commands.try_recv().ok()).collect()
        }

        fn render_skin(&mut self, state: &OverlayState) -> Result<(), String> {
            let mut next_skin_render = None;
            render_native_display_skin(
                &mut self.document,
                &self.canvas,
                &mut self.skin,
                state,
                &mut next_skin_render,
            )
        }

        fn shutdown(mut self, operations: &mut Vec<String>) {
            self.skin
                .tree
                .unmount(&mut self.document.inner.borrow_mut());
            operations.push(format!("runtime-drop:{}", self.canvas.id));
            operations.push(format!("unmap:{}", self.canvas.id));
            operations.push(format!("join:{}", self.canvas.id));
        }
    }

    impl NativeEventConsumer for FakeDisplay {
        fn configure_event(
            &mut self,
            _logical: [u32; 2],
            physical: [u32; 2],
            scale_120: u32,
        ) -> Result<(), String> {
            let scale =
                f32::from(u16::try_from(scale_120).map_err(|error| error.to_string())?) / 120.0;
            self.document.inner.borrow_mut().set_viewport(Viewport::new(
                physical[0],
                physical[1],
                scale,
                ColorScheme::Dark,
            ));
            Ok(())
        }

        fn pointer_motion_event(&mut self, point: [f64; 2]) {
            self.pointer
                .dispatch(&mut self.document, point, 0x110, None);
        }

        fn pointer_button_event(&mut self, button: u32, pressed: bool, point: [f64; 2]) {
            self.pointer
                .dispatch(&mut self.document, point, button, Some(pressed));
        }

        fn pointer_scroll_event(&mut self, delta: [f64; 2], point: [f64; 2]) {
            self.pointer.wheel(&mut self.document, point, delta);
        }

        fn text_event(&mut self, command: &scorepeek_overlay_wayland_handles::TextCommand) {
            text::dispatch_control_key(&mut self.document, command, self.text_composing);
        }

        fn ime_event(&mut self, update: scorepeek_overlay_wayland_handles::TextUpdate) {
            if let Some(composing) = text::dispatch_control_composition(&mut self.document, update)
            {
                self.text_composing = composing;
            }
        }

        fn keyboard_focus_event(&mut self, focused: bool) {
            if !focused {
                self.text_composing = false;
            }
        }
    }

    struct FakeAdapter {
        projection_cache: NativeProjectionCache,
        published: Arc<std::sync::Mutex<PublishedStages>>,
        surfaces: std::collections::BTreeMap<String, String>,
        stages: std::collections::BTreeMap<String, FakeStage>,
        displays: std::collections::BTreeMap<String, FakeDisplay>,
        assets: Arc<SkinAssetCache>,
        operations: Vec<String>,
        config_conversions: u64,
        work: FrameWorkProfile,
        transported_effects: Vec<EditorEffectKind>,
        transported_inputs: Vec<String>,
    }
    impl FakeAdapter {
        fn new(
            published: Arc<std::sync::Mutex<PublishedStages>>,
            assets: Arc<SkinAssetCache>,
        ) -> Self {
            Self {
                projection_cache: NativeProjectionCache::default(),
                published,
                surfaces: std::collections::BTreeMap::new(),
                stages: std::collections::BTreeMap::new(),
                displays: std::collections::BTreeMap::new(),
                assets,
                operations: Vec::new(),
                config_conversions: 0,
                work: FrameWorkProfile::default(),
                transported_effects: Vec::new(),
                transported_inputs: Vec::new(),
            }
        }

        fn drain_stage_transport(&mut self, authority: &mut NativeEditorAuthority) {
            let commands = self
                .stages
                .values_mut()
                .flat_map(FakeStage::drain_commands)
                .chain(
                    self.displays
                        .values_mut()
                        .flat_map(FakeDisplay::drain_commands),
                )
                .collect::<Vec<_>>();
            for command in commands {
                let effects = match command {
                    CoordinatorCommand::EditorInput { input, .. } => {
                        self.transported_inputs.push(format!("{input:?}"));
                        authority.dispatch(input)
                    }
                    CoordinatorCommand::Open {
                        output,
                        canvas,
                        preview_screen,
                    } => authority.dispatch(EditorInput::Open {
                        output,
                        canvas: Some(canvas),
                        preview: preview_screen
                            .unwrap_or(scorepeek_overlay::ScreenKind::MusicSelect),
                    }),
                };
                for effect in effects {
                    let kind = effect.kind();
                    self.transported_effects.push(kind);
                    let requested_draft = effect.requested_draft().map(<[_]>::to_vec);
                    let canvases = requested_draft
                        .clone()
                        .unwrap_or_else(|| authority.session().draft.clone());
                    let followups = authority.dispatch(EditorInput::BackendCompleted {
                        effect: kind,
                        requested_draft,
                        reply: EditorBackendReply {
                            ok: true,
                            readonly: false,
                            error: None,
                            canvases,
                            dirty: kind == EditorEffectKind::Update,
                        },
                    });
                    for followup in followups {
                        self.transported_effects.push(followup.kind());
                    }
                }
            }
        }

        fn apply_at(
            &mut self,
            authority: &mut NativeEditorAuthority,
            refresh_hz: u32,
        ) -> Result<(), String> {
            self.drain_stage_transport(authority);
            authority.poll();
            let session = authority.session();
            let display = if session.editing {
                Vec::new()
            } else {
                let started = Instant::now();
                let canvases = session
                    .draft
                    .iter()
                    .map(|presentation| {
                        let mut canvas = crate::config::empty_canvas(
                            presentation.id.clone(),
                            crate::bridge::data::Backend::Wayland,
                            presentation.skin,
                        );
                        canvas.apply_presentation(presentation);
                        canvas
                    })
                    .collect::<Vec<_>>();
                self.work.record("canvas_config", started.elapsed());
                canvases
            };
            let started = Instant::now();
            let projected = self
                .projection_cache
                .resolve(Some(cyan_skin()), &session, &display)?;
            self.work.record("projection", started.elapsed());
            let lifecycle = reconcile_worker_lifecycle(
                self.surfaces
                    .iter()
                    .map(|(id, output)| (id.as_str(), Some(output.as_str()), false)),
                projected,
            );
            for id in lifecycle.stop_join {
                if let Some(output) = self.surfaces.remove(&id) {
                    self.operations.push(format!("stop-object:{id}"));
                    self.operations.push(format!("stop:{output}"));
                    if let Some(mut stage) = self.stages.remove(&output) {
                        let wake = dispatch_native_event(&mut stage, Event::Wake)?;
                        assert!(!wake.closed);
                        self.operations.push(format!("wake:{output}"));
                        let closed = dispatch_native_event(&mut stage, Event::Closed)?;
                        assert!(closed.closed);
                        self.operations.push(format!("close:{output}"));
                        stage.shutdown(&mut self.operations);
                    } else if let Some(display) = self.displays.remove(&id) {
                        display.shutdown(&mut self.operations);
                    }
                }
            }
            for id in lifecycle.start {
                let canvas = projected
                    .iter()
                    .find(|canvas| canvas.id == id)
                    .expect("production lifecycle returned an unknown canvas");
                self.config_conversions += 1;
                self.operations.push(format!("configure:{}", canvas.output));
                self.surfaces.insert(id, canvas.output.clone());
                if !session.editing {
                    let mut display = FakeDisplay::new(canvas, Arc::clone(&self.assets))?;
                    let configured = dispatch_native_event(
                        &mut display,
                        Event::Configure {
                            logical: [canvas.width, canvas.height],
                            physical: [canvas.width, canvas.height],
                            scale_120: 120,
                        },
                    )?;
                    assert!(configured.configured);
                    display.turn(configured, refresh_hz)?;
                    let frame = dispatch_native_event(&mut display, Event::Frame)?;
                    display.turn(frame, refresh_hz)?;
                    self.displays.insert(canvas.id.clone(), display);
                }
            }
            if session.editing {
                let published = self
                    .published
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .by_output
                    .clone();
                for projection in published.values().rev().cloned() {
                    if let Some(stage) = self.stages.get_mut(&projection.output.name) {
                        stage.accept(&projection);
                    } else {
                        let output = projection.output.name.clone();
                        let logical = projection.output.logical_size.unwrap_or([1920, 1080]);
                        let mut stage = FakeStage::new(projection, Arc::clone(&self.assets))?;
                        let configured = dispatch_native_event(
                            &mut stage,
                            Event::Configure {
                                logical,
                                physical: logical,
                                scale_120: 120,
                            },
                        )?;
                        assert!(configured.configured);
                        stage.turn(configured, refresh_hz)?;
                        self.operations.push(format!("surface-create:{output}"));
                        self.operations.push(format!("surface-configure:{output}"));
                        self.stages.insert(output, stage);
                    }
                }
                for projection in published.into_values() {
                    let stage = self
                        .stages
                        .get_mut(&projection.output.name)
                        .expect("stage created for every output");
                    stage.accept(&projection);
                    let outcome = dispatch_native_event(stage, Event::Frame)?;
                    stage.turn(outcome, refresh_hz)?;
                }
            }
            Ok(())
        }

        fn apply(&mut self, authority: &mut NativeEditorAuthority) -> Result<(), String> {
            self.apply_at(authority, 120)
        }

        fn runtime_creates(&self) -> u64 {
            self.stages
                .values()
                .map(|stage| stage.runtime_creates)
                .sum()
        }

        fn output_event(
            &mut self,
            authority: &mut NativeEditorAuthority,
            outputs: &[OutputDescription],
        ) -> Result<(), String> {
            authority.dispatch(EditorInput::SetOutputs(editor_outputs_from_descriptions(
                outputs,
            )));
            self.operations.push("output-discovery".into());
            self.apply(authority)
        }

        fn event(&mut self, output: &str, event: Event) -> Result<NativeEventOutcome, String> {
            let stage = self
                .stages
                .get_mut(output)
                .ok_or_else(|| format!("fake event targets missing output {output}"))?;
            dispatch_native_event(stage, event)
        }

        fn display_event(
            &mut self,
            canvas: &str,
            event: Event,
        ) -> Result<NativeEventOutcome, String> {
            let display = self
                .displays
                .get_mut(canvas)
                .ok_or_else(|| format!("fake event targets missing canvas {canvas}"))?;
            dispatch_native_event(display, event)
        }

        #[allow(clippy::cast_possible_truncation)]
        fn stage_point(&self, output: &str, selector: &str) -> Result<[f64; 2], String> {
            let stage = self
                .stages
                .get(output)
                .ok_or_else(|| format!("missing stage {output}"))?;
            let inner = stage.document.inner.borrow();
            let node = inner
                .query_selector(selector)
                .map_err(|error| format!("invalid selector {selector}: {error:?}"))?
                .ok_or_else(|| format!("selector did not match: {selector}"))?;
            let rect = inner
                .get_client_bounding_rect(node)
                .ok_or_else(|| format!("selector has no layout: {selector}"))?;
            let point = [rect.x + rect.width / 2.0, rect.y + rect.height / 2.0];
            let mut hit = inner.element_from_point(point[0] as f32, point[1] as f32);
            let mut reaches_target = false;
            while let Some(hit_node) = hit {
                if hit_node == node {
                    reaches_target = true;
                    break;
                }
                hit = inner.get_node(hit_node).and_then(|item| item.parent);
            }
            if !reaches_target {
                let obscurer = inner
                    .element_from_point(point[0] as f32, point[1] as f32)
                    .and_then(|id| {
                        let rect = inner.get_client_bounding_rect(id);
                        inner
                            .get_node(id)
                            .and_then(|item| item.element_data())
                            .map(|element| (element, rect))
                    })
                    .map_or_else(
                        || "non-element".into(),
                        |(element, rect)| {
                            let class = element
                                .attrs
                                .iter()
                                .find(|attribute| attribute.name.local.as_ref() == "class")
                                .map_or("", |attribute| attribute.value.as_str());
                            let section = element
                                .attrs
                                .iter()
                                .find(|attribute| attribute.name.local.as_ref() == "data-section")
                                .map_or("", |attribute| attribute.value.as_str());
                            format!("{}.{class}[{section}] {rect:?}", element.name.local)
                        },
                    );
                return Err(format!(
                    "selector center is obscured: {selector} at {},{} by {obscurer}",
                    point[0], point[1],
                ));
            }
            Ok(point)
        }

        #[allow(clippy::cast_possible_truncation)]
        fn stage_descendant_point(&self, output: &str, selector: &str) -> Result<[f64; 2], String> {
            let stage = self
                .stages
                .get(output)
                .ok_or_else(|| format!("missing stage {output}"))?;
            let inner = stage.document.inner.borrow();
            let node = inner
                .query_selector(selector)
                .map_err(|error| format!("invalid selector {selector}: {error:?}"))?
                .ok_or_else(|| format!("selector did not match: {selector}"))?;
            let [width, height] = stage.physical_size;
            for y_step in 1..20 {
                for x_step in 1..20 {
                    let point = [
                        f64::from(width * x_step / 20),
                        f64::from(height * y_step / 20),
                    ];
                    let mut hit = inner.element_from_point(point[0] as f32, point[1] as f32);
                    while let Some(hit_node) = hit {
                        if hit_node == node {
                            return Ok(point);
                        }
                        hit = inner.get_node(hit_node).and_then(|item| item.parent);
                    }
                }
            }
            Err(format!("selector has no visible descendant: {selector}"))
        }

        fn click_stage(
            &mut self,
            authority: &mut NativeEditorAuthority,
            output: &str,
            selector: &str,
        ) -> Result<(), String> {
            let [x, y] = self
                .stage_point(output, selector)
                .or_else(|_| self.stage_descendant_point(output, selector))?;
            for event in [
                Event::PointerMotion { x, y },
                Event::PointerButton {
                    button: 0x110,
                    pressed: true,
                    x,
                    y,
                },
                Event::PointerButton {
                    button: 0x110,
                    pressed: false,
                    x,
                    y,
                },
            ] {
                self.event(output, event)?;
                self.apply(authority)?;
            }
            self.frame(authority, 120)
        }

        fn drag_stage(
            &mut self,
            authority: &mut NativeEditorAuthority,
            output: &str,
            selector: &str,
            delta: [f64; 2],
        ) -> Result<(), String> {
            let [x, y] = self.stage_point(output, selector)?;
            self.event(output, Event::PointerMotion { x, y })?;
            self.event(
                output,
                Event::PointerButton {
                    button: 0x110,
                    pressed: true,
                    x,
                    y,
                },
            )?;
            self.apply(authority)?;
            for step in 1..=4 {
                let fraction = f64::from(step) / 4.0;
                self.event(
                    output,
                    Event::PointerMotion {
                        x: x + delta[0] * fraction,
                        y: y + delta[1] * fraction,
                    },
                )?;
                self.apply(authority)?;
                self.frame(authority, 120)?;
            }
            self.event(
                output,
                Event::PointerButton {
                    button: 0x110,
                    pressed: false,
                    x: x + delta[0],
                    y: y + delta[1],
                },
            )?;
            self.apply(authority)?;
            self.frame(authority, 120)
        }

        fn scroll_stage(
            &mut self,
            authority: &mut NativeEditorAuthority,
            output: &str,
            selector: &str,
            dy: f64,
        ) -> Result<(), String> {
            let [x, y] = self.stage_descendant_point(output, selector)?;
            self.event(output, Event::PointerScroll { dx: 0.0, dy, x, y })?;
            self.apply(authority)?;
            self.frame(authority, 120)
        }

        fn frame(&mut self, authority: &mut NativeEditorAuthority, hz: u32) -> Result<(), String> {
            for stage in self.stages.values_mut() {
                if stage.pending_frame_start.is_none() {
                    stage.pending_frame_start = Some(stage.work.snapshot());
                }
            }
            self.apply_at(authority, hz)
        }
    }

    fn assert_converged(fake: &FakeAdapter, authority: &NativeEditorAuthority) {
        let session = authority.session();
        let expected_surfaces = fake
            .projection_cache
            .canvases
            .iter()
            .map(|canvas| (canvas.id.clone(), canvas.output.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            fake.surfaces, expected_surfaces,
            "fake surfaces must exactly equal the production lifecycle projection"
        );
        let mut paint_targets = fake
            .displays
            .iter()
            .filter(|(_, display)| display.presents > 0)
            .map(|(id, _)| id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        for (output, stage) in &fake.stages {
            if stage.presents > 0
                && let Some((id, _)) = fake
                    .surfaces
                    .iter()
                    .find(|(_, surface_output)| *surface_output == output)
            {
                paint_targets.insert(id.clone());
            }
        }
        assert_eq!(
            paint_targets,
            fake.surfaces.keys().cloned().collect(),
            "fake presentation must exactly cover live surfaces"
        );

        let owners = fake
            .assets
            .editor_owners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if !session.editing {
            assert!(
                fake.stages.is_empty(),
                "closed editor must have no stage replica"
            );
            assert!(
                owners.is_empty(),
                "closed editor must retain no canvas owner"
            );
            assert_eq!(
                fake.displays
                    .keys()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>(),
                fake.surfaces.keys().cloned().collect(),
                "display skin runtime/tree set must exactly equal live display surfaces"
            );
            for (id, display) in &fake.displays {
                assert_eq!(&display.canvas.id, id);
                assert!(
                    display.paint_count > 0,
                    "every live display must be painted"
                );
                assert!(display.presents > 0, "every live display must be presented");
                assert!(display.commits > 0, "every live display must be committed");
                assert!(
                    display
                        .document
                        .inner
                        .borrow()
                        .query_selector("#scorepeek-skin-root > *")
                        .unwrap()
                        .is_some(),
                    "every live display must retain a mounted production skin tree"
                );
                assert!(
                    !display.skin.release.is_empty(),
                    "every live display must retain its production skin runtime metadata"
                );
            }
            return;
        }
        assert!(
            fake.displays.is_empty(),
            "editing mode must not retain display skin runtimes"
        );

        let expected_stages = session
            .outputs
            .iter()
            .map(|output| (output.name.clone(), session.stage_projection(output)))
            .collect::<std::collections::BTreeMap<_, _>>();
        let published = fake
            .published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .by_output
            .clone();
        assert!(
            published == expected_stages,
            "published projections must exactly equal authority-derived projections"
        );
        assert_eq!(
            fake.stages
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            expected_stages
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            "one replica stage must exist for every current output and no other output"
        );

        let mut expected_owners = std::collections::BTreeMap::new();
        for (output, expected) in expected_stages {
            let stage = fake.stages.get(&output).expect("expected stage exists");
            match &*stage.projection.borrow() {
                NativeDocumentProjection::Editor(actual) => assert!(
                    actual == &expected,
                    "stage replica must atomically accept the complete published projection"
                ),
                NativeDocumentProjection::Display { .. } => {
                    panic!("editor stage cannot retain a display projection")
                }
            }
            let expected_canvases = expected
                .canvases
                .iter()
                .map(|canvas| canvas.id.clone())
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(
                stage
                    .previews
                    .keys()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>(),
                expected_canvases,
                "mounted skin runtimes must exactly equal visible projected canvases"
            );
            let inner = stage.document.inner.borrow();
            assert_eq!(
                inner
                    .query_selector_all(".scorepeek-skin-scope .overlay-canvas")
                    .unwrap()
                    .len(),
                expected.canvases.len(),
                "mounted skin DOM roots must exactly equal visible projected canvases"
            );
            assert_eq!(
                inner
                    .query_selector_all(".editor-widget-hit")
                    .unwrap()
                    .len(),
                if expected.interactive {
                    expected
                        .canvases
                        .iter()
                        .map(|canvas| canvas.widgets.len())
                        .sum::<usize>()
                } else {
                    0
                },
                "hit regions must exactly equal projected widgets"
            );
            for canvas in &expected.canvases {
                assert!(
                    inner
                        .query_selector(&format!(
                            ".scorepeek-skin-scope .overlay-canvas[data-canvas-id='{}']",
                            canvas.id
                        ))
                        .unwrap()
                        .is_some(),
                    "every projected canvas must own one skin DOM root"
                );
                assert!(
                    inner
                        .query_selector(&format!(".editor-canvas[data-canvas='{}']", canvas.id))
                        .unwrap()
                        .is_some(),
                    "every projected canvas must own one editor hit root"
                );
                expected_owners.insert(canvas.id.clone(), output.clone());
                if expected.interactive {
                    for widget in &canvas.widgets {
                        assert!(
                                inner
                                    .query_selector(&format!(
                                        ".editor-canvas[data-canvas='{}'] .editor-widget-hit[data-widget='{}']",
                                        canvas.id, widget.id
                                    ))
                                    .unwrap()
                                    .is_some(),
                                "every interactive projected widget must own one matching hit region"
                            );
                    }
                }
            }
        }
        assert_eq!(
            owners, expected_owners,
            "canvas leases must exactly equal mounted stage/runtime ownership"
        );
    }

    let mut canvases = crate::config::visual_debug_config(cyan_skin())
        .canvases
        .into_iter()
        .filter(|canvas| canvas.backend == crate::bridge::data::Backend::Wayland)
        .take(2)
        .map(|canvas| canvas.presentation())
        .collect::<Vec<_>>();
    canvases[0].output = Some("WL-1".into());
    canvases[1].output = Some("WL-2".into());
    canvases[0]
        .skin_properties
        .insert("background".into(), serde_json::json!("static"));
    canvases[0].x = 0;
    canvases[0].y = 0;
    canvases[0].width = canvases[0].width.min(1_200);
    canvases[0].height = canvases[0].height.min(700);
    let mut animated_widget = canvases[1].widgets[0].clone();
    animated_widget.x = 600;
    animated_widget.y = 160;
    animated_widget.width = animated_widget.width.min(480);
    animated_widget.height = animated_widget.height.min(200);
    let mut unrelated_widget = animated_widget.clone();
    unrelated_widget.id = "selection-unrelated".into();
    unrelated_widget.y = 420;
    canvases[1].widgets = vec![animated_widget, unrelated_widget];
    canvases[1]
        .skin_properties
        .insert("background".into(), serde_json::json!("animated"));
    canvases[1].width = 1_200;
    canvases[1].height = 700;
    canvases[1].x = 0;
    canvases[1].y = 0;
    let mut session = EditorSession::new(canvases, [1920, 1080], "fake-wayland");
    session.set_session_id(41);
    session.set_skins(embedded_editor_skins());
    let initial_outputs = vec![
        OutputDescription {
            name: "WL-1".into(),
            model: "fake one".into(),
            logical_size: Some([1920, 1080]),
        },
        OutputDescription {
            name: "WL-2".into(),
            model: "fake two".into(),
            logical_size: Some([1280, 720]),
        },
    ];
    session.set_outputs(editor_outputs_from_descriptions(&initial_outputs));
    session.readonly = false;
    session.reduce(EditorInput::Open {
        output: Some("WL-1".into()),
        canvas: Some(session.draft[0].id.clone()),
        preview: scorepeek_overlay::ScreenKind::MusicSelect,
    });
    let published = Arc::new(std::sync::Mutex::new(PublishedStages::default()));
    let mut authority = NativeEditorAuthority::new(session, Arc::clone(&published));
    let acquired = authority.session().draft.clone();
    authority.dispatch(EditorInput::BackendCompleted {
        effect: EditorEffectKind::Acquire,
        requested_draft: None,
        reply: EditorBackendReply {
            ok: true,
            readonly: false,
            error: None,
            canvases: acquired,
            dirty: false,
        },
    });
    let store_path = std::env::temp_dir().join(format!(
        "scorepeek-fake-wayland-skins-{}-{}",
        std::process::id(),
        NEXT_STORE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let store_guard = TestSkinStore(store_path.clone());
    let store = crate::skin::StoreRoot::new(store_path);
    let package_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/skins");
    for package in ["cyan-system.zip", "result-aurora.zip", "dj-blackbox.zip"] {
        store
            .install(&package_root.join(package))
            .unwrap_or_else(|error| panic!("install test skin {package}: {error}"));
    }
    let assets = Arc::new(SkinAssetCache::new(store));
    let mut cold_load_work = FrameWorkProfile::default();
    let cold_start = cold_load_work.snapshot();
    assets
        .load_profiled(cyan_skin().name(), &mut cold_load_work)
        .unwrap();
    cold_load_work.finish_frame(&cold_start, 0, 0);
    let cold_load = cold_load_work.frames.back().unwrap();
    assert_eq!(cold_load.phases["package_open"].calls, 1);
    assert_eq!(cold_load.phases["package_clone"].calls, 1);
    assert_eq!(
        cold_load.phases["package_open"].total_ns,
        assets.open_ns.load(std::sync::atomic::Ordering::Relaxed)
    );
    assert_eq!(
        cold_load.phases["package_clone"].total_ns,
        assets.clone_ns.load(std::sync::atomic::Ordering::Relaxed)
    );
    let mut fake = FakeAdapter::new(published, assets);
    fake.apply(&mut authority).unwrap();
    let configured = fake
        .event(
            "WL-1",
            Event::Configure {
                logical: [2_000, 1_200],
                physical: [2_000, 1_200],
                scale_120: 120,
            },
        )
        .unwrap();
    assert!(configured.configured);
    fake.apply(&mut authority).unwrap();
    assert_eq!(
        authority
            .session()
            .outputs
            .iter()
            .find(|output| output.name == "WL-1")
            .and_then(|output| output.logical_size),
        Some([2_000, 1_200]),
        "fake configure must traverse the production resize transport into Dioxus authority"
    );
    let configured = fake
        .event(
            "WL-1",
            Event::Configure {
                logical: [1_920, 1_080],
                physical: [1_920, 1_080],
                scale_120: 120,
            },
        )
        .unwrap();
    assert!(configured.configured);
    fake.apply(&mut authority).unwrap();
    for event in [
        Event::PointerMotion { x: 8.0, y: 8.0 },
        Event::PointerButton {
            button: 0x110,
            pressed: true,
            x: 8.0,
            y: 8.0,
        },
        Event::PointerButton {
            button: 0x110,
            pressed: false,
            x: 8.0,
            y: 8.0,
        },
        Event::PointerScroll {
            dx: 0.0,
            dy: -20.0,
            x: 8.0,
            y: 8.0,
        },
        Event::Text(scorepeek_overlay_wayland_handles::TextCommand::Cancel),
        Event::Ime(scorepeek_overlay_wayland_handles::TextUpdate::default()),
        Event::KeyboardFocus(false),
        Event::Wake,
    ] {
        let outcome = fake.event("WL-1", event).unwrap();
        assert!(!outcome.closed);
    }
    fake.scroll_stage(&mut authority, "WL-1", ".navigator-scroll", -800.0)
        .unwrap();
    fake.click_stage(
            &mut authority,
            "WL-1",
            ".workspace-output-option[data-output='WL-2'] > .navigator-item-line > .navigator-item-select",
        )
        .unwrap();
    assert_eq!(authority.session().active_output.as_deref(), Some("WL-2"));
    fake.scroll_stage(&mut authority, "WL-2", ".navigator-scroll", -800.0)
        .unwrap();
    fake.click_stage(
            &mut authority,
            "WL-2",
            ".canvas-select[data-canvas-id='wayland-selection'] > .navigator-item-line > .navigator-item-select",
        )
        .unwrap();
    let (drag_output, dragged_canvas, dragged_widget, before_drag) = {
        let session = authority.session();
        let canvas = session.current().expect("opened canvas remains selected");
        let widget = canvas
            .widgets
            .iter()
            .find(|widget| {
                let horizontal_center = canvas
                    .x
                    .saturating_add(widget.x)
                    .saturating_add(i32::try_from(widget.width / 2).unwrap_or(i32::MAX));
                let center = canvas
                    .y
                    .saturating_add(widget.y)
                    .saturating_add(i32::try_from(widget.height / 2).unwrap_or(i32::MAX));
                horizontal_center > 500 && (100..680).contains(&center)
            })
            .expect("fixture has a draggable widget inside the logical viewport");
        (
            session.active_output.clone().unwrap(),
            canvas.id.clone(),
            widget.id.clone(),
            [widget.x, widget.y],
        )
    };
    let revision_before_drag = authority.session().revision;
    fake.drag_stage(
            &mut authority,
            &drag_output,
            &format!(
                ".editor-canvas[data-canvas='{dragged_canvas}'] .editor-widget-hit[data-widget='{dragged_widget}']"
            ),
            [64.0, 40.0],
        )
        .unwrap();
    assert!(
        authority.session().revision >= revision_before_drag + 4,
        "transported inputs: {:?}",
        fake.transported_inputs
    );
    let after_drag = {
        let session = authority.session();
        let widget = session
            .draft
            .iter()
            .find(|canvas| canvas.id == dragged_canvas)
            .unwrap()
            .widgets
            .iter()
            .find(|widget| widget.id == dragged_widget)
            .unwrap();
        [widget.x, widget.y]
    };
    assert_eq!(after_drag, [before_drag[0] + 64, before_drag[1] + 40]);
    assert!(
        fake.transported_effects.contains(&EditorEffectKind::Update),
        "protocol pointer events must traverse Dioxus and coordinator transport to authority; inputs={:?}",
        fake.transported_inputs
    );
    authority.dispatch(EditorInput::Action(EditorAction::SelectOutput(
        "WL-1".into(),
    )));
    authority.dispatch(EditorInput::Action(EditorAction::SelectCanvas(
        "wayland-status".into(),
    )));
    fake.apply(&mut authority).unwrap();
    assert_eq!(authority.session().active_output.as_deref(), Some("WL-1"));
    assert_converged(&fake, &authority);
    assert_eq!((fake.surfaces.len(), fake.stages.len()), (2, 2));
    let package_clones_before = fake
        .assets
        .clone_count
        .load(std::sync::atomic::Ordering::Relaxed);
    let wl1 = fake.stages.get_mut("WL-1").unwrap();
    wl1.frame().unwrap();
    let measured_package_clones = wl1
        .work
        .frames
        .back()
        .expect("a frame callback records one workload sample")
        .phases["package_clone"]
        .calls;
    let package_clones_after = fake
        .assets
        .clone_count
        .load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        measured_package_clones,
        package_clones_after.saturating_sub(package_clones_before),
        "NetProvider package clones must be attributed to the frame that fetched the resource"
    );
    let pixels = wl1.render_pixels();
    let background_offset = (40 * 1920 + 400) * 4;
    let background_pixel = pixels[background_offset..background_offset + 4].to_vec();
    assert_eq!(
        background_pixel[3], 255,
        "the production native tree/resource/layout/scene path must paint the skin background"
    );
    assert_ne!(
        &background_pixel[..3],
        &[14, 25, 37],
        "the native pixel oracle must see package artwork, not only its fallback color"
    );
    assert_eq!(
        fake.stages
            .values()
            .map(|stage| stage.previews.len())
            .sum::<usize>(),
        2
    );
    let retained = (
        fake.projection_cache.rebuilds,
        fake.config_conversions,
        fake.runtime_creates(),
        fake.assets
            .open_count
            .load(std::sync::atomic::Ordering::Relaxed),
        fake.assets
            .resource_lookup_count
            .load(std::sync::atomic::Ordering::Relaxed),
    );
    let work_calls = |fake: &FakeAdapter, phase: &'static str| {
        fake.work.calls(phase)
            + fake
                .stages
                .values()
                .map(|stage| stage.work.calls(phase))
                .sum::<u64>()
    };
    let retained_work =
        ["package_clone", "wasm_runtime_create"].map(|phase| work_calls(&fake, phase));
    let frame_counts = |fake: &FakeAdapter| {
        fake.stages
            .values()
            .map(|stage| {
                (
                    stage.dioxus_polls,
                    stage.wasm_calls,
                    stage.tree_updates,
                    stage.layouts,
                    stage.resource_resolves,
                    stage.scenes,
                    stage.presents,
                    stage.commits,
                )
            })
            .fold((0, 0, 0, 0, 0, 0, 0, 0), |a, b| {
                (
                    a.0 + b.0,
                    a.1 + b.1,
                    a.2 + b.2,
                    a.3 + b.3,
                    a.4 + b.4,
                    a.5 + b.5,
                    a.6 + b.6,
                    a.7 + b.7,
                )
            })
    };
    for hz in [60_u32, 120] {
        fake.apply(&mut authority).unwrap();
        let sample_counts = fake
            .stages
            .iter()
            .map(|(output, stage)| (output.clone(), stage.work.frames.len()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let frame_before = frame_counts(&fake);
        let projection_before = work_calls(&fake, "projection");
        let config_before = work_calls(&fake, "canvas_config");
        let work_before = [
            "dioxus_poll",
            "blitz_layout",
            "resource_decode",
            "scene",
            "gpu_present",
            "surface_commit",
        ]
        .map(|phase| work_calls(&fake, phase));
        for _ in 0..hz {
            fake.frame(&mut authority, hz).unwrap();
        }
        assert_eq!(
            (
                fake.projection_cache.rebuilds,
                fake.config_conversions,
                fake.runtime_creates(),
                fake.assets
                    .open_count
                    .load(std::sync::atomic::Ordering::Relaxed),
                fake.assets
                    .resource_lookup_count
                    .load(std::sync::atomic::Ordering::Relaxed),
            ),
            retained,
            "steady frames must not rebuild projection/config/runtime or reload resources"
        );
        assert_eq!(
            ["package_clone", "wasm_runtime_create",].map(|phase| work_calls(&fake, phase)),
            retained_work,
            "steady production turns must retain packages and runtimes"
        );
        let frame_after = frame_counts(&fake);
        let expected = u64::from(hz) * u64::try_from(fake.stages.len()).unwrap();
        assert_eq!(
            work_calls(&fake, "projection") - projection_before,
            u64::from(hz)
        );
        assert_eq!(work_calls(&fake, "canvas_config"), config_before);
        let work_after = [
            "dioxus_poll",
            "blitz_layout",
            "resource_decode",
            "scene",
            "gpu_present",
            "surface_commit",
        ]
        .map(|phase| work_calls(&fake, phase));
        assert!(
            (expected..=expected + u64::try_from(fake.stages.len()).unwrap())
                .contains(&(work_after[0] - work_before[0]))
        );
        assert_eq!(work_after[1] - work_before[1], expected * 2);
        for index in 2..work_after.len() {
            assert_eq!(work_after[index] - work_before[index], expected);
        }
        assert!(
            (expected..=expected + u64::try_from(fake.stages.len()).unwrap())
                .contains(&(frame_after.0 - frame_before.0))
        );
        assert_eq!(
            (
                frame_after.1 - frame_before.1,
                frame_after.2 - frame_before.2
            ),
            (expected, expected),
            "the skin-owned native schedule must rerender each animated preview"
        );
        assert_eq!(frame_after.3 - frame_before.3, expected);
        assert_eq!(frame_after.4 - frame_before.4, expected);
        assert_eq!(frame_after.5 - frame_before.5, expected);
        assert_eq!(frame_after.6 - frame_before.6, expected);
        assert!(
            (expected..=expected + u64::try_from(fake.stages.len()).unwrap())
                .contains(&(frame_after.7 - frame_before.7))
        );
        for (output, stage) in &fake.stages {
            let before = sample_counts[output];
            let samples = stage.work.frames.iter().skip(before).collect::<Vec<_>>();
            assert_eq!(samples.len(), usize::try_from(hz).unwrap());
            let expected_live_canvases = u64::try_from(stage.previews.len()).unwrap_or(u64::MAX);
            let expected_live_widgets = stage.previews.values().fold(0_u64, |count, preview| {
                count
                    .saturating_add(u64::try_from(preview.canvas.widgets.len()).unwrap_or(u64::MAX))
            });
            for sample in samples {
                assert!(
                    FrameWorkProfile::REQUIRED_PHASES
                        .iter()
                        .all(|phase| sample.phases.contains_key(phase)),
                    "every frame must represent every required phase as measured work, zero work, or unmeasured"
                );
                for phase in [
                    "projection_rebuild",
                    "canvas_config",
                    "package_open",
                    "package_clone",
                    "wasm_runtime_create",
                ] {
                    assert_eq!(sample.phases[phase].calls, 0, "unexpected {phase} work");
                }
                for phase in [
                    "skin_input",
                    "wasm_render",
                    "json_tree",
                    "tree_reconciliation",
                ] {
                    assert_eq!(sample.phases[phase].calls, expected_live_canvases);
                }
                assert_eq!(sample.phases["dioxus_poll"].calls, 1);
                assert_eq!(sample.phases["blitz_layout"].calls, 2);
                assert_eq!(sample.phases["scene"].calls, 1);
                assert_eq!(sample.phases["gpu_present"].calls, 1);
                assert_eq!(sample.phases["surface_commit"].calls, 1);
                assert_eq!(sample.live_canvases, expected_live_canvases);
                assert_eq!(sample.live_widgets, expected_live_widgets);
            }
        }
    }

    authority.dispatch(EditorInput::Action(EditorAction::CanvasVisibleNone));
    fake.apply(&mut authority).unwrap();
    assert!(
        fake.stages.values().any(|stage| {
            stage
                .work
                .frames
                .back()
                .is_some_and(|sample| sample.phases["projection_rebuild"].calls > 0)
        }),
        "a projection accepted on a deferred turn must remain attributed to the frame that presents it"
    );
    assert_converged(&fake, &authority);
    let pixels_after_unmount = fake.stages.get_mut("WL-1").unwrap().render_pixels();
    assert_ne!(
        pixels_after_unmount[background_offset..background_offset + 4],
        background_pixel,
        "the retained renderer must not preserve pixels from an unmounted canvas"
    );
    assert_eq!(
        fake.stages
            .values()
            .map(|stage| stage.previews.len())
            .sum::<usize>(),
        1,
        "visibility removal must unmount the selected canvas preview"
    );
    authority.dispatch(EditorInput::Action(EditorAction::CanvasVisibleAll));
    fake.apply(&mut authority).unwrap();
    assert_converged(&fake, &authority);
    assert_eq!(
        fake.stages
            .values()
            .map(|stage| stage.previews.len())
            .sum::<usize>(),
        2
    );

    let runtime_creates = fake.runtime_creates();
    let replacement_skin = if authority.session().current().unwrap().skin == result_skin() {
        cyan_skin()
    } else {
        result_skin()
    };
    authority.dispatch(EditorInput::Action(EditorAction::Skin(replacement_skin)));
    fake.apply(&mut authority).unwrap();
    assert_converged(&fake, &authority);
    assert_eq!(
        fake.runtime_creates(),
        runtime_creates + 1,
        "skin replacement must create exactly one replacement runtime"
    );

    let _deleted_canvas = authority.session().selected_canvas.clone().unwrap();
    for _ in 0..8 {
        fake.scroll_stage(&mut authority, "WL-1", ".inspector-scroll", -2_000.0)
            .unwrap();
    }
    fake.click_stage(&mut authority, "WL-1", ".delete-canvas")
        .unwrap();
    assert_converged(&fake, &authority);
    assert_eq!(
        fake.stages
            .values()
            .map(|stage| stage.previews.len())
            .sum::<usize>(),
        1
    );
    assert_eq!(
        fake.assets
            .editor_owners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        1,
        "canvas deletion must release its runtime owner"
    );
    for _ in 0..8 {
        authority.dispatch(EditorInput::Action(EditorAction::AddCanvas));
        fake.apply(&mut authority).unwrap();
        authority.dispatch(EditorInput::Action(EditorAction::DeleteCanvas));
        fake.apply(&mut authority).unwrap();
    }
    assert_converged(&fake, &authority);
    let deleted_history_sample_sequences = fake
        .stages
        .iter()
        .map(|(output, stage)| {
            (
                output.clone(),
                stage.work.frames.back().map_or(0, |sample| sample.sequence),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let work_before_delete_frame = fake
        .stages
        .values()
        .map(|stage| {
            (
                stage.input_generations,
                stage.wasm_calls,
                stage.tree_updates,
            )
        })
        .fold((0, 0, 0), |sum, count| {
            (sum.0 + count.0, sum.1 + count.1, sum.2 + count.2)
        });
    for _ in 0..60 {
        fake.frame(&mut authority, 60).unwrap();
    }
    let work_after_delete_frame = fake
        .stages
        .values()
        .map(|stage| {
            (
                stage.input_generations,
                stage.wasm_calls,
                stage.tree_updates,
            )
        })
        .fold((0, 0, 0), |sum, count| {
            (sum.0 + count.0, sum.1 + count.1, sum.2 + count.2)
        });
    let scheduled = 60_u64
        * fake
            .stages
            .values()
            .map(|stage| u64::try_from(stage.previews.len()).unwrap_or(u64::MAX))
            .sum::<u64>();
    assert_eq!(
        (
            work_after_delete_frame.0 - work_before_delete_frame.0,
            work_after_delete_frame.1 - work_before_delete_frame.1,
            work_after_delete_frame.2 - work_before_delete_frame.2,
        ),
        (scheduled, scheduled, scheduled),
        "skin scheduling after deletion must visit only the remaining live previews"
    );
    for (output, stage) in &fake.stages {
        let expected_live_canvases = u64::try_from(stage.previews.len()).unwrap_or(u64::MAX);
        let expected_live_widgets = stage.previews.values().fold(0_u64, |count, preview| {
            count.saturating_add(u64::try_from(preview.canvas.widgets.len()).unwrap_or(u64::MAX))
        });
        let samples = stage
            .work
            .frames
            .iter()
            .filter(|sample| sample.sequence > deleted_history_sample_sequences[output])
            .collect::<Vec<_>>();
        assert_eq!(samples.len(), 60);
        for sample in samples {
            assert_eq!(sample.live_canvases, expected_live_canvases);
            assert_eq!(sample.live_widgets, expected_live_widgets);
            assert_eq!(sample.phases["skin_input"].calls, expected_live_canvases);
            assert_eq!(sample.phases["wasm_render"].calls, expected_live_canvases);
            assert_eq!(sample.phases["json_tree"].calls, expected_live_canvases);
            assert_eq!(
                sample.phases["tree_reconciliation"].calls,
                expected_live_canvases
            );
        }
    }
    fake.click_stage(&mut authority, "WL-1", ".add-canvas")
        .unwrap();
    assert_converged(&fake, &authority);
    assert_eq!(
        fake.stages
            .values()
            .map(|stage| stage.previews.len())
            .sum::<usize>(),
        2
    );

    let stale_wl2 = fake
        .published
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .by_output
        .get("WL-2")
        .cloned()
        .unwrap();
    let _moved_canvas = authority.session().selected_canvas.clone().unwrap();
    for _ in 0..8 {
        fake.scroll_stage(&mut authority, "WL-1", ".inspector-scroll", -2_000.0)
            .unwrap();
    }
    fake.click_stage(&mut authority, "WL-1", ".output-option[data-output='WL-2']")
        .unwrap();
    assert_converged(&fake, &authority);
    assert_eq!(
        fake.stages
            .values()
            .map(|stage| stage.previews.len())
            .sum::<usize>(),
        2
    );
    assert_eq!(
        fake.assets
            .editor_owners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        2,
        "reverse delivery still leaves one owner per canvas"
    );
    assert_eq!(fake.stages["WL-2"].previews.len(), 2);
    assert!(
        fake.stages["WL-2"].previews["wayland-selection"]
            .canvas
            .widgets
            .len()
            > 1,
        "the animated workload must include multiple widgets"
    );
    let wl2 = fake.stages.get_mut("WL-2").unwrap();
    let accepted = wl2.projection_accepts;
    let current_revision = match &*wl2.projection.borrow() {
        NativeDocumentProjection::Editor(stage) => stage.revision,
        NativeDocumentProjection::Display { .. } => unreachable!(),
    };
    wl2.accept(&stale_wl2);
    assert_eq!(wl2.projection_accepts, accepted);
    assert_eq!(
        match &*wl2.projection.borrow() {
            NativeDocumentProjection::Editor(stage) => stage.revision,
            NativeDocumentProjection::Display { .. } => unreachable!(),
        },
        current_revision,
        "an out-of-order transport replica must not replace the current stage"
    );

    fake.output_event(
        &mut authority,
        &[OutputDescription {
            name: "WL-2".into(),
            model: "fake two".into(),
            logical_size: Some([1280, 720]),
        }],
    )
    .unwrap();
    assert_converged(&fake, &authority);
    assert_eq!(fake.surfaces.len(), 1);
    let stop = fake
        .operations
        .iter()
        .position(|op| op == "stop:WL-1")
        .unwrap();
    let suspend = fake
        .operations
        .iter()
        .position(|op| op == "suspend:WL-1")
        .unwrap();
    let unmap = fake
        .operations
        .iter()
        .position(|op| op == "unmap:WL-1")
        .unwrap();
    let join = fake
        .operations
        .iter()
        .position(|op| op == "join:WL-1")
        .unwrap();
    assert!(stop < suspend && suspend < unmap && unmap < join);
    assert!(
        fake.operations
            .iter()
            .enumerate()
            .filter(|(_, op)| op.starts_with("runtime-drop:") && op.ends_with(":WL-1"))
            .all(|(index, _)| index < suspend),
        "all retained skin resources must drop before renderer suspension"
    );

    fake.click_stage(&mut authority, "WL-2", ".discard-action")
        .unwrap();
    assert!(
        fake.transported_effects
            .contains(&EditorEffectKind::Discard)
    );
    assert_converged(&fake, &authority);
    assert!(fake.stages.is_empty());
    let closed_display_ids = fake
        .displays
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    assert!(!closed_display_ids.is_empty());
    let state = scorepeek_overlay::editor_sample_state();
    let display = fake
        .displays
        .values_mut()
        .next()
        .expect("closed editor must create a display surface");
    let paints_before_state = display.paint_count;
    display.render_skin(&state).unwrap();
    let frame = dispatch_native_event(display, Event::Frame).unwrap();
    display.turn(frame, 120).unwrap();
    assert!(
        display.paint_count > paints_before_state,
        "a state-driven skin tree update must reach native paint"
    );
    let paints_before_hide = display.paint_count;
    display
        .turn_visibility(NativeEventOutcome::default(), 120, false)
        .unwrap();
    assert_eq!(display.unmaps, 1);
    assert_eq!(display.paint_count, paints_before_hide);
    display
        .turn_visibility(
            NativeEventOutcome {
                frame: true,
                ..NativeEventOutcome::default()
            },
            120,
            false,
        )
        .unwrap();
    assert_eq!(display.unmaps, 1);
    assert_eq!(display.paint_count, paints_before_hide);
    display
        .turn_visibility(NativeEventOutcome::default(), 120, true)
        .unwrap();
    assert!(!display.surface_state.is_mapped());
    assert_eq!(display.paint_count, paints_before_hide);
    display
        .turn_visibility(
            NativeEventOutcome {
                frame: true,
                ..NativeEventOutcome::default()
            },
            120,
            true,
        )
        .unwrap();
    assert!(!display.surface_state.is_mapped());
    assert_eq!(display.paint_count, paints_before_hide);
    display
        .turn_visibility(
            NativeEventOutcome {
                configured: true,
                ..NativeEventOutcome::default()
            },
            120,
            true,
        )
        .unwrap();
    assert!(display.surface_state.is_mapped());
    assert!(display.paint_count > paints_before_hide);
    let paints_before_callbacks = display.paint_count;
    for expected in 1..=2 {
        let frame = dispatch_native_event(display, Event::Frame).unwrap();
        display.turn(frame, 120).unwrap();
        assert_eq!(display.paint_count, paints_before_callbacks + expected);
    }
    let paints_before_interrupted_remap = display.paint_count;
    let unmaps_before_interrupted_remap = display.unmaps;
    display
        .turn_visibility(NativeEventOutcome::default(), 120, false)
        .unwrap();
    display
        .turn_visibility(NativeEventOutcome::default(), 120, true)
        .unwrap();
    assert_eq!(
        display.surface_state,
        NativeDisplaySurfaceState::AwaitingConfigure
    );
    display
        .turn_visibility(NativeEventOutcome::default(), 120, false)
        .unwrap();
    assert_eq!(display.unmaps, unmaps_before_interrupted_remap + 2);
    display
        .turn_visibility(
            NativeEventOutcome {
                configured: true,
                ..NativeEventOutcome::default()
            },
            120,
            false,
        )
        .unwrap();
    assert_eq!(display.unmaps, unmaps_before_interrupted_remap + 3);
    display
        .turn_visibility(NativeEventOutcome::default(), 120, true)
        .unwrap();
    display
        .turn_visibility(
            NativeEventOutcome {
                configured: true,
                ..NativeEventOutcome::default()
            },
            120,
            true,
        )
        .unwrap();
    assert!(display.surface_state.is_mapped());
    assert!(display.paint_count > paints_before_interrupted_remap);
    for display in fake.displays.values() {
        let sample = display
            .work
            .frames
            .back()
            .expect("production display turn must retain its frame sample");
        assert_eq!(sample.live_canvases, 1);
        assert_eq!(sample.live_widgets, display.live_widgets);
        assert!(sample.phases["scene"].calls > 0);
        assert!(sample.phases["gpu_present"].calls > 0);
        assert!(sample.phases["surface_commit"].calls > 0);
        assert!(
            FrameWorkProfile::REQUIRED_PHASES
                .iter()
                .all(|phase| sample.phases.contains_key(phase))
        );
    }
    let reopen_canvas = authority
        .session()
        .draft
        .first()
        .map(|canvas| canvas.id.clone())
        .unwrap();
    for event in [
        Event::PointerMotion { x: 100.0, y: 100.0 },
        Event::PointerButton {
            button: 0x111,
            pressed: true,
            x: 100.0,
            y: 100.0,
        },
        Event::PointerButton {
            button: 0x111,
            pressed: false,
            x: 100.0,
            y: 100.0,
        },
    ] {
        fake.display_event(&reopen_canvas, event).unwrap();
        fake.apply(&mut authority).unwrap();
    }
    fake.apply(&mut authority).unwrap();
    assert_converged(&fake, &authority);
    assert_eq!(fake.stages.len(), 1);
    let operation_ids = |prefix: &str| {
        fake.operations
            .iter()
            .filter_map(|operation| operation.strip_prefix(prefix).map(str::to_owned))
            .filter(|id| closed_display_ids.contains(id))
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(operation_ids("stop-object:"), closed_display_ids);
    assert_eq!(operation_ids("runtime-drop:"), closed_display_ids);
    assert_eq!(operation_ids("unmap:"), closed_display_ids);
    assert_eq!(operation_ids("join:"), closed_display_ids);
    drop(fake);
    drop(store_guard);
}

#[test]
fn canvas_position_does_not_invalidate_skin_but_content_geometry_does() {
    let mut before = crate::config::visual_debug_config(cyan_skin())
        .canvases
        .into_iter()
        .find(|canvas| canvas.backend == crate::bridge::data::Backend::Wayland)
        .expect("visual debug config must contain a Wayland canvas")
        .presentation();
    let mut after = before.clone();
    after.x += 40;
    after.y += 24;
    assert!(!editor_skin_presentation_changed(&before, &after));

    after.width += 4;
    assert!(editor_skin_presentation_changed(&before, &after));

    after = before.clone();
    before
        .widgets
        .first_mut()
        .expect("visual debug canvas must contain a widget")
        .x += 4;
    assert!(editor_skin_presentation_changed(&after, &before));
}

#[test]
fn retained_skin_tree_can_restore_live_css_after_preview() {
    let scenario: VisualDebugScenario = serde_json::from_str(include_str!(
        "../../../../scorepeek-overlay/tests/fixtures/visual-composition.json"
    ))
    .unwrap();
    let session = VisualDebugSession::new(&scenario, [1920, 1080]).unwrap();
    let root = session
        .document
        .inner
        .borrow()
        .query_selector("#scorepeek-skin-root")
        .unwrap()
        .unwrap();
    let output = crate::skin::RenderOutput {
        schedule: crate::skin::Schedule::Idle,
        tree: crate::skin::Node::Element {
            key: "probe".into(),
            tag: "div".into(),
            attributes: std::collections::BTreeMap::from([(
                "id".into(),
                "native-css-probe".into(),
            )]),
            children: Vec::new(),
        },
    };
    let mut tree = crate::skin::NativeTree::new(
        &mut session.document.inner.borrow_mut(),
        root,
        "#native-css-probe { display: block; width: 80px; height: 20px; }",
    );
    tree.apply(&mut session.document.inner.borrow_mut(), &output);
    session.document.inner.borrow_mut().resolve(0.0);
    let width = |session: &VisualDebugSession| {
        let inner = session.document.inner.borrow();
        let probe = inner.query_selector("#native-css-probe").unwrap().unwrap();
        inner.get_client_bounding_rect(probe).unwrap().width
    };
    assert!((width(&session) - 80.0).abs() < f64::EPSILON);

    tree.set_css(
        &mut session.document.inner.borrow_mut(),
        "#native-css-probe { display: block; width: 160px; height: 20px; }",
    );
    session.document.inner.borrow_mut().resolve(0.0);
    assert!((width(&session) - 160.0).abs() < f64::EPSILON);
}

fn resize_widget(
    widget: &mut WidgetLayout,
    original: &WidgetLayout,
    start: [f64; 2],
    corner: ResizeCorner,
    x: f64,
    y: f64,
    canvas: [u32; 2],
) {
    let rect = scorepeek_overlay::editor_model::resize(
        [
            original.x,
            original.y,
            i32::try_from(original.width).unwrap_or(i32::MAX),
            i32::try_from(original.height).unwrap_or(i32::MAX),
        ],
        [snap_i32(x - start[0]), snap_i32(y - start[1])],
        Some(corner.name()),
        canvas,
        [16, 16],
        if original.kind == scorepeek_overlay::WidgetKind::Empty {
            original.settings.aspect_ratio
        } else {
            scorepeek_overlay::AspectRatio::Free
        },
    );
    widget.x = rect[0];
    widget.y = rect[1];
    widget.width = rect[2].unsigned_abs();
    widget.height = rect[3].unsigned_abs();
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResizeCorner {
    NorthWest,
    NorthEast,
    SouthWest,
    SouthEast,
}
impl ResizeCorner {
    fn name(self) -> &'static str {
        match self {
            Self::NorthWest => "nw",
            Self::NorthEast => "ne",
            Self::SouthWest => "sw",
            Self::SouthEast => "se",
        }
    }
}

#[test]
fn pointer_motion_delivers_actual_button_state() {
    use dioxus::html::input_data::MouseButton;
    type Moves = Rc<RefCell<Vec<bool>>>;
    fn probe(moves: Moves) -> Element {
        rsx! { div { style: "position:fixed;inset:0", onpointermove: move |event| moves.borrow_mut().push(event.held_buttons().contains(MouseButton::Primary)), "selectable text" } }
    }
    let moves = Moves::default();
    let mut document = DioxusDocument::new(
        VirtualDom::new_with_props(probe, moves.clone()),
        document_config(),
    );
    document.initial_build();
    document
        .inner
        .borrow_mut()
        .set_viewport(Viewport::new(100, 100, 1.0, ColorScheme::Dark));
    document.inner.borrow_mut().resolve(1.0);
    let mut pointer = PointerInput::default();
    pointer.dispatch(&mut document, [10.0, 10.0], 0x110, None);
    pointer.dispatch(&mut document, [10.0, 10.0], 0x110, Some(true));
    pointer.dispatch(&mut document, [11.0, 10.0], 0x110, None);
    pointer.dispatch(&mut document, [11.0, 10.0], 0x110, Some(false));
    pointer.dispatch(&mut document, [12.0, 10.0], 0x110, None);
    assert_eq!(*moves.borrow(), [false, true, false]);
}

#[test]
fn stage_shutdown_is_broadcast_before_any_worker_is_reaped() {
    struct Worker(Arc<std::sync::atomic::AtomicBool>);
    impl WorkerControl for Worker {
        fn request_stop(&self) {
            self.0.store(true, std::sync::atomic::Ordering::Release);
        }
    }
    let workers = (0..3)
        .map(|index| {
            (
                index.to_string(),
                Worker(Arc::new(std::sync::atomic::AtomicBool::new(false))),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let wakes = std::sync::Mutex::new(std::collections::BTreeMap::new());
    let ids = workers.keys().cloned().collect::<Vec<_>>();

    stop_workers(ids.iter(), &workers, &wakes);

    assert!(
        workers
            .values()
            .all(|worker| worker.0.load(std::sync::atomic::Ordering::Acquire))
    );
}

#[test]
fn editor_stage_is_removed_when_display_projection_replaces_it() {
    let desired = std::collections::BTreeSet::from(["wayland-canvas".to_owned()]);
    let projected = vec![crate::config::empty_canvas(
        "wayland-canvas".into(),
        crate::bridge::data::Backend::Wayland,
        cyan_skin(),
    )];

    assert!(worker_needs_replacement(
        "__scorepeek-editor-stage-0",
        Some("WL-1"),
        &desired,
        &projected,
        false,
    ));
}

#[test]
fn surface_unmap_failure_has_a_stable_worker_error_type() {
    assert_eq!(
        canvas_worker_error_type("unmap Wayland surface: connection closed"),
        "wayland_surface_unmap_failed"
    );
    assert_eq!(
        canvas_worker_error_type("gpu_adapter"),
        "canvas_worker_failed"
    );
}

#[test]
fn unmap_failure_is_primary_when_the_app_loop_also_failed() {
    let (result, secondary) = native_shutdown_result(
        Err("dispatch Wayland events: connection closed".into()),
        Err("connection closed".into()),
    );
    let primary = result.unwrap_err();

    assert_eq!(
        canvas_worker_error_type(&primary),
        "wayland_surface_unmap_failed"
    );
    assert_eq!(
        secondary.as_deref(),
        Some("dispatch Wayland events: connection closed")
    );
    assert_eq!(
        secondary.as_deref().map(canvas_worker_error_type),
        Some("canvas_worker_failed")
    );
}

#[test]
fn only_active_editor_stage_accepts_input() {
    assert!(surface_input_enabled(true, true, true));
    assert!(!surface_input_enabled(true, false, true));
    assert!(!surface_input_enabled(true, true, false));
    assert!(surface_input_enabled(false, false, true));
}

#[test]
fn editor_stages_are_output_owned_when_canvas_assignment_changes() {
    let mut canvases = crate::config::visual_debug_config(cyan_skin())
        .canvases
        .into_iter()
        .filter(|canvas| canvas.backend == crate::bridge::data::Backend::Wayland)
        .collect::<Vec<_>>();
    let outputs = vec![
        OutputDescription {
            name: "WL-1".into(),
            model: "Nested output 1".into(),
            logical_size: Some([1280, 720]),
        },
        OutputDescription {
            name: "WL-2".into(),
            model: "Nested output 2".into(),
            logical_size: Some([1920, 1080]),
        },
    ];
    let before = editor_stage_projections(&canvases, &outputs, canvases[0].skin);
    canvases[0].output = "WL-2".into();
    let after = editor_stage_projections(&canvases, &outputs, canvases[0].skin);

    assert_eq!(before, after);
    assert!(before.iter().all(|stage| stage.x == 0 && stage.y == 0));
    assert_eq!(
        before
            .iter()
            .map(|stage| (stage.id.as_str(), stage.output.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("__scorepeek-editor-stage-0", "WL-1"),
            ("__scorepeek-editor-stage-1", "WL-2"),
        ]
    );
}

#[test]
fn empty_geometry_uses_viewport_coordinates_for_every_visible_aperture() {
    let mut scenario: VisualDebugScenario = serde_json::from_str(include_str!(
        "../../../../scorepeek-overlay/tests/fixtures/visual-composition.json"
    ))
    .unwrap();
    scenario.editing = true;
    scenario.actions.clear();
    let session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
    let inner = session.document.inner.borrow();
    let geometries = inner.query_selector_all(".empty-geometry").unwrap();
    assert_eq!(geometries.len(), 4);
    let first = inner.get_client_bounding_rect(geometries[0]).unwrap();
    assert_eq!(
        (first.x, first.y, first.width, first.height),
        (40.0, 40.0, 1200.0, 680.0)
    );
}

#[test]
fn visual_debug_surface_contains_every_visible_canvas_in_one_stage_projection() {
    let scenario = VisualDebugScenario {
        canvases: None,
        skin: None,
        logical_size: [1920, 1080],
        scale: 1.0,
        canvas_id: Some("wayland-status".into()),
        editing: true,
        selectors: Vec::new(),
        actions: Vec::new(),
    };
    let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
    session
        .click(".screen-picker .list-picker-trigger")
        .unwrap();
    session
        .focus(".screen-picker .list-picker-trigger")
        .unwrap();
    session
        .click(".screen-picker .list-picker-option[data-index='4']")
        .unwrap();
    session.scroll(".navigator-scroll", 0.0, -2000.0).unwrap();
    session
        .click(".canvas-select[data-canvas-id='wayland-result']")
        .unwrap();

    let (expected_canvases, expected_widgets) = match &*session.projection.borrow() {
        NativeDocumentProjection::Editor(stage) => (
            stage.canvases.len(),
            stage
                .canvases
                .iter()
                .map(|canvas| canvas.widgets.len())
                .sum::<usize>(),
        ),
        NativeDocumentProjection::Display { .. } => panic!("expected editor projection"),
    };
    let inner = session.document.inner.borrow();
    let canvas_rects = inner
        .query_selector_all(".editor-canvas")
        .unwrap()
        .into_iter()
        .filter_map(|id| inner.get_client_bounding_rect(id))
        .filter(|rect| rect.width > 0.0 && rect.height > 0.0)
        .count();
    let widget_rects = inner
        .query_selector_all(".editor-widget-hit")
        .unwrap()
        .into_iter()
        .filter_map(|id| inner.get_client_bounding_rect(id))
        .filter(|rect| rect.width > 0.0 && rect.height > 0.0)
        .count();
    let rendered_skin_roots = inner
        .query_selector_all(".scorepeek-skin-scope .overlay-canvas")
        .unwrap()
        .len();
    assert_eq!(canvas_rects, expected_canvases);
    assert_eq!(widget_rects, expected_widgets);
    assert_eq!(rendered_skin_roots, expected_canvases);
}

#[test]
fn visual_debug_canvas_delete_drops_runtime_tree_and_dom_together() {
    let scenario = VisualDebugScenario {
        canvases: None,
        skin: None,
        logical_size: [1920, 1080],
        scale: 1.0,
        canvas_id: Some("wayland-status".into()),
        editing: true,
        selectors: Vec::new(),
        actions: Vec::new(),
    };
    let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
    session
        .authority
        .dispatch(EditorInput::Action(EditorAction::DeleteCanvas));
    let output = match &*session.projection.borrow() {
        NativeDocumentProjection::Editor(stage) => stage.output.clone(),
        NativeDocumentProjection::Display { .. } => panic!("expected editor stage"),
    };
    session.projection.set(NativeDocumentProjection::Editor(
        session.authority.session().stage_projection(&output),
    ));
    session.resolve();

    let expected = match &*session.projection.borrow() {
        NativeDocumentProjection::Editor(stage) => stage.canvases.len(),
        NativeDocumentProjection::Display { .. } => unreachable!(),
    };
    assert_eq!(session.skins.len(), expected);
    assert_eq!(
        session
            .document
            .inner
            .borrow()
            .query_selector_all(".scorepeek-skin-scope .overlay-canvas")
            .unwrap()
            .len(),
        expected
    );
}

#[test]
fn visual_debug_new_canvas_mounts_its_skin_on_the_same_reactive_turn() {
    let scenario = VisualDebugScenario {
        canvases: None,
        skin: None,
        logical_size: [1920, 1080],
        scale: 1.0,
        canvas_id: Some("wayland-status".into()),
        editing: true,
        selectors: Vec::new(),
        actions: Vec::new(),
    };
    let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
    loop {
        let id = {
            session
                .authority
                .session()
                .draft
                .first()
                .map(|canvas| canvas.id.clone())
        };
        let Some(id) = id else { break };
        session
            .authority
            .dispatch(EditorInput::Action(EditorAction::SelectCanvas(id)));
        session
            .authority
            .dispatch(EditorInput::Action(EditorAction::DeleteCanvas));
    }
    session
        .authority
        .dispatch(EditorInput::Action(EditorAction::AddCanvas));
    let output = session.authority.session().outputs[0].clone();
    session.projection.set(NativeDocumentProjection::Editor(
        session.authority.session().stage_projection(&output),
    ));

    session.resolve();

    assert_eq!(session.skins.len(), 1);
    assert_eq!(
        session
            .document
            .inner
            .borrow()
            .query_selector_all(".scorepeek-skin-scope .overlay-canvas")
            .unwrap()
            .len(),
        1,
        "projection, Dioxus root creation, and skin mount must complete without another input"
    );
}

#[test]
fn visual_debug_role_transition_remounts_skin_content_in_the_display_root() {
    let scenario = VisualDebugScenario {
        canvases: None,
        skin: None,
        logical_size: [1920, 1080],
        scale: 1.0,
        canvas_id: Some("wayland-status".into()),
        editing: true,
        selectors: Vec::new(),
        actions: Vec::new(),
    };
    let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();

    session.set_editing(false);

    assert_eq!(
        session
            .document
            .inner
            .borrow()
            .query_selector_all("#scorepeek-skin-root .overlay-canvas")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn display_context_menu_enters_through_the_shared_dioxus_surface_action() {
    let scenario = VisualDebugScenario {
        canvases: None,
        skin: None,
        logical_size: [1920, 1080],
        scale: 1.0,
        canvas_id: Some("wayland-status".into()),
        editing: true,
        selectors: Vec::new(),
        actions: Vec::new(),
    };
    let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
    session.set_editing(false);
    while session.commands.try_recv().is_ok() {}
    session
        .pointer
        .dispatch(&mut session.document, [960.0, 540.0], 0x111, None);
    session
        .pointer
        .dispatch(&mut session.document, [960.0, 540.0], 0x111, Some(true));
    session
        .pointer
        .dispatch(&mut session.document, [960.0, 540.0], 0x111, Some(false));
    while poll_native_document(&mut session.document, Waker::noop()) {}

    assert!(
        std::iter::from_fn(|| session.commands.try_recv().ok())
            .any(|command| matches!(command, CoordinatorCommand::Open { .. }))
    );
}

#[test]
fn native_keyboard_edits_the_focused_dioxus_number_field() {
    let scenario = VisualDebugScenario {
        canvases: None,
        skin: None,
        logical_size: [1920, 1080],
        scale: 1.0,
        canvas_id: Some("wayland-status".into()),
        editing: true,
        selectors: Vec::new(),
        actions: Vec::new(),
    };
    let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
    session.click(".editor-number-field").unwrap();
    session.key(&scorepeek_overlay_wayland_handles::TextCommand::SelectAll);
    session.key(&scorepeek_overlay_wayland_handles::TextCommand::Insert(
        "16".into(),
    ));
    session.key(&scorepeek_overlay_wayland_handles::TextCommand::Accept);

    assert_eq!(session.authority.session().current().unwrap().x, 16);
    assert!(session.authority.session().chrome.field_drafts.is_empty());
}

#[test]
fn native_keyboard_uses_the_shared_title_input_contract() {
    let scenario = VisualDebugScenario {
        canvases: None,
        skin: None,
        logical_size: [1920, 1080],
        scale: 1.0,
        canvas_id: Some("wayland-status".into()),
        editing: true,
        selectors: Vec::new(),
        actions: Vec::new(),
    };
    let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
    session
        .click(".widget-picker .list-picker-trigger")
        .unwrap();
    session
        .click(".widget-picker .list-picker-option[data-index='5']")
        .unwrap();
    session.click(".empty-title-input").unwrap();
    session.click("#editor-title-input").unwrap();
    session.key(&scorepeek_overlay_wayland_handles::TextCommand::Insert(
        "手元".into(),
    ));
    session.key(&scorepeek_overlay_wayland_handles::TextCommand::Accept);

    let model = session.authority.session();
    let widget = model
        .current()
        .unwrap()
        .widgets
        .iter()
        .find(|widget| widget.kind == scorepeek_overlay::WidgetKind::Empty)
        .unwrap();
    assert_eq!(widget.settings.title, "手元");
    assert!(model.title.is_none());
}

#[test]
fn native_ime_batch_uses_browser_order_and_shared_composition_state() {
    let scenario = VisualDebugScenario {
        canvases: None,
        skin: None,
        logical_size: [1920, 1080],
        scale: 1.0,
        canvas_id: Some("wayland-status".into()),
        editing: true,
        selectors: Vec::new(),
        actions: Vec::new(),
    };
    let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
    session
        .click(".widget-picker .list-picker-trigger")
        .unwrap();
    session
        .click(".widget-picker .list-picker-option[data-index='5']")
        .unwrap();
    session.click(".empty-title-input").unwrap();
    session.focus("#editor-title-input").unwrap();

    session.ime(scorepeek_overlay_wayland_handles::TextUpdate {
        commit: Some("日本".into()),
        ..Default::default()
    });
    assert_eq!(
        session.authority.session().title.as_ref().unwrap().text,
        "日本"
    );

    session.ime(scorepeek_overlay_wayland_handles::TextUpdate {
        preedit: Some("ほん".into()),
        preedit_cursor: [6, 6],
        ..Default::default()
    });
    assert!(
        session
            .authority
            .session()
            .title
            .as_ref()
            .unwrap()
            .composing
    );

    session.ime(scorepeek_overlay_wayland_handles::TextUpdate {
        commit: Some("語".into()),
        preedit: Some(String::new()),
        delete_before: 3,
        ..Default::default()
    });
    let model = session.authority.session();
    let title = model.title.as_ref().unwrap();
    assert_eq!(title.text, "日語");
    assert!(!title.composing);
}

#[test]
fn native_ime_targets_the_focused_shared_text_control() {
    let scenario = VisualDebugScenario {
        canvases: None,
        skin: None,
        logical_size: [1920, 1080],
        scale: 1.0,
        canvas_id: Some("wayland-status".into()),
        editing: true,
        selectors: Vec::new(),
        actions: Vec::new(),
    };
    let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
    session.click(".editor-text-field").unwrap();
    session.key(&scorepeek_overlay_wayland_handles::TextCommand::SelectAll);
    {
        let document = session.document.inner.borrow();
        let node = document.get_focussed_node_id().unwrap();
        let input = document
            .get_node(node)
            .unwrap()
            .element_data()
            .unwrap()
            .text_input_data()
            .unwrap();
        assert_eq!(input.editor.raw_selection().text_range(), 0..6);
    }
    session.ime(scorepeek_overlay_wayland_handles::TextUpdate {
        preedit: Some("はいしん".into()),
        preedit_cursor: [12, 12],
        ..Default::default()
    });
    {
        let document = session.document.inner.borrow();
        let node = document.get_focussed_node_id().unwrap();
        let input = document
            .get_node(node)
            .unwrap()
            .element_data()
            .unwrap()
            .text_input_data()
            .unwrap();
        assert_eq!(input.editor.raw_text(), "はいしん");
    }
    assert!(
        session
            .authority
            .session()
            .chrome
            .field_drafts
            .values()
            .any(|draft| draft.composing)
    );
    session.ime(scorepeek_overlay_wayland_handles::TextUpdate {
        commit: Some("配信画面".into()),
        preedit: Some(String::new()),
        ..Default::default()
    });

    assert_eq!(
        session.authority.session().current().unwrap().name,
        "配信画面"
    );
    assert!(
        session
            .authority
            .session()
            .chrome
            .field_drafts
            .values()
            .all(|draft| !draft.composing)
    );
}

#[test]
fn native_skin_property_draft_survives_rebuild_and_commits_through_shared_state() {
    let scenario = VisualDebugScenario {
        canvases: None,
        skin: None,
        logical_size: [1920, 1080],
        scale: 1.0,
        canvas_id: Some("wayland-status".into()),
        editing: true,
        selectors: Vec::new(),
        actions: Vec::new(),
    };
    let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
    session
        .scroll(
            ".workspace-output-option .navigator-item-select",
            0.0,
            -120.0,
        )
        .unwrap();
    session
        .click(".widget-row[data-widget-id='status']")
        .unwrap();
    assert_eq!(
        session.authority.session().selected_widget.as_deref(),
        Some("status"),
        "the native navigator click must select the status widget"
    );
    session.scroll(".inspector-scroll", 0.0, 1200.0).unwrap();
    session.focus(".property-value-input").unwrap();
    session.key(&scorepeek_overlay_wayland_handles::TextCommand::SelectAll);
    session.key(&scorepeek_overlay_wayland_handles::TextCommand::Insert(
        "999".into(),
    ));
    assert!(
        session
            .authority
            .session()
            .chrome
            .field_drafts
            .values()
            .any(|draft| draft.text == "999" && !draft.valid)
    );
    session.click(".editor-panel-toggle").unwrap();
    session.click(".editor-panel-toggle").unwrap();

    assert!(
        session
            .authority
            .session()
            .chrome
            .field_drafts
            .values()
            .any(|draft| draft.text == "999" && !draft.valid)
    );

    session.focus(".property-value-input").unwrap();
    session.key(&scorepeek_overlay_wayland_handles::TextCommand::SelectAll);
    session.key(&scorepeek_overlay_wayland_handles::TextCommand::Insert(
        "25".into(),
    ));
    session.key_composing(
        &scorepeek_overlay_wayland_handles::TextCommand::Accept,
        true,
    );
    assert!(
        session
            .authority
            .session()
            .chrome
            .field_drafts
            .values()
            .any(|draft| draft.text == "25")
    );
    session.key(&scorepeek_overlay_wayland_handles::TextCommand::Accept);

    let model = session.authority.session();
    assert!(model.chrome.field_drafts.is_empty());
    let widget = model
        .current()
        .unwrap()
        .widgets
        .iter()
        .find(|widget| widget.id == "status")
        .unwrap();
    assert_eq!(
        widget
            .skin_properties
            .get("fill-opacity-percent")
            .and_then(serde_json::Value::as_i64),
        Some(25)
    );
}

#[test]
fn native_keyboard_drives_the_shared_list_picker_contract() {
    let scenario = VisualDebugScenario {
        canvases: None,
        skin: None,
        logical_size: [1920, 1080],
        scale: 1.0,
        canvas_id: Some("wayland-status".into()),
        editing: true,
        selectors: Vec::new(),
        actions: Vec::new(),
    };
    let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
    let point = {
        let inner = session.document.inner.borrow();
        let trigger = inner
            .query_selector(".screen-picker .list-picker-trigger")
            .unwrap()
            .unwrap();
        let rect = inner.get_client_bounding_rect(trigger).unwrap();
        [rect.x + rect.width / 2.0, rect.y + rect.height / 2.0]
    };
    session
        .pointer
        .dispatch_blitz(&mut session.document, point, 0x110, None);
    session
        .pointer
        .dispatch_blitz(&mut session.document, point, 0x110, Some(true));
    session
        .pointer
        .dispatch_blitz(&mut session.document, point, 0x110, Some(false));
    session.resolve();
    let raw_trigger = session
        .document
        .inner
        .borrow()
        .query_selector(".screen-picker .list-picker-trigger")
        .unwrap()
        .unwrap();
    assert_ne!(
        session.document.inner.borrow().get_focussed_node_id(),
        Some(raw_trigger),
        "raw Blitz does not implement the browser button-click focus default"
    );

    let mut session = VisualDebugSession::new(&scenario, scenario.logical_size).unwrap();
    session
        .click(".screen-picker .list-picker-trigger")
        .unwrap();
    let trigger = session
        .document
        .inner
        .borrow()
        .query_selector(".screen-picker .list-picker-trigger")
        .unwrap()
        .unwrap();
    assert_eq!(
        session.document.inner.borrow().get_focussed_node_id(),
        Some(trigger)
    );
    session.key(&scorepeek_overlay_wayland_handles::TextCommand::Down);
    let trigger = session
        .document
        .inner
        .borrow()
        .query_selector(".screen-picker .list-picker-trigger")
        .unwrap()
        .unwrap();
    assert_eq!(
        session.document.inner.borrow().get_focussed_node_id(),
        Some(trigger)
    );
    session.key(&scorepeek_overlay_wayland_handles::TextCommand::Accept);

    assert_eq!(
        session.authority.session().preview,
        scorepeek_overlay::ScreenKind::ModeSelect
    );
    assert!(!session.authority.session().chrome.screen_picker_open);
}

#[test]
fn every_native_scene_retains_an_image_atlas_generation() {
    let mut scene = anyrender::Scene::new();
    retain_native_image_atlas(&mut scene);
    assert!(matches!(
        scene.commands.as_slice(),
        [anyrender::recording::RenderCommand::Fill(command)]
            if matches!(command.brush, anyrender::Paint::Image(_))
    ));
}

#[test]
fn compact_canvas_editor_expands_inside_the_output() {
    assert_eq!(
        editor_geometry([1700, 1000], [560, 72], Some([1920, 1080])),
        ([0, 0], [1920, 1080])
    );
    assert_eq!(
        editor_geometry([20, 20], [800, 640], Some([1920, 1080])),
        ([0, 0], [1920, 1080])
    );
    assert_eq!(editor_panel_width(Some(1920)), 384);
    assert_eq!(editor_panel_width(Some(5120)), 480);
    assert_eq!(editor_panel_width(Some(1280)), 360);
    assert_eq!(editor_panel_width(None), 400);
    assert_eq!(grid_floor(1366), 1364);
    assert_eq!(maximum_grid_position(1366, 560), 804);
}

#[test]
fn aggregate_canvas_visibility_preserves_explicit_screen_membership() {
    use scorepeek_overlay::editor_model::SCREENS;
    let mut canvas = crate::config::visual_debug_config(cyan_skin()).canvases[0].presentation();
    canvas.show_on = None;
    let mut model = EditorSession::new(vec![canvas.clone()], [1920, 1080], "wayland");
    model.readonly = false;
    for screen in SCREENS {
        model.action(&EditorAction::CanvasVisible(screen, false));
    }
    assert_eq!(model.draft[0].show_on, Some(Vec::new()));
    for screen in SCREENS {
        model.action(&EditorAction::CanvasVisible(screen, true));
    }
    assert_eq!(model.draft[0].show_on, Some(SCREENS.to_vec()));
}

#[test]
fn minimum_widget_resizes_from_every_corner() {
    for (x, y) in [(8, 8), (-24, -24), (40, 40)] {
        let original = WidgetLayout {
            id: "minimum".into(),
            kind: scorepeek_overlay::WidgetKind::Empty,
            x,
            y,
            width: 16,
            height: 16,
            settings: scorepeek_overlay::WidgetSettings::default(),
            skin_properties: std::collections::BTreeMap::new(),
        };
        for corner in [
            ResizeCorner::NorthWest,
            ResizeCorner::NorthEast,
            ResizeCorner::SouthWest,
            ResizeCorner::SouthEast,
        ] {
            let mut widget = original.clone();
            resize_widget(
                &mut widget,
                &original,
                [0.0, 0.0],
                corner,
                4.0,
                4.0,
                [32, 32],
            );
            assert!(widget.width >= 16 && widget.height >= 16);
            assert!(widget.width <= 64 && widget.height <= 64);
        }
    }
}
#[test]
fn locked_empty_resize_keeps_ratio_at_minimum_and_canvas_bounds() {
    for (mode, ratio) in [
        (scorepeek_overlay::AspectRatio::Wide, 16.0 / 9.0),
        (scorepeek_overlay::AspectRatio::Standard, 4.0 / 3.0),
        (scorepeek_overlay::AspectRatio::Current([1, 8]), 0.125),
        (scorepeek_overlay::AspectRatio::Current([8, 1]), 8.0),
    ] {
        let mut original = WidgetLayout {
            id: "empty".into(),
            kind: scorepeek_overlay::WidgetKind::Empty,
            x: 0,
            y: 0,
            width: 640,
            height: 360,
            settings: scorepeek_overlay::WidgetSettings::default(),
            skin_properties: std::collections::BTreeMap::new(),
        };
        original.x = 40;
        original.y = 40;
        original.width = 640;
        original.height = 360;
        original.settings.aspect_ratio = mode;
        for corner in [
            ResizeCorner::NorthWest,
            ResizeCorner::NorthEast,
            ResizeCorner::SouthWest,
            ResizeCorner::SouthEast,
        ] {
            for delta in [-4000.0, 4000.0] {
                let mut widget = original.clone();
                resize_widget(
                    &mut widget,
                    &original,
                    [0.0, 0.0],
                    corner,
                    delta,
                    delta,
                    [1920, 1080],
                );
                assert!(widget.width >= 16 && widget.height >= 16);
                assert!(widget.x >= 0 && widget.y >= 0);
                assert!(i64::from(widget.x) + i64::from(widget.width) <= 1920);
                assert!(i64::from(widget.y) + i64::from(widget.height) <= 1080);
                let error = if ratio >= 1.0 {
                    (f64::from(widget.width) / ratio - f64::from(widget.height)).abs()
                } else {
                    (f64::from(widget.height) * ratio - f64::from(widget.width)).abs()
                };
                assert!(error <= 4.0, "{mode:?}: {}x{}", widget.width, widget.height);
            }
        }
    }
}
