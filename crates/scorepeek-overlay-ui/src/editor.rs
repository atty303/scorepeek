//! Editor controls share their layout and state styles across rendering backends.
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

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
    AspectRatio, Background, CanvasPresentation, FrameWidth, ScreenKind, ScreenView, Skin,
    WidgetKind, WidgetLayout, canvas_visible,
};

#[derive(Clone, Debug, PartialEq)]
pub enum EditorAction {
    TogglePanel,
    PreviewScreen(ScreenKind),
    SelectCanvas(String),
    ToggleCanvas(String),
    AddCanvas,
    DeleteCanvas,
    Skin(Skin),
    CanvasSkinProperty(String, serde_json::Value),
    WidgetSkinProperty(String, serde_json::Value),
    Background(Background),
    Opacity(u8),
    Output(String),
    SelectWidget(String),
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
    RefreshRateAuto,
    EditRefreshRate,
    AcceptRefreshRate,
    CancelRefreshRate,
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
}
#[derive(Clone, Copy, PartialEq)]
pub enum EditorTitleState {
    Closed,
    Editing,
    Composing,
}

#[derive(Clone, PartialEq)]
pub struct RefreshRateEditor {
    pub rate: crate::WaylandRefreshRate,
    pub editing: bool,
    pub error: Option<String>,
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
    pub outputs: Option<Vec<EditorOutput>>,
    pub panel_width: u32,
    pub chrome: EditorChrome,
    pub access: EditorAccess,
    pub title: EditorTitleState,
    pub refresh_rate: Option<RefreshRateEditor>,
    pub skins: Vec<EditorSkin>,
}

