mod connection;
mod model;
use connection::{Compatibility, Connection};
use dioxus::prelude::*;
use dioxus_web::WebEventExt;
use model::EditorSession;
use scorepeek_overlay_ui::CanvasPresentation;
use scorepeek_overlay_ui::editor::{EditorAction, EditorOutput, EditorPanel};
use scorepeek_overlay_ui::editor_model::EditorInput;
use scorepeek_overlay_ui::editor_runtime::use_editor_runtime;
use scorepeek_overlay_ui::editor_surface::{
    EditorCanvas, EditorSelectionMetrics, EditorSurface, PlacementPreview, SurfaceAction,
};
use std::rc::Rc;
use wasm_bindgen::{JsCast as _, closure::Closure};

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
        model.editing = true;
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
        if compatibility() != Compatibility::Ready {
            return;
        }
        for effect in dispatch.call(EditorInput::Surface(action)) {
            transport.send(&effect);
        }
    });
    let capture = Callback::new(move |event: PointerEvent| {
        let raw = event.data().as_web_event();
        if let Some(target) = raw
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        {
            let _ = target.set_pointer_capture(raw.pointer_id());
        }
    });
    let projection = runtime.stages.read().first().cloned();
    rsx! {
        EditorSurface {onaction:surface,
            div { id:"stage",
                if let Some(projection)=&projection {
                for canvas in &projection.canvases {
                    EditorCanvas {key:"{canvas.id}",canvas:canvas.clone(),editing:projection.interactive,selected:projection.selected_canvas.as_ref().is_some_and(|selected|selected.id==canvas.id),selected_widget:projection.selected_widget.clone(),onaction:surface,oncapture:capture,
                        iframe {src:format!("/canvas/{}?sample={}&skin={}",encode_id(&canvas.id),u8::from(projection.interactive&&projection.view.chrome.sample),encode_id(canvas.skin.name())),tabindex:-1}
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
                        PlacementPreview {kind,point:projection.point.map(f64::from)}
                    }
                }
                if let Some(notice)=&projection.notice {
                    div {id:"notice",class:"show error","{notice}"}
                }
            }
            if compatibility() == Compatibility::Mismatch {
                div { class:"version-mismatch", role:"alert",
                    h2 { "UIが更新されました" }
                    p { "未保存の変更は破棄されました。再読み込みして、保存済み設定からやり直してください。" }
                    scorepeek_overlay_ui::editor::Button {
                        tone:scorepeek_overlay_ui::editor::ButtonTone::Primary,
                        onclick:move |_| {if let Some(window)=web_sys::window(){let _=window.location().reload();}},
                        "再読み込み"
                    }
                }
            }
        }
    }
}

fn encode_id(id: &str) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::new();
    for byte in id.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}
fn read_initial() -> (
    Vec<CanvasPresentation>,
    Vec<scorepeek_overlay_ui::editor::EditorSkin>,
) {
    let document = web_sys::window().and_then(|window| window.document());
    let read = |id: &str| {
        document
            .as_ref()
            .and_then(|doc| doc.get_element_by_id(id))
            .and_then(|node| node.text_content())
    };
    (
        read("scorepeek-stage")
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default(),
        read("scorepeek-skins")
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default(),
    )
}
fn viewport() -> [u32; 2] {
    web_sys::window()
        .and_then(|window| window.document())
        .and_then(|doc| doc.document_element())
        .map_or([1920, 1080], |element| {
            [
                u32::try_from(element.client_width()).unwrap_or(1920),
                u32::try_from(element.client_height()).unwrap_or(1080),
            ]
        })
}
struct ResizeListener(Closure<dyn FnMut(web_sys::Event)>);
impl ResizeListener {
    fn new(
        dispatch: Callback<EditorInput, Vec<scorepeek_overlay_ui::editor_model::EditorEffect>>,
    ) -> Self {
        let callback = Closure::wrap(Box::new(move |_: web_sys::Event| {
            let _ = dispatch.call(EditorInput::Resize {
                output: "obs-output".into(),
                logical_size: viewport(),
            });
        }) as Box<dyn FnMut(_)>);
        if let Some(window) = web_sys::window() {
            let _ = window
                .add_event_listener_with_callback("resize", callback.as_ref().unchecked_ref());
        }
        Self(callback)
    }
}
impl Drop for ResizeListener {
    fn drop(&mut self) {
        if let Some(window) = web_sys::window() {
            let _ = window
                .remove_event_listener_with_callback("resize", self.0.as_ref().unchecked_ref());
        }
    }
}
