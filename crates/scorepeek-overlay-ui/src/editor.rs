//! Editor controls share their layout and state styles across rendering backends.
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

mod components;
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

use crate::{
    AspectRatio, Background, CanvasPresentation, FrameWidth, ScreenKind, Skin, WidgetKind,
    WidgetLayout,
};

#[derive(Clone, Debug, PartialEq)]
pub enum EditorAction {
    TogglePanel,
    SetScreenPickerOpen(bool),
    SetPickerCursor(String, usize),
    BeginFieldEdit(String, String),
    UpdateFieldDraft(String, String, bool),
    EndFieldEdit(String, bool),
    CommitFieldDraft(String, EditorFieldCommit),
    ToggleOutputExpanded(String),
    ToggleCanvasExpanded(String),
    ToggleAccordion(String),
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
    TitleText(String),
    TextComposition {
        field_key: String,
        composing: bool,
    },
    FillDelta(i8),
    FillOpacity(u8),
    AspectRatio(usize),
    HistoryCount(u32),
    GraphMonths(u32),
    DeleteWidget,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EditorFieldCommit {
    CanvasGeometry(GeometryField),
    WidgetGeometry(GeometryField),
    CanvasSkinProperty(String),
    WidgetSkinProperty(String),
}

impl EditorAction {
    /// Value-free semantic label attached before an action crosses a backend transport.
    #[must_use]
    pub const fn diagnostic_name(&self) -> &'static str {
        match self {
            Self::TogglePanel => "toggle_panel",
            Self::SetScreenPickerOpen(_) => "screen_picker",
            Self::SetPickerCursor(_, _) => "picker_cursor",
            Self::BeginFieldEdit(_, _) => "field_focus",
            Self::UpdateFieldDraft(_, _, _) => "field_input",
            Self::EndFieldEdit(_, _) => "field_blur",
            Self::CommitFieldDraft(_, _) => "field_commit",
            Self::ToggleOutputExpanded(_) => "output_disclosure",
            Self::ToggleCanvasExpanded(_) => "canvas_disclosure",
            Self::ToggleAccordion(_) => "inspector_disclosure",
            Self::PreviewScreen(_) => "preview_screen",
            Self::SelectOutput(_) => "select_output",
            Self::SelectCanvas(_) => "select_canvas",
            Self::CanvasName(_) => "canvas_name",
            Self::CanvasVisible(_, _) => "canvas_visibility",
            Self::CanvasVisibleAll => "canvas_visibility_all",
            Self::CanvasVisibleNone => "canvas_visibility_none",
            Self::CanvasGeometry(_, _) => "canvas_geometry",
            Self::WidgetGeometry(_, _) => "widget_geometry",
            Self::AddCanvas => "add_canvas",
            Self::DeleteCanvas => "delete_canvas",
            Self::NewCanvasSkin(_) => "new_canvas_skin",
            Self::Skin(_) => "skin",
            Self::CanvasSkinProperty(_, _) => "canvas_skin_property",
            Self::WidgetSkinProperty(_, _) => "widget_skin_property",
            Self::Background(_) => "background",
            Self::Opacity(_) => "opacity",
            Self::Output(_) => "output",
            Self::FitToOutput => "fit_to_output",
            Self::SelectWidget { .. } => "select_widget",
            Self::ToggleWidgetAdd => "toggle_widget_add",
            Self::AddWidget(_) => "add_widget",
            Self::Undo => "undo",
            Self::Discard => "discard",
            Self::Save => "save",
            Self::Close => "close",
            Self::FrameWidth(_) => "frame_width",
            Self::EditTitle => "edit_title",
            Self::AcceptTitle => "accept_title",
            Self::CancelTitle => "cancel_title",
            Self::TitleText(_) => "title_text",
            Self::TextComposition { .. } => "text_composition",
            Self::FillDelta(_) => "fill_delta",
            Self::FillOpacity(_) => "fill_opacity",
            Self::AspectRatio(_) => "aspect_ratio",
            Self::HistoryCount(_) => "history_count",
            Self::GraphMonths(_) => "graph_months",
            Self::DeleteWidget => "delete_widget",
        }
    }
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
// These controls are independent disclosures, not mutually exclusive states.
#[allow(clippy::struct_excessive_bools)]
pub struct EditorChrome {
    pub panel_open: bool,
    pub widget_add_open: bool,
    pub sample: bool,
    pub screen_picker_open: bool,
    pub expanded_outputs: std::collections::BTreeSet<String>,
    pub collapsed_outputs: std::collections::BTreeSet<String>,
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
    title: Option<crate::editor_model::TitleDraft>,
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
                cursor: view.chrome.picker_cursors.get("screen").copied().unwrap_or(selected),
                open: view.chrome.screen_picker_open,
                onopen: move |open| onaction.call(EditorAction::SetScreenPickerOpen(open)),
                onselect: move |index| if let Some(screen) = crate::editor_model::SCREENS.get(index) { onaction.call(EditorAction::PreviewScreen(*screen)); },
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
    title: Option<crate::editor_model::TitleDraft>,
    onaction: EventHandler<EditorAction>,
) -> Element {
    rsx! {
        div { class: "editor-workspace",
            ObjectNavigator { view: view.clone(), onaction }
            Inspector { view, title, onaction }
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
                                || view.chrome.expanded_outputs.contains(&output.name)
                                || view.active_output.as_ref() == Some(&output.name)
                                    && !view.chrome.collapsed_outputs.contains(&output.name);
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
                                    ontoggle: move |_| onaction.call(EditorAction::ToggleOutputExpanded(output_name.clone())),
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
                if canvas.is_some() {
                    ListPicker {
                        class: "widget-picker",
                        label: "ADD",
                        value: "Widget",
                        options: widget_options,
                        selected: 0,
                        cursor: view.chrome.picker_cursors.get("widget-add").copied().unwrap_or(0),
                        open: view.chrome.widget_add_open,
                        disabled: view.access.readonly,
                        onopen: move |open| if open != view.chrome.widget_add_open { onaction.call(EditorAction::ToggleWidgetAdd); },
                        onselect: move |index| onaction.call(EditorAction::AddWidget(index)),
                        oncursor: move |index| onaction.call(EditorAction::SetPickerCursor("widget-add".into(), index)),
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
    title: Option<crate::editor_model::TitleDraft>,
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
                                {editor_accordion(format!("widget:{}:{}:geometry", canvas.id, widget.id), "Geometry", &view, onaction, geometry_fields(GeometrySpec { rect: [widget.x, widget.y, i32::try_from(widget.width).unwrap_or(i32::MAX), i32::try_from(widget.height).unwrap_or(i32::MAX)], bounds: [canvas.width, canvas.height], minimum: [16, 16], key: &format!("{}:{}", canvas.id, widget.id), widget: true, readonly: view.access.readonly }, &view, onaction))}
                                {editor_accordion(format!("widget:{}:{}:settings", canvas.id, widget.id), "Widget settings", &view, onaction, widget_settings(widget, &view, title.clone(), onaction))}
                                if let Some(skin) = view.skins.iter().find(|skin| skin.id == canvas.skin) { if let Some(properties) = skin.widget_properties.get(widget.kind.name()).or_else(|| skin.widget_properties.get("*")) { {editor_accordion(format!("widget:{}:{}:style", canvas.id, widget.id), "Style", &view, onaction, property_controls(properties, &widget.skin_properties, &format!("{}:{}",canvas.id,widget.id), false, &view, onaction))} } }
                            }
                        } else {
                            div { key: "{canvas.id}", {canvas_inspector(canvas, &view, onaction)} }
                        }
                    } else {
                        div { class: "inspector-empty", strong { "Select an object" } p { "Choose an output or add a canvas to begin editing." } }
                        {editor_accordion("new-canvas:skin".into(), "New canvas skin", &view, onaction, skin_picker(&view, view.new_canvas_skin, true, onaction))}
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
    view: &EditorView,
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
    let commit = move |field| {
        if spec.widget {
            EditorFieldCommit::WidgetGeometry(field)
        } else {
            EditorFieldCommit::CanvasGeometry(field)
        }
    };
    rsx! { div { class: "geometry-grid",
        NumberField { field_key: format!("{}:x", spec.key), label: "X", value: x, minimum: 0, maximum: maximum_x, disabled: spec.readonly, draft:view.chrome.field_drafts.get(&format!("{}:x", spec.key)).cloned(), commit: commit(GeometryField::X), onstate:onaction }
        NumberField { field_key: format!("{}:y", spec.key), label: "Y", value: y, minimum: 0, maximum: maximum_y, disabled: spec.readonly, draft:view.chrome.field_drafts.get(&format!("{}:y", spec.key)).cloned(), commit: commit(GeometryField::Y), onstate:onaction }
        NumberField { field_key: format!("{}:width", spec.key), label: "Width", value: width, minimum: i32::try_from(spec.minimum[0]).unwrap_or(16), maximum: maximum_width, allow_maximum_off_grid: !spec.widget, disabled: spec.readonly, draft:view.chrome.field_drafts.get(&format!("{}:width", spec.key)).cloned(), commit: commit(GeometryField::Width), onstate:onaction }
        NumberField { field_key: format!("{}:height", spec.key), label: "Height", value: height, minimum: i32::try_from(spec.minimum[1]).unwrap_or(16), maximum: maximum_height, allow_maximum_off_grid: !spec.widget, disabled: spec.readonly, draft:view.chrome.field_drafts.get(&format!("{}:height", spec.key)).cloned(), commit: commit(GeometryField::Height), onstate:onaction }
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
    let bounds = canvas_geometry_bounds(canvas, &view.outputs);
    let child_min = canvas.widgets.iter().fold([32, 32], |minimum, widget| {
        [
            minimum[0].max(u32::try_from(widget.x).unwrap_or_default() + widget.width),
            minimum[1].max(u32::try_from(widget.y).unwrap_or_default() + widget.height),
        ]
    });
    rsx! {
        {editor_accordion(format!("canvas:{}:identity", canvas.id), "Identity", view, onaction, rsx! { TextField { field_key: format!("{}:name", canvas.id), label: format!("Name · {}", canvas.id), value: canvas.name.clone(), disallowed, disabled: view.access.readonly, update_on_input: true, draft:view.chrome.field_drafts.get(&format!("{}:name", canvas.id)).cloned(), onchange: move |value| onaction.call(EditorAction::CanvasName(value)), onstate:onaction } })}
        {editor_accordion(format!("canvas:{}:geometry", canvas.id), "Geometry", view, onaction, rsx! { {geometry_fields(GeometrySpec { rect: [canvas.x, canvas.y, i32::try_from(canvas.width).unwrap_or(i32::MAX), i32::try_from(canvas.height).unwrap_or(i32::MAX)], bounds, minimum: child_min, key: &canvas.id, widget: false, readonly: view.access.readonly }, view, onaction)} div { class: "geometry-action", Button { class: "fit-output", disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::FitToOutput), "Fit to output" } } })}
        {editor_accordion(format!("canvas:{}:visibility", canvas.id), "Visibility", view, onaction, rsx! { div { class: "visibility-actions", Button { disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::CanvasVisibleAll), "All" } Button { disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::CanvasVisibleNone), "None" } } {visibility_toggles(&canvas.id, canvas.show_on.as_deref(), view.access.readonly, onaction)} })}
        {editor_accordion(format!("canvas:{}:appearance", canvas.id), "Appearance", view, onaction, rsx! { {skin_picker(view, canvas.skin, false, onaction)} div { class: "control-heading", "Background" } SegmentedControl { class: "background-control", label: "Canvas background", for (value,label) in [(Background::None,"None"),(Background::Static,"Static"),(Background::Animated,"Animated")] { Button { class: "background-option", selected: canvas.background == value, disabled: view.access.readonly, onclick: move |_| onaction.call(EditorAction::Background(value)), "{label}" } } } div { class: "control-heading", "Opacity" } SegmentedControl { class: "opacity-control", label: "Canvas opacity", for value in [25, 50, 75, 100] { Button { class: "opacity-option", selected: canvas.opacity_percent == value, disabled: view.access.readonly, "data-value": value, onclick: move |_| onaction.call(EditorAction::Opacity(value)), "{value}%" } } } if let Some(skin) = view.skins.iter().find(|skin| skin.id == canvas.skin) { {property_controls(&skin.canvas_properties, &canvas.skin_properties, &canvas.id, true, view, onaction)} } })}
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
    title: Option<crate::editor_model::TitleDraft>,
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
