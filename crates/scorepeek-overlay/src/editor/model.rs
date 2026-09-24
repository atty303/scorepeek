use crate::editor::{
    EditorAccess, EditorAction, EditorChrome, EditorFieldCommit, EditorProperty, EditorSkin,
    EditorTitleState, EditorView, GeometryField,
};
use crate::{
    AspectRatio, CanvasPresentation, ScreenKind, Skin, WidgetKind, WidgetLayout, WidgetSettings,
    next_widget_id,
};

pub const SCREENS: [ScreenKind; 6] = [
    ScreenKind::Unknown,
    ScreenKind::MusicSelect,
    ScreenKind::ModeSelect,
    ScreenKind::DecideTransition,
    ScreenKind::Play,
    ScreenKind::Result,
];

fn migrate_properties(
    old: Option<&std::collections::BTreeMap<String, EditorProperty>>,
    target: &std::collections::BTreeMap<String, EditorProperty>,
    values: &std::collections::BTreeMap<String, serde_json::Value>,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    target
        .iter()
        .map(|(key, property)| {
            let candidate = old
                .and_then(|old| old.get(key))
                .filter(|old| old.kind() == property.kind())
                .and_then(|_| values.get(key));
            (key.clone(), property.effective(candidate))
        })
        .collect()
}

fn normalize_properties(canvases: &mut [CanvasPresentation], skins: &[EditorSkin]) {
    for canvas in canvases {
        let Some(skin) = skins.iter().find(|skin| skin.id == canvas.skin) else {
            continue;
        };
        canvas.skin_properties = migrate_properties(
            Some(&skin.canvas_properties),
            &skin.canvas_properties,
            &canvas.skin_properties,
        );
        for widget in &mut canvas.widgets {
            let properties = skin
                .widget_properties
                .get(widget.kind.name())
                .or_else(|| skin.widget_properties.get("*"));
            widget.skin_properties =
                properties.map_or_else(std::collections::BTreeMap::new, |properties| {
                    migrate_properties(Some(properties), properties, &widget.skin_properties)
                });
        }
    }
}
#[derive(Clone, PartialEq)]
pub struct TitleDraft {
    pub canvas: String,
    pub widget: String,
    pub text: String,
    pub composing: bool,
}

fn single_line(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control())
        .collect()
}
#[derive(Clone, PartialEq)]
pub struct Drag {
    pub canvas: String,
    pub widget: Option<String>,
    pub corner: Option<String>,
    pub start: [i32; 2],
    pub original: Vec<CanvasPresentation>,
}
#[derive(Clone, PartialEq)]
pub struct EditorSession {
    pub session_id: u64,
    pub revision: u64,
    pub saved: Vec<CanvasPresentation>,
    pub draft: Vec<CanvasPresentation>,
    pub selected_canvas: Option<String>,
    pub selected_widget: Option<String>,
    pub screen: Option<ScreenKind>,
    pub preview: ScreenKind,
    pub editing: bool,
    pub readonly: bool,
    pub chrome: EditorChrome,
    pub undo: Option<Vec<CanvasPresentation>>,
    pub placing: Option<WidgetKind>,
    pub point: [i32; 2],
    pub drag: Option<Drag>,
    pub title: Option<TitleDraft>,
    pub viewport: [u32; 2],
    pub outputs: Vec<crate::editor::EditorOutput>,
    pub active_output: Option<String>,
    pub discard_pending: bool,
    pub notice: Option<String>,
    pub skins: Vec<EditorSkin>,
    pub new_canvas_skin: Option<Skin>,
}

