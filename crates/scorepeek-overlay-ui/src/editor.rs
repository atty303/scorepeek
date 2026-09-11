//! Editor controls share their layout and state styles across rendering backends.
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

mod components;
use components::{AccordionSection, NumberField, TextField, Toggle};

#[derive(Clone, Copy, Default, PartialEq)]
pub enum ButtonLayout {
    #[default]
    Action,
    Row,
    Stack,
    Icon,
}

impl ButtonLayout {
    fn class(self) -> &'static str {
        match self {
            Self::Action => "editor-button-action",
            Self::Row => "editor-button-row",
            Self::Stack => "editor-button-stack",
            Self::Icon => "editor-button-icon",
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
pub enum ButtonTone {
    #[default]
    Normal,
    Primary,
    Danger,
}

impl ButtonTone {
    fn class(self) -> &'static str {
        match self {
            Self::Normal => "",
            Self::Primary => "primary",
            Self::Danger => "danger",
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct EditorButtonProps {
    #[props(default)]
    class: String,
    #[props(default)]
    layout: ButtonLayout,
    #[props(default)]
    tone: ButtonTone,
    #[props(default)]
    selected: Option<bool>,
    #[props(default)]
    disabled: bool,
    #[props(extends = button, extends = GlobalAttributes)]
    attributes: Vec<Attribute>,
    onclick: EventHandler<MouseEvent>,
    children: Element,
}

#[component]
pub fn EditorButton(props: EditorButtonProps) -> Element {
    let selected = if props.selected == Some(true) {
        "selected"
    } else {
        ""
    };
    rsx! {
        button {
            class: "editor-button {props.layout.class()} {props.tone.class()} {selected} {props.class}",
            r#type: "button",
            disabled: props.disabled.then_some(true),
            onclick:move |event| props.onclick.call(event),
            "aria-pressed": props.selected.map(|value| value.to_string()),
            ..props.attributes,
            {props.children}
        }
    }
}

use crate::{
    AspectRatio, Background, CanvasPresentation, FrameWidth, ScreenKind, Skin, WidgetKind,
    WidgetLayout,
};

#[derive(Clone, Debug, PartialEq)]
pub enum EditorAction {
    TogglePanel,
    PreviewScreen(ScreenKind),
    SelectOutput(String),
    SelectCanvas(String),
    CanvasName(String),
    CanvasVisible(ScreenKind, bool),
    CanvasVisibleAll,
    CanvasVisibleNone,
    CanvasGeometry(GeometryField, i32),
    WidgetGeometry(GeometryField, i32),
    AddCanvas,
    DeleteCanvas,
    NewCanvasSkin(Skin),
    Skin(Skin),
    CanvasSkinProperty(String, serde_json::Value),
    WidgetSkinProperty(String, serde_json::Value),
    Background(Background),
    Opacity(u8),
    Output(String),
    FitToOutput,
    SelectWidget {
        canvas_id: String,
        widget_id: String,
    },
    ToggleWidgetAdd,
    AddWidget(usize),
    Undo,
    Discard,
    Save,
    Close,
    FrameWidth(FrameWidth),
    EditTitle,
    AcceptTitle,
    CancelTitle,
    FillDelta(i8),
    FillOpacity(u8),
    AspectRatio(usize),
    HistoryCount(u32),
    GraphMonths(u32),
    DeleteWidget,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GeometryField {
    X,
    Y,
    Width,
    Height,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EditorOutput {
    pub name: String,
    pub model: String,
    pub logical_size: Option<[u32; 2]>,
}

#[derive(Clone, PartialEq)]
pub struct EditorChrome {
    pub panel_open: bool,
    pub widget_add_open: bool,
    pub sample: bool,
}
#[derive(Clone, PartialEq)]
pub struct EditorAccess {
    pub dirty: bool,
    pub readonly: bool,
    pub undo_available: bool,
    pub save_validity: SaveValidity,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SaveValidity {
    Valid,
    Invalid,
}
#[derive(Clone, Copy, PartialEq)]
pub enum EditorTitleState {
    Closed,
    Editing,
    Composing,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EditorSkin {
    pub id: Skin,
    pub name: String,
    pub release: String,
    pub preview: String,
    pub preview_video: Option<String>,
    #[serde(default)]
    pub canvas_properties: std::collections::BTreeMap<String, EditorProperty>,
    #[serde(default)]
    pub widget_properties:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, EditorProperty>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum EditorProperty {
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

impl EditorProperty {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Boolean { .. } => "boolean",
            Self::Integer { .. } => "integer",
            Self::Number { .. } => "number",
            Self::Color { .. } => "color",
            Self::Enum { .. } => "enum",
            Self::String { .. } => "string",
        }
    }
    pub(crate) fn effective(&self, value: Option<&serde_json::Value>) -> serde_json::Value {
        let valid = value.filter(|value| match self {
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
            Self::Color { .. } => value.as_str().is_some_and(|value| {
                matches!(value.len(), 4 | 5 | 7 | 9)
                    && value.starts_with('#')
                    && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
            }),
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
}

#[derive(Clone, PartialEq)]
pub struct EditorView {
    pub backend_label: String,
    pub canvases: Vec<CanvasPresentation>,
    pub selected_canvas: Option<String>,
    pub selected_widget: Option<String>,
    pub preview_screen: ScreenKind,
    pub outputs: Vec<EditorOutput>,
    pub active_output: Option<String>,
    pub panel_width: u32,
    pub chrome: EditorChrome,
    pub access: EditorAccess,
    pub title: EditorTitleState,
    pub skins: Vec<EditorSkin>,
    pub new_canvas_skin: Skin,
}

#[must_use]
pub fn document_valid(canvases: &[CanvasPresentation], outputs: &[EditorOutput]) -> bool {
    let mut names = std::collections::BTreeSet::new();
    canvases.iter().all(|canvas| {
        let output_size = canvas.output.as_ref().and_then(|name| {
            outputs
                .iter()
                .find(|output| &output.name == name)
                .map(|output| output.logical_size)
        });
        let right = u32::try_from(canvas.x)
            .ok()
            .and_then(|x| x.checked_add(canvas.width));
        let bottom = u32::try_from(canvas.y)
            .ok()
            .and_then(|y| y.checked_add(canvas.height));
        let canvas_grid_valid = canvas.x >= 0
            && canvas.y >= 0
            && canvas.x % 4 == 0
            && canvas.y % 4 == 0
            && canvas.width >= 32
            && canvas.height >= 32
            && (canvas.width.is_multiple_of(4)
                || output_size
                    .flatten()
                    .is_some_and(|[width, _]| right == Some(width)))
            && (canvas.height.is_multiple_of(4)
                || output_size
                    .flatten()
                    .is_some_and(|[_, height]| bottom == Some(height)));
        let output_valid = output_size.is_some_and(|size| {
            size.is_none_or(|[width, height]| {
                right.is_some_and(|right| right <= width)
                    && bottom.is_some_and(|bottom| bottom <= height)
            })
        });
        let widgets_valid = canvas.widgets.iter().all(|widget| {
            widget.x >= 0
                && widget.y >= 0
                && widget.x % 4 == 0
                && widget.y % 4 == 0
                && widget.width >= 16
                && widget.height >= 16
                && widget.width.is_multiple_of(4)
                && widget.height.is_multiple_of(4)
                && u32::try_from(widget.x).ok().is_some_and(|x| {
                    x.checked_add(widget.width)
                        .is_some_and(|right| right <= canvas.width)
                })
                && u32::try_from(widget.y).ok().is_some_and(|y| {
                    y.checked_add(widget.height)
                        .is_some_and(|bottom| bottom <= canvas.height)
                })
        });
        !canvas.name.trim().is_empty()
            && names.insert(canvas.name.clone())
            && canvas_grid_valid
            && output_valid
            && widgets_valid
    })
}

#[component]
pub fn EditorPanel(
    view: EditorView,
    title_input: Element,
    onaction: EventHandler<EditorAction>,
) -> Element {
    let mut screen_picker_open = use_signal(|| false);
    let mut expanded = use_signal(std::collections::BTreeSet::<String>::new);
    let mut invalid_fields = use_signal(std::collections::BTreeSet::<String>::new);
    let canvas = view
        .canvases
        .iter()
        .find(|canvas| Some(canvas.id.as_str()) == view.selected_canvas.as_deref());
    let active_canvases = view
        .canvases
        .iter()
        .filter(|canvas| canvas.output.as_ref() == view.active_output.as_ref())
        .collect::<Vec<_>>();
    let validity = Callback::new(move |(key, valid): (String, bool)| {
        let mut fields = invalid_fields.write();
        if valid {
            fields.remove(&key);
        } else {
            fields.insert(key);
        }
    });
    let parent_action = onaction;
    let selected_canvas = view.selected_canvas.clone();
    let selected_widget = view.selected_widget.clone();
    let onaction = Callback::new(move |action: EditorAction| {
        let scope_changes = match &action {
            EditorAction::SelectOutput(_)
            | EditorAction::DeleteCanvas
            | EditorAction::DeleteWidget
            | EditorAction::Undo
            | EditorAction::Discard => true,
            EditorAction::SelectCanvas(id) => {
                selected_canvas.as_ref() != Some(id) || selected_widget.is_some()
            }
            EditorAction::SelectWidget {
                canvas_id,
                widget_id,
            } => {
                selected_canvas.as_ref() != Some(canvas_id)
                    || selected_widget.as_ref() != Some(widget_id)
            }
            _ => false,
        };
        if scope_changes {
            invalid_fields.write().clear();
        }
        parent_action.call(action);
    });
    let active_field_prefix = canvas.map(|canvas| {
        view.selected_widget.as_ref().map_or_else(
            || format!("{}:", canvas.id),
            |widget| format!("{}:{widget}:", canvas.id),
        )
    });
    let invalid_input = active_field_prefix.as_ref().is_some_and(|prefix| {
        invalid_fields
            .read()
            .iter()
            .any(|field| field.starts_with(prefix))
    });
    let active_output_label = view.active_output.as_deref().unwrap_or("NONE");
    rsx! {
        if !view.chrome.panel_open {
            EditorButton { class: if view.access.dirty { "native-panel-toggle dirty" } else { "native-panel-toggle" }, layout: ButtonLayout::Icon, onclick: move |_| onaction.call(EditorAction::TogglePanel), "aria-label": "Show editor panel", "›" span { class: "dirty-dot" } }
        } else {
            aside { class: "native-canvas-manager", style: format!("width:{}px", view.panel_width),
                onkeydown: move |event| if event.key() == Key::Escape {
                    let mut handled = false;
                    if screen_picker_open() {
                        screen_picker_open.set(false);
                        handled = true;
                    }
                    if view.chrome.widget_add_open {
                        onaction.call(EditorAction::ToggleWidgetAdd);
                        handled = true;
                    }
                    if handled {
                        event.prevent_default();
                        event.stop_propagation();
                    }
                },
                div { class: "editor-context-bar",
                    EditorButton { class: "native-panel-toggle", layout: ButtonLayout::Icon, onclick: move |_| onaction.call(EditorAction::TogglePanel), "aria-label": "Hide editor panel", "‹" }
                    div { class: "context-picker",
                        EditorButton { class: "context-picker-trigger", onclick: move |_| screen_picker_open.toggle(), "GAME SCREEN" b { "{screen_label(view.preview_screen)}" } span { "⌄" } }
                        if screen_picker_open() { div { class: "context-picker-list", for (index, kind) in crate::editor_model::SCREENS.into_iter().enumerate() { EditorButton { class: "preview-screen", selected: view.preview_screen == kind, "data-index": index, onclick: move |_| { onaction.call(EditorAction::PreviewScreen(kind)); screen_picker_open.set(false); }, "{screen_label(kind)}" } } } }
                    }
                    div { class: "context-output", small { "OUTPUT" } strong { "{active_output_label}" } }
                    if view.chrome.sample { span { class: "status-badge", "SAMPLE" } }
                    if view.access.dirty { span { class: "dirty-status", "UNSAVED" } }
                }
                div { class: "editor-work-area",
                    nav { class: "object-navigator", "aria-label": "Overlay objects",
                        div { class: "pane-heading", strong { "OBJECTS" } }
                        div { class: "navigator-scroll",
                            div { class: "navigator-root", span { "Workspace" } }
                            for output in view.outputs.iter() {
                                div { class: "navigator-output",
                                    EditorButton { class: "workspace-output-option", layout: ButtonLayout::Row, selected: view.active_output.as_deref() == Some(output.name.as_str()), "data-output": "{output.name}", onclick: { let output = output.name.clone(); move |_| onaction.call(EditorAction::SelectOutput(output.clone())) }, strong { "{output.name}" } small { "{output.model}" } }
                                }
                            }
                            div { class: "navigator-active-output", for canvas in active_canvases.iter() {
                                div { class: "navigator-canvas",
                                    div { class: "navigator-item-row",
                                        EditorButton { class: "tree-disclosure", layout: ButtonLayout::Icon, onclick: { let id = canvas.id.clone(); move |_| { let mut values = expanded.write(); if !values.remove(&id) { values.insert(id.clone()); } } }, "aria-label": "Toggle canvas children", if expanded.read().contains(&canvas.id) || view.selected_canvas.as_deref() == Some(canvas.id.as_str()) { "−" } else { "+" } }
                                        EditorButton { class: "canvas-select", layout: ButtonLayout::Row, selected: view.selected_canvas.as_deref() == Some(canvas.id.as_str()), "data-canvas-id": "{canvas.id}", onclick: { let id = canvas.id.clone(); move |_| { expanded.write().insert(id.clone()); onaction.call(EditorAction::SelectCanvas(id.clone())); } }, "{canvas.name}" }
                                    }
                                    if expanded.read().contains(&canvas.id) || view.selected_canvas.as_deref() == Some(canvas.id.as_str()) {
                                        div { class: "navigator-widgets", for widget in canvas.widgets.iter() { EditorButton { class: "widget-row", layout: ButtonLayout::Row, selected: view.selected_canvas.as_deref() == Some(canvas.id.as_str()) && view.selected_widget.as_deref() == Some(widget.id.as_str()), "data-widget-id": "{widget.id}", onclick: { let canvas_id = canvas.id.clone(); let widget_id = widget.id.clone(); move |_| onaction.call(EditorAction::SelectWidget { canvas_id: canvas_id.clone(), widget_id: widget_id.clone() }) }, "{widget_label(widget, &canvas.widgets)}" } } }
                                    }
                                }
                            } }
                        }
                        div { class: "navigator-add",
                            if canvas.is_some() {
                                EditorButton { class: "widget-add-summary", disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::ToggleWidgetAdd), "+ Add widget" }
                                if view.chrome.widget_add_open { div { class: "widget-type-picker", for (index, label) in ["Status", "Selection", "Score", "History list", "History graph", "Empty"].into_iter().enumerate() { EditorButton { class: "add-widget", disabled: view.access.readonly, "data-index": index, onclick: move |_| onaction.call(EditorAction::AddWidget(index)), "{label}" } } } }
                            } else {
                                EditorButton { class: "add-canvas", disabled: view.access.readonly || view.active_output.is_none(), onclick: move |_| onaction.call(EditorAction::AddCanvas), "+ Add canvas" }
                            }
                        }
                    }
                    main { class: "object-inspector",
                        div { class: "pane-heading", strong { "INSPECTOR" } if let Some(canvas) = canvas { small { if let Some(widget) = canvas.widgets.iter().find(|widget| view.selected_widget.as_deref() == Some(widget.id.as_str())) { "{widget_label(widget, &canvas.widgets)}" } else { "{canvas.name}" } } } }
                        div { class: "inspector-scroll",
                            if let Some(canvas) = canvas {
                                if let Some(widget) = canvas.widgets.iter().find(|widget| view.selected_widget.as_deref() == Some(widget.id.as_str())) {
                                    div { key: "{canvas.id}:{widget.id}", AccordionSection { title: "Geometry", {geometry_fields(GeometrySpec { rect: [widget.x, widget.y, i32::try_from(widget.width).unwrap_or(i32::MAX), i32::try_from(widget.height).unwrap_or(i32::MAX)], bounds: [canvas.width, canvas.height], minimum: [16, 16], key: &format!("{}:{}", canvas.id, widget.id), widget: true, readonly: view.access.readonly }, validity, onaction)} }
                                    AccordionSection { title: "Widget settings", {widget_settings(widget, &view, title_input.clone(), onaction)} }
                                    if let Some(skin) = view.skins.iter().find(|skin| skin.id == canvas.skin) { if let Some(properties) = skin.widget_properties.get(widget.kind.name()).or_else(|| skin.widget_properties.get("*")) { AccordionSection { title: "Style", {property_controls(properties, &widget.skin_properties, false, view.access.readonly, onaction)} } } }
                                    }
                                } else {
                                    div { key: "{canvas.id}", {canvas_inspector(canvas, &view, validity, onaction)} }
                                }
                            } else {
                                div { class: "inspector-empty", strong { "Select an object" } p { "Choose an output or add a canvas to begin editing." } }
                                AccordionSection { title: "New canvas skin", {skin_picker(&view, view.new_canvas_skin, true, onaction)} }
                            }
                        }
                    }
                }
                footer { class: "editor-action-bar",
                    EditorButton { class: "undo-action", onclick: move |_| onaction.call(EditorAction::Undo), disabled: view.access.readonly || !view.access.undo_available, "Undo" }
                    div { class: "footer-actions",
                        if view.access.dirty {
                            EditorButton { class: "discard-action", disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::Discard), "Discard" }
                            EditorButton { class: "save-action", tone: ButtonTone::Primary, disabled: view.access.readonly || view.access.save_validity == SaveValidity::Invalid || invalid_input, onclick: move |_| onaction.call(EditorAction::Save), "Save & Close" }
                        } else {
                            EditorButton { class: "close-action", onclick: move |_| onaction.call(EditorAction::Close), "Close" }
                        }
                    }
                }
            }
        }
    }
}

fn screen_label(screen: ScreenKind) -> &'static str {
    match screen {
        ScreenKind::Unknown => "Unknown",
        ScreenKind::MusicSelect => "Music Select",
        ScreenKind::ModeSelect => "Mode Select",
        ScreenKind::DecideTransition => "Decide",
        ScreenKind::Play => "Play",
        ScreenKind::Result => "Result",
    }
}

fn widget_label(widget: &WidgetLayout, widgets: &[WidgetLayout]) -> String {
    let same = widgets
        .iter()
        .filter(|candidate| candidate.kind == widget.kind)
        .collect::<Vec<_>>();
    let base = property_token(widget.kind.name());
    if same.len() == 1 {
        base
    } else {
        format!(
            "{base} {}",
            same.iter()
                .position(|candidate| candidate.id == widget.id)
                .unwrap_or_default()
                + 1
        )
    }
}

#[derive(Clone, Copy)]
struct GeometrySpec<'a> {
    rect: [i32; 4],
    bounds: [u32; 2],
    minimum: [u32; 2],
    key: &'a str,
    widget: bool,
    readonly: bool,
}

fn geometry_fields(
    spec: GeometrySpec<'_>,
    onvalidity: EventHandler<(String, bool)>,
    onaction: EventHandler<EditorAction>,
) -> Element {
    let [x, y, width, height] = spec.rect;
    let width_u32 = u32::try_from(width).unwrap_or_default();
    let height_u32 = u32::try_from(height).unwrap_or_default();
    let maximum_x = i32::try_from(spec.bounds[0].saturating_sub(width_u32)).unwrap_or(i32::MAX);
    let maximum_y = i32::try_from(spec.bounds[1].saturating_sub(height_u32)).unwrap_or(i32::MAX);
    let maximum_width =
        i32::try_from(spec.bounds[0].saturating_sub(u32::try_from(x).unwrap_or_default()))
            .unwrap_or(i32::MAX);
    let maximum_height =
        i32::try_from(spec.bounds[1].saturating_sub(u32::try_from(y).unwrap_or_default()))
            .unwrap_or(i32::MAX);
    let emit = move |field, value| {
        if spec.widget {
            onaction.call(EditorAction::WidgetGeometry(field, value));
        } else {
            onaction.call(EditorAction::CanvasGeometry(field, value));
        }
    };
    rsx! { div { class: "geometry-grid",
        NumberField { field_key: format!("{}:x", spec.key), label: "X", value: x, minimum: 0, maximum: maximum_x, disabled: spec.readonly, onchange: move |value| emit(GeometryField::X, value), onvalidity }
        NumberField { field_key: format!("{}:y", spec.key), label: "Y", value: y, minimum: 0, maximum: maximum_y, disabled: spec.readonly, onchange: move |value| emit(GeometryField::Y, value), onvalidity }
        NumberField { field_key: format!("{}:width", spec.key), label: "Width", value: width, minimum: i32::try_from(spec.minimum[0]).unwrap_or(16), maximum: maximum_width, allow_maximum_off_grid: !spec.widget, disabled: spec.readonly, onchange: move |value| emit(GeometryField::Width, value), onvalidity }
        NumberField { field_key: format!("{}:height", spec.key), label: "Height", value: height, minimum: i32::try_from(spec.minimum[1]).unwrap_or(16), maximum: maximum_height, allow_maximum_off_grid: !spec.widget, disabled: spec.readonly, onchange: move |value| emit(GeometryField::Height, value), onvalidity }
    } }
}

fn canvas_inspector(
    canvas: &CanvasPresentation,
    view: &EditorView,
    onvalidity: EventHandler<(String, bool)>,
    onaction: EventHandler<EditorAction>,
) -> Element {
    let disallowed = view
        .canvases
        .iter()
        .filter(|candidate| candidate.id != canvas.id)
        .map(|candidate| candidate.name.clone())
        .collect::<Vec<_>>();
    let bounds = canvas
        .output
        .as_ref()
        .and_then(|name| view.outputs.iter().find(|output| &output.name == name))
        .and_then(|output| output.logical_size)
        .unwrap_or([
            u32::try_from(canvas.x.max(0)).unwrap_or_default() + canvas.width,
            u32::try_from(canvas.y.max(0)).unwrap_or_default() + canvas.height,
        ]);
    let child_min = canvas.widgets.iter().fold([32, 32], |minimum, widget| {
        [
            minimum[0].max(u32::try_from(widget.x).unwrap_or_default() + widget.width),
            minimum[1].max(u32::try_from(widget.y).unwrap_or_default() + widget.height),
        ]
    });
    rsx! {
        AccordionSection { title: "Identity", TextField { field_key: format!("{}:name", canvas.id), label: "Name", value: canvas.name.clone(), disallowed, disabled: view.access.readonly, onchange: move |value| onaction.call(EditorAction::CanvasName(value)), onvalidity } small { class: "stable-id", "ID · {canvas.id}" } }
        AccordionSection { title: "Geometry", {geometry_fields(GeometrySpec { rect: [canvas.x, canvas.y, i32::try_from(canvas.width).unwrap_or(i32::MAX), i32::try_from(canvas.height).unwrap_or(i32::MAX)], bounds, minimum: child_min, key: &canvas.id, widget: false, readonly: view.access.readonly }, onvalidity, onaction)} EditorButton { class: "fit-output", disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::FitToOutput), "Fit to output" } }
        AccordionSection { title: "Visibility", div { class: "visibility-actions", EditorButton { disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::CanvasVisibleAll), "All" } EditorButton { disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::CanvasVisibleNone), "None" } } {visibility_toggles(&canvas.id, canvas.show_on.as_deref(), view.access.readonly, onaction)} }
        AccordionSection { title: "Appearance", {skin_picker(view, canvas.skin, false, onaction)} div { class: "native-opacity button-grid four", for value in [25, 50, 75, 100] { EditorButton { class: "opacity-option", selected: canvas.opacity_percent == value, disabled: view.access.readonly, "data-value": value, onclick: move |_| onaction.call(EditorAction::Opacity(value)), "{value}%" } } } if let Some(skin) = view.skins.iter().find(|skin| skin.id == canvas.skin) { {property_controls(&skin.canvas_properties, &canvas.skin_properties, true, view.access.readonly, onaction)} } }
        AccordionSection { title: "Output", div { class: "output-list", for output in view.outputs.iter() { EditorButton { class: "output-option", layout: ButtonLayout::Stack, selected: canvas.output.as_deref() == Some(output.name.as_str()), disabled: view.access.readonly, "data-output": "{output.name}", onclick: { let output = output.name.clone(); move |_| onaction.call(EditorAction::Output(output.clone())) }, strong { "{output.name}" } small { "{output.model}" } } } } }
        AccordionSection { title: "Danger zone", EditorButton { class: "delete-canvas", tone: ButtonTone::Danger, disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::DeleteCanvas), "Delete canvas" } }
    }
}

fn visibility_toggles(
    canvas_id: &str,
    show_on: Option<&[ScreenKind]>,
    readonly: bool,
    onaction: EventHandler<EditorAction>,
) -> Element {
    let canvas_id = canvas_id.to_owned();
    let show_on = show_on.map(<[ScreenKind]>::to_vec);
    rsx! { div { class: "visibility-grid", for (index, screen) in crate::editor_model::SCREENS.into_iter().enumerate() {
        Toggle {
            class: "screen-toggle",
            label: screen_label(screen),
            selected: show_on.as_ref().is_none_or(|screens| screens.contains(&screen)),
            disabled: readonly,
            data_index: index,
            data_canvas_id: canvas_id.clone(),
            onclick: {
                let show_on = show_on.clone();
                move |_| onaction.call(EditorAction::CanvasVisible(
                    screen,
                    !show_on.as_ref().is_none_or(|screens| screens.contains(&screen)),
                ))
            },
        }
    } } }
}

fn skin_picker(
    view: &EditorView,
    selected: Skin,
    new_canvas: bool,
    onaction: EventHandler<EditorAction>,
) -> Element {
    rsx! { div { class: "native-skin-options", for (index, skin) in view.skins.iter().enumerate() { EditorButton { class: "skin-option", layout: ButtonLayout::Row, selected: selected == skin.id, disabled: view.access.readonly, "data-index": index, onclick: { let skin = skin.id; move |_| if new_canvas { onaction.call(EditorAction::NewCanvasSkin(skin)); } else { onaction.call(EditorAction::Skin(skin)); } }, if !skin.preview.is_empty() { img { src: "{skin.preview}", alt: "" } } span { strong { "{skin.name}" } small { "{skin.release}" } } } } } }
}

fn property_controls(
    properties: &std::collections::BTreeMap<String, EditorProperty>,
    values: &std::collections::BTreeMap<String, serde_json::Value>,
    canvas: bool,
    readonly: bool,
    onaction: EventHandler<EditorAction>,
) -> Element {
    let action = move |key: String, value: serde_json::Value| {
        if canvas {
            onaction.call(EditorAction::CanvasSkinProperty(key, value));
        } else {
            onaction.call(EditorAction::WidgetSkinProperty(key, value));
        }
    };
    rsx! { section { class:"skin-property-list",
        h3 { class:"property-section-title", if canvas {"CANVAS STYLE"} else {"WIDGET STYLE"} }
        for (key,property) in properties {
        div { class:"skin-property", "data-property":key,
            div { class:"property-heading",
                label { class:"property-label", title:"{key}", "{property_key_display(properties,key)}" }
                span { class:"property-kind", "{property_kind_label(property)}" }
            }
            match property {
                EditorProperty::Boolean{default} => { let value=values.get(key).and_then(serde_json::Value::as_bool).unwrap_or(*default); rsx!{EditorButton{class:"property-toggle",disabled:readonly,selected:value,onclick:{let key=key.clone();move |_|action(key.clone(),(!value).into())},span{class:"property-toggle-state",if value{"ENABLED"}else{"DISABLED"}} span{class:"property-toggle-mark",if value{"ON"}else{"OFF"}}}} },
                EditorProperty::Enum{default,values:options} => { let value=values.get(key).and_then(serde_json::Value::as_str).unwrap_or(default); rsx!{div{class:"property-options",for (index,option) in options.iter().enumerate(){EditorButton{class:"property-option",disabled:readonly,selected:value==option,onclick:{let key=key.clone();let option=option.clone();move |_|action(key.clone(),option.clone().into())},"data-index":index,"data-value":option,title:"{option}",span{class:"property-option-label","{property_option_display(options,option)}"}}}}} },
                EditorProperty::Integer{default,minimum,maximum} => { let value=values.get(key).and_then(serde_json::Value::as_i64).unwrap_or(*default); let unit=property_unit(key); rsx!{div{class:"property-number-control",div{class:"property-value",input{class:"property-value-input",r#type:"number",value:"{value}",min:"{minimum}",max:"{maximum}",disabled:readonly,oninput:{let key=key.clone();let minimum=*minimum;let maximum=*maximum;move |event|if let Some(value)=integer_property_value(&event.value(),minimum,maximum){action(key.clone(),value.into())}},onblur:{let key=key.clone();move |_|action(key.clone(),value.into())}} if !unit.is_empty(){span{"{unit}"}}} small{class:"property-range","{minimum}–{maximum}{unit}"}}} },
                EditorProperty::Number{default,minimum,maximum} => { let value=values.get(key).and_then(serde_json::Value::as_f64).unwrap_or(*default); let unit=property_unit(key); rsx!{div{class:"property-number-control",div{class:"property-value",input{class:"property-value-input",r#type:"number",value:"{value}",min:"{minimum}",max:"{maximum}",disabled:readonly,oninput:{let key=key.clone();let minimum=*minimum;let maximum=*maximum;move |event|if let Some(value)=number_property_value(&event.value(),minimum,maximum){action(key.clone(),value.into())}},onblur:{let key=key.clone();move |_|action(key.clone(),serde_json::Number::from_f64(value).expect("effective number is finite").into())}} if !unit.is_empty(){span{"{unit}"}}} small{class:"property-range","{minimum}–{maximum}{unit}"}}} },
                EditorProperty::Color{default} => { let value=values.get(key).and_then(serde_json::Value::as_str).unwrap_or(default); rsx!{div{class:"property-color-control",span{class:"property-color-swatch",style:"background-color:{value}",aria_hidden:"true"} input{class:"property-color-input",r#type:"text",value:"{value}",disabled:readonly,oninput:{let key=key.clone();move |event|if valid_property_color(&event.value()){action(key.clone(),event.value().into())}},onblur:{let key=key.clone();let value=value.to_owned();move |_|action(key.clone(),value.clone().into())}}}} },
                EditorProperty::String{default,maximum_length} => { let value=values.get(key).and_then(serde_json::Value::as_str).unwrap_or(default); rsx!{div{class:"property-string-control",input{class:"property-string-input",r#type:"text",value:"{value}",disabled:readonly,oninput:{let key=key.clone();let maximum_length=*maximum_length;move |event|if valid_property_string(&event.value(),maximum_length){action(key.clone(),event.value().into())}},onblur:{let key=key.clone();let value=value.to_owned();move |_|action(key.clone(),value.clone().into())}} small{class:"property-limit","MAX {maximum_length}"}}} },
            }
        }
    } } }
}

fn property_label(key: &str) -> String {
    property_token(key.strip_suffix("-percent").unwrap_or(key))
}

fn property_token(value: &str) -> String {
    value
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_uppercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn property_display_label(raw: &str, label: &str, ambiguous: bool) -> String {
    if label.is_empty() {
        raw.to_ascii_uppercase()
    } else if ambiguous {
        format!("{label} · {raw}")
    } else {
        label.to_owned()
    }
}

fn property_key_display(
    properties: &std::collections::BTreeMap<String, EditorProperty>,
    key: &str,
) -> String {
    let label = property_label(key);
    let ambiguous = properties
        .keys()
        .filter(|candidate| property_label(candidate) == label)
        .count()
        > 1;
    property_display_label(key, &label, ambiguous)
}

fn property_option_display(options: &[String], option: &str) -> String {
    let label = property_token(option);
    let ambiguous = options
        .iter()
        .filter(|candidate| property_token(candidate) == label)
        .count()
        > 1;
    property_display_label(option, &label, ambiguous)
}

fn valid_property_color(value: &str) -> bool {
    matches!(value.len(), 4 | 5 | 7 | 9)
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_property_string(value: &str, maximum_length: usize) -> bool {
    value.chars().count() <= maximum_length
}

fn integer_property_value(value: &str, minimum: i64, maximum: i64) -> Option<i64> {
    value
        .parse()
        .ok()
        .filter(|value| minimum <= *value && *value <= maximum)
}

fn number_property_value(value: &str, minimum: f64, maximum: f64) -> Option<serde_json::Number> {
    let value = value.parse::<f64>().ok()?;
    (value.is_finite() && minimum <= value && value <= maximum)
        .then(|| serde_json::Number::from_f64(value))
        .flatten()
}

fn property_unit(key: &str) -> &'static str {
    if key.ends_with("-percent") { "%" } else { "" }
}

fn property_kind_label(property: &EditorProperty) -> &'static str {
    match property {
        EditorProperty::Boolean { .. } => "TOGGLE",
        EditorProperty::Integer { .. } | EditorProperty::Number { .. } => "NUMBER",
        EditorProperty::Color { .. } => "COLOR",
        EditorProperty::Enum { .. } => "CHOICE",
        EditorProperty::String { .. } => "TEXT",
    }
}

#[cfg(test)]
mod property_tests {
    use super::{
        integer_property_value, number_property_value, property_display_label, property_token,
        valid_property_color, valid_property_string,
    };

    #[test]
    fn property_labels_preserve_empty_and_ambiguous_raw_tokens() {
        assert_eq!(
            property_display_label("-", &property_token("-"), false),
            "-"
        );
        assert_eq!(
            property_display_label("foo_bar", &property_token("foo_bar"), true),
            "FOO BAR · foo_bar"
        );
    }

    #[test]
    fn property_colors_retain_every_manifest_hex_form() {
        for color in ["#123", "#1234", "#123456", "#12345678"] {
            assert!(valid_property_color(color));
        }
        assert!(!valid_property_color("#12"));
        assert!(!valid_property_color("#xyz"));
    }

    #[test]
    fn property_string_length_counts_unicode_scalars() {
        assert!(valid_property_string("😀", 1));
        assert!(!valid_property_string("😀a", 1));
    }

    #[test]
    fn numeric_property_edits_ignore_out_of_range_prefixes() {
        assert_eq!(integer_property_value("2", 10, 99), None);
        assert_eq!(integer_property_value("25", 10, 99), Some(25));
        assert_eq!(number_property_value("2", 10.0, 99.0), None);
        assert_eq!(
            number_property_value("25.5", 10.0, 99.0).and_then(|value| value.as_f64()),
            Some(25.5)
        );
    }
}

fn widget_settings(
    widget: &WidgetLayout,
    view: &EditorView,
    title_input: Element,
    onaction: EventHandler<EditorAction>,
) -> Element {
    rsx! {
                        div { class:"native-widget-settings",
                            strong { "{widget.id}" }
                            if widget.kind == WidgetKind::Empty {
                                h3 { "TITLE" }
                                if view.title!=EditorTitleState::Closed {
                                    {title_input}
                                    div { class:"button-grid", EditorButton { class:"title-accept", onclick:move |_| onaction.call(EditorAction::AcceptTitle), disabled:view.access.readonly || view.title==EditorTitleState::Composing, "APPLY TITLE" } EditorButton { class:"title-cancel", onclick:move |_| onaction.call(EditorAction::CancelTitle), disabled:view.access.readonly, "CANCEL" } }
                                } else { EditorButton { class:"empty-title-input", onclick:move |_| onaction.call(EditorAction::EditTitle), disabled:view.access.readonly, if widget.settings.title.is_empty(){"Enter title…"}else{"{widget.settings.title}"} } }
                                h3 { "ASPECT RATIO" }
                                div { class:"aspect-ratio-options button-grid four", role:"group", "aria-label":"Aspect ratio",
                                    for (index,label) in ["FREE","16:9","4:3","CURRENT"].into_iter().enumerate() { EditorButton { class:"aspect-ratio", onclick:move |_| onaction.call(EditorAction::AspectRatio(index)), disabled:view.access.readonly, selected:aspect_ratio_index(widget.settings.aspect_ratio)==index , "data-index":index, "{label}" } }
                                }
                            }
                            if widget.kind == WidgetKind::HistoryList {
                                for value in [5,10,20,50] { EditorButton { class:"history-count", onclick:move |_| onaction.call(EditorAction::HistoryCount(value)), disabled:view.access.readonly, selected:widget.settings.history_count==value, "data-value":value, if widget.settings.history_count==value{"✓ "} "{value}" } }
                            }
                            if widget.kind == WidgetKind::HistoryGraph {
                                for value in [1,3,6,12] { EditorButton { class:"graph-months", onclick:move |_| onaction.call(EditorAction::GraphMonths(value)), disabled:view.access.readonly, selected:widget.settings.graph_months==value, "data-value":value, if widget.settings.graph_months==value{"✓ "} "{value}M" } }
                            }
                            div { class:"widget-delete-actions", EditorButton { class:"delete-widget", onclick:move |_| onaction.call(EditorAction::DeleteWidget), disabled:view.access.readonly, tone:ButtonTone::Danger, "DELETE WIDGET" } }
                        }
    }
}

#[must_use]
pub fn aspect_ratio_index(ratio: AspectRatio) -> usize {
    use AspectRatio;
    match ratio {
        AspectRatio::Free => 0,
        AspectRatio::Wide => 1,
        AspectRatio::Standard => 2,
        AspectRatio::Current(_) => 3,
    }
}

#[component]
pub fn ResizeHandles(
    #[props(default)] class: String,
    onstart: EventHandler<(PointerEvent, String)>,
) -> Element {
    rsx! { for corner in ["nw", "ne", "sw", "se"] {
        i { class: "resize-handle {class} {corner}", aria_hidden: "true",
            onpointerdown: move |event| onstart.call((event, corner.into())) }
    } }
}
