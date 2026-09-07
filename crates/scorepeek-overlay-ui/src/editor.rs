//! Editor controls share their layout and state styles across rendering backends.
use dioxus::prelude::*;

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
}

#[component]
pub fn EditorPanel(
    view: EditorView,
    title_input: Element,
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
                if let Some(canvas) = canvas { section { class:"appearance-pane", h2 { "APPEARANCE" } h3 { "SKIN" }
                    div { class:"native-skin-options button-grid three", for (index,(label,skin)) in [("CYAN",Skin::CyanSystem),("AURORA",Skin::ResultAurora),("BLACKBOX",Skin::DjBlackbox)].into_iter().enumerate() { EditorButton { class:"skin-option", disabled:view.access.readonly, onclick:move |_| onaction.call(EditorAction::Skin(skin)), selected:canvas.skin==skin, "data-index":index, if canvas.skin==skin{"✓ "} "{label}" } } }
                    h3 { "BACKGROUND" }
                    div { class:"button-grid three", for (index,(label,mode)) in [("NONE",Background::None),("STATIC",Background::Static),("ANIMATED",Background::Animated)].into_iter().enumerate() { EditorButton { class:"background-option", disabled:view.access.readonly, onclick:move |_| onaction.call(EditorAction::Background(mode)), selected:canvas.background == mode , "data-index":index, "{label}" } } }
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
                    }
                } } else { div { class:"canvas-hidden-state", strong { "HIDDEN ON THIS GAME SCREEN" } span { "Turn this canvas ON in the list to edit its widgets." } } } } else { div { class:"canvas-hidden-state", strong { "NO CANVAS ON THIS GAME SCREEN" } span { "Turn a canvas ON or add one for this game screen." } } }
                }
                footer { EditorButton { class:"undo-action", onclick:move |_| onaction.call(EditorAction::Undo), disabled:view.access.readonly || !view.access.undo_available, "UNDO" } div { class:"footer-actions", if view.access.dirty { EditorButton { class:"discard-action", disabled:view.access.readonly, onclick:move |_| onaction.call(EditorAction::Discard), "DISCARD CHANGES" } EditorButton { class:"save-action", disabled:view.access.readonly, onclick:move |_| onaction.call(EditorAction::Save), tone:ButtonTone::Primary, "SAVE ALL CHANGES AND CLOSE" } } else { EditorButton { class:"close-action", onclick:move |_| onaction.call(EditorAction::Close), "CLOSE EDITOR" } } } }
            } }

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
                            h3 { "FRAME WIDTH" }
                            for (index,value) in [FrameWidth::S,FrameWidth::M,FrameWidth::L].into_iter().enumerate() { EditorButton { class:"frame-width", onclick:move |_| onaction.call(EditorAction::FrameWidth(value)), disabled:view.access.readonly, selected:widget.settings.frame_width==value , "data-index":index, "{value:?}" } }
                            if widget.kind == WidgetKind::Empty {
                                h3 { "TITLE" }
                                if view.title!=EditorTitleState::Closed {
                                    {title_input}
                                    div { class:"button-grid", EditorButton { class:"title-accept", onclick:move |_| onaction.call(EditorAction::AcceptTitle), disabled:view.access.readonly || view.title==EditorTitleState::Composing, "APPLY TITLE" } EditorButton { class:"title-cancel", onclick:move |_| onaction.call(EditorAction::CancelTitle), disabled:view.access.readonly, "CANCEL" } }
                                } else { EditorButton { class:"empty-title-input", onclick:move |_| onaction.call(EditorAction::EditTitle), disabled:view.access.readonly, if widget.settings.title.is_empty(){"Enter title…"}else{"{widget.settings.title}"} } }
                                h3 { "INTERIOR OPACITY · {widget.settings.fill_opacity_percent}%" }
                                EditorButton { class:"fill-decrease", onclick:move |_| onaction.call(EditorAction::FillDelta(-1)), disabled:view.access.readonly, "−1" } EditorButton { class:"fill-increase", onclick:move |_| onaction.call(EditorAction::FillDelta(1)), disabled:view.access.readonly, "+1" }
                                for value in [0,25,50,75,100] { EditorButton { class:"fill-opacity", onclick:move |_| onaction.call(EditorAction::FillOpacity(value)), disabled:view.access.readonly, selected:widget.settings.fill_opacity_percent==value , "data-value":value, "{value}%" } }
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