#[derive(Clone, PartialEq)]
pub struct StageProjection {
    pub session_id: u64,
    pub revision: u64,
    pub output: crate::editor::EditorOutput,
    pub interactive: bool,
    pub view: EditorView,
    pub canvases: Vec<CanvasPresentation>,
    pub selected_canvas: Option<CanvasPresentation>,
    pub selected_widget: Option<String>,
    pub placing: Option<WidgetKind>,
    pub point: [i32; 2],
    pub drag: Option<Drag>,
    pub title: Option<TitleDraft>,
    pub notice: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorEffectKind {
    Acquire,
    KeepAlive,
    Update,
    Save,
    Discard,
    Close,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EditorEffect {
    Acquire,
    KeepAlive,
    Update { canvases: Vec<CanvasPresentation> },
    Save { canvases: Vec<CanvasPresentation> },
    Discard,
    Close,
}

impl EditorEffect {
    #[must_use]
    pub const fn kind(&self) -> EditorEffectKind {
        match self {
            Self::Acquire => EditorEffectKind::Acquire,
            Self::KeepAlive => EditorEffectKind::KeepAlive,
            Self::Update { .. } => EditorEffectKind::Update,
            Self::Save { .. } => EditorEffectKind::Save,
            Self::Discard => EditorEffectKind::Discard,
            Self::Close => EditorEffectKind::Close,
        }
    }

    #[must_use]
    pub fn requested_draft(&self) -> Option<&[CanvasPresentation]> {
        match self {
            Self::Update { canvases } | Self::Save { canvases } => Some(canvases),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EditorBackendReply {
    pub ok: bool,
    pub readonly: bool,
    pub error: Option<String>,
    pub canvases: Vec<CanvasPresentation>,
    pub dirty: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EditorInput {
    Open {
        output: Option<String>,
        canvas: Option<String>,
        preview: ScreenKind,
    },
    SetOutputs(Vec<crate::editor::EditorOutput>),
    Action(EditorAction),
    Surface(crate::editor::effect::SurfaceAction),
    SurfaceOnOutput {
        output: String,
        action: crate::editor::effect::SurfaceAction,
    },
    Resize {
        output: String,
        logical_size: [u32; 2],
    },
    TransportReady {
        screen: Option<ScreenKind>,
        sample: bool,
        canvases: Vec<CanvasPresentation>,
        first: bool,
    },
    TransportLost,
    KeepAliveTick,
    VersionMismatch,
    BackendCompleted {
        effect: EditorEffectKind,
        requested_draft: Option<Vec<CanvasPresentation>>,
        reply: EditorBackendReply,
    },
}

impl EditorInput {
    /// Stable semantic label for transport and reducer diagnostics.
    #[must_use]
    pub const fn diagnostic_name(&self) -> &'static str {
        match self {
            Self::Open { .. } => "open",
            Self::SetOutputs(_) => "set_outputs",
            Self::Action(_) => "action",
            Self::Surface(_) | Self::SurfaceOnOutput { .. } => "surface",
            Self::Resize { .. } => "resize",
            Self::TransportReady { .. } => "transport_ready",
            Self::TransportLost => "transport_lost",
            Self::KeepAliveTick => "keep_alive_tick",
            Self::VersionMismatch => "version_mismatch",
            Self::BackendCompleted { .. } => "backend_completed",
        }
    }
}

impl EditorSession {
    #[must_use]
    pub fn new(
        canvases: Vec<CanvasPresentation>,
        viewport: [u32; 2],
        _namespace: &'static str,
    ) -> Self {
        let active_output = canvases.first().and_then(|canvas| canvas.output.clone());
        let new_canvas_skin = canvases.first().map(|canvas| canvas.skin);
        let skins = canvases
            .iter()
            .map(|canvas| {
                (
                    canvas.skin,
                    EditorSkin {
                        id: canvas.skin,
                        name: canvas.skin.name().into(),
                        release: String::new(),
                        preview: String::new(),
                        preview_video: None,
                        widget_defaults: std::collections::BTreeMap::new(),
                        canvas_properties: std::collections::BTreeMap::new(),
                        widget_properties: std::collections::BTreeMap::new(),
                    },
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_values()
            .collect();
        let expanded_canvases = canvases
            .first()
            .map(|canvas| canvas.id.clone())
            .into_iter()
            .collect();
        Self {
            session_id: 0,
            revision: 0,
            selected_canvas: None,
            saved: canvases.clone(),
            draft: canvases,
            selected_widget: None,
            screen: None,
            preview: ScreenKind::MusicSelect,
            editing: false,
            readonly: true,
            chrome: EditorChrome {
                panel_open: true,
                widget_add_open: false,
                sample: true,
                screen_picker_open: false,
                output_picker_open: false,
                expanded_outputs: active_output.clone().into_iter().collect(),
                expanded_canvases,
                collapsed_accordions: std::collections::BTreeSet::new(),
                field_drafts: std::collections::BTreeMap::new(),
                picker_cursors: std::collections::BTreeMap::new(),
            },
            undo: None,
            placing: None,
            point: [0; 2],
            drag: None,
            title: None,
            viewport,
            outputs: Vec::new(),
            active_output,
            discard_pending: false,
            notice: None,
            new_canvas_skin,
            skins,
        }
    }

    pub fn set_session_id(&mut self, session_id: u64) {
        self.session_id = session_id;
    }

    pub fn advance_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    #[must_use]
    pub fn stage_projection(&self, output: &crate::editor::EditorOutput) -> StageProjection {
        let canvases = self
            .draft
            .iter()
            .filter(|canvas| {
                canvas.output.as_deref().or(self.active_output.as_deref())
                    == Some(output.name.as_str())
                    && self.visible(canvas)
            })
            .cloned()
            .collect::<Vec<_>>();
        let selected_canvas = canvases
            .iter()
            .find(|canvas| Some(&canvas.id) == self.selected_canvas.as_ref())
            .cloned();
        StageProjection {
            session_id: self.session_id,
            revision: self.revision,
            output: output.clone(),
            interactive: self.editing
                && self.active_output.as_deref() == Some(output.name.as_str()),
            view: self.view(),
            canvases,
            selected_canvas,
            selected_widget: self.selected_widget.clone(),
            placing: self.placing,
            point: self.point,
            drag: self.drag.clone(),
            title: self.title.clone(),
            notice: self.notice.clone(),
        }
    }

    #[must_use]
    pub fn stage_projections(&self) -> Vec<StageProjection> {
        self.outputs
            .iter()
            .map(|output| self.stage_projection(output))
            .collect()
    }

    pub fn reduce(&mut self, input: EditorInput) -> Vec<EditorEffect> {
        let before = self.clone();
        let effects = match input {
            EditorInput::Open {
                output,
                canvas,
                preview,
            } => {
                self.activate_output(output.as_deref());
                self.enter(canvas, preview);
                vec![EditorEffect::Acquire]
            }
            EditorInput::SurfaceOnOutput { output, action } => {
                self.reduce_output_surface_action(&output, action)
            }
            EditorInput::SetOutputs(outputs) => {
                self.set_outputs(outputs);
                Vec::new()
            }
            EditorInput::Action(action) => self.reduce_editor_action(action),
            EditorInput::Surface(action) => self.reduce_surface_action(action),
            EditorInput::Resize {
                output,
                logical_size,
            } => {
                self.resize_output(&output, logical_size);
                Vec::new()
            }
            EditorInput::TransportReady {
                screen,
                sample,
                canvases,
                first,
            } => self.reduce_transport_ready(screen, sample, canvases, first),
            EditorInput::TransportLost => {
                self.readonly = true;
                self.drag = None;
                self.notice = Some("Editor connection lost; reconnecting.".into());
                Vec::new()
            }
            EditorInput::KeepAliveTick => {
                if self.editing {
                    vec![if self.readonly {
                        EditorEffect::Acquire
                    } else {
                        EditorEffect::KeepAlive
                    }]
                } else {
                    Vec::new()
                }
            }
            EditorInput::VersionMismatch => {
                let saved = self.saved.clone();
                let viewport = self.viewport;
                let session_id = self.session_id;
                *self = Self::new(saved, viewport, "editor");
                self.session_id = session_id;
                self.selected_canvas = None;
                Vec::new()
            }
            EditorInput::BackendCompleted {
                effect,
                requested_draft,
                reply,
            } => self.reduce_backend_completed(effect, requested_draft.as_deref(), reply),
        };
        if *self != before {
            self.revision = before.revision.saturating_add(1);
        }
        effects
    }

    fn reduce_transport_ready(
        &mut self,
        screen: Option<ScreenKind>,
        sample: bool,
        canvases: Vec<CanvasPresentation>,
        first: bool,
    ) -> Vec<EditorEffect> {
        self.screen = screen;
        self.chrome.sample = sample;
        self.receive_stage(canvases);
        if first && self.editing {
            vec![EditorEffect::Acquire]
        } else {
            Vec::new()
        }
    }

    fn reduce_editor_action(&mut self, action: EditorAction) -> Vec<EditorEffect> {
        match action {
            EditorAction::Save if !self.readonly && self.document_valid() => {
                self.normalize_for_save();
                vec![EditorEffect::Save {
                    canvases: self.draft.clone(),
                }]
            }
            EditorAction::Discard if !self.readonly && !self.discard_pending => {
                self.discard_pending = true;
                vec![EditorEffect::Discard]
            }
            EditorAction::Close => vec![EditorEffect::Close],
            other => {
                let changed = self.action(&other);
                if changed && self.document_valid() {
                    vec![EditorEffect::Update {
                        canvases: self.draft.clone(),
                    }]
                } else {
                    Vec::new()
                }
            }
        }
    }

    fn reduce_output_surface_action(
        &mut self,
        output: &str,
        action: crate::editor::effect::SurfaceAction,
    ) -> Vec<EditorEffect> {
        use crate::editor::effect::SurfaceAction;
        let canvas_id = match &action {
            SurfaceAction::Select(canvas) | SurfaceAction::Start { canvas, .. } => {
                Some(canvas.as_str())
            }
            SurfaceAction::Move(_) | SurfaceAction::End | SurfaceAction::Cancel => self
                .drag
                .as_ref()
                .map(|drag| drag.canvas.as_str())
                .or(self.selected_canvas.as_deref()),
            SurfaceAction::Place(_) => self.selected_canvas.as_deref(),
            SurfaceAction::Enter(_) | SurfaceAction::CancelFromKeyboard => None,
        };
        if self.editing
            && canvas_id.is_some_and(|id| {
                self.draft
                    .iter()
                    .find(|canvas| canvas.id == id)
                    .is_none_or(|canvas| {
                        canvas.output.as_deref().or(self.active_output.as_deref()) != Some(output)
                    })
            })
        {
            return Vec::new();
        }
        self.reduce_surface_action(action)
    }

    fn reduce_surface_action(
        &mut self,
        action: crate::editor::effect::SurfaceAction,
    ) -> Vec<EditorEffect> {
        if let crate::editor::effect::SurfaceAction::Enter(canvas) = action {
            if !self.editing {
                self.enter(canvas, self.screen.unwrap_or(ScreenKind::MusicSelect));
                return vec![EditorEffect::Acquire];
            }
            return Vec::new();
        }
        if self.surface(action) && self.document_valid() {
            vec![EditorEffect::Update {
                canvases: self.draft.clone(),
            }]
        } else {
            Vec::new()
        }
    }

    fn reduce_backend_completed(
        &mut self,
        effect: EditorEffectKind,
        requested_draft: Option<&[CanvasPresentation]>,
        reply: EditorBackendReply,
    ) -> Vec<EditorEffect> {
        self.readonly = reply.readonly;
        self.notice = reply.error;
        let current_request = requested_draft.is_none_or(|requested| self.draft == requested);
        if current_request
            && matches!(
                effect,
                EditorEffectKind::Acquire
                    | EditorEffectKind::Update
                    | EditorEffectKind::Save
                    | EditorEffectKind::Discard
            )
        {
            self.draft = reply.canvases;
            if !reply.dirty {
                self.saved.clone_from(&self.draft);
            }
            if !self
                .draft
                .iter()
                .any(|canvas| Some(&canvas.id) == self.selected_canvas.as_ref())
            {
                self.clear_selection();
            }
        }
        if !reply.ok {
            if effect == EditorEffectKind::Discard {
                self.discard_pending = false;
            }
            return Vec::new();
        }
        if effect == EditorEffectKind::Save && !current_request {
            return vec![EditorEffect::Save {
                canvases: self.draft.clone(),
            }];
        }
        match effect {
            EditorEffectKind::Save => {
                self.saved.clone_from(&self.draft);
                vec![EditorEffect::Close]
            }
            EditorEffectKind::Discard | EditorEffectKind::Close => {
                self.close();
                Vec::new()
            }
            EditorEffectKind::Acquire if self.discard_pending => vec![EditorEffect::Discard],
            _ => Vec::new(),
        }
    }

    pub fn set_skins(&mut self, skins: Vec<EditorSkin>) {
        self.skins = skins;
        if !self
            .skins
            .iter()
            .any(|skin| Some(skin.id) == self.new_canvas_skin)
        {
            self.new_canvas_skin = self.skins.first().map(|skin| skin.id);
        }
    }
    pub fn set_outputs(&mut self, outputs: Vec<crate::editor::EditorOutput>) {
        let previous_active = self.active_output.clone();
        self.outputs = outputs;
        let active = if self
            .active_output
            .as_ref()
            .is_none_or(|active| !self.outputs.iter().any(|output| &output.name == active))
        {
            self.outputs.first().map(|output| output.name.as_str())
        } else {
            self.active_output.as_deref()
        }
        .map(str::to_owned);
        self.activate_output(active.as_deref());
        if self.active_output != previous_active {
            self.chrome.output_picker_open = false;
        }
    }
    pub fn activate_output(&mut self, output: Option<&str>) {
        self.active_output = output
            .filter(|name| self.outputs.iter().any(|candidate| candidate.name == *name))
            .map(str::to_owned);
        if let Some(size) = self
            .outputs
            .iter()
            .find(|output| Some(&output.name) == self.active_output.as_ref())
            .and_then(|output| output.logical_size)
        {
            self.viewport = size;
        }
    }
    pub fn resize_output(&mut self, name: &str, size: [u32; 2]) {
        if let Some(output) = self.outputs.iter_mut().find(|output| output.name == name) {
            output.logical_size = Some(size);
        }
        if self.active_output.as_deref() == Some(name) {
            self.viewport = size;
        }
    }
    pub fn receive_stage(&mut self, canvases: Vec<CanvasPresentation>) {
        if !self.editing && self.draft != canvases {
            self.saved.clone_from(&canvases);
            self.draft = canvases;
        }
    }
    pub fn normalize_for_save(&mut self) {
        normalize_properties(&mut self.draft, &self.skins);
    }
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.saved != self.draft
    }
    #[must_use]
    pub fn document_valid(&self) -> bool {
        crate::editor::document_valid(&self.draft, &self.outputs) && !self.active_field_invalid()
    }
    fn active_field_invalid(&self) -> bool {
        let Some(canvas) = self.current() else {
            return false;
        };
        let prefix = self.selected_widget.as_ref().map_or_else(
            || format!("{}:", canvas.id),
            |widget| format!("{}:{widget}:", canvas.id),
        );
        self.chrome
            .field_drafts
            .iter()
            .any(|(field, draft)| field.starts_with(&prefix) && (!draft.valid || draft.composing))
    }
    #[must_use]
    pub fn current(&self) -> Option<&CanvasPresentation> {
        self.draft
            .iter()
            .find(|canvas| Some(&canvas.id) == self.selected_canvas.as_ref())
    }
    #[must_use]
    pub fn widget_default_size(&self, kind: WidgetKind) -> Option<[u32; 2]> {
        let canvas = self.current()?;
        let default = self
            .skins
            .iter()
            .find(|skin| skin.id == canvas.skin)?
            .widget_defaults
            .get(kind.name())?;
        Some([default.width, default.height])
    }
    #[must_use]
    pub fn visible(&self, canvas: &CanvasPresentation) -> bool {
        visible_on(
            canvas,
            if self.editing {
                Some(self.preview)
            } else {
                self.screen
            },
        )
    }
    #[must_use]
    pub fn view(&self) -> EditorView {
        EditorView {
            canvases: self.draft.clone(),
            selected_canvas: self.selected_canvas.clone(),
            selected_widget: self.selected_widget.clone(),
            preview_screen: self.preview,
            outputs: self.outputs.clone(),
            active_output: self.active_output.clone(),
            panel_width: (self.viewport[0] / 5).clamp(360, 480),
            chrome: self.chrome.clone(),
            access: EditorAccess {
                dirty: self.dirty(),
                readonly: self.readonly || self.discard_pending,
                undo_available: self.undo.is_some(),
                save_validity: if self.document_valid() {
                    crate::editor::SaveValidity::Valid
                } else {
                    crate::editor::SaveValidity::Invalid
                },
            },
            title: self
                .title
                .as_ref()
                .map_or(EditorTitleState::Closed, |title| {
                    if title.composing {
                        EditorTitleState::Composing
                    } else {
                        EditorTitleState::Editing
                    }
                }),
            skins: self.skins.clone(),
            new_canvas_skin: self.new_canvas_skin,
        }
    }
    pub fn clear_selection(&mut self) {
        self.selected_canvas = None;
        self.selected_widget = None;
        self.title = None;
        self.placing = None;
        self.drag = None;
        self.chrome.field_drafts.clear();
    }
    pub fn enter(&mut self, _canvas: Option<String>, preview: ScreenKind) {
        self.editing = true;
        self.chrome.panel_open = true;
        self.preview = preview;
        self.clear_selection();
        self.readonly = true;
    }
    fn select_canvas(&mut self, id: &str) {
        let Some(canvas) = self.draft.iter().find(|canvas| canvas.id == id).cloned() else {
            return;
        };
        let scope_changed =
            self.selected_canvas.as_deref() != Some(id) || self.selected_widget.is_some();
        select_canvas_preview(&canvas, &mut self.preview);
        self.chrome.expanded_canvases.insert(id.to_owned());
        self.selected_canvas = Some(id.to_owned());
        self.selected_widget = None;
        self.title = None;
        if scope_changed {
            self.placing = None;
            self.chrome.field_drafts.clear();
        }
    }
    fn select_widget(&mut self, canvas_id: &str, widget_id: &str) {
        let Some(canvas) = self
            .draft
            .iter()
            .find(|canvas| {
                canvas.id == canvas_id && canvas.widgets.iter().any(|widget| widget.id == widget_id)
            })
            .cloned()
        else {
            return;
        };
        let scope_changed = self.selected_canvas.as_deref() != Some(canvas_id)
            || self.selected_widget.as_deref() != Some(widget_id);
        select_canvas_preview(&canvas, &mut self.preview);
        self.selected_canvas = Some(canvas_id.to_owned());
        self.selected_widget = Some(widget_id.to_owned());
        self.title = None;
        if scope_changed {
            self.placing = None;
            self.chrome.field_drafts.clear();
        }
    }
    pub fn close(&mut self) {
        self.editing = false;
        self.discard_pending = false;
        self.chrome.screen_picker_open = false;
        self.chrome.output_picker_open = false;
        self.chrome.widget_add_open = false;
        self.chrome.field_drafts.clear();
        self.selected_widget = None;
        self.undo = None;
        self.drag = None;
        self.placing = None;
        self.title = None;
    }
    fn navigate(&mut self, action: &EditorAction) -> bool {
        if self.navigate_control_state(action) {
            return true;
        }
        match action {
            EditorAction::TogglePanel => self.chrome.panel_open = !self.chrome.panel_open,
            EditorAction::SetScreenPickerOpen(open) => self.chrome.screen_picker_open = *open,
            EditorAction::SetOutputPickerOpen(open) => self.chrome.output_picker_open = *open,
            EditorAction::ToggleOutputExpanded(output) => {
                if !self.chrome.expanded_outputs.remove(output) {
                    self.chrome.expanded_outputs.insert(output.clone());
                }
            }
            EditorAction::ToggleCanvasExpanded(canvas) => {
                if !self.chrome.expanded_canvases.remove(canvas) {
                    self.chrome.expanded_canvases.insert(canvas.clone());
                }
            }
            EditorAction::ToggleAccordion(section) => {
                if !self.chrome.collapsed_accordions.remove(section) {
                    self.chrome.collapsed_accordions.insert(section.clone());
                }
            }
            EditorAction::PreviewScreen(screen) => {
                self.preview = *screen;
            }
            EditorAction::SelectOutput(output) => {
                if self
                    .outputs
                    .iter()
                    .any(|candidate| candidate.name == *output)
                    && self.active_output.as_deref() != Some(output.as_str())
                {
                    self.activate_output(Some(output));
                }
                self.chrome.output_picker_open = false;
            }
            EditorAction::ClearSelection => self.clear_selection(),
            EditorAction::SelectCanvas(id) => {
                if self.selected_canvas.as_deref() == Some(id) && self.selected_widget.is_none() {
                    self.clear_selection();
                } else {
                    self.select_canvas(id);
                }
            }
            EditorAction::SelectWidget {
                canvas_id,
                widget_id,
            } => {
                if self.selected_canvas.as_deref() == Some(canvas_id)
                    && self.selected_widget.as_deref() == Some(widget_id)
                {
                    self.clear_selection();
                } else {
                    self.select_widget(canvas_id, widget_id);
                }
            }
            EditorAction::ToggleWidgetAdd => {
                self.chrome.widget_add_open = !self.chrome.widget_add_open;
            }
            EditorAction::NewCanvasSkin(skin) => {
                if self.skins.iter().any(|candidate| candidate.id == *skin) {
                    self.new_canvas_skin = Some(*skin);
                }
            }
            EditorAction::CancelTitle => self.title = None,
            _ => return false,
        }
        true
    }

    fn navigate_control_state(&mut self, action: &EditorAction) -> bool {
        match action {
            EditorAction::SetPickerCursor(key, cursor) => {
                self.chrome.picker_cursors.insert(key.clone(), *cursor);
                true
            }
            EditorAction::BeginFieldEdit(key, value) => {
                self.chrome.field_drafts.insert(
                    key.clone(),
                    crate::editor::EditorFieldDraft {
                        text: value.clone(),
                        focused: true,
                        valid: true,
                        composing: false,
                    },
                );
                true
            }
            EditorAction::UpdateFieldDraft(key, value, valid) => {
                let draft = self
                    .chrome
                    .field_drafts
                    .entry(key.clone())
                    .or_insert_with(|| crate::editor::EditorFieldDraft {
                        text: String::new(),
                        focused: true,
                        valid: *valid,
                        composing: false,
                    });
                draft.text.clone_from(value);
                draft.focused = true;
                draft.valid = *valid;
                true
            }
            EditorAction::TextComposition {
                field_key,
                composing,
            } if field_key != "editor-title-input" => {
                if let Some(draft) = self.chrome.field_drafts.get_mut(field_key) {
                    draft.composing = *composing;
                }
                true
            }
            EditorAction::EndFieldEdit(key, true) => {
                self.chrome.field_drafts.remove(key);
                true
            }
            EditorAction::EndFieldEdit(key, false) => {
                if let Some(draft) = self.chrome.field_drafts.get_mut(key) {
                    draft.focused = false;
                }
                true
            }
            _ => false,
        }
    }

    fn commit_field_draft(&mut self, key: &str, commit: &EditorFieldCommit) -> bool {
        if self.readonly || self.discard_pending {
            return false;
        }
        let Some(draft) = self.chrome.field_drafts.get(key).cloned() else {
            return false;
        };
        if draft.composing {
            if let Some(draft) = self.chrome.field_drafts.get_mut(key) {
                draft.focused = false;
            }
            return true;
        }
        let semantic = (|| match commit {
            EditorFieldCommit::CanvasGeometry(field) => {
                let value = draft.text.parse().ok()?;
                let mut canvas = self.current()?.clone();
                let bounds = crate::editor::canvas_geometry_bounds(&canvas, &self.outputs);
                apply_canvas_geometry(&mut canvas, *field, value, bounds);
                (canvas_geometry_value(&canvas, *field) == value)
                    .then_some(EditorAction::CanvasGeometry(*field, value))
            }
            EditorFieldCommit::WidgetGeometry(field) => {
                let value = draft.text.parse().ok()?;
                let canvas = self.current()?;
                let mut widget = canvas
                    .widgets
                    .iter()
                    .find(|widget| Some(&widget.id) == self.selected_widget.as_ref())?
                    .clone();
                apply_widget_geometry(&mut widget, *field, value, [canvas.width, canvas.height]);
                (widget_geometry_value(&widget, *field) == value)
                    .then_some(EditorAction::WidgetGeometry(*field, value))
            }
            EditorFieldCommit::CanvasSkinProperty(key) => self.current().and_then(|canvas| {
                self.skins
                    .iter()
                    .find(|skin| skin.id == canvas.skin)
                    .and_then(|skin| skin.canvas_properties.get(key))
                    .and_then(|property| property.parse_text(&draft.text))
                    .map(|value| EditorAction::CanvasSkinProperty(key.clone(), value))
            }),
            EditorFieldCommit::WidgetSkinProperty(key) => self.current().and_then(|canvas| {
                let widget = canvas
                    .widgets
                    .iter()
                    .find(|widget| Some(&widget.id) == self.selected_widget.as_ref())?;
                self.skins
                    .iter()
                    .find(|skin| skin.id == canvas.skin)
                    .and_then(|skin| {
                        skin.widget_properties
                            .get(widget.kind.name())
                            .or_else(|| skin.widget_properties.get("*"))
                    })
                    .and_then(|properties| properties.get(key))
                    .and_then(|property| property.parse_text(&draft.text))
                    .map(|value| EditorAction::WidgetSkinProperty(key.clone(), value))
            }),
        })();
        let Some(semantic) = semantic else {
            if let Some(draft) = self.chrome.field_drafts.get_mut(key) {
                draft.focused = false;
                draft.valid = false;
            }
            return true;
        };
        self.chrome.field_drafts.remove(key);
        let _ = self.action(&semantic);
        true
    }
    pub fn action(&mut self, action: &EditorAction) -> bool {
        if let EditorAction::CommitFieldDraft(key, commit) = action {
            return self.commit_field_draft(key, commit);
        }
        if self.navigate(action) || self.readonly || self.discard_pending {
            return false;
        }
        match action {
            EditorAction::Undo => {
                if let Some(previous) = self.undo.take() {
                    self.draft = previous;
                    self.title = None;
                    self.drag = None;
                    self.placing = None;
                    self.chrome.field_drafts.clear();
                    if self.current().is_none() {
                        self.clear_selection();
                    }
                    if !self.current().is_some_and(|canvas| {
                        canvas
                            .widgets
                            .iter()
                            .any(|widget| Some(&widget.id) == self.selected_widget.as_ref())
                    }) {
                        self.selected_widget = None;
                    }
                    return true;
                }
                return false;
            }
            EditorAction::AddWidget(index) => {
                let Some(kind) = [
                    WidgetKind::Status,
                    WidgetKind::Selection,
                    WidgetKind::Score,
                    WidgetKind::HistoryList,
                    WidgetKind::HistoryGraph,
                    WidgetKind::Empty,
                ]
                .get(*index)
                .copied() else {
                    return false;
                };
                return self.add_widget_centered(kind);
            }
            EditorAction::EditTitle => {
                self.title = self.current().and_then(|canvas| {
                    canvas
                        .widgets
                        .iter()
                        .find(|widget| Some(&widget.id) == self.selected_widget.as_ref())
                        .map(|widget| TitleDraft {
                            canvas: canvas.id.clone(),
                            widget: widget.id.clone(),
                            text: widget.settings.title.clone(),
                            composing: false,
                        })
                });
                return false;
            }
            EditorAction::TitleText(text) => {
                if let Some(title) = self.title.as_mut() {
                    title.text = single_line(text);
                }
                return false;
            }
            EditorAction::TextComposition {
                field_key,
                composing,
            } => {
                if field_key == "editor-title-input"
                    && let Some(title) = self.title.as_mut()
                {
                    title.composing = *composing;
                }
                return false;
            }
            _ => {}
        }
        let before = self.draft.clone();
        self.apply_settings(action);
        if self.draft == before {
            false
        } else {
            self.undo = Some(before);
            true
        }
    }
    #[allow(clippy::too_many_lines)]
    fn apply_settings(&mut self, action: &EditorAction) {
        let skins = self.skins.clone();
        let outputs = self.outputs.clone();
        let output_sizes = self
            .outputs
            .iter()
            .filter_map(|output| output.logical_size.map(|size| (output.name.clone(), size)))
            .collect::<std::collections::BTreeMap<_, _>>();
        match action {
            EditorAction::CanvasName(value) => {
                if let Some(canvas) = self
                    .draft
                    .iter_mut()
                    .find(|canvas| Some(&canvas.id) == self.selected_canvas.as_ref())
                {
                    canvas.name.clone_from(value);
                }
            }
            EditorAction::CanvasVisible(screen, visible) => {
                if let Some(canvas) = self
                    .draft
                    .iter_mut()
                    .find(|canvas| Some(&canvas.id) == self.selected_canvas.as_ref())
                {
                    let mut screens = canvas.show_on.clone().unwrap_or(SCREENS.to_vec());
                    screens.retain(|candidate| candidate != screen);
                    if *visible {
                        screens.push(*screen);
                    }
                    canvas.show_on = Some(screens);
                }
                self.title = None;
            }
            EditorAction::CanvasVisibleAll => {
                if let Some(canvas) = self
                    .draft
                    .iter_mut()
                    .find(|canvas| Some(&canvas.id) == self.selected_canvas.as_ref())
                {
                    canvas.show_on = Some(SCREENS.to_vec());
                }
            }
            EditorAction::CanvasVisibleNone => {
                if let Some(canvas) = self
                    .draft
                    .iter_mut()
                    .find(|canvas| Some(&canvas.id) == self.selected_canvas.as_ref())
                {
                    canvas.show_on = Some(Vec::new());
                }
            }
            EditorAction::AddCanvas => {
                let Some(new_canvas_skin) = self.new_canvas_skin else {
                    return;
                };
                let id = (1..=self.draft.len() + 1)
                    .map(|i| format!("canvas-{i}"))
                    .find(|id| self.draft.iter().all(|canvas| &canvas.id != id))
                    .unwrap();
                self.draft.push(CanvasPresentation {
                    id: id.clone(),
                    name: (1..=self.draft.len() + 1)
                        .map(|i| format!("Canvas {i}"))
                        .find(|name| self.draft.iter().all(|canvas| &canvas.name != name))
                        .unwrap(),
                    skin: new_canvas_skin,
                    skin_properties: self
                        .skins
                        .iter()
                        .find(|skin| skin.id == new_canvas_skin)
                        .map_or_else(std::collections::BTreeMap::new, |skin| {
                            migrate_properties(
                                None,
                                &skin.canvas_properties,
                                &std::collections::BTreeMap::new(),
                            )
                        }),
                    show_on: Some(vec![self.preview]),
                    opacity_percent: 100,
                    output: self.active_output.clone(),
                    x: 0,
                    y: 0,
                    width: self.viewport[0],
                    height: self.viewport[1],
                    widgets: vec![],
                });
                self.selected_canvas = Some(id);
                self.selected_widget = None;
                self.chrome.field_drafts.clear();
            }
            EditorAction::DeleteCanvas => {
                self.draft
                    .retain(|canvas| Some(&canvas.id) != self.selected_canvas.as_ref());
                self.clear_selection();
            }
            _ => {
                if let Some(canvas) = self
                    .draft
                    .iter_mut()
                    .find(|canvas| Some(&canvas.id) == self.selected_canvas.as_ref())
                {
                    match action {
                        EditorAction::Skin(value) => {
                            let old = skins.iter().find(|skin| skin.id == canvas.skin);
                            let target = skins.iter().find(|skin| skin.id == *value);
                            if let Some(target) = target {
                                canvas.skin_properties = migrate_properties(
                                    old.map(|skin| &skin.canvas_properties),
                                    &target.canvas_properties,
                                    &canvas.skin_properties,
                                );
                                for widget in &mut canvas.widgets {
                                    let old = old.and_then(|skin| {
                                        skin.widget_properties
                                            .get(widget.kind.name())
                                            .or_else(|| skin.widget_properties.get("*"))
                                    });
                                    let target = target
                                        .widget_properties
                                        .get(widget.kind.name())
                                        .or_else(|| target.widget_properties.get("*"));
                                    widget.skin_properties = target.map_or_else(
                                        std::collections::BTreeMap::new,
                                        |target| {
                                            migrate_properties(old, target, &widget.skin_properties)
                                        },
                                    );
                                }
                                canvas.skin = *value;
                            }
                        }
                        EditorAction::CanvasSkinProperty(key, value) => {
                            if let Some(property) = skins
                                .iter()
                                .find(|skin| skin.id == canvas.skin)
                                .and_then(|skin| skin.canvas_properties.get(key))
                            {
                                canvas
                                    .skin_properties
                                    .insert(key.clone(), property.effective(Some(value)));
                            }
                        }
                        EditorAction::Opacity(value) => canvas.opacity_percent = *value,
                        EditorAction::Output(value) => canvas.output = Some(value.clone()),
                        EditorAction::FitToOutput => {
                            if let Some(size) = canvas
                                .output
                                .as_ref()
                                .and_then(|output| output_sizes.get(output))
                            {
                                canvas.width = canvas.width.min(grid(size[0])).max(32);
                                canvas.height = canvas.height.min(grid(size[1])).max(32);
                                canvas.x = canvas.x.clamp(
                                    0,
                                    i32::try_from(size[0].saturating_sub(canvas.width))
                                        .unwrap_or(i32::MAX),
                                );
                                canvas.y = canvas.y.clamp(
                                    0,
                                    i32::try_from(size[1].saturating_sub(canvas.height))
                                        .unwrap_or(i32::MAX),
                                );
                            }
                        }
                        EditorAction::CanvasGeometry(field, value) => {
                            let bounds = crate::editor::canvas_geometry_bounds(canvas, &outputs);
                            apply_canvas_geometry(canvas, *field, *value, bounds);
                        }
                        EditorAction::DeleteWidget => {
                            canvas
                                .widgets
                                .retain(|widget| Some(&widget.id) != self.selected_widget.as_ref());
                            self.selected_widget = None;
                            self.clear_selection();
                        }
                        _ => {
                            if let Some(widget) = canvas
                                .widgets
                                .iter_mut()
                                .find(|widget| Some(&widget.id) == self.selected_widget.as_ref())
                            {
                                if let EditorAction::WidgetSkinProperty(key, value) = action {
                                    if let Some(property) = skins
                                        .iter()
                                        .find(|skin| skin.id == canvas.skin)
                                        .and_then(|skin| {
                                            skin.widget_properties
                                                .get(widget.kind.name())
                                                .or_else(|| skin.widget_properties.get("*"))
                                        })
                                        .and_then(|properties| properties.get(key))
                                    {
                                        widget
                                            .skin_properties
                                            .insert(key.clone(), property.effective(Some(value)));
                                    }
                                } else if let EditorAction::WidgetGeometry(field, value) = action {
                                    apply_widget_geometry(
                                        widget,
                                        *field,
                                        *value,
                                        [canvas.width, canvas.height],
                                    );
                                } else {
                                    apply_widget_action(
                                        widget,
                                        &canvas.id,
                                        &mut self.title,
                                        action,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    fn add_widget_centered(&mut self, kind: WidgetKind) -> bool {
        if self.readonly {
            return false;
        }
        let before = self.draft.clone();
        let skin_properties = self
            .current()
            .and_then(|canvas| self.skins.iter().find(|skin| skin.id == canvas.skin))
            .and_then(|skin| {
                skin.widget_properties
                    .get(kind.name())
                    .or_else(|| skin.widget_properties.get("*"))
            })
            .map_or_else(std::collections::BTreeMap::new, |properties| {
                migrate_properties(None, properties, &std::collections::BTreeMap::new())
            });
        let Some([width, height]) = self.widget_default_size(kind) else {
            return false;
        };
        let Some(canvas) = self
            .draft
            .iter_mut()
            .find(|canvas| Some(&canvas.id) == self.selected_canvas.as_ref())
        else {
            return false;
        };
        let width = grid(width.min(canvas.width));
        let height = grid(height.min(canvas.height));
        let id = next_widget_id(kind, &canvas.widgets);
        canvas.widgets.push(WidgetLayout {
            id: id.clone(),
            kind,
            x: snap(i32::try_from(canvas.width.saturating_sub(width) / 2).unwrap_or_default()),
            y: snap(i32::try_from(canvas.height.saturating_sub(height) / 2).unwrap_or_default()),
            width,
            height,
            settings: WidgetSettings::default(),
            skin_properties,
        });
        self.selected_widget = Some(id);
        self.chrome.field_drafts.clear();
        self.undo = Some(before);
        true
    }
    pub fn place(&mut self, point: [i32; 2]) -> bool {
        if self.readonly {
            return false;
        }
        let Some(kind) = self.placing.take() else {
            return false;
        };
        let before = self.draft.clone();
        let skin_properties = self
            .current()
            .and_then(|canvas| self.skins.iter().find(|skin| skin.id == canvas.skin))
            .and_then(|skin| {
                skin.widget_properties
                    .get(kind.name())
                    .or_else(|| skin.widget_properties.get("*"))
            })
            .map_or_else(std::collections::BTreeMap::new, |properties| {
                migrate_properties(None, properties, &std::collections::BTreeMap::new())
            });
        let Some([width, height]) = self.widget_default_size(kind) else {
            return false;
        };
        let Some(canvas) = self
            .draft
            .iter_mut()
            .find(|canvas| Some(&canvas.id) == self.selected_canvas.as_ref())
        else {
            return false;
        };
        let width = width.min(canvas.width);
        let height = height.min(canvas.height);
        let id = next_widget_id(kind, &canvas.widgets);
        canvas.widgets.push(WidgetLayout {
            id: id.clone(),
            kind,
            x: snap(point[0] - canvas.x)
                .clamp(0, i32::try_from(canvas.width - width).unwrap_or(i32::MAX)),
            y: snap(point[1] - canvas.y)
                .clamp(0, i32::try_from(canvas.height - height).unwrap_or(i32::MAX)),
            width,
            height,
            settings: WidgetSettings::default(),
            skin_properties,
        });
        self.selected_widget = Some(id);
        self.chrome.field_drafts.clear();
        self.undo = Some(before);
        true
    }
    pub fn surface(&mut self, action: crate::editor::effect::SurfaceAction) -> bool {
        use crate::editor::effect::SurfaceAction;
        match action {
            SurfaceAction::Enter(canvas) => {
                self.enter(canvas, self.screen.unwrap_or(ScreenKind::MusicSelect));
            }
            SurfaceAction::Select(canvas) => {
                self.action(&EditorAction::SelectCanvas(canvas));
            }
            SurfaceAction::Start {
                canvas,
                widget,
                corner,
                point,
            } => self.begin_drag(canvas, widget, corner, point),
            SurfaceAction::Move(point) => {
                if self.drag.is_some() || self.placing.is_some() {
                    self.move_pointer(point);
                }
            }
            SurfaceAction::End => return self.end_drag(),
            SurfaceAction::Place(point) => return self.place(point),
            SurfaceAction::Cancel | SurfaceAction::CancelFromKeyboard => {
                if let Some(drag) = self.drag.take() {
                    self.draft = drag.original;
                }
                self.placing = None;
            }
        }
        false
    }
    pub fn begin_drag(
        &mut self,
        canvas: String,
        widget: Option<String>,
        corner: Option<String>,
        start: [i32; 2],
    ) {
        if self.readonly || self.placing.is_some() {
            return;
        }
        self.selected_canvas = Some(canvas.clone());
        self.selected_widget.clone_from(&widget);
        self.title = None;
        self.drag = Some(Drag {
            canvas,
            widget,
            corner,
            start,
            original: self.draft.clone(),
        });
    }
    pub fn end_drag(&mut self) -> bool {
        if let Some(drag) = self.drag.take()
            && self.draft != drag.original
        {
            self.undo = Some(drag.original);
            return true;
        }
        false
    }
    pub fn move_pointer(&mut self, point: [i32; 2]) {
        self.point = point;
        let Some(drag) = &self.drag else {
            return;
        };
        if self.readonly {
            return;
        }
        let Some(original) = drag.original.iter().find(|canvas| canvas.id == drag.canvas) else {
            return;
        };
        let Some(canvas) = self
            .draft
            .iter_mut()
            .find(|canvas| canvas.id == drag.canvas)
        else {
            return;
        };
        let delta = [point[0] - drag.start[0], point[1] - drag.start[1]];
        if let Some(id) = &drag.widget {
            let Some(old) = original.widgets.iter().find(|widget| &widget.id == id) else {
                return;
            };
            let Some(widget) = canvas.widgets.iter_mut().find(|widget| &widget.id == id) else {
                return;
            };
            let rect = resize(
                [
                    old.x,
                    old.y,
                    i32::try_from(old.width).unwrap_or(i32::MAX),
                    i32::try_from(old.height).unwrap_or(i32::MAX),
                ],
                delta,
                drag.corner.as_deref(),
                [canvas.width, canvas.height],
                [16, 16],
                if old.kind == WidgetKind::Empty {
                    old.settings.aspect_ratio
                } else {
                    AspectRatio::Free
                },
            );
            widget.x = rect[0];
            widget.y = rect[1];
            widget.width = u32::try_from(rect[2]).unwrap_or_default();
            widget.height = u32::try_from(rect[3]).unwrap_or_default();
        } else {
            let min = canvas.widgets.iter().fold([32, 32], |m, widget| {
                [
                    m[0].max(u32::try_from(widget.x).unwrap_or_default() + widget.width),
                    m[1].max(u32::try_from(widget.y).unwrap_or_default() + widget.height),
                ]
            });
            let rect = resize(
                [
                    original.x,
                    original.y,
                    i32::try_from(original.width).unwrap_or(i32::MAX),
                    i32::try_from(original.height).unwrap_or(i32::MAX),
                ],
                delta,
                drag.corner.as_deref(),
                crate::editor::canvas_geometry_bounds(original, &self.outputs),
                min,
                AspectRatio::Free,
            );
            canvas.x = rect[0];
            canvas.y = rect[1];
            canvas.width = u32::try_from(rect[2]).unwrap_or_default();
            canvas.height = u32::try_from(rect[3]).unwrap_or_default();
        }
    }
}
#[must_use]
pub fn visible_on(canvas: &CanvasPresentation, screen: Option<ScreenKind>) -> bool {
    canvas
        .show_on
        .as_ref()
        .is_none_or(|screens| screens.contains(&screen.unwrap_or(ScreenKind::Unknown)))
}

fn select_canvas_preview(canvas: &CanvasPresentation, preview: &mut ScreenKind) {
    if !visible_on(canvas, Some(*preview))
        && let Some(screen) = SCREENS
            .into_iter()
            .find(|screen| visible_on(canvas, Some(*screen)))
    {
        *preview = screen;
    }
}

fn apply_canvas_geometry(
    canvas: &mut CanvasPresentation,
    field: GeometryField,
    value: i32,
    bounds: [u32; 2],
) {
    if value < 0 {
        return;
    }
    let Ok(value_u32) = u32::try_from(value) else {
        return;
    };
    let child_min = canvas.widgets.iter().fold([32, 32], |minimum, widget| {
        [
            minimum[0].max(u32::try_from(widget.x).unwrap_or_default() + widget.width),
            minimum[1].max(u32::try_from(widget.y).unwrap_or_default() + widget.height),
        ]
    });
    match field {
        GeometryField::X
            if value % 4 == 0 && value_u32.saturating_add(canvas.width) <= bounds[0] =>
        {
            canvas.x = value;
        }
        GeometryField::Y
            if value % 4 == 0 && value_u32.saturating_add(canvas.height) <= bounds[1] =>
        {
            canvas.y = value;
        }
        GeometryField::Width
            if value_u32 >= child_min[0]
                && u32::try_from(canvas.x)
                    .unwrap_or(u32::MAX)
                    .saturating_add(value_u32)
                    <= bounds[0] =>
        {
            let right = u32::try_from(canvas.x)
                .unwrap_or(u32::MAX)
                .saturating_add(value_u32);
            if value % 4 == 0 || right == bounds[0] {
                canvas.width = value_u32;
            }
        }
        GeometryField::Height
            if value_u32 >= child_min[1]
                && u32::try_from(canvas.y)
                    .unwrap_or(u32::MAX)
                    .saturating_add(value_u32)
                    <= bounds[1] =>
        {
            let bottom = u32::try_from(canvas.y)
                .unwrap_or(u32::MAX)
                .saturating_add(value_u32);
            if value % 4 == 0 || bottom == bounds[1] {
                canvas.height = value_u32;
            }
        }
        _ => {}
    }
}

fn canvas_geometry_value(canvas: &CanvasPresentation, field: GeometryField) -> i32 {
    match field {
        GeometryField::X => canvas.x,
        GeometryField::Y => canvas.y,
        GeometryField::Width => i32::try_from(canvas.width).unwrap_or(i32::MAX),
        GeometryField::Height => i32::try_from(canvas.height).unwrap_or(i32::MAX),
    }
}

fn apply_widget_geometry(
    widget: &mut WidgetLayout,
    field: GeometryField,
    value: i32,
    bounds: [u32; 2],
) {
    if value < 0 || value % 4 != 0 {
        return;
    }
    let Ok(value_u32) = u32::try_from(value) else {
        return;
    };
    match field {
        GeometryField::X if value_u32.saturating_add(widget.width) <= bounds[0] => {
            widget.x = value;
        }
        GeometryField::Y if value_u32.saturating_add(widget.height) <= bounds[1] => {
            widget.y = value;
        }
        GeometryField::Width
            if value_u32 >= 16
                && u32::try_from(widget.x)
                    .unwrap_or(u32::MAX)
                    .saturating_add(value_u32)
                    <= bounds[0] =>
        {
            widget.width = value_u32;
        }
        GeometryField::Height
            if value_u32 >= 16
                && u32::try_from(widget.y)
                    .unwrap_or(u32::MAX)
                    .saturating_add(value_u32)
                    <= bounds[1] =>
        {
            widget.height = value_u32;
        }
        _ => {}
    }
}

fn widget_geometry_value(widget: &WidgetLayout, field: GeometryField) -> i32 {
    match field {
        GeometryField::X => widget.x,
        GeometryField::Y => widget.y,
        GeometryField::Width => i32::try_from(widget.width).unwrap_or(i32::MAX),
        GeometryField::Height => i32::try_from(widget.height).unwrap_or(i32::MAX),
    }
}
fn grid(value: u32) -> u32 {
    value / 4 * 4
}
fn snap(value: i32) -> i32 {
    value.saturating_add(2).div_euclid(4) * 4
}
fn dimension(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}
#[must_use]
pub fn resize(
    old: [i32; 4],
    delta: [i32; 2],
    corner: Option<&str>,
    bounds: [u32; 2],
    min: [u32; 2],
    aspect: AspectRatio,
) -> [i32; 4] {
    let [x, y, w, h] = old;
    let Some(corner) = corner else {
        return [
            snap(x.saturating_add(delta[0])).clamp(0, (dimension(grid(bounds[0])) - w).max(0)),
            snap(y.saturating_add(delta[1])).clamp(0, (dimension(grid(bounds[1])) - h).max(0)),
            w,
            h,
        ];
    };
    let west = corner.contains('w');
    let north = corner.contains('n');
    let mut left = if west {
        snap(x.saturating_add(delta[0]))
    } else {
        x
    };
    let mut top = if north {
        snap(y.saturating_add(delta[1]))
    } else {
        y
    };
    let mut right = if west {
        x + w
    } else {
        snap((x + w).saturating_add(delta[0]))
    };
    let mut bottom = if north {
        y + h
    } else {
        snap((y + h).saturating_add(delta[1]))
    };
    left = left.clamp(0, (right - dimension(min[0])).max(0));
    top = top.clamp(0, (bottom - dimension(min[1])).max(0));
    right = right
        .max(left + dimension(min[0]))
        .min(dimension(grid(bounds[0])).max(left + dimension(min[0])));
    bottom = bottom
        .max(top + dimension(min[1]))
        .min(dimension(grid(bounds[1])).max(top + dimension(min[1])));
    let ratio = match aspect {
        AspectRatio::Free => None,
        AspectRatio::Wide => Some([16, 9]),
        AspectRatio::Standard => Some([4, 3]),
        AspectRatio::Current(ratio) => Some(ratio),
    };
    if let Some([rw, rh]) = ratio {
        if rw == 0 || rh == 0 {
            return old;
        }
        let rw = i64::from(rw);
        let rh = i64::from(rh);
        let anchor_x = if west { x + w } else { x };
        let anchor_y = if north { y + h } else { y };
        let max_w = i64::from(if west {
            anchor_x
        } else {
            dimension(bounds[0]) - anchor_x
        });
        let max_h = i64::from(if north {
            anchor_y
        } else {
            dimension(bounds[1]) - anchor_y
        })
        .min(max_w * rh / rw);
        let min_h = 16_i64.max((16 * rh + rw - 1) / rw);
        if max_h < min_h {
            return old;
        }
        let height = if i64::from(delta[0]).abs() * rh >= i64::from(delta[1]).abs() * rw {
            i64::from(right - left) * rh / rw
        } else {
            i64::from(bottom - top)
        }
        .clamp(min_h, max_h);
        let width = snap(i32::try_from(height * rw / rh).unwrap_or(i32::MAX))
            .min(i32::try_from(max_w).unwrap_or(i32::MAX));
        let height = snap(i32::try_from(height).unwrap_or(i32::MAX))
            .min(snap(i32::try_from(max_h).unwrap_or(i32::MAX)));
        left = if west { anchor_x - width } else { anchor_x };
        right = left + width;
        top = if north { anchor_y - height } else { anchor_y };
        bottom = top + height;
    }
    [left, top, right - left, bottom - top]
}

fn apply_widget_action(
    widget: &mut WidgetLayout,
    canvas_id: &str,
    title: &mut Option<TitleDraft>,
    action: &EditorAction,
) {
    match action {
        EditorAction::AspectRatio(index) => {
            if let Some(ratio) = [
                AspectRatio::Free,
                AspectRatio::Wide,
                AspectRatio::Standard,
                AspectRatio::Current([widget.width, widget.height]),
            ]
            .get(*index)
            {
                widget.settings.aspect_ratio = *ratio;
            }
        }
        EditorAction::HistoryCount(value) => widget.settings.history_count = *value,
        EditorAction::GraphMonths(value) => widget.settings.graph_months = *value,
        EditorAction::AcceptTitle => {
            if let Some(edit) = title
                && !edit.composing
                && edit.canvas == canvas_id
                && edit.widget == widget.id
            {
                widget.settings.title.clone_from(&edit.text);
                *title = None;
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod skin_tests {
    use super::*;

    #[derive(Clone, PartialEq)]
    struct Model(EditorSession);

    impl Model {
        fn new(
            canvases: Vec<CanvasPresentation>,
            viewport: [u32; 2],
            namespace: &'static str,
        ) -> Self {
            let mut installed = canvases
                .iter()
                .map(|canvas| skin(canvas.skin.name(), 1))
                .collect::<Vec<_>>();
            if installed.is_empty() {
                installed.push(skin("dev.example.test-skin", 1));
            }
            let mut model = EditorSession::new(canvases, viewport, namespace);
            model.set_skins(installed);
            Self(model)
        }
    }

    impl std::ops::Deref for Model {
        type Target = EditorSession;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl std::ops::DerefMut for Model {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.0
        }
    }

    fn skin(id: &str, default: i64) -> EditorSkin {
        EditorSkin {
            id: id.parse().unwrap(),
            name: id.into(),
            release: "1".into(),
            preview: "/preview.png".into(),
            preview_video: None,
            widget_defaults: [
                ("status", [544, 56]),
                ("selection", [544, 132]),
                ("score", [544, 208]),
                ("history-list", [544, 164]),
                ("history-graph", [544, 208]),
                ("empty", [320, 180]),
            ]
            .into_iter()
            .map(|(kind, [width, height])| {
                (
                    kind.into(),
                    crate::editor::EditorWidgetDefault { width, height },
                )
            })
            .collect(),
            canvas_properties: std::collections::BTreeMap::from([(
                "amount".into(),
                EditorProperty::Integer {
                    default,
                    minimum: 0,
                    maximum: 10,
                },
            )]),
            widget_properties: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn empty_workspace_creates_a_full_output_canvas_for_only_the_preview_context() {
        let mut model = Model::new(Vec::new(), [1, 1], "ignored");
        model.set_outputs(vec![crate::editor::EditorOutput {
            name: "DP-1".into(),
            model: "test".into(),
            logical_size: Some([1716, 1494]),
        }]);
        model.editing = true;
        model.readonly = false;
        model.preview = ScreenKind::Unknown;
        model.set_skins(vec![skin("dev.atty303.scorepeek.skin.dj-blackbox", 1)]);
        let selected: Skin = "dev.atty303.scorepeek.skin.dj-blackbox".parse().unwrap();
        model.action(&EditorAction::NewCanvasSkin(selected));

        assert!(model.action(&EditorAction::AddCanvas));
        let canvas = &model.draft[0];
        assert_eq!(canvas.output.as_deref(), Some("DP-1"));
        assert_eq!([canvas.x, canvas.y], [0, 0]);
        assert_eq!([canvas.width, canvas.height], [1716, 1494]);
        assert_eq!(canvas.skin, selected);
        assert_eq!(canvas.show_on, Some(vec![ScreenKind::Unknown]));
        assert_eq!(canvas.name, "Canvas 1");

        assert!(model.action(&EditorAction::DeleteCanvas));
        assert!(model.draft.is_empty());
        assert!(model.selected_canvas.is_none());
    }

    #[test]
    fn empty_workspace_without_installed_skins_cannot_add_a_canvas() {
        let mut model = EditorSession::new(Vec::new(), [800, 600], "test");
        model.set_outputs(vec![crate::editor::EditorOutput {
            name: "DP-1".into(),
            model: "test".into(),
            logical_size: Some([800, 600]),
        }]);
        model.editing = true;
        model.readonly = false;

        assert!(!model.action(&EditorAction::AddCanvas));
        assert!(model.draft.is_empty());
        assert!(model.new_canvas_skin.is_none());
    }

    #[test]
    fn adding_a_widget_centers_it_and_selects_it_immediately() {
        let mut model = Model::new(Vec::new(), [800, 600], "ignored");
        model.editing = true;
        model.readonly = false;
        model.action(&EditorAction::AddCanvas);

        assert!(model.action(&EditorAction::AddWidget(0)));
        let canvas = &model.draft[0];
        let widget = &canvas.widgets[0];
        assert_eq!(model.selected_widget.as_deref(), Some(widget.id.as_str()));
        assert_eq!(widget.kind, WidgetKind::Status);
        assert_eq!(
            widget.x,
            snap(i32::try_from((canvas.width - widget.width) / 2).unwrap())
        );
        assert_eq!(
            widget.y,
            snap(i32::try_from((canvas.height - widget.height) / 2).unwrap())
        );
        assert!(model.placing.is_none());
    }

    #[test]
    fn widget_picker_closes_before_adding_and_stays_closed() {
        let mut model = Model::new(Vec::new(), [800, 600], "ignored");
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        model.action(&EditorAction::ToggleWidgetAdd);
        assert!(model.chrome.widget_add_open);

        model.action(&EditorAction::ToggleWidgetAdd);
        assert!(model.action(&EditorAction::AddWidget(0)));

        assert!(!model.chrome.widget_add_open);
        assert_eq!(model.draft[0].widgets.len(), 1);
        assert!(model.selected_widget.is_some());
    }

    #[test]
    fn widget_selection_changes_its_canvas_owner_atomically() {
        let mut model = Model::new(Vec::new(), [800, 600], "ignored");
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        assert!(model.action(&EditorAction::AddWidget(0)));
        let first_canvas = model.draft[0].id.clone();
        let shared_widget_id = model.draft[0].widgets[0].id.clone();
        assert!(model.action(&EditorAction::AddCanvas));
        assert!(model.action(&EditorAction::AddWidget(0)));
        let second_canvas = model.draft[1].id.clone();
        assert_eq!(model.draft[1].widgets[0].id, shared_widget_id);

        model.action(&EditorAction::SelectWidget {
            canvas_id: first_canvas.clone(),
            widget_id: shared_widget_id.clone(),
        });
        assert_eq!(
            model.selected_canvas.as_deref(),
            Some(first_canvas.as_str())
        );
        assert!(model.action(&EditorAction::DeleteWidget));
        assert!(model.draft[0].widgets.is_empty());
        assert!(model.selected_canvas.is_none());
        assert!(model.selected_widget.is_none());
        assert_eq!(model.draft[1].id, second_canvas);
        assert_eq!(model.draft[1].widgets[0].id, shared_widget_id);
    }

    #[test]
    fn selecting_a_widget_switches_to_a_screen_where_its_canvas_is_visible() {
        let mut model = Model::new(Vec::new(), [800, 600], "ignored");
        model.editing = true;
        model.readonly = false;
        model.preview = ScreenKind::Play;
        assert!(model.action(&EditorAction::AddCanvas));
        assert!(model.action(&EditorAction::AddWidget(0)));
        let canvas_id = model.draft[0].id.clone();
        let widget_id = model.draft[0].widgets[0].id.clone();
        model.draft[0].show_on = Some(vec![ScreenKind::Result]);
        model.preview = ScreenKind::Play;
        model.clear_selection();

        model.action(&EditorAction::SelectWidget {
            canvas_id,
            widget_id,
        });

        assert_eq!(model.preview, ScreenKind::Result);
    }

    #[test]
    fn editor_entry_and_repeated_object_selection_allow_no_selection() {
        let mut model = Model::new(Vec::new(), [800, 600], "ignored");
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        let canvas = model.selected_canvas.clone().unwrap();
        model.enter(Some(canvas.clone()), ScreenKind::Play);
        assert!(model.selected_canvas.is_none());
        model.action(&EditorAction::SelectCanvas(canvas.clone()));
        assert_eq!(model.selected_canvas.as_deref(), Some(canvas.as_str()));
        model.action(&EditorAction::SelectCanvas(canvas.clone()));
        assert!(model.selected_canvas.is_none());
        model.action(&EditorAction::SelectCanvas(canvas));
        model.action(&EditorAction::ClearSelection);
        assert!(model.selected_canvas.is_none());
    }

    #[test]
    fn canvas_names_and_geometry_participate_in_undo_and_validation() {
        let mut model = Model::new(Vec::new(), [800, 600], "ignored");
        model.editing = true;
        model.readonly = false;
        model.action(&EditorAction::AddCanvas);
        let draft = model.draft.clone();
        model.saved.clone_from(&draft);
        model.undo = None;

        assert!(model.action(&EditorAction::CanvasName(String::new())));
        assert!(!model.document_valid());
        assert!(model.action(&EditorAction::Undo));
        assert_eq!(model.current().unwrap().name, "Canvas 1");
        assert!(!model.action(&EditorAction::CanvasGeometry(GeometryField::X, 40)));
        assert_eq!(
            model.current().unwrap().x,
            0,
            "full-size canvas cannot move outside output"
        );
    }

    #[test]
    fn output_navigation_preserves_selection_without_changing_preview_or_draft() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "ignored");
        model.set_outputs(vec![
            crate::editor::EditorOutput {
                name: "DP-1".into(),
                model: "first".into(),
                logical_size: Some([1920, 1080]),
            },
            crate::editor::EditorOutput {
                name: "DP-2".into(),
                model: "second".into(),
                logical_size: Some([1080, 1920]),
            },
        ]);
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        let draft = model.draft.clone();
        model.preview = ScreenKind::Result;

        assert!(!model.action(&EditorAction::SelectOutput("DP-2".into())));
        assert_eq!(model.active_output.as_deref(), Some("DP-2"));
        assert_eq!(model.preview, ScreenKind::Result);
        assert_eq!(model.draft, draft);
        assert_eq!(model.selected_canvas, Some(draft[0].id.clone()));
    }

    #[test]
    fn selecting_the_active_output_preserves_canvas_selection() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "ignored");
        model.set_outputs(vec![crate::editor::EditorOutput {
            name: "DP-1".into(),
            model: "first".into(),
            logical_size: Some([1920, 1080]),
        }]);
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        let selected = model.selected_canvas.clone();

        assert!(!model.action(&EditorAction::SelectOutput("DP-1".into())));

        assert_eq!(model.selected_canvas, selected);
        assert!(model.stage_projections()[0].selected_canvas.is_some());
    }

    #[test]
    fn selecting_a_peer_canvas_or_widget_preserves_editor_output() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "ignored");
        model.set_outputs(vec![
            crate::editor::EditorOutput {
                name: "DP-1".into(),
                model: "first".into(),
                logical_size: Some([1920, 1080]),
            },
            crate::editor::EditorOutput {
                name: "DP-2".into(),
                model: "second".into(),
                logical_size: Some([1080, 1920]),
            },
        ]);
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        model.action(&EditorAction::SelectOutput("DP-2".into()));
        assert!(model.action(&EditorAction::AddCanvas));
        assert!(model.action(&EditorAction::AddWidget(0)));
        let canvas = model.selected_canvas.clone().unwrap();
        let widget = model.selected_widget.clone().unwrap();
        model.action(&EditorAction::SelectOutput("DP-1".into()));
        model.clear_selection();

        model.action(&EditorAction::SelectCanvas(canvas.clone()));
        assert_eq!(model.active_output.as_deref(), Some("DP-1"));
        assert!(model.stage_projections()[1].selected_canvas.is_some());

        model.action(&EditorAction::SelectOutput("DP-1".into()));
        model.action(&EditorAction::SelectWidget {
            canvas_id: canvas,
            widget_id: widget,
        });
        assert_eq!(model.active_output.as_deref(), Some("DP-1"));
        assert!(model.stage_projections()[1].selected_widget.is_some());
    }

    #[test]
    fn selecting_a_canvas_without_a_connected_output_preserves_the_active_output() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "ignored");
        model.set_outputs(vec![crate::editor::EditorOutput {
            name: "DP-1".into(),
            model: "first".into(),
            logical_size: Some([1920, 1080]),
        }]);
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        let canvas = model.selected_canvas.clone().unwrap();
        model
            .draft
            .iter_mut()
            .find(|candidate| candidate.id == canvas)
            .unwrap()
            .output = Some("disconnected".into());

        model.action(&EditorAction::SelectCanvas(canvas));

        assert_eq!(model.active_output.as_deref(), Some("DP-1"));
    }

    #[test]
    fn explicit_open_preview_is_not_replaced_by_the_observed_screen() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "ignored");
        model.screen = Some(ScreenKind::MusicSelect);
        model.reduce(EditorInput::Open {
            output: None,
            canvas: None,
            preview: ScreenKind::Result,
        });

        assert_eq!(model.preview, ScreenKind::Result);
    }

    #[test]
    fn passive_transport_ready_does_not_acquire_until_surface_entry() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "obs");

        let effects = model.reduce(EditorInput::TransportReady {
            screen: Some(ScreenKind::MusicSelect),
            sample: false,
            canvases: Vec::new(),
            first: true,
        });

        assert!(effects.is_empty());
        assert!(!model.editing);
        assert_eq!(
            model.reduce(EditorInput::Surface(
                crate::editor::effect::SurfaceAction::Enter(None)
            )),
            vec![EditorEffect::Acquire]
        );
        assert!(model.editing);
    }

    #[test]
    fn surface_entry_before_transport_ready_is_acquired_when_transport_connects() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "obs");

        assert_eq!(
            model.reduce(EditorInput::Surface(
                crate::editor::effect::SurfaceAction::Enter(None)
            )),
            vec![EditorEffect::Acquire]
        );
        assert!(model.editing);
        assert_eq!(
            model.reduce(EditorInput::TransportReady {
                screen: Some(ScreenKind::MusicSelect),
                sample: false,
                canvases: Vec::new(),
                first: true,
            }),
            vec![EditorEffect::Acquire]
        );
    }

    #[test]
    fn chrome_navigation_is_revisioned_editor_session_state() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "ignored");
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        let canvas = model.selected_canvas.clone().unwrap();
        model.chrome.expanded_canvases.clear();
        model.clear_selection();
        let revision = model.revision;

        assert!(
            model
                .reduce(EditorInput::Action(EditorAction::SelectCanvas(
                    canvas.clone()
                )))
                .is_empty()
        );

        assert_eq!(model.revision, revision + 1);
        assert!(model.chrome.expanded_canvases.contains(&canvas));
        assert_eq!(model.selected_canvas.as_deref(), Some(canvas.as_str()));
    }

    #[test]
    fn field_validation_is_revisioned_editor_session_state() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "ignored");
        model.set_outputs(vec![crate::editor::EditorOutput {
            name: "DP-1".into(),
            model: "first".into(),
            logical_size: Some([1920, 1080]),
        }]);
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        let canvas = model.selected_canvas.clone().unwrap();
        let field = format!("{canvas}:width");
        let revision = model.revision;

        model.reduce(EditorInput::Action(EditorAction::UpdateFieldDraft(
            field.clone(),
            "99999".into(),
            false,
        )));

        assert_eq!(model.revision, revision + 1);
        assert!(!model.chrome.field_drafts[&field].valid);
        assert!(!model.document_valid());
        model.reduce(EditorInput::Action(EditorAction::TogglePanel));
        model.reduce(EditorInput::Action(EditorAction::TogglePanel));
        assert_eq!(model.chrome.field_drafts[&field].text, "99999");
        assert!(!model.document_valid());
        assert_eq!(model.selected_canvas.as_deref(), Some(canvas.as_str()));
        model.reduce(EditorInput::Action(EditorAction::SelectOutput(
            "DP-1".into(),
        )));
        assert_eq!(model.chrome.field_drafts[&field].text, "99999");
        assert!(!model.document_valid());

        model.reduce(EditorInput::Action(EditorAction::EndFieldEdit(field, true)));
        assert!(model.document_valid());
    }

    #[test]
    fn field_commit_uses_the_authoritative_draft_and_preserves_composition() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "ignored");
        model.set_outputs(vec![crate::editor::EditorOutput {
            name: "DP-1".into(),
            model: "first".into(),
            logical_size: Some([1920, 1080]),
        }]);
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        let canvas = model.selected_canvas.clone().unwrap();
        let field = format!("{canvas}:width");
        let initial_width = model.current().unwrap().width;

        model.reduce(EditorInput::Action(EditorAction::BeginFieldEdit(
            field.clone(),
            initial_width.to_string(),
        )));
        model.reduce(EditorInput::Action(EditorAction::TextComposition {
            field_key: field.clone(),
            composing: true,
        }));
        model.reduce(EditorInput::Action(EditorAction::UpdateFieldDraft(
            field.clone(),
            "400".into(),
            true,
        )));
        assert!(model.chrome.field_drafts[&field].composing);

        model.reduce(EditorInput::Action(EditorAction::CommitFieldDraft(
            field.clone(),
            EditorFieldCommit::CanvasGeometry(GeometryField::Width),
        )));
        assert_eq!(model.current().unwrap().width, initial_width);
        assert!(model.chrome.field_drafts.contains_key(&field));

        model.reduce(EditorInput::Action(EditorAction::TextComposition {
            field_key: field.clone(),
            composing: false,
        }));
        model.reduce(EditorInput::Action(EditorAction::CommitFieldDraft(
            field.clone(),
            EditorFieldCommit::CanvasGeometry(GeometryField::Width),
        )));
        assert_eq!(model.current().unwrap().width, 400);
        assert!(!model.chrome.field_drafts.contains_key(&field));
    }

    #[test]
    fn field_commit_revalidates_against_current_output_bounds() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "ignored");
        model.set_outputs(vec![crate::editor::EditorOutput {
            name: "DP-1".into(),
            model: "first".into(),
            logical_size: Some([1920, 1080]),
        }]);
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        let canvas = model.selected_canvas.clone().unwrap();
        let field = format!("{canvas}:width");
        model.reduce(EditorInput::Action(EditorAction::UpdateFieldDraft(
            field.clone(),
            "1600".into(),
            true,
        )));
        model.reduce(EditorInput::Resize {
            output: "DP-1".into(),
            logical_size: [1280, 720],
        });

        let effects = model.reduce(EditorInput::Action(EditorAction::CommitFieldDraft(
            field.clone(),
            EditorFieldCommit::CanvasGeometry(GeometryField::Width),
        )));

        assert!(effects.is_empty());
        assert_eq!(model.current().unwrap().width, 1920);
        assert_eq!(model.chrome.field_drafts[&field].text, "1600");
        assert!(!model.chrome.field_drafts[&field].valid);
    }

