//! Editor controls share their layout and state styles across rendering backends.
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

mod components;
pub use components::{
    Accordion, AccordionSection, Button, IconButton, ListPicker, ListPickerOption, NavigatorItem,
    NavigatorTree, NumberField, SegmentedControl, StatusBadge, TextField, Toggle, ToggleGroup,
};

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

#[must_use]
pub fn canvas_geometry_bounds(canvas: &CanvasPresentation, outputs: &[EditorOutput]) -> [u32; 2] {
    canvas
        .output
        .as_ref()
        .and_then(|name| outputs.iter().find(|output| &output.name == name))
        .and_then(|output| output.logical_size)
        .unwrap_or([
            u32::try_from(canvas.x.max(0)).unwrap_or_default() + canvas.width,
            u32::try_from(canvas.y.max(0)).unwrap_or_default() + canvas.height,
        ])
}

#[component]
pub fn EditorPanel(
    view: EditorView,
    title_input: Element,
    onaction: EventHandler<EditorAction>,
) -> Element {
    let mut invalid_fields = use_signal(std::collections::BTreeSet::<String>::new);
    let canvas = view
        .canvases
        .iter()
        .find(|canvas| Some(canvas.id.as_str()) == view.selected_canvas.as_deref());
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
    rsx! {
        if !view.chrome.panel_open {
            CollapsedEditorButton { dirty: view.access.dirty, onaction }
        } else {
            aside { class: "editor-panel", style: format!("width:{}px", view.panel_width),
                ContextBar { view: view.clone(), onaction }
                EditorWorkspace { view: view.clone(), title_input, onvalidity: validity, onaction }
                ActionBar { view: view.clone(), invalid_input, onaction }
            }
        }
    }
}

#[component]
pub fn CollapsedEditorButton(dirty: bool, onaction: EventHandler<EditorAction>) -> Element {
    rsx! {
        IconButton { class: if dirty { "editor-panel-toggle dirty" } else { "editor-panel-toggle" }, label: "Show editor panel", onclick: move |_| onaction.call(EditorAction::TogglePanel), "›" span { class: "dirty-dot" } }
    }
}

#[component]
pub fn ContextBar(view: EditorView, onaction: EventHandler<EditorAction>) -> Element {
    let mut picker_open = use_signal(|| false);
    let options = crate::editor_model::SCREENS
        .into_iter()
        .map(|screen| ListPickerOption {
            label: screen_label(screen).into(),
            detail: None,
        })
        .collect::<Vec<_>>();
    let selected = crate::editor_model::SCREENS
        .iter()
        .position(|screen| *screen == view.preview_screen)
        .unwrap_or_default();
    rsx! {
        header { class: "editor-context-bar",
            IconButton { class: "editor-panel-toggle", label: "Hide editor panel", onclick: move |_| onaction.call(EditorAction::TogglePanel), "‹" }
            ListPicker {
                class: "screen-picker",
                label: "GAME SCREEN",
                value: screen_label(view.preview_screen),
                options,
                selected,
                open: picker_open(),
                onopen: move |open| picker_open.set(open),
                onselect: move |index| if let Some(screen) = crate::editor_model::SCREENS.get(index) { onaction.call(EditorAction::PreviewScreen(*screen)); },
            }
            if view.access.dirty {
                span { class: "context-dirty dirty-dot", role: "status", "aria-label": "Unsaved changes" }
            }
        }
    }
}

#[component]
pub fn EditorWorkspace(
    view: EditorView,
    title_input: Element,
    onvalidity: EventHandler<(String, bool)>,
    onaction: EventHandler<EditorAction>,
) -> Element {
    rsx! {
        div { class: "editor-workspace",
            ObjectNavigator { view: view.clone(), onaction }
            Inspector { view, title_input, onvalidity, onaction }
        }
    }
}

