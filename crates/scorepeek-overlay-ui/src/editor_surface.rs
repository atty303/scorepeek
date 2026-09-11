use crate::{CanvasPresentation, WidgetKind, editor::ResizeHandles};
use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;

#[derive(Clone, Debug)]
pub enum SurfaceAction {
    Enter(Option<String>),
    Select(String),
    Start {
        canvas: String,
        widget: Option<String>,
        corner: Option<String>,
        point: [i32; 2],
    },
    Move([i32; 2]),
    End,
    Cancel,
    Place([i32; 2]),
}
fn point(event: &PointerEvent) -> [i32; 2] {
    let point = event.client_coordinates().to_i32();
    [point.x, point.y]
}

#[component]
pub fn EditorSelectionMetrics(
    canvas: CanvasPresentation,
    selected_widget: Option<String>,
) -> Element {
    let (label, x, y, width, height) = selected_widget
        .as_deref()
        .and_then(|id| canvas.widgets.iter().find(|widget| widget.id == id))
        .map_or_else(
            || {
                (
                    canvas.name.clone(),
                    canvas.x,
                    canvas.y,
                    canvas.width,
                    canvas.height,
                )
            },
            |widget| {
                (
                    crate::editor::widget_label(widget, &canvas.widgets),
                    canvas.x.saturating_add(widget.x),
                    canvas.y.saturating_add(widget.y),
                    widget.width,
                    widget.height,
                )
            },
        );
    rsx! {
        div {
            class: "selection-metrics",
            aria_hidden: "true",
            span { class: "selection-label", "{label}" }
            span { aria_hidden: "true", " · " }
            span { class: "selection-geometry", "{x},{y} · {width}×{height}" }
        }
    }
}

#[component]
pub fn EditorSurface(onaction: EventHandler<SurfaceAction>, children: Element) -> Element {
    rsx! { crate::OverlayStyles {}
    div { class:"editor-surface",
        onkeydown:move |event|{if event.key()==Key::Escape {onaction.call(SurfaceAction::Cancel);}},
        onpointermove: move |event| onaction.call(SurfaceAction::Move(point(&event))),
        onpointerup: move |_| onaction.call(SurfaceAction::End),
        onpointercancel: move |_| onaction.call(SurfaceAction::Cancel),
        oncontextmenu: move |event| {event.prevent_default(); onaction.call(SurfaceAction::Enter(None));},
        {children}
    } }
}

#[component]
pub fn EditorCanvas(
    canvas: CanvasPresentation,
    editing: bool,
    selected: bool,
    selected_widget: Option<String>,
    onaction: EventHandler<SurfaceAction>,
    #[props(default)] oncapture: EventHandler<PointerEvent>,
    children: Element,
) -> Element {
    let mut selecting = use_signal(|| false);
    let start = Callback::new(
        move |(event, canvas, widget, corner): (
            PointerEvent,
            String,
            Option<String>,
            Option<String>,
        )| {
            selecting.set(false);
            if !selected && event.trigger_button() == Some(MouseButton::Primary) {
                selecting.set(true);
                event.prevent_default();
                event.stop_propagation();
                onaction.call(SurfaceAction::Select(canvas));
                return;
            }
            let expected = if widget.is_none() && corner.is_none() {
                MouseButton::Secondary
            } else {
                MouseButton::Primary
            };
            if event.trigger_button() != Some(expected) {
                if event.trigger_button() == Some(MouseButton::Primary) {
                    event.stop_propagation();
                    onaction.call(SurfaceAction::Select(canvas));
                }
                return;
            }
            event.prevent_default();
            event.stop_propagation();
            oncapture.call(event.clone());
            onaction.call(SurfaceAction::Start {
                canvas,
                widget,
                corner,
                point: point(&event),
            });
        },
    );
    let id = canvas.id.clone();
    let context_id = id.clone();
    rsx! { div {
        class: if selected_widget.is_some() && selected { "editor-canvas selected widget-selected" } else if selected { "editor-canvas selected" } else { "editor-canvas" },
        "data-canvas":"{canvas.id}",
        style:format!("left:{}px;top:{}px;width:{}px;height:{}px",canvas.x,canvas.y,canvas.width,canvas.height),
        oncontextmenu:move |event| {event.prevent_default();event.stop_propagation();onaction.call(SurfaceAction::Enter(Some(context_id.clone())));},
        onpointerdown:move |event| {if editing {start.call((event,id.clone(),None,None));}},
        onclick:move |event| {if selecting.replace(false){return;}let point=event.client_coordinates().to_i32();onaction.call(SurfaceAction::Place([point.x,point.y]));},
        div {class:"editor-canvas-content",style:format!("opacity:{}",f32::from(canvas.opacity_percent)/100.0), {children}}
        if editing {
            for widget in &canvas.widgets {
                Fragment { key:"{widget.id}",
                    if widget.kind == WidgetKind::Empty && !(selected && selected_widget.as_deref() == Some(widget.id.as_str())) {
                        div { class:"empty-geometry", aria_hidden:"true",
                            style:format!("left:{}px;top:{}px;width:{}px;height:{}px",widget.x,widget.y,widget.width,widget.height),
                            span { {format!("{},{} · {}×{}",canvas.x.saturating_add(widget.x),canvas.y.saturating_add(widget.y),widget.width,widget.height)} }
                        }
                    }
                    div {class:if selected&&selected_widget.as_deref()==Some(widget.id.as_str()){"editor-widget-hit selected"}else{"editor-widget-hit"},
                        "data-widget":"{widget.id}",
                        style:format!("left:{}px;top:{}px;width:{}px;height:{}px",widget.x,widget.y,widget.width,widget.height),
                        onpointerdown:{let canvas=canvas.id.clone();let widget=widget.id.clone();move |event|start.call((event,canvas.clone(),Some(widget.clone()),None))},
                        ResizeHandles {onstart:{let canvas=canvas.id.clone();let widget=widget.id.clone();move |(event,corner)|start.call((event,canvas.clone(),Some(widget.clone()),Some(corner)))}}
                    }
                }
            }
            if selected { ResizeHandles {class:"canvas-resize",onstart:{let canvas=canvas.id.clone();move |(event,corner)|start.call((event,canvas.clone(),None,Some(corner)))}} }
        }
    } }
}

#[component]
pub fn PlacementPreview(kind: crate::WidgetKind, point: [f64; 2]) -> Element {
    let (width, height) = crate::default_widget_size(kind);
    rsx! {div {class:"placement-ghost",style:format!("left:{}px;top:{}px;width:{width}px;height:{height}px",point[0],point[1])}}
}