    #[test]
    fn field_commit_accepts_a_draft_that_current_output_bounds_make_valid() {
        let mut model = Model::new(Vec::new(), [1280, 720], "ignored");
        model.set_outputs(vec![crate::editor::EditorOutput {
            name: "DP-1".into(),
            model: "first".into(),
            logical_size: Some([1280, 720]),
        }]);
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        let canvas = model.selected_canvas.clone().unwrap();
        let field = format!("{canvas}:width");
        model.reduce(EditorInput::Action(EditorAction::UpdateFieldDraft(
            field.clone(),
            "1600".into(),
            false,
        )));
        model.reduce(EditorInput::Resize {
            output: "DP-1".into(),
            logical_size: [1920, 1080],
        });

        let effects = model.reduce(EditorInput::Action(EditorAction::CommitFieldDraft(
            field.clone(),
            EditorFieldCommit::CanvasGeometry(GeometryField::Width),
        )));

        assert!(matches!(effects.as_slice(), [EditorEffect::Update { .. }]));
        assert_eq!(model.current().unwrap().width, 1600);
        assert!(!model.chrome.field_drafts.contains_key(&field));
    }

    #[test]
    fn canvas_can_be_selected_after_round_trip_between_outputs() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "ignored");
        model.set_outputs(vec![
            crate::editor::EditorOutput {
                name: "DP-1".into(),
                model: "first".into(),
                logical_size: Some([1920, 1080]),
            },
            crate::editor::EditorOutput {
                name: "DP-2".into(),
                model: "second".into(),
                logical_size: Some([1920, 1080]),
            },
        ]);
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        let canvas = model.selected_canvas.clone().unwrap();

        model.reduce(EditorInput::Action(EditorAction::SelectOutput(
            "DP-2".into(),
        )));
        model.reduce(EditorInput::Action(EditorAction::SelectOutput(
            "DP-1".into(),
        )));
        assert_eq!(model.selected_canvas.as_deref(), Some(canvas.as_str()));
        assert_eq!(
            model.stage_projections()[0]
                .selected_canvas
                .as_ref()
                .map(|item| item.id.as_str()),
            Some(canvas.as_str())
        );
    }

    #[test]
    fn disconnected_editor_output_selects_a_peer_and_preserves_selection() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "ignored");
        model.set_outputs(vec![
            crate::editor::EditorOutput {
                name: "DP-1".into(),
                model: "first".into(),
                logical_size: Some([1920, 1080]),
            },
            crate::editor::EditorOutput {
                name: "DP-2".into(),
                model: "second".into(),
                logical_size: Some([1920, 1080]),
            },
        ]);
        model.editing = true;
        model.readonly = false;
        model.reduce(EditorInput::Action(EditorAction::SelectOutput(
            "DP-2".into(),
        )));
        assert!(model.action(&EditorAction::AddCanvas));
        model.set_outputs(vec![crate::editor::EditorOutput {
            name: "DP-1".into(),
            model: "first".into(),
            logical_size: Some([1920, 1080]),
        }]);

        assert_eq!(model.active_output.as_deref(), Some("DP-1"));
        assert!(model.selected_canvas.is_some());
        assert!(model.selected_widget.is_none());
        assert!(model.drag.is_none());
    }

    #[test]
    fn every_drag_pointer_event_advances_the_projection_revision() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "ignored");
        model.set_outputs(vec![crate::editor::EditorOutput {
            name: "DP-1".into(),
            model: "first".into(),
            logical_size: Some([1920, 1080]),
        }]);
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        let canvas = model.selected_canvas.clone().unwrap();
        model.reduce(EditorInput::Surface(
            crate::editor::effect::SurfaceAction::Start {
                canvas,
                widget: None,
                corner: None,
                point: [0, 0],
            },
        ));
        let before = model.revision;
        let effects = model.reduce(EditorInput::Surface(
            crate::editor::effect::SurfaceAction::Move([20, 20]),
        ));

        assert!(
            effects.is_empty(),
            "drag motion is projected before persistence"
        );
        assert_eq!(model.revision, before + 1);
        assert_eq!(model.stage_projections()[0].revision, model.revision);
    }

    #[test]
    fn delayed_update_reply_does_not_replace_a_newer_drag_draft() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "ignored");
        model.set_outputs(vec![crate::editor::EditorOutput {
            name: "DP-1".into(),
            model: "first".into(),
            logical_size: Some([1920, 1080]),
        }]);
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        let canvas = model.selected_canvas.clone().unwrap();
        let requested = model.draft.clone();
        model.reduce(EditorInput::Surface(
            crate::editor::effect::SurfaceAction::Start {
                canvas,
                widget: None,
                corner: None,
                point: [0, 0],
            },
        ));
        model.reduce(EditorInput::Surface(
            crate::editor::effect::SurfaceAction::Move([40, 24]),
        ));
        let current = model.draft.clone();

        model.reduce(EditorInput::BackendCompleted {
            effect: EditorEffectKind::Update,
            requested_draft: Some(requested.clone()),
            reply: EditorBackendReply {
                ok: true,
                readonly: false,
                error: None,
                canvases: requested,
                dirty: true,
            },
        });

        assert_eq!(model.draft, current);
        assert!(model.drag.is_some());
    }

    #[test]
    fn delayed_save_reply_resubmits_the_current_draft_before_closing() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "ignored");
        model.set_outputs(vec![crate::editor::EditorOutput {
            name: "DP-1".into(),
            model: "first".into(),
            logical_size: Some([1920, 1080]),
        }]);
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        let requested = model.draft.clone();
        model.draft[0].x = 40;

        let effects = model.reduce(EditorInput::BackendCompleted {
            effect: EditorEffectKind::Save,
            requested_draft: Some(requested.clone()),
            reply: EditorBackendReply {
                ok: true,
                readonly: false,
                error: None,
                canvases: requested,
                dirty: false,
            },
        });

        assert_eq!(
            effects,
            vec![EditorEffect::Save {
                canvases: model.draft.clone(),
            }]
        );
        assert!(model.editing);
        assert_ne!(model.saved, model.draft);
    }

    #[test]
    fn passive_pointer_motion_does_not_change_editor_authority_or_projection() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "ignored");
        model.set_outputs(vec![crate::editor::EditorOutput {
            name: "DP-1".into(),
            model: "first".into(),
            logical_size: Some([1920, 1080]),
        }]);
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        let before = model.clone();
        let projections = model.stage_projections();

        let effects = model.reduce(EditorInput::Surface(
            crate::editor::effect::SurfaceAction::Move([320, 180]),
        ));

        assert!(effects.is_empty());
        assert!(model == before);
        assert!(model.stage_projections() == projections);
    }

    #[test]
    fn placement_pointer_motion_remains_revisioned() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "ignored");
        model.set_outputs(vec![crate::editor::EditorOutput {
            name: "DP-1".into(),
            model: "first".into(),
            logical_size: Some([1920, 1080]),
        }]);
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        model.placing = Some(crate::WidgetKind::Empty);
        let before = model.revision;

        let effects = model.reduce(EditorInput::Surface(
            crate::editor::effect::SurfaceAction::Move([320, 180]),
        ));

        assert!(effects.is_empty());
        assert_eq!(model.point, [320, 180]);
        assert_eq!(model.revision, before + 1);
    }

    #[test]
    fn output_reassignment_revalidates_canvas_bounds_before_save() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "ignored");
        model.set_outputs(vec![
            crate::editor::EditorOutput {
                name: "large".into(),
                model: "large".into(),
                logical_size: Some([1920, 1080]),
            },
            crate::editor::EditorOutput {
                name: "small".into(),
                model: "small".into(),
                logical_size: Some([800, 600]),
            },
        ]);
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        assert!(model.document_valid());
        assert!(model.action(&EditorAction::Output("small".into())));
        assert!(!model.document_valid());
        assert_eq!(
            model.view().access.save_validity,
            crate::editor::SaveValidity::Invalid
        );
    }

    #[test]
    fn canvas_geometry_uses_its_assigned_output_instead_of_the_active_output() {
        let canvas = CanvasPresentation {
            id: "large-canvas".into(),
            name: "Large canvas".into(),
            skin: "dev.example.skin".parse().unwrap(),
            skin_properties: std::collections::BTreeMap::new(),
            show_on: None,
            opacity_percent: 100,
            output: Some("large".into()),
            x: 0,
            y: 0,
            width: 560,
            height: 560,
            widgets: Vec::new(),
        };
        let mut model = Model::new(vec![canvas], [800, 600], "ignored");
        model.set_outputs(vec![
            crate::editor::EditorOutput {
                name: "small".into(),
                model: "small".into(),
                logical_size: Some([800, 600]),
            },
            crate::editor::EditorOutput {
                name: "large".into(),
                model: "large".into(),
                logical_size: Some([1920, 1080]),
            },
        ]);
        model.active_output = Some("small".into());
        model.selected_canvas = Some("large-canvas".into());
        model.readonly = false;

        assert!(model.action(&EditorAction::CanvasGeometry(GeometryField::X, 1000)));
        assert_eq!(model.current().unwrap().x, 1000);
        assert!(model.document_valid());
    }

    #[test]
    fn exact_non_grid_output_edge_and_resized_active_output_remain_saveable() {
        let mut model = Model::new(Vec::new(), [1716, 1494], "ignored");
        model.set_outputs(vec![crate::editor::EditorOutput {
            name: "output".into(),
            model: "output".into(),
            logical_size: Some([1716, 1494]),
        }]);
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        assert_eq!([model.draft[0].width, model.draft[0].height], [1716, 1494]);
        assert!(model.document_valid());

        model.resize_output("output", [1922, 1082]);
        assert_eq!(model.viewport, [1922, 1082]);
        assert_eq!(model.outputs[0].logical_size, Some([1922, 1082]));
        assert!(model.action(&EditorAction::FitToOutput));
        assert!(model.document_valid());
    }

    #[test]
    fn peer_output_resize_does_not_change_active_output_bounds() {
        let mut model = Model::new(Vec::new(), [1920, 1080], "ignored");
        model.set_outputs(vec![
            crate::editor::EditorOutput {
                name: "DP-1".into(),
                model: "primary".into(),
                logical_size: Some([1920, 1080]),
            },
            crate::editor::EditorOutput {
                name: "DP-2".into(),
                model: "peer".into(),
                logical_size: Some([1280, 720]),
            },
        ]);

        model.reduce(EditorInput::Resize {
            output: "DP-2".into(),
            logical_size: [1024, 768],
        });

        assert_eq!(model.active_output.as_deref(), Some("DP-1"));
        assert_eq!(model.viewport, [1920, 1080]);
        assert_eq!(model.outputs[0].logical_size, Some([1920, 1080]));
        assert_eq!(model.outputs[1].logical_size, Some([1024, 768]));
    }

    #[test]
    fn canvas_geometry_accepts_an_exact_non_grid_output_edge() {
        let mut model = Model::new(Vec::new(), [1716, 1494], "ignored");
        model.set_outputs(vec![crate::editor::EditorOutput {
            name: "output".into(),
            model: "output".into(),
            logical_size: Some([1716, 1494]),
        }]);
        model.editing = true;
        model.readonly = false;
        assert!(model.action(&EditorAction::AddCanvas));
        assert!(model.action(&EditorAction::CanvasGeometry(GeometryField::Height, 1200,)));

        assert!(model.action(&EditorAction::CanvasGeometry(GeometryField::Height, 1494,)));

        assert_eq!(model.current().unwrap().height, 1494);
        assert!(model.document_valid());
    }

    #[test]
    fn remote_canvas_drag_uses_its_own_output_bounds() {
        let canvas = CanvasPresentation {
            id: "wide-canvas".into(),
            name: "Wide canvas".into(),
            skin: "dev.example.skin".parse().unwrap(),
            skin_properties: std::collections::BTreeMap::new(),
            show_on: None,
            opacity_percent: 100,
            output: Some("DP-2".into()),
            x: 0,
            y: 0,
            width: 560,
            height: 960,
            widgets: Vec::new(),
        };
        let mut model = Model::new(vec![canvas], [1, 1], "ignored");
        model.set_outputs(vec![
            crate::editor::EditorOutput {
                name: "DP-2".into(),
                model: "portrait".into(),
                logical_size: Some([1728, 3072]),
            },
            crate::editor::EditorOutput {
                name: "DP-1".into(),
                model: "ultrawide".into(),
                logical_size: Some([5120, 1440]),
            },
        ]);
        model.activate_output(Some("DP-1"));
        model.editing = true;
        model.readonly = false;
        model.reduce(EditorInput::SurfaceOnOutput {
            output: "DP-2".into(),
            action: crate::editor::effect::SurfaceAction::Start {
                canvas: "wide-canvas".into(),
                widget: None,
                corner: None,
                point: [0, 0],
            },
        });
        model.reduce(EditorInput::SurfaceOnOutput {
            output: "DP-1".into(),
            action: crate::editor::effect::SurfaceAction::Move([10_000, 0]),
        });
        assert_eq!(model.draft[0].x, 0);
        model.reduce(EditorInput::SurfaceOnOutput {
            output: "DP-2".into(),
            action: crate::editor::effect::SurfaceAction::Move([10_000, 0]),
        });

        assert_eq!(model.active_output.as_deref(), Some("DP-1"));
        assert_eq!(model.draft[0].x, 1168);
        model.reduce(EditorInput::SurfaceOnOutput {
            output: "DP-1".into(),
            action: crate::editor::effect::SurfaceAction::Cancel,
        });
        assert_eq!(model.draft[0].x, 1168);
        model.reduce(EditorInput::SurfaceOnOutput {
            output: "DP-1".into(),
            action: crate::editor::effect::SurfaceAction::CancelFromKeyboard,
        });
        assert_eq!(model.draft[0].x, 0);
    }

    #[test]
    fn installed_catalog_preserves_values_until_switch_or_save() {
        let first: Skin = "dev.example.first".parse().unwrap();
        let second: Skin = "dev.example.second".parse().unwrap();
        let canvas = CanvasPresentation {
            id: "canvas".into(),
            name: "Canvas".into(),
            skin: first,
            skin_properties: std::collections::BTreeMap::from([(
                "amount".into(),
                serde_json::json!(99),
            )]),
            show_on: None,
            opacity_percent: 100,
            output: None,
            x: 0,
            y: 0,
            width: 560,
            height: 1040,
            widgets: Vec::new(),
        };
        let mut model = Model::new(vec![canvas], [1920, 1080], "test");
        model.select_canvas("canvas");
        model.set_skins(vec![skin(first.name(), 2), skin(second.name(), 7)]);
        assert_eq!(
            model.draft[0].skin_properties["amount"],
            serde_json::json!(99)
        );
        model.apply_settings(&EditorAction::Skin(second));
        assert_eq!(model.draft[0].skin, second);
        assert_eq!(
            model.draft[0].skin_properties["amount"],
            serde_json::json!(7)
        );
        let mut received = model.draft[0].clone();
        received
            .skin_properties
            .insert("amount".into(), serde_json::json!(-1));
        model.receive_stage(vec![received]);
        assert_eq!(
            model.draft[0].skin_properties["amount"],
            serde_json::json!(-1)
        );
        model.normalize_for_save();
        assert_eq!(
            model.draft[0].skin_properties["amount"],
            serde_json::json!(7)
        );
    }

    #[test]
    fn installing_catalog_does_not_create_a_draft_change() {
        let id: Skin = "dev.example.skin".parse().unwrap();
        let canvas = CanvasPresentation {
            id: "canvas".into(),
            name: "Canvas".into(),
            skin: id,
            skin_properties: std::collections::BTreeMap::from([(
                "amount".into(),
                serde_json::json!(99),
            )]),
            show_on: None,
            opacity_percent: 100,
            output: None,
            x: 0,
            y: 0,
            width: 560,
            height: 1040,
            widgets: Vec::new(),
        };
        let mut model = Model::new(vec![canvas], [1920, 1080], "test");

        model.set_skins(vec![skin(id.name(), 2)]);

        assert!(!model.dirty());
        assert_eq!(
            model.draft[0].skin_properties["amount"],
            serde_json::json!(99)
        );
    }

    #[test]
    fn readonly_keeps_navigation_available_and_rejects_mutation() {
        let canvas = CanvasPresentation {
            id: "canvas".into(),
            name: "Canvas".into(),
            skin: "dev.example.skin".parse().unwrap(),
            skin_properties: std::collections::BTreeMap::new(),
            show_on: None,
            opacity_percent: 100,
            output: None,
            x: 0,
            y: 0,
            width: 560,
            height: 1040,
            widgets: Vec::new(),
        };
        let mut model = Model::new(vec![canvas], [1920, 1080], "test");
        model.readonly = true;
        model.chrome.panel_open = true;
        let before = model.draft.clone();

        assert!(!model.action(&EditorAction::TogglePanel));
        assert!(!model.chrome.panel_open);
        assert!(!model.action(&EditorAction::AddCanvas));
        assert_eq!(model.draft, before);
    }
}
