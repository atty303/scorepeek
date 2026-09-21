use super::bootstrap::read_initial;
use super::replica::EditorSession;
use crate::browser::pointer;
use crate::browser::post_message::{encode_id, publish_canvas_replica};
use crate::browser::viewport::{ResizeListener, viewport};
use crate::transport::connection::Connection;
use crate::transport::reconnect::Compatibility;
use dioxus::prelude::*;
use scorepeek_overlay::CanvasPresentation;
use scorepeek_overlay::editor::{EditorAction, EditorOutput, EditorPanel};
use scorepeek_overlay::editor_model::EditorInput;
use scorepeek_overlay::editor_runtime::use_editor_runtime;
use scorepeek_overlay::editor_surface::{
    EditorCanvas, EditorSelectionMetrics, EditorSurface, PlacementPreview, SurfaceAction,
};
use std::rc::Rc;

#[allow(clippy::too_many_lines)]
pub fn app() -> Element {
    let runtime = use_editor_runtime(|| {
        let (canvases, skins) = read_initial();
        let mut model = EditorSession::new(canvases, viewport(), "obs");
        model.set_session_id(1);
        model.set_skins(skins);
        model.set_outputs(vec![EditorOutput {
            name: "obs-output".into(),
            model: "OBS Browser Source".into(),
            logical_size: Some(viewport()),
        }]);
        model.readonly = true;
        model
    });
    let dispatch = runtime.dispatch;
    let compatibility = use_signal(|| Compatibility::Checking);
    let connection = use_hook(move || Connection::new(dispatch, compatibility));
    let _resize = use_hook(move || Rc::new(ResizeListener::new(dispatch)));
    let transport = connection.clone();
    let action = Callback::new(move |action: EditorAction| {
        if compatibility() != Compatibility::Ready {
            return;
        }
        for effect in dispatch.call(EditorInput::Action(action)) {
            transport.send(&effect);
        }
    });
    let transport = connection.clone();
    let surface = Callback::new(move |action: SurfaceAction| {
        let compatibility = compatibility();
        if compatibility != Compatibility::Ready
            && !(compatibility == Compatibility::Checking
                && matches!(action, SurfaceAction::Enter(_)))
        {
            return;
        }
        for effect in dispatch.call(EditorInput::Surface(action)) {
            transport.send(&effect);
        }
    });
    let capture = Callback::new(move |event: PointerEvent| pointer::capture(&event));
    let stages = runtime.stages;
    use_effect(move || {
        if let Some(projection) = stages.read().first() {
            for canvas in &projection.canvases {
                if let Some(specification) =
                    canvas_replica_specification(canvas, &projection.view.skins)
                {
                    publish_canvas_replica(
                        &canvas.id,
                        &specification,
                        projection.session_id,
                        projection.revision,
                    );
                }
            }
        }
    });
    let projection = runtime.stages.read().first().cloned();
    rsx! {
        EditorSurface {onaction:surface,
            div { id:"stage",
                if let Some(projection)=&projection {
                for canvas in &projection.canvases {
                    EditorCanvas {key:"{canvas.id}",canvas:canvas.clone(),editing:projection.interactive,selected:projection.selected_canvas.as_ref().is_some_and(|selected|selected.id==canvas.id),selected_widget:projection.selected_widget.clone(),onaction:surface,oncapture:capture,
                        iframe {
                            id: "scorepeek-replica-{encode_id(&canvas.id)}",
                            "data-replica-canvas": "{canvas.id}",
                            src:format!("/canvas/{}?editor=1&sample={}&skin={}",encode_id(&canvas.id),u8::from(projection.interactive&&projection.view.chrome.sample),encode_id(canvas.skin.name())),
                            tabindex:-1,
                            onload: {
                                let canvas_id = canvas.id.clone();
                                let specification = canvas_replica_specification(canvas, &projection.view.skins);
                                let session_id = projection.session_id;
                                let revision = projection.revision;
                                move |_| if let Some(specification) = specification.clone() {
                                    publish_canvas_replica(&canvas_id, &specification, session_id, revision);
                                }
                            }
                        }
                    }
                }
                if let Some(canvas) = &projection.selected_canvas {
                    EditorSelectionMetrics { canvas:canvas.clone(), selected_widget: projection.selected_widget.clone() }
                }
                }
            }
            if let Some(projection)=&projection {
                if projection.interactive {
                    EditorPanel {view:runtime.inspector.read().clone(),title:projection.title.clone(),onaction:action}
                    if let Some(kind)=projection.placing {
                        if let Some(size) = projection.view.skins.iter().find(|skin| projection.selected_canvas.as_ref().is_some_and(|canvas| canvas.skin == skin.id)).and_then(|skin| skin.widget_defaults.get(kind.name())).map(|default| [default.width, default.height]) {
                            PlacementPreview {kind,point:projection.point.map(f64::from),size}
                        }
                    }
                }
                if let Some(notice)=&projection.notice {
                    div {id:"notice",class:"show error","{notice}"}
                }
            }
            if compatibility() == Compatibility::Mismatch {
                div { class:"version-mismatch", role:"alert",
                    h2 { "UIを自動更新できませんでした" }
                    p { "未保存の変更は破棄されました。再読み込みして、保存済み設定からやり直してください。" }
                    scorepeek_overlay::editor::Button {
                        tone:scorepeek_overlay::editor::ButtonTone::Primary,
                        onclick:move |_| {if let Some(window)=web_sys::window(){let _=window.location().reload();}},
                        "再読み込み"
                    }
                }
            }
        }
    }
}

