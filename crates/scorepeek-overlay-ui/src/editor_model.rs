use crate::editor::{
    EditorAccess, EditorAction, EditorChrome, EditorProperty, EditorSkin, EditorTitleState,
    EditorView,
};
use crate::{
    AspectRatio, Background, CanvasPresentation, ScreenKind, Skin, WidgetKind, WidgetLayout,
    WidgetSettings, default_widget_size, next_widget_id,
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
#[derive(Clone)]
pub struct TitleDraft {
    pub canvas: String,
    pub widget: String,
    pub text: String,
    pub composing: bool,
}
#[derive(Clone)]
pub struct Drag {
    pub canvas: String,
    pub widget: Option<String>,
    pub corner: Option<String>,
    pub start: [i32; 2],
    pub original: Vec<CanvasPresentation>,
}
#[derive(Clone)]
pub struct Model {
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
    pub new_canvas_skin: Skin,
}
impl Model {
    #[must_use]
    pub fn new(
        canvases: Vec<CanvasPresentation>,
        viewport: [u32; 2],
        _namespace: &'static str,
    ) -> Self {
        let active_output = canvases.first().and_then(|canvas| canvas.output.clone());
        let new_canvas_skin = canvases
            .first()
            .map_or(Skin::CyanSystem, |canvas| canvas.skin);
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
                        canvas_properties: std::collections::BTreeMap::new(),
                        widget_properties: std::collections::BTreeMap::new(),
                    },
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_values()
            .collect();
        Self {
            selected_canvas: canvases.first().map(|canvas| canvas.id.clone()),
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

    pub fn set_skins(&mut self, skins: Vec<EditorSkin>) {
        self.skins = skins;
        if !self
            .skins
            .iter()
            .any(|skin| skin.id == self.new_canvas_skin)
        {
            self.new_canvas_skin = self.skins.first().map_or(Skin::CyanSystem, |skin| skin.id);
        }
    }
    pub fn set_outputs(&mut self, outputs: Vec<crate::editor::EditorOutput>) {
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
    pub fn current(&self) -> Option<&CanvasPresentation> {
        self.draft
            .iter()
            .find(|canvas| Some(&canvas.id) == self.selected_canvas.as_ref())
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
            backend_label: "EDITOR".into(),
            canvases: self
                .draft
                .iter()
                .filter(|canvas| canvas.output.as_ref() == self.active_output.as_ref())
                .cloned()
                .collect(),
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
            refresh_rate: None,
            skins: self.skins.clone(),
            new_canvas_skin: self.new_canvas_skin,
        }
    }
    pub fn select_visible(&mut self) {
        self.selected_canvas = self
            .draft
            .iter()
            .find(|canvas| {
                canvas.output.as_ref() == self.active_output.as_ref()
                    && visible_on(canvas, Some(self.preview))
            })
            .map(|canvas| canvas.id.clone());
        self.selected_widget = None;
        self.title = None;
    }
    pub fn enter(&mut self, canvas: Option<String>) {
        self.editing = true;
        self.chrome.panel_open = true;
        self.preview = self.screen.unwrap_or(ScreenKind::MusicSelect);
        self.select_visible();
        if canvas.is_some() {
            self.selected_canvas = canvas;
        }
        self.readonly = true;
    }
    pub fn close(&mut self) {
        self.editing = false;
        self.discard_pending = false;
        self.selected_widget = None;
        self.undo = None;
        self.drag = None;
        self.placing = None;
        self.title = None;
    }
    fn navigate(&mut self, action: &EditorAction) -> bool {
        match action {
            EditorAction::TogglePanel => self.chrome.panel_open = !self.chrome.panel_open,
            EditorAction::PreviewScreen(screen) => {
                self.preview = *screen;
            }
            EditorAction::SelectOutput(output) => {
                if self
                    .outputs
                    .iter()
                    .any(|candidate| candidate.name == *output)
                {
                    self.activate_output(Some(output));
                    self.selected_canvas = None;
                    self.selected_widget = None;
                    self.title = None;
                }
            }
            EditorAction::SelectCanvas(id) => {
                if let Some(canvas) = self.draft.iter().find(|canvas| &canvas.id == id) {
                    if !visible_on(canvas, Some(self.preview))
                        && let Some(screen) = SCREENS
                            .into_iter()
                            .find(|screen| visible_on(canvas, Some(*screen)))
                    {
                        self.preview = screen;
                    }
                    self.selected_canvas = Some(id.clone());
                    self.selected_widget = None;
                    self.title = None;
                }
            }
            EditorAction::SelectWidget(id) => {
                self.selected_widget = Some(id.clone());
                self.title = None;
            }
            EditorAction::ToggleWidgetAdd => {
                self.chrome.widget_add_open = !self.chrome.widget_add_open;
            }
            EditorAction::NewCanvasSkin(skin) => {
                if self.skins.iter().any(|candidate| candidate.id == *skin) {
                    self.new_canvas_skin = *skin;
                }
            }
            EditorAction::CancelTitle => self.title = None,
            _ => return false,
        }
        true
    }
    pub fn action(&mut self, action: &EditorAction) -> bool {
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
                    if self.current().is_none() {
                        self.select_visible();
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
                self.placing = [
                    WidgetKind::Status,
                    WidgetKind::Selection,
                    WidgetKind::Score,
                    WidgetKind::HistoryList,
                    WidgetKind::HistoryGraph,
                    WidgetKind::Empty,
                ]
                .get(*index)
                .copied();
                return false;
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
        let output_sizes = self
            .outputs
            .iter()
            .filter_map(|output| output.logical_size.map(|size| (output.name.clone(), size)))
            .collect::<std::collections::BTreeMap<_, _>>();
        match action {
            EditorAction::ToggleCanvas(id) => {
                if let Some(canvas) = self.draft.iter_mut().find(|canvas| &canvas.id == id) {
                    let shown = visible_on(canvas, Some(self.preview));
                    let mut screens = canvas.show_on.clone().unwrap_or(SCREENS.to_vec());
                    screens.retain(|screen| *screen != self.preview);
                    if !shown {
                        screens.push(self.preview);
                    }
                    canvas.show_on = Some(screens);
                }
                self.title = None;
            }
            EditorAction::AddCanvas => {
                let id = (1..=self.draft.len() + 1)
                    .map(|i| format!("canvas-{i}"))
                    .find(|id| self.draft.iter().all(|canvas| &canvas.id != id))
                    .unwrap();
                self.draft.push(CanvasPresentation {
                    id: id.clone(),
                    skin: self.new_canvas_skin,
                    skin_properties: self
                        .skins
                        .iter()
                        .find(|skin| skin.id == self.new_canvas_skin)
                        .map_or_else(std::collections::BTreeMap::new, |skin| {
                            migrate_properties(
                                None,
                                &skin.canvas_properties,
                                &std::collections::BTreeMap::new(),
                            )
                        }),
                    background: Background::None,
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
            }
            EditorAction::DeleteCanvas => {
                self.draft
                    .retain(|canvas| Some(&canvas.id) != self.selected_canvas.as_ref());
                self.select_visible();
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
                        EditorAction::Background(value) => canvas.background = *value,
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
                        EditorAction::DeleteWidget => {
                            canvas
                                .widgets
                                .retain(|widget| Some(&widget.id) != self.selected_widget.as_ref());
                            self.selected_widget = None;
                            self.title = None;
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
        let Some(canvas) = self
            .draft
            .iter_mut()
            .find(|canvas| Some(&canvas.id) == self.selected_canvas.as_ref())
        else {
            return false;
        };
        let (width, height) = default_widget_size(kind);
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
        self.undo = Some(before);
        true
    }
    pub fn surface(&mut self, action: crate::editor_surface::SurfaceAction) -> bool {
        use crate::editor_surface::SurfaceAction;
        match action {
            SurfaceAction::Enter(canvas) => self.enter(canvas),
            SurfaceAction::Select(canvas) => {
                self.action(&EditorAction::SelectCanvas(canvas));
            }
            SurfaceAction::Start {
                canvas,
                widget,
                corner,
                point,
            } => self.begin_drag(canvas, widget, corner, point),
            SurfaceAction::Move(point) => self.move_pointer(point),
            SurfaceAction::End => return self.end_drag(),
            SurfaceAction::Place(point) => return self.place(point),
            SurfaceAction::Cancel => {
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
                self.viewport,
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
        EditorAction::FrameWidth(value) => widget.settings.frame_width = *value,
        EditorAction::FillDelta(delta) => {
            widget.settings.fill_opacity_percent = u8::try_from(
                (i16::from(widget.settings.fill_opacity_percent) + i16::from(*delta)).clamp(0, 100),
            )
            .unwrap_or_default();
        }
        EditorAction::FillOpacity(value) => widget.settings.fill_opacity_percent = *value,
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

    fn skin(id: &str, default: i64) -> EditorSkin {
        EditorSkin {
            id: id.parse().unwrap(),
            name: id.into(),
            release: "1".into(),
            preview: "/preview.png".into(),
            preview_video: None,
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
        model.action(&EditorAction::NewCanvasSkin(Skin::DjBlackbox));

        assert!(model.action(&EditorAction::AddCanvas));
        let canvas = &model.draft[0];
        assert_eq!(canvas.output.as_deref(), Some("DP-1"));
        assert_eq!([canvas.x, canvas.y], [0, 0]);
        assert_eq!([canvas.width, canvas.height], [1716, 1494]);
        assert_eq!(canvas.skin, Skin::DjBlackbox);
        assert_eq!(canvas.show_on, Some(vec![ScreenKind::Unknown]));

        assert!(model.action(&EditorAction::DeleteCanvas));
        assert!(model.draft.is_empty());
    }

    #[test]
    fn output_navigation_clears_selection_without_changing_preview_or_draft() {
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
        assert!(model.selected_canvas.is_none());
    }

    #[test]
    fn active_output_updates_the_drag_bounds_for_mixed_resolutions() {
        let canvas = CanvasPresentation {
            id: "wide-canvas".into(),
            skin: Skin::CyanSystem,
            skin_properties: std::collections::BTreeMap::new(),
            show_on: None,
            background: Background::None,
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
        model.draft[0].output = Some("DP-1".into());
        model.readonly = false;
        model.begin_drag("wide-canvas".into(), None, None, [0, 0]);
        model.move_pointer([10_000, 0]);

        assert_eq!(model.viewport, [5120, 1440]);
        assert_eq!(model.draft[0].x, 4560);
    }

    #[test]
    fn installed_catalog_preserves_values_until_switch_or_save() {
        let first: Skin = "dev.example.first".parse().unwrap();
        let second: Skin = "dev.example.second".parse().unwrap();
        let canvas = CanvasPresentation {
            id: "canvas".into(),
            skin: first,
            skin_properties: std::collections::BTreeMap::from([(
                "amount".into(),
                serde_json::json!(99),
            )]),
            show_on: None,
            background: Background::None,
            opacity_percent: 100,
            output: None,
            x: 0,
            y: 0,
            width: 560,
            height: 1040,
            widgets: Vec::new(),
        };
        let mut model = Model::new(vec![canvas], [1920, 1080], "test");
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
            skin: id,
            skin_properties: std::collections::BTreeMap::from([(
                "amount".into(),
                serde_json::json!(99),
            )]),
            show_on: None,
            background: Background::None,
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
            skin: "dev.example.skin".parse().unwrap(),
            skin_properties: std::collections::BTreeMap::new(),
            show_on: None,
            background: Background::None,
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