#[component]
pub fn ObjectNavigator(view: EditorView, onaction: EventHandler<EditorAction>) -> Element {
    let initial_output = view.active_output.clone();
    let initial_canvas = view.selected_canvas.clone();
    let mut expanded_outputs = use_signal(move || {
        initial_output
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
    });
    let mut collapsed_outputs = use_signal(std::collections::BTreeSet::<String>::new);
    let mut expanded_canvases = use_signal(move || {
        initial_canvas
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
    });
    let canvas = view
        .canvases
        .iter()
        .find(|canvas| Some(canvas.id.as_str()) == view.selected_canvas.as_deref());
    let mut navigator_outputs = view
        .outputs
        .iter()
        .cloned()
        .map(|output| {
            let assigned = Some(output.name.clone());
            (output, assigned)
        })
        .collect::<Vec<_>>();
    for output in view
        .canvases
        .iter()
        .filter_map(|canvas| canvas.output.as_ref())
    {
        if navigator_outputs
            .iter()
            .all(|(candidate, _)| &candidate.name != output)
        {
            navigator_outputs.push((
                EditorOutput {
                    name: output.clone(),
                    model: "Missing output".into(),
                    logical_size: None,
                },
                Some(output.clone()),
            ));
        }
    }
    if view.canvases.iter().any(|canvas| canvas.output.is_none()) {
        navigator_outputs.push((
            EditorOutput {
                name: "Unassigned".into(),
                model: "No output".into(),
                logical_size: None,
            },
            None,
        ));
    }
    let widget_options = [
        "Status",
        "Selection",
        "Score",
        "History list",
        "History graph",
        "Empty",
    ]
    .into_iter()
    .map(|label| ListPickerOption {
        label: label.into(),
        detail: None,
    })
    .collect::<Vec<_>>();
    rsx! {
        nav { class: "object-navigator", "aria-label": "Overlay objects",
            div { class: "pane-heading", strong { "OBJECTS" } }
            div { class: "navigator-scroll",
                NavigatorTree { label: "Overlay workspace",
                    div { class: "navigator-root", span { "Workspace" } }
                    for (output, assigned_output) in navigator_outputs.iter() {
                        {
                            let output_name = output.name.clone();
                            let selected_ancestor = view.selected_canvas.as_ref().is_some_and(|selected| view.canvases.iter().any(|canvas| &canvas.id == selected && canvas.output.as_ref() == assigned_output.as_ref()));
                            let output_open = selected_ancestor
                                || expanded_outputs.read().contains(&output.name)
                                || view.active_output.as_ref() == Some(&output.name)
                                    && !collapsed_outputs.read().contains(&output.name);
                            rsx! {
                                NavigatorItem {
                                    key: "{output.name}",
                                    class: "workspace-output-option",
                                    label: output.name.clone(),
                                    depth: 0,
                                    selected: assigned_output.as_ref().is_some_and(|assigned| view.active_output.as_ref() == Some(assigned)),
                                    expanded: output_open,
                                    "data-output": output.name.clone(),
                                    onclick: { let assigned_output = assigned_output.clone(); move |_| if let Some(output) = &assigned_output { onaction.call(EditorAction::SelectOutput(output.clone())); } },
                                    ontoggle: move |_| {
                                        if output_open {
                                            expanded_outputs.write().remove(&output_name);
                                            collapsed_outputs.write().insert(output_name.clone());
                                        } else {
                                            collapsed_outputs.write().remove(&output_name);
                                            expanded_outputs.write().insert(output_name.clone());
                                        }
                                    },
                                    for canvas in view.canvases.iter().filter(|canvas| canvas.output.as_ref() == assigned_output.as_ref()) {
                                        {
                                            let canvas_id = canvas.id.clone();
                                            let canvas_open = expanded_canvases.read().contains(&canvas.id) || view.selected_canvas.as_ref() == Some(&canvas.id);
                                            rsx! {
                                                NavigatorItem {
                                                    key: "{canvas.id}",
                                                    class: "canvas-select",
                                                    label: canvas.name.clone(),
                                                    depth: 1,
                                                    selected: view.selected_canvas.as_ref() == Some(&canvas.id) && view.selected_widget.is_none(),
                                                    expanded: canvas_open,
                                                    "data-canvas-id": canvas.id.clone(),
                                                    onclick: { let canvas_id = canvas_id.clone(); move |_| { expanded_canvases.write().insert(canvas_id.clone()); onaction.call(EditorAction::SelectCanvas(canvas_id.clone())); } },
                                                    ontoggle: move |_| { let mut values = expanded_canvases.write(); if !values.remove(&canvas_id) { values.insert(canvas_id.clone()); } },
                                                    for widget in canvas.widgets.iter() {
                                                        NavigatorItem {
                                                            key: "{canvas.id}:{widget.id}",
                                                            class: "widget-row",
                                                            label: widget_label(widget, &canvas.widgets),
                                                            depth: 2,
                                                            selected: view.selected_canvas.as_ref() == Some(&canvas.id) && view.selected_widget.as_ref() == Some(&widget.id),
                                                            "data-widget-id": widget.id.clone(),
                                                            onclick: { let canvas_id = canvas.id.clone(); let widget_id = widget.id.clone(); move |_| onaction.call(EditorAction::SelectWidget { canvas_id: canvas_id.clone(), widget_id: widget_id.clone() }) },
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "navigator-add",
                if canvas.is_some() {
                    ListPicker {
                        class: "widget-picker",
                        label: "ADD",
                        value: "Widget",
                        options: widget_options,
                        selected: 0,
                        open: view.chrome.widget_add_open,
                        disabled: view.access.readonly,
                        onopen: move |open| if open != view.chrome.widget_add_open { onaction.call(EditorAction::ToggleWidgetAdd); },
                        onselect: move |index| onaction.call(EditorAction::AddWidget(index)),
                    }
                } else {
                    Button { class: "add-canvas", disabled: view.access.readonly || view.active_output.is_none(), onclick: move |_| onaction.call(EditorAction::AddCanvas), "+ Add canvas" }
                }
            }
        }
    }
}

#[component]
pub fn Inspector(
    view: EditorView,
    title_input: Element,
    onvalidity: EventHandler<(String, bool)>,
    onaction: EventHandler<EditorAction>,
) -> Element {
    let canvas = view
        .canvases
        .iter()
        .find(|canvas| Some(canvas.id.as_str()) == view.selected_canvas.as_deref());
    rsx! {
        main { class: "object-inspector",
            div { class: "pane-heading", strong { "INSPECTOR" } }
            div { class: "inspector-scroll",
                Accordion { label: "Object properties",
                    if let Some(canvas) = canvas {
                        if let Some(widget) = canvas.widgets.iter().find(|widget| view.selected_widget.as_deref() == Some(widget.id.as_str())) {
                            div { key: "{canvas.id}:{widget.id}",
                                AccordionSection { title: "Geometry", {geometry_fields(GeometrySpec { rect: [widget.x, widget.y, i32::try_from(widget.width).unwrap_or(i32::MAX), i32::try_from(widget.height).unwrap_or(i32::MAX)], bounds: [canvas.width, canvas.height], minimum: [16, 16], key: &format!("{}:{}", canvas.id, widget.id), widget: true, readonly: view.access.readonly }, onvalidity, onaction)} }
                                AccordionSection { title: "Widget settings", {widget_settings(widget, &view, title_input.clone(), onaction)} }
                                if let Some(skin) = view.skins.iter().find(|skin| skin.id == canvas.skin) { if let Some(properties) = skin.widget_properties.get(widget.kind.name()).or_else(|| skin.widget_properties.get("*")) { AccordionSection { title: "Style", {property_controls(properties, &widget.skin_properties, false, view.access.readonly, onaction)} } } }
                            }
                        } else {
                            div { key: "{canvas.id}", {canvas_inspector(canvas, &view, onvalidity, onaction)} }
                        }
                    } else {
                        div { class: "inspector-empty", strong { "Select an object" } p { "Choose an output or add a canvas to begin editing." } }
                        AccordionSection { title: "New canvas skin", {skin_picker(&view, view.new_canvas_skin, true, onaction)} }
                    }
                }
            }
        }
    }
}

#[component]
pub fn ActionBar(
    view: EditorView,
    invalid_input: bool,
    onaction: EventHandler<EditorAction>,
) -> Element {
    rsx! {
        footer { class: "editor-action-bar",
            Button { class: "undo-action", onclick: move |_| onaction.call(EditorAction::Undo), disabled: view.access.readonly || !view.access.undo_available, "Undo" }
            div { class: "footer-actions",
                if view.access.dirty {
                    Button { class: "discard-action", disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::Discard), "Discard" }
                    Button { class: "save-action", tone: ButtonTone::Primary, disabled: view.access.readonly || view.access.save_validity == SaveValidity::Invalid || invalid_input, onclick: move |_| onaction.call(EditorAction::Save), "Save & Close" }
                } else {
                    Button { class: "close-action", onclick: move |_| onaction.call(EditorAction::Close), "Close" }
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

pub(crate) fn widget_label(widget: &WidgetLayout, widgets: &[WidgetLayout]) -> String {
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
    let bounds = canvas_geometry_bounds(canvas, &view.outputs);
    let child_min = canvas.widgets.iter().fold([32, 32], |minimum, widget| {
        [
            minimum[0].max(u32::try_from(widget.x).unwrap_or_default() + widget.width),
            minimum[1].max(u32::try_from(widget.y).unwrap_or_default() + widget.height),
        ]
    });
    rsx! {
        AccordionSection { title: "Identity", TextField { field_key: format!("{}:name", canvas.id), label: format!("Name · {}", canvas.id), value: canvas.name.clone(), disallowed, disabled: view.access.readonly, update_on_input: true, onchange: move |value| onaction.call(EditorAction::CanvasName(value)), onvalidity } }
        AccordionSection { title: "Geometry", {geometry_fields(GeometrySpec { rect: [canvas.x, canvas.y, i32::try_from(canvas.width).unwrap_or(i32::MAX), i32::try_from(canvas.height).unwrap_or(i32::MAX)], bounds, minimum: child_min, key: &canvas.id, widget: false, readonly: view.access.readonly }, onvalidity, onaction)} div { class: "geometry-action", Button { class: "fit-output", disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::FitToOutput), "Fit to output" } } }
        AccordionSection { title: "Visibility", div { class: "visibility-actions", Button { disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::CanvasVisibleAll), "All" } Button { disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::CanvasVisibleNone), "None" } } {visibility_toggles(&canvas.id, canvas.show_on.as_deref(), view.access.readonly, onaction)} }
        AccordionSection { title: "Appearance", {skin_picker(view, canvas.skin, false, onaction)} div { class: "control-heading", "Opacity" } SegmentedControl { class: "opacity-control", label: "Canvas opacity", for value in [25, 50, 75, 100] { Button { class: "opacity-option", selected: canvas.opacity_percent == value, disabled: view.access.readonly, "data-value": value, onclick: move |_| onaction.call(EditorAction::Opacity(value)), "{value}%" } } } if let Some(skin) = view.skins.iter().find(|skin| skin.id == canvas.skin) { {property_controls(&skin.canvas_properties, &canvas.skin_properties, true, view.access.readonly, onaction)} } }
        AccordionSection { title: "Output", div { class: "output-list", for output in view.outputs.iter() { Button { class: "output-option", layout: ButtonLayout::Row, selected: canvas.output.as_deref() == Some(output.name.as_str()), disabled: view.access.readonly, "data-output": "{output.name}", onclick: { let output = output.name.clone(); move |_| onaction.call(EditorAction::Output(output.clone())) }, strong { "{output.name}" } } } } }
        AccordionSection { title: "Danger zone", Button { class: "delete-canvas", tone: ButtonTone::Danger, disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::DeleteCanvas), "Delete canvas" } }
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
    rsx! { ToggleGroup { class: "visibility-grid", label: "Canvas visibility", for (index, screen) in crate::editor_model::SCREENS.into_iter().enumerate() {
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
    rsx! { div { class: "skin-picker", for (index, skin) in view.skins.iter().enumerate() { Button { class: "skin-option", layout: ButtonLayout::Row, selected: selected == skin.id, disabled: view.access.readonly, "data-index": index, onclick: { let skin = skin.id; move |_| if new_canvas { onaction.call(EditorAction::NewCanvasSkin(skin)); } else { onaction.call(EditorAction::Skin(skin)); } }, if !skin.preview.is_empty() { img { src: "{skin.preview}", alt: "" } } span { strong { "{skin.name}" } small { "{skin.release}" } } } } } }
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
    rsx! { section { class:if canvas {"skin-property-list canvas-property-list"} else {"skin-property-list widget-property-list"},
        for (key,property) in properties {
        div { class:"skin-property", "data-property":key,
            div { class:"property-heading",
                label { class:"property-label", title:"{key}", "{property_key_display(properties,key)}" }
            }
            match property {
                EditorProperty::Boolean{default} => { let value=values.get(key).and_then(serde_json::Value::as_bool).unwrap_or(*default); rsx!{Toggle{class:"property-toggle",label:if value{"Enabled"}else{"Disabled"},selected:value,disabled:readonly,onclick:{let key=key.clone();move |_|action(key.clone(),(!value).into())}}} },
                EditorProperty::Enum{default,values:options} => { let value=values.get(key).and_then(serde_json::Value::as_str).unwrap_or(default); rsx!{SegmentedControl{class:"property-options",label:property_key_display(properties,key),for (index,option) in options.iter().enumerate(){Button{class:"property-option",disabled:readonly,selected:value==option,onclick:{let key=key.clone();let option=option.clone();move |_|action(key.clone(),option.clone().into())},"data-index":index,"data-value":option,title:"{option}",span{class:"property-option-label","{property_option_display(options,option)}"}}}}} },
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
                        div { class:"widget-settings",
                            strong { "{widget.id}" }
                            if widget.kind == WidgetKind::Empty {
                                h3 { "TITLE" }
                                if view.title!=EditorTitleState::Closed {
                                    {title_input}
                                    div { class:"button-grid", Button { class:"title-accept", onclick:move |_| onaction.call(EditorAction::AcceptTitle), disabled:view.access.readonly || view.title==EditorTitleState::Composing, "APPLY TITLE" } Button { class:"title-cancel", onclick:move |_| onaction.call(EditorAction::CancelTitle), disabled:view.access.readonly, "CANCEL" } }
                                } else { Button { class:"empty-title-input", onclick:move |_| onaction.call(EditorAction::EditTitle), disabled:view.access.readonly, if widget.settings.title.is_empty(){"Enter title…"}else{"{widget.settings.title}"} } }
                                h3 { "ASPECT RATIO" }
                                SegmentedControl { class:"aspect-ratio-options", label:"Aspect ratio", for (index,label) in ["FREE","16:9","4:3","CURRENT"].into_iter().enumerate() { Button { class:"aspect-ratio", onclick:move |_| onaction.call(EditorAction::AspectRatio(index)), disabled:view.access.readonly, selected:aspect_ratio_index(widget.settings.aspect_ratio)==index , "data-index":index, "{label}" } } }
                            }
                            if widget.kind == WidgetKind::HistoryList {
                                SegmentedControl { class:"history-control", label:"History rows", for value in [5,10,20,50] { Button { class:"history-count", onclick:move |_| onaction.call(EditorAction::HistoryCount(value)), disabled:view.access.readonly, selected:widget.settings.history_count==value, "data-value":value, "{value}" } } }
                            }
                            if widget.kind == WidgetKind::HistoryGraph {
                                SegmentedControl { class:"graph-control", label:"Graph range", for value in [1,3,6,12] { Button { class:"graph-months", onclick:move |_| onaction.call(EditorAction::GraphMonths(value)), disabled:view.access.readonly, selected:widget.settings.graph_months==value, "data-value":value, "{value}M" } } }
                            }
                            div { class:"widget-delete-actions", Button { class:"delete-widget", onclick:move |_| onaction.call(EditorAction::DeleteWidget), disabled:view.access.readonly, tone:ButtonTone::Danger, "DELETE WIDGET" } }
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
