//! Editor controls share their layout and state styles across rendering backends.
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

mod components;
const UNASSIGNED_OUTPUT_NAME: &str = "Unassigned";
pub mod effect;
pub mod model;
pub mod runtime;
pub use components::{
    Accordion, AccordionSection, Button, IconButton, ListPicker, ListPickerOption, NavigatorItem,
    NavigatorTree, NumberField, SegmentedControl, TextField, Toggle, ToggleGroup,
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

pub use crate::action::{EditorAction, EditorFieldCommit, GeometryField};
use crate::{AspectRatio, CanvasPresentation, ScreenKind, Skin, WidgetKind, WidgetLayout};

#[derive(Clone, Debug, PartialEq)]
pub struct EditorOutput {
    pub name: String,
    pub model: String,
    pub logical_size: Option<[u32; 2]>,
}

#[derive(Clone, PartialEq)]
// These controls are independent disclosures, not mutually exclusive states.
#[allow(clippy::struct_excessive_bools)]
pub struct EditorChrome {
    pub panel_open: bool,
    pub widget_add_open: bool,
    pub sample: bool,
    pub screen_picker_open: bool,
    pub output_picker_open: bool,
    pub expanded_outputs: std::collections::BTreeSet<String>,
    pub expanded_canvases: std::collections::BTreeSet<String>,
    pub collapsed_accordions: std::collections::BTreeSet<String>,
    pub field_drafts: std::collections::BTreeMap<String, EditorFieldDraft>,
    pub picker_cursors: std::collections::BTreeMap<String, usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EditorFieldDraft {
    pub text: String,
    pub focused: bool,
    pub valid: bool,
    pub composing: bool,
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
    pub widget_defaults: std::collections::BTreeMap<String, EditorWidgetDefault>,
    #[serde(default)]
    pub canvas_properties: std::collections::BTreeMap<String, EditorProperty>,
    #[serde(default)]
    pub widget_properties:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, EditorProperty>>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct EditorWidgetDefault {
    pub width: u32,
    pub height: u32,
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
    #[must_use]
    pub fn effective(&self, value: Option<&serde_json::Value>) -> serde_json::Value {
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

    pub(crate) fn parse_text(&self, text: &str) -> Option<serde_json::Value> {
        match self {
            Self::Integer {
                minimum, maximum, ..
            } => text
                .parse::<i64>()
                .ok()
                .filter(|value| minimum <= value && value <= maximum)
                .map(Into::into),
            Self::Number {
                minimum, maximum, ..
            } => text.parse::<f64>().ok().and_then(|value| {
                (value.is_finite() && *minimum <= value && value <= *maximum)
                    .then(|| serde_json::Number::from_f64(value).map(Into::into))
                    .flatten()
            }),
            Self::Color { .. } if valid_property_color(text) => Some(text.into()),
            Self::String { maximum_length, .. } if valid_property_string(text, *maximum_length) => {
                Some(text.into())
            }
            Self::Boolean { .. } | Self::Color { .. } | Self::Enum { .. } | Self::String { .. } => {
                None
            }
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct EditorView {
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
    pub new_canvas_skin: Option<Skin>,
}

#[must_use]
pub fn document_valid(canvases: &[CanvasPresentation]) -> bool {
    let mut names = std::collections::BTreeSet::new();
    canvases.iter().all(|canvas| {
        let dimensions_valid = (32..=crate::geometry::MAX_DIMENSION).contains(&canvas.width)
            && (32..=crate::geometry::MAX_DIMENSION).contains(&canvas.height);
        let widgets_valid = canvas.widgets.iter().all(|widget| {
            (16..=crate::geometry::MAX_DIMENSION).contains(&widget.width)
                && (16..=crate::geometry::MAX_DIMENSION).contains(&widget.height)
        });
        !canvas.name.trim().is_empty()
            && names.insert(canvas.name.clone())
            && canvas
                .output
                .as_ref()
                .is_some_and(|output| !output.is_empty())
            && dimensions_valid
            && widgets_valid
    })
}

#[component]
pub fn EditorPanel(
    view: EditorView,
    title: Option<crate::editor::model::TitleDraft>,
    onaction: EventHandler<EditorAction>,
) -> Element {
    let canvas = view
        .canvases
        .iter()
        .find(|canvas| Some(canvas.id.as_str()) == view.selected_canvas.as_deref());
    let active_field_prefix = canvas.map(|canvas| {
        view.selected_widget.as_ref().map_or_else(
            || format!("{}:", canvas.id),
            |widget| format!("{}:{widget}:", canvas.id),
        )
    });
    let invalid_input = active_field_prefix.as_ref().is_some_and(|prefix| {
        view.chrome
            .field_drafts
            .iter()
            .any(|(field, draft)| field.starts_with(prefix) && !draft.valid)
    });
    rsx! {
        if !view.chrome.panel_open {
            CollapsedEditorButton { dirty: view.access.dirty, onaction }
        } else {
            aside { class: "editor-panel", style: format!("width:{}px", view.panel_width),
                ContextBar { view: view.clone(), onaction }
                EditorWorkspace { view: view.clone(), title, onaction }
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
    let output_names = view
        .outputs
        .iter()
        .map(|output| output.name.clone())
        .collect::<Vec<_>>();
    let options = crate::editor::model::SCREENS
        .into_iter()
        .map(|screen| ListPickerOption {
            label: screen_label(screen).into(),
            detail: None,
        })
        .collect::<Vec<_>>();
    let selected = crate::editor::model::SCREENS
        .iter()
        .position(|screen| *screen == view.preview_screen)
        .unwrap_or_default();
    rsx! {
        header { class: "editor-context-bar",
            IconButton { class: "editor-panel-toggle", label: "Hide editor panel", onclick: move |_| onaction.call(EditorAction::TogglePanel), "‹" }
            ListPicker {
                class: "editor-output-picker",
                label: "Editor output",
                value: "▣",
                options: view.outputs.iter().map(|output| ListPickerOption { label: output.name.clone(), detail: None }).collect(),
                selected: view.outputs.iter().position(|output| Some(&output.name) == view.active_output.as_ref()).unwrap_or_default(),
                cursor: view.chrome.picker_cursors.get("editor-output").copied().unwrap_or_default(),
                open: view.chrome.output_picker_open,
                onopen: move |open| onaction.call(EditorAction::SetOutputPickerOpen(open)),
                onselect: move |index: usize| if let Some(output) = output_names.get(index) { onaction.call(EditorAction::SelectOutput(output.clone())); },
                oncursor: move |index| onaction.call(EditorAction::SetPickerCursor("editor-output".into(), index)),
            }
            ListPicker {
                class: "screen-picker",
                label: "GAME SCREEN",
                value: screen_label(view.preview_screen),
                options,
                selected,
                cursor: view.chrome.picker_cursors.get("screen").copied().unwrap_or(selected),
                open: view.chrome.screen_picker_open,
                onopen: move |open| onaction.call(EditorAction::SetScreenPickerOpen(open)),
                onselect: move |index| if let Some(screen) = crate::editor::model::SCREENS.get(index) { onaction.call(EditorAction::PreviewScreen(*screen)); },
                oncursor: move |index| onaction.call(EditorAction::SetPickerCursor("screen".into(), index)),
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
    title: Option<crate::editor::model::TitleDraft>,
    onaction: EventHandler<EditorAction>,
) -> Element {
    rsx! {
        div { class: if view.selected_canvas.is_some() { "editor-workspace has-selection" } else { "editor-workspace" },
            ObjectNavigator { view: view.clone(), onaction }
            if view.selected_canvas.is_some() { Inspector { view: view.clone(), title, onaction } }
        }
    }
}

#[component]
pub fn ObjectNavigator(view: EditorView, onaction: EventHandler<EditorAction>) -> Element {
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
                name: UNASSIGNED_OUTPUT_NAME.into(),
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
                            let output_open = view.chrome.expanded_outputs.contains(&output.name);
                            rsx! {
                                NavigatorItem {
                                    key: "{output.name}",
                                    class: "workspace-output-option",
                                    label: output.name.clone(),
                                    depth: 0,
                                    selected: false,
                                    expanded: output_open,
                                    "data-output": output.name.clone(),
                                    onclick: move |_| onaction.call(EditorAction::ToggleOutputExpanded(output_name.clone())),
                                    ontoggle: { let output_name = output.name.clone(); move |_| onaction.call(EditorAction::ToggleOutputExpanded(output_name.clone())) },
                                    for canvas in view.canvases.iter().filter(|canvas| canvas.output.as_ref() == assigned_output.as_ref()) {
                                        {
                                            let canvas_id = canvas.id.clone();
                                            let canvas_open = view.chrome.expanded_canvases.contains(&canvas.id) || view.selected_canvas.as_ref() == Some(&canvas.id);
                                            rsx! {
                                                NavigatorItem {
                                                    key: "{canvas.id}",
                                                    class: "canvas-select",
                                                    label: canvas.name.clone(),
                                                    depth: 1,
                                                    selected: view.selected_canvas.as_ref() == Some(&canvas.id) && view.selected_widget.is_none(),
                                                    expanded: canvas_open,
                                                    "data-canvas-id": canvas.id.clone(),
                                                    onclick: { let canvas_id = canvas_id.clone(); move |_| onaction.call(EditorAction::SelectCanvas(canvas_id.clone())) },
                                                    ontoggle: move |_| onaction.call(EditorAction::ToggleCanvasExpanded(canvas_id.clone())),
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
                ListPicker {
                        class: "widget-picker",
                        label: "Add widget",
                        value: "+ Add widget",
                        options: widget_options,
                        selected: 0,
                        cursor: view.chrome.picker_cursors.get("widget-add").copied().unwrap_or(0),
                        open: view.chrome.widget_add_open,
                        disabled: view.access.readonly || canvas.is_none(),
                        onopen: move |open| if open != view.chrome.widget_add_open { onaction.call(EditorAction::ToggleWidgetAdd); },
                        onselect: move |index| onaction.call(EditorAction::AddWidget(index)),
                        oncursor: move |index| onaction.call(EditorAction::SetPickerCursor("widget-add".into(), index)),
                }
                Button { class: "add-canvas", disabled: view.access.readonly || view.active_output.is_none() || view.new_canvas_skin.is_none(), onclick: move |_| onaction.call(EditorAction::AddCanvas), "+ Add canvas" }
            }
        }
    }
}

#[component]
pub fn Inspector(
    view: EditorView,
    title: Option<crate::editor::model::TitleDraft>,
    onaction: EventHandler<EditorAction>,
) -> Element {
    let canvas = view
        .canvases
        .iter()
        .find(|canvas| Some(canvas.id.as_str()) == view.selected_canvas.as_deref());
    rsx! {
        main { class: "object-inspector",
            div { class: "pane-heading", strong { "INSPECTOR" } IconButton { class: "inspector-close", label: "Clear selection", onclick: move |_| onaction.call(EditorAction::ClearSelection), "×" } }
            div { class: "inspector-scroll",
                Accordion { label: "Object properties",
                    if let Some(canvas) = canvas {
                        if let Some(widget) = canvas.widgets.iter().find(|widget| view.selected_widget.as_deref() == Some(widget.id.as_str())) {
                            div { key: "{canvas.id}:{widget.id}",
                                {editor_accordion(format!("widget:{}:{}:geometry", canvas.id, widget.id), "Geometry", &view, onaction, geometry_fields(GeometrySpec { rect: [widget.x, widget.y, i32::try_from(widget.width).unwrap_or(i32::MAX), i32::try_from(widget.height).unwrap_or(i32::MAX)], key: &format!("{}:{}", canvas.id, widget.id), widget: true, readonly: view.access.readonly }, &view, onaction))}
                                {editor_accordion(format!("widget:{}:{}:settings", canvas.id, widget.id), "Widget settings", &view, onaction, widget_settings(widget, &view, title.clone(), onaction))}
                                if let Some(skin) = view.skins.iter().find(|skin| skin.id == canvas.skin) { if let Some(properties) = skin.widget_properties.get(widget.kind.name()).or_else(|| skin.widget_properties.get("*")) { {editor_accordion(format!("widget:{}:{}:style", canvas.id, widget.id), "Style", &view, onaction, property_controls(properties, &widget.skin_properties, &format!("{}:{}",canvas.id,widget.id), false, &view, onaction))} } }
                            }
                        } else {
                            div { key: "{canvas.id}", {canvas_inspector(canvas, &view, onaction)} }
                        }
                    } else {
                        div { class: "inspector-empty", strong { "Select an object" } p { "Choose an output or add a canvas to begin editing." } }
                        if let Some(skin) = view.new_canvas_skin {
                            {editor_accordion("new-canvas:skin".into(), "New canvas skin", &view, onaction, skin_picker(&view, skin, true, onaction))}
                        } else {
                            p { class: "editor-help", "Install a skin before adding a canvas." }
                        }
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
    key: &'a str,
    widget: bool,
    readonly: bool,
}

fn geometry_fields(
    spec: GeometrySpec<'_>,
    view: &EditorView,
    onaction: EventHandler<EditorAction>,
) -> Element {
    let [x, y, width, height] = spec.rect;
    let minimum = if spec.widget { 16 } else { 32 };
    let maximum = i32::try_from(crate::geometry::MAX_DIMENSION).unwrap_or(i32::MAX);
    let commit = move |field| {
        if spec.widget {
            EditorFieldCommit::WidgetGeometry(field)
        } else {
            EditorFieldCommit::CanvasGeometry(field)
        }
    };
    rsx! { div { class: "geometry-grid",
        NumberField { field_key: format!("{}:x", spec.key), label: "X", value: x, minimum: i32::MIN, maximum: i32::MAX, disabled: spec.readonly, draft:view.chrome.field_drafts.get(&format!("{}:x", spec.key)).cloned(), commit: commit(GeometryField::X), onstate:onaction }
        NumberField { field_key: format!("{}:y", spec.key), label: "Y", value: y, minimum: i32::MIN, maximum: i32::MAX, disabled: spec.readonly, draft:view.chrome.field_drafts.get(&format!("{}:y", spec.key)).cloned(), commit: commit(GeometryField::Y), onstate:onaction }
        NumberField { field_key: format!("{}:width", spec.key), label: "Width", value: width, minimum, maximum, disabled: spec.readonly, draft:view.chrome.field_drafts.get(&format!("{}:width", spec.key)).cloned(), commit: commit(GeometryField::Width), onstate:onaction }
        NumberField { field_key: format!("{}:height", spec.key), label: "Height", value: height, minimum, maximum, disabled: spec.readonly, draft:view.chrome.field_drafts.get(&format!("{}:height", spec.key)).cloned(), commit: commit(GeometryField::Height), onstate:onaction }
    } }
}

fn canvas_inspector(
    canvas: &CanvasPresentation,
    view: &EditorView,
    onaction: EventHandler<EditorAction>,
) -> Element {
    let disallowed = view
        .canvases
        .iter()
        .filter(|candidate| candidate.id != canvas.id)
        .map(|candidate| candidate.name.clone())
        .collect::<Vec<_>>();
    rsx! {
        {editor_accordion(format!("canvas:{}:identity", canvas.id), "Identity", view, onaction, rsx! { TextField { field_key: format!("{}:name", canvas.id), label: format!("Name · {}", canvas.id), value: canvas.name.clone(), disallowed, disabled: view.access.readonly, update_on_input: true, draft:view.chrome.field_drafts.get(&format!("{}:name", canvas.id)).cloned(), onchange: move |value| onaction.call(EditorAction::CanvasName(value)), onstate:onaction } })}
        {editor_accordion(format!("canvas:{}:geometry", canvas.id), "Geometry", view, onaction, rsx! { {geometry_fields(GeometrySpec { rect: [canvas.x, canvas.y, i32::try_from(canvas.width).unwrap_or(i32::MAX), i32::try_from(canvas.height).unwrap_or(i32::MAX)], key: &canvas.id, widget: false, readonly: view.access.readonly }, view, onaction)} div { class: "geometry-action", Button { class: "fit-output", disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::FitToOutput), "Fit to output" } } })}
        {editor_accordion(format!("canvas:{}:visibility", canvas.id), "Visibility", view, onaction, rsx! { div { class: "visibility-actions", Button { disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::CanvasVisibleAll), "All" } Button { disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::CanvasVisibleNone), "None" } } {visibility_toggles(&canvas.id, canvas.show_on.as_deref(), view.access.readonly, onaction)} })}
        {editor_accordion(format!("canvas:{}:appearance", canvas.id), "Appearance", view, onaction, rsx! { {skin_picker(view, canvas.skin, false, onaction)} div { class: "control-heading", "Opacity" } SegmentedControl { class: "opacity-control", label: "Canvas opacity", for value in [25, 50, 75, 100] { Button { class: "opacity-option", selected: canvas.opacity_percent == value, disabled: view.access.readonly, "data-value": value, onclick: move |_| onaction.call(EditorAction::Opacity(value)), "{value}%" } } } if let Some(skin) = view.skins.iter().find(|skin| skin.id == canvas.skin) { {property_controls(&skin.canvas_properties, &canvas.skin_properties, &canvas.id, true, view, onaction)} } })}
        {editor_accordion(format!("canvas:{}:output", canvas.id), "Output", view, onaction, rsx! { div { class: "output-list", for output in view.outputs.iter() { Button { class: "output-option", layout: ButtonLayout::Row, selected: canvas.output.as_deref() == Some(output.name.as_str()), disabled: view.access.readonly, "data-output": "{output.name}", onclick: { let output = output.name.clone(); move |_| onaction.call(EditorAction::Output(output.clone())) }, strong { "{output.name}" } } } } })}
        {editor_accordion(format!("canvas:{}:danger", canvas.id), "Danger zone", view, onaction, rsx! { Button { class: "delete-canvas", tone: ButtonTone::Danger, disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::DeleteCanvas), "Delete canvas" } })}
    }
}

fn editor_accordion(
    section_key: String,
    title: &str,
    view: &EditorView,
    onaction: EventHandler<EditorAction>,
    children: Element,
) -> Element {
    let open = !view.chrome.collapsed_accordions.contains(&section_key);
    let action_key = section_key.clone();
    rsx! {
        AccordionSection {
            title,
            section_key,
            open,
            ontoggle: move |_| onaction.call(EditorAction::ToggleAccordion(action_key.clone())),
            {children}
        }
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
    rsx! { ToggleGroup { class: "visibility-grid", label: "Canvas visibility", for (index, screen) in crate::editor::model::SCREENS.into_iter().enumerate() {
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

#[derive(Props, Clone, PartialEq)]
struct PropertyInputProps {
    field_key: String,
    class: String,
    value: String,
    #[props(default)]
    numeric: bool,
    disabled: bool,
    draft: Option<EditorFieldDraft>,
    validate: Callback<String, bool>,
    commit: EditorFieldCommit,
    onstate: EventHandler<EditorAction>,
}

#[component]
fn PropertyInput(props: PropertyInputProps) -> Element {
    let displayed = props
        .draft
        .as_ref()
        .map_or_else(|| props.value.clone(), |draft| draft.text.clone());
    let valid = props.validate.call(displayed.clone());
    let commit = Callback::new({
        let props = props.clone();
        move |()| {
            props.onstate.call(EditorAction::CommitFieldDraft(
                props.field_key.clone(),
                props.commit.clone(),
            ));
        }
    });
    let input_props = props.clone();
    let composition_start_props = props.clone();
    let composition_end_props = props.clone();
    rsx! {
        input {
            id: props.field_key.clone(),
            class: props.class,
            r#type: "text",
            inputmode: props.numeric.then_some("decimal"),
            value: displayed.clone(),
            disabled: props.disabled,
            "aria-invalid": (!valid).then_some("true"),
            onfocus: move |_| props.onstate.call(EditorAction::BeginFieldEdit(
                props.field_key.clone(), displayed.clone(),
            )),
            oninput: move |event| {
                let text = event.value();
                let valid = input_props.validate.call(text.clone());
                input_props.onstate.call(EditorAction::UpdateFieldDraft(
                    input_props.field_key.clone(), text, valid,
                ));
            },
            oncompositionstart: move |_| composition_start_props.onstate.call(EditorAction::TextComposition { field_key:composition_start_props.field_key.clone(), composing:true }),
            oncompositionend: move |_| composition_end_props.onstate.call(EditorAction::TextComposition { field_key:composition_end_props.field_key.clone(), composing:false }),
            onblur: move |_| commit.call(()),
            onkeydown: move |event| if event.key() == Key::Enter && !event.is_composing() {
                event.prevent_default();
                commit.call(());
            },
        }
    }
}

fn property_controls(
    properties: &std::collections::BTreeMap<String, EditorProperty>,
    values: &std::collections::BTreeMap<String, serde_json::Value>,
    scope: &str,
    canvas: bool,
    view: &EditorView,
    onaction: EventHandler<EditorAction>,
) -> Element {
    let readonly = view.access.readonly;
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
                EditorProperty::Integer{default,minimum,maximum} => {
                    let value=values.get(key).and_then(serde_json::Value::as_i64).unwrap_or(*default);
                    let unit=property_unit(key);
                    let field_key=format!("{scope}:property:{key}");
                    let draft=view.chrome.field_drafts.get(&field_key).cloned();
                    let minimum=*minimum;
                    let maximum=*maximum;
                    rsx!{div{class:"property-number-control",div{class:"property-value",
                        PropertyInput{field_key,class:"property-value-input",value:value.to_string(),numeric:true,disabled:readonly,draft,
                            validate:Callback::new(move|text:String|integer_property_value(&text,minimum,maximum).is_some()),
                            commit:if canvas {EditorFieldCommit::CanvasSkinProperty(key.clone())} else {EditorFieldCommit::WidgetSkinProperty(key.clone())},onstate:onaction}
                        if !unit.is_empty(){span{"{unit}"}}} small{class:"property-range","{minimum}–{maximum}{unit}"}}}
                },
                EditorProperty::Number{default,minimum,maximum} => {
                    let value=values.get(key).and_then(serde_json::Value::as_f64).unwrap_or(*default);
                    let unit=property_unit(key);
                    let field_key=format!("{scope}:property:{key}");
                    let draft=view.chrome.field_drafts.get(&field_key).cloned();
                    let minimum=*minimum;
                    let maximum=*maximum;
                    rsx!{div{class:"property-number-control",div{class:"property-value",
                        PropertyInput{field_key,class:"property-value-input",value:value.to_string(),numeric:true,disabled:readonly,draft,
                            validate:Callback::new(move|text:String|number_property_value(&text,minimum,maximum).is_some()),
                            commit:if canvas {EditorFieldCommit::CanvasSkinProperty(key.clone())} else {EditorFieldCommit::WidgetSkinProperty(key.clone())},onstate:onaction}
                        if !unit.is_empty(){span{"{unit}"}}} small{class:"property-range","{minimum}–{maximum}{unit}"}}}
                },
                EditorProperty::Color{default} => {
                    let value=values.get(key).and_then(serde_json::Value::as_str).unwrap_or(default).to_owned();
                    let field_key=format!("{scope}:property:{key}");
                    let draft=view.chrome.field_drafts.get(&field_key).cloned();
                    let displayed=draft.as_ref().map_or_else(||value.clone(),|draft|draft.text.clone());
                    rsx!{div{class:"property-color-control",span{class:"property-color-swatch",style:"background-color:{displayed}",aria_hidden:"true"}
                        PropertyInput{field_key,class:"property-color-input",value,disabled:readonly,draft,
                            validate:Callback::new(move|text:String|valid_property_color(&text)),
                            commit:if canvas {EditorFieldCommit::CanvasSkinProperty(key.clone())} else {EditorFieldCommit::WidgetSkinProperty(key.clone())},onstate:onaction}
                    }}
                },
                EditorProperty::String{default,maximum_length} => {
                    let value=values.get(key).and_then(serde_json::Value::as_str).unwrap_or(default).to_owned();
                    let field_key=format!("{scope}:property:{key}");
                    let draft=view.chrome.field_drafts.get(&field_key).cloned();
                    let maximum_length=*maximum_length;
                    rsx!{div{class:"property-string-control",
                        PropertyInput{field_key,class:"property-string-input",value,disabled:readonly,draft,
                            validate:Callback::new(move|text:String|valid_property_string(&text,maximum_length)),
                            commit:if canvas {EditorFieldCommit::CanvasSkinProperty(key.clone())} else {EditorFieldCommit::WidgetSkinProperty(key.clone())},onstate:onaction}
                        small{class:"property-limit","MAX {maximum_length}"}}}
                },
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
    title: Option<crate::editor::model::TitleDraft>,
    onaction: EventHandler<EditorAction>,
) -> Element {
    rsx! {
                        div { class:"widget-settings",
                            strong { "{widget.id}" }
                            if widget.kind == WidgetKind::Empty {
                                h3 { "TITLE" }
                                if view.title!=EditorTitleState::Closed {
                                    if let Some(title)=title {
                                        input {
                                            id: "editor-title-input",
                                            class: "empty-title-edit",
                                            r#type: "text",
                                            "aria-label": "Widget title",
                                            value: "{title.text}",
                                            disabled: view.access.readonly,
                                            onmounted: move |event| async move { let _ = event.set_focus(true).await; },
                                            oninput: move |event| onaction.call(EditorAction::TitleText(event.value())),
                                            oncompositionstart: move |_| onaction.call(EditorAction::TextComposition { field_key:"editor-title-input".into(), composing:true }),
                                            oncompositionend: move |_| onaction.call(EditorAction::TextComposition { field_key:"editor-title-input".into(), composing:false }),
                                            onkeydown: move |event| {
                                                if event.key() == Key::Escape {
                                                    event.prevent_default();
                                                    onaction.call(EditorAction::CancelTitle);
                                                } else if event.key() == Key::Enter && !event.is_composing() {
                                                    event.prevent_default();
                                                    onaction.call(EditorAction::AcceptTitle);
                                                }
                                            },
                                        }
                                    }
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