fn canvas_replica_specification(
    canvas: &CanvasPresentation,
    skins: &[scorepeek_overlay::editor::EditorSkin],
) -> Option<serde_json::Value> {
    let skin = skins.iter().find(|skin| skin.id == canvas.skin)?;
    let canvas_properties = skin
        .canvas_properties
        .iter()
        .map(|(key, property)| {
            (
                key.clone(),
                property.effective(canvas.skin_properties.get(key)),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let widgets = canvas
        .widgets
        .iter()
        .map(|widget| {
            let properties = skin
                .widget_properties
                .get(widget.kind.name())
                .or_else(|| skin.widget_properties.get("*"))
                .map(|definitions| {
                    definitions
                        .iter()
                        .map(|(key, property)| {
                            (
                                key.clone(),
                                property.effective(widget.skin_properties.get(key)),
                            )
                        })
                        .collect::<std::collections::BTreeMap<_, _>>()
                })
                .unwrap_or_default();
            serde_json::json!({
                "id": widget.id,
                "kind": widget.kind,
                "x": widget.x,
                "y": widget.y,
                "width": widget.width,
                "height": widget.height,
                "settings": widget.settings,
                "properties": properties,
            })
        })
        .collect::<Vec<_>>();
    Some(serde_json::json!({
        "canvas": {
            "id": canvas.id,
            "skin": canvas.skin,
            "width": canvas.width,
            "height": canvas.height,
            "properties": canvas_properties,
        },
        "widgets": widgets,
        "wasm": format!("/skin/{}/{}", canvas.skin.name(), "skin.wasm"),
    }))
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use scorepeek_overlay::editor::{EditorProperty, EditorSkin};
    use scorepeek_overlay::{WidgetKind, WidgetLayout, WidgetSettings};

    #[test]
    fn iframe_replica_specification_contains_effective_properties_and_complete_geometry() {
        let skin = EditorSkin {
            id: "dev.example.skin".parse().unwrap(),
            name: "test".into(),
            release: "1.0.0".into(),
            preview: String::new(),
            preview_video: None,
            widget_defaults: std::collections::BTreeMap::new(),
            canvas_properties: std::collections::BTreeMap::from([
                (
                    "tint".into(),
                    EditorProperty::Color {
                        default: "#ffffff".into(),
                    },
                ),
                (
                    "background".into(),
                    EditorProperty::Enum {
                        default: "none".into(),
                        values: vec!["none".into(), "static".into(), "animated".into()],
                    },
                ),
            ]),
            widget_properties: std::collections::BTreeMap::from([(
                "empty".into(),
                std::collections::BTreeMap::from([(
                    "amount".into(),
                    EditorProperty::Integer {
                        default: 1,
                        minimum: 0,
                        maximum: 10,
                    },
                )]),
            )]),
        };
        let mut canvas = CanvasPresentation {
            id: "canvas-1".into(),
            name: "Canvas 1".into(),
            skin: "dev.example.skin".parse().unwrap(),
            skin_properties: std::collections::BTreeMap::from([
                ("tint".into(), serde_json::json!("#112233")),
                ("background".into(), serde_json::json!("static")),
            ]),
            show_on: None,
            opacity_percent: 100,
            output: Some("OBS".into()),
            x: 0,
            y: 0,
            width: 640,
            height: 360,
            widgets: vec![WidgetLayout {
                id: "widget-1".into(),
                kind: WidgetKind::Empty,
                x: 12,
                y: 16,
                width: 120,
                height: 80,
                settings: WidgetSettings::default(),
                skin_properties: std::collections::BTreeMap::from([(
                    "amount".into(),
                    serde_json::json!(7),
                )]),
            }],
        };

        let customized =
            canvas_replica_specification(&canvas, std::slice::from_ref(&skin)).unwrap();
        assert_eq!(customized["canvas"]["properties"]["tint"], "#112233");
        assert_eq!(customized["canvas"]["properties"]["background"], "static");
        assert_eq!(customized["widgets"][0]["properties"]["amount"], 7);
        assert_eq!(customized["widgets"][0]["x"], 12);

        canvas.skin_properties.clear();
        canvas.widgets[0].skin_properties.clear();
        let reset = canvas_replica_specification(&canvas, &[skin]).unwrap();
        assert_eq!(reset["canvas"]["properties"]["tint"], "#ffffff");
        assert_eq!(reset["widgets"][0]["properties"]["amount"], 1);
    }
}
