//! Backend-neutral editor actions.

use crate::{ScreenKind, Skin};

#[derive(Clone, Debug, PartialEq)]
pub enum EditorAction {
    TogglePanel,
    SetScreenPickerOpen(bool),
    SetOutputPickerOpen(bool),
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
    ClearSelection,
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
    EditTitle,
    AcceptTitle,
    CancelTitle,
    TitleText(String),
    TextComposition {
        field_key: String,
        composing: bool,
    },
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
    #[must_use]
    pub const fn diagnostic_name(&self) -> &'static str {
        match self {
            Self::TogglePanel => "toggle_panel",
            Self::SetScreenPickerOpen(_) => "screen_picker",
            Self::SetOutputPickerOpen(_) => "output_picker",
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
            Self::ClearSelection => "clear_selection",
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
            Self::EditTitle => "edit_title",
            Self::AcceptTitle => "accept_title",
            Self::CancelTitle => "cancel_title",
            Self::TitleText(_) => "title_text",
            Self::TextComposition { .. } => "text_composition",
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
