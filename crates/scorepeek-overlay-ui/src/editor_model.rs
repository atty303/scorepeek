use crate::editor::{EditorAccess, EditorAction, EditorChrome, EditorTitleState, EditorView};
use crate::{
    AspectRatio, Background, CanvasPresentation, ScreenKind, Skin, WidgetKind, WidgetLayout,
    WidgetSettings, default_widget_size, next_widget_id,
};

pub const SCREENS: [ScreenKind; 5] = [
    ScreenKind::MusicSelect,
    ScreenKind::ModeSelect,
    ScreenKind::DecideTransition,
    ScreenKind::Play,
    ScreenKind::Result,
];
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
    namespace: &'static str,
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
    pub generation: u64,
    pub backend_revision: u64,
    pub discard_pending: bool,
    pub notice: Option<String>,
}
impl Model {
    #[must_use]
    pub fn new(
        canvases: Vec<CanvasPresentation>,
        viewport: [u32; 2],
        namespace: &'static str,
    ) -> Self {
        Self {
            namespace,
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
            generation: 0,
            backend_revision: 0,
            discard_pending: false,
            notice: None,
        }
    }
    pub fn receive_stage(&mut self, canvases: Vec<CanvasPresentation>) {
        if !self.editing && self.draft != canvases {
            self.saved.clone_from(&canvases);
            self.draft = canvases;
            self.generation += 1;
        }
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
            backend_label: format!("{} EDITOR", self.namespace.to_ascii_uppercase()),
            canvases: self.draft.clone(),
            selected_canvas: self.selected_canvas.clone(),
            selected_widget: self.selected_widget.clone(),
            preview_screen: self.preview,
            outputs: None,
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
        }
    }
    pub fn select_visible(&mut self) {
        self.selected_canvas = self
            .draft
            .iter()
            .find(|canvas| visible_on(canvas, Some(self.preview)))
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
                self.select_visible();
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
    fn apply_settings(&mut self, action: &EditorAction) {
        match action {
            EditorAction::ToggleCanvas(id) => {
                if let Some(canvas) = self.draft.iter_mut().find(|canvas| &canvas.id == id) {
                    let shown = visible_on(canvas, Some(self.preview));
                    let mut screens = canvas.show_on.clone().unwrap_or(SCREENS.to_vec());
                    screens.retain(|screen| *screen != self.preview);
                    if !shown {
                        screens.push(self.preview);
                    }
                    canvas.show_on = if screens.len() == SCREENS.len() {
                        None
                    } else {
                        Some(screens)
                    };
                }
                self.title = None;
            }
            EditorAction::AddCanvas => {
                let id = (1..=self.draft.len() + 1)
                    .map(|i| format!("{}-canvas-{i}", self.namespace))
                    .find(|id| self.draft.iter().all(|canvas| &canvas.id != id))
                    .unwrap();
                self.draft.push(CanvasPresentation {
                    id: id.clone(),
                    skin: Skin::CyanSystem,
                    background: Background::None,
                    revision: 0,
                    show_on: Some(vec![self.preview]),
                    opacity_percent: 100,
                    output: None,
                    x: 0,
                    y: 0,
                    width: grid(self.viewport[0].min(560)),
                    height: grid(self.viewport[1].min(1040)),
                    widgets: vec![],
                });
                self.selected_canvas = Some(id);
                self.selected_widget = None;
            }
            EditorAction::DeleteCanvas => {
                if self.draft.len() > 1 {
                    self.draft
                        .retain(|canvas| Some(&canvas.id) != self.selected_canvas.as_ref());
                    self.select_visible();
                }
            }
            _ => {
                if let Some(canvas) = self
                    .draft
                    .iter_mut()
                    .find(|canvas| Some(&canvas.id) == self.selected_canvas.as_ref())
                {
                    match action {
                        EditorAction::Skin(value) => canvas.skin = *value,
                        EditorAction::Background(value) => canvas.background = *value,
                        EditorAction::Opacity(value) => canvas.opacity_percent = *value,
                        EditorAction::Output(value) => canvas.output = Some(value.clone()),
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
                                apply_widget_action(widget, &canvas.id, &mut self.title, action);
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
        .is_none_or(|screens| screen.is_some_and(|screen| screens.contains(&screen)))
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