#[component]
pub fn EditorPanel(
    view: EditorView,
    title_input: Element,
    refresh_rate_input: Element,
    onaction: EventHandler<EditorAction>,
) -> Element {
    let canvas = view
        .canvases
        .iter()
        .find(|canvas| Some(canvas.id.as_str()) == view.selected_canvas.as_deref());
    let selected_visible = canvas.is_some_and(|canvas| {
        canvas_visible(
            canvas.show_on.as_deref(),
            ScreenView {
                kind: Some(view.preview_screen),
                ..ScreenView::default()
            },
        )
    });
    let refresh_rate_valid = view
        .refresh_rate
        .as_ref()
        .is_none_or(|refresh| refresh.error.is_none());
    rsx! {
            EditorButton { onclick:move |_| onaction.call(EditorAction::TogglePanel), layout:ButtonLayout::Icon, class:if view.access.dirty{"native-panel-toggle dirty"}else{"native-panel-toggle"}, "aria-label":if view.chrome.panel_open{"Hide editor panel"}else{"Show editor panel"}, "data-state":if view.chrome.panel_open{"open"}else{"closed"}, if view.chrome.panel_open{"‹"}else{"›"} span { class:"dirty-dot" } }
            if view.chrome.panel_open { div { class:"native-canvas-manager", style:format!("width:{}px",view.panel_width),
                header { strong { "SCOREPEEK OVERLAY" } small { "{view.backend_label}" } if view.chrome.sample { b { "SAMPLE DATA" } } if view.access.dirty { i { class:"unsaved-dot" } } }
                div { class:"editor-fixed-top", p { "GAME SCREEN" } div { class:"preview-tabs",
                    for (index,(label,kind)) in [("MUSIC SELECT",ScreenKind::MusicSelect),("MODE SELECT",ScreenKind::ModeSelect),("DECIDE",ScreenKind::DecideTransition),("PLAY",ScreenKind::Play),("RESULT",ScreenKind::Result)].into_iter().enumerate() {
                        EditorButton { class:"preview-screen", onclick:move |_| onaction.call(EditorAction::PreviewScreen(kind)), selected:view.preview_screen==kind, "aria-selected":view.preview_screen==kind, "data-index":index, "{label}" }
                    }
                }
                section { class:"canvas-section", h2 { "CANVASES" }
                    nav { class:"canvas-list", for canvas in view.canvases.iter() {
                        div { class:"canvas-row",
                            EditorButton { class:"canvas-select", onclick:{let value=canvas.id.clone(); move |_| onaction.call(EditorAction::SelectCanvas(value.clone()))}, layout:ButtonLayout::Row, selected:view.selected_canvas.is_some() && canvas.id==view.selected_canvas.as_deref().unwrap_or_default(), "aria-selected":view.selected_canvas.is_some() && canvas.id==view.selected_canvas.as_deref().unwrap_or_default(), "data-canvas-id":"{canvas.id}", "{canvas.id}" }
                            EditorButton { class:"screen-toggle", disabled:view.access.readonly, onclick:{let value=canvas.id.clone(); move |_| onaction.call(EditorAction::ToggleCanvas(value.clone()))}, selected:canvas_visible(canvas.show_on.as_deref(), ScreenView { kind:Some(view.preview_screen), suspended_since_unix_ms:None, revision:0 }), "data-canvas-id":"{canvas.id}", if canvas_visible(canvas.show_on.as_deref(), ScreenView { kind:Some(view.preview_screen), suspended_since_unix_ms:None, revision:0 }){"ON"}else{"OFF"} }
                        }
                    } }
                    div { class:"canvas-actions", EditorButton { class:"add-canvas", disabled:view.access.readonly, onclick:move |_| onaction.call(EditorAction::AddCanvas), "+ ADD CANVAS" } EditorButton { class:"delete-canvas", onclick:move |_| onaction.call(EditorAction::DeleteCanvas), tone:ButtonTone::Danger, disabled:view.access.readonly || view.canvases.len()<=1 || view.selected_canvas.is_none(), "DELETE SELECTED" } }
                }
                }
                div { class:"editor-tab-body",
                if let Some(refresh)=&view.refresh_rate { section { class:"rendering-pane", h2 { "RENDERING" } h3 { "REFRESH RATE" }
                    div { class:"refresh-rate-controls",
                        EditorButton { class:"refresh-rate-auto", disabled:view.access.readonly, onclick:move |_| onaction.call(EditorAction::RefreshRateAuto), selected:refresh.rate==crate::WaylandRefreshRate::Auto, "AUTO" }
                        if refresh.editing {
                            {refresh_rate_input}
                            EditorButton { class:"refresh-rate-accept", disabled:view.access.readonly || refresh.error.is_some(), onclick:move |_| onaction.call(EditorAction::AcceptRefreshRate), "APPLY" }
                            EditorButton { class:"refresh-rate-cancel", disabled:view.access.readonly, onclick:move |_| onaction.call(EditorAction::CancelRefreshRate), "CANCEL" }
                        } else {
                            EditorButton { class:"refresh-rate-input", disabled:view.access.readonly, onclick:move |_| onaction.call(EditorAction::EditRefreshRate), if let Some(hz)=refresh.rate.hz(){"{hz} HZ"}else{"SET HZ"} }
                        }
                    }
                    if let Some(error)=&refresh.error { p { class:"refresh-rate-error", role:"alert", "{error}" } }
                } }
                if let Some(canvas) = canvas { section { class:"appearance-pane", h2 { "APPEARANCE" } h3 { "SKIN" }
                    div { class:"native-skin-options button-grid", for (index,skin) in view.skins.iter().enumerate() { EditorButton { class:"skin-option", disabled:view.access.readonly, onclick:{let value=skin.id; move |_| onaction.call(EditorAction::Skin(value))}, selected:canvas.skin==skin.id, "data-index":index, if canvas.skin==skin.id{"✓ "} "{skin.name}" small { "{skin.release}" } } } }
                    if let Some(skin)=view.skins.iter().find(|skin|skin.id==canvas.skin) {
                        if !skin.preview.is_empty() { img { class:"skin-preview", src:"{skin.preview}", alt:"{skin.name} preview" } }
                        if view.outputs.is_none() { if let Some(preview_video)=&skin.preview_video { video { class:"skin-preview-video", src:"{preview_video}", autoplay:true, muted:true, r#loop:true } } }
                        {property_controls(&skin.canvas_properties,&canvas.skin_properties,true,view.access.readonly,onaction)}
                    }
                    if view.outputs.is_some() { h3 { "OPACITY" }
                    div { class:"native-opacity button-grid four", for value in [25,50,75,100] { EditorButton { class:"opacity-option", disabled:view.access.readonly, onclick:move |_| onaction.call(EditorAction::Opacity(value)), selected:canvas.opacity_percent==value, "data-value":value, if canvas.opacity_percent==value{"✓ "} "{value}" } } }
                }
                }
                if let Some(outputs) = &view.outputs { section { class:"output-pane", h2 { "OUTPUT" } div { class:"output-list", for output in outputs.iter() { EditorButton { class:"output-option", disabled:view.access.readonly, onclick:{let value=output.name.clone(); move |_| onaction.call(EditorAction::Output(value.clone()))}, layout:ButtonLayout::Stack, selected:canvas.output.as_deref()==Some(output.name.as_str()), "aria-selected":canvas.output.as_deref()==Some(output.name.as_str()), "data-output":"{output.name}", strong { if canvas.output.as_deref()==Some(output.name.as_str()){"✓ "} "{output.name}" } small { "{output.model}" if let Some([width,height])=output.logical_size { " · {width}×{height}" } } } } } }
                }
                if selected_visible { section { class:"widgets-pane", h2 { "WIDGETS" }
                    div {class:"widget-list", for widget in canvas.widgets.iter() { EditorButton { class:"widget-row", onclick:{let value=widget.id.clone(); move |_| onaction.call(EditorAction::SelectWidget(value.clone()))}, layout:ButtonLayout::Row, selected:view.selected_widget.as_deref()==Some(widget.id.as_str()), "aria-selected":view.selected_widget.as_deref()==Some(widget.id.as_str()), "data-widget-id":"{widget.id}", "{widget.id}" } }
                    }
                    details { class:"widget-add", open:view.chrome.widget_add_open, summary { class:"widget-add-summary", onclick:move |event| {event.prevent_default(); onaction.call(EditorAction::ToggleWidgetAdd);}, "+ ADD WIDGET" } if view.chrome.widget_add_open { div { class:"button-grid", for (index,label) in ["STATUS","SELECTION","SCORE","HISTORY LIST","HISTORY GRAPH","EMPTY"].into_iter().enumerate() { EditorButton { class:"add-widget", disabled:view.access.readonly, onclick:move |_| onaction.call(EditorAction::AddWidget(index)), "data-index":index, "+ {label}" } } } } }
                    if let Some(widget) = canvas.widgets.iter().find(|widget| view.selected_widget.as_deref()==Some(widget.id.as_str())) {
                        {widget_settings(widget, &view, title_input.clone(), onaction)}
                        if let Some(skin)=view.skins.iter().find(|skin|skin.id==canvas.skin) { if let Some(properties)=skin.widget_properties.get(widget.kind.name()).or_else(||skin.widget_properties.get("*")) { {property_controls(properties,&widget.skin_properties,false,view.access.readonly,onaction)} } }
                    }
                } } else { div { class:"canvas-hidden-state", strong { "HIDDEN ON THIS GAME SCREEN" } span { "Turn this canvas ON in the list to edit its widgets." } } } } else { div { class:"canvas-hidden-state", strong { "NO CANVAS ON THIS GAME SCREEN" } span { "Turn a canvas ON or add one for this game screen." } } }
                }
                footer { EditorButton { class:"undo-action", onclick:move |_| onaction.call(EditorAction::Undo), disabled:view.access.readonly || !view.access.undo_available, "UNDO" } div { class:"footer-actions", if view.access.dirty { EditorButton { class:"discard-action", disabled:view.access.readonly, onclick:move |_| onaction.call(EditorAction::Discard), "DISCARD CHANGES" } EditorButton { class:"save-action", disabled:view.access.readonly || !refresh_rate_valid, onclick:move |_| onaction.call(EditorAction::Save), tone:ButtonTone::Primary, "SAVE ALL CHANGES AND CLOSE" } } else { EditorButton { class:"close-action", onclick:move |_| onaction.call(EditorAction::Close), "CLOSE EDITOR" } } } }
            } }

    }
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
