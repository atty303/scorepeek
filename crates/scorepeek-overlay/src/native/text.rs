use scorepeek_overlay_handles::{TextCommand, TextUpdate};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TitleEdit {
    pub widget: String,
    pub text: String,
    pub cursor: usize,
    pub anchor: usize,
    pub preedit: String,
    pub preedit_cursor: Option<std::ops::Range<usize>>,
}
impl TitleEdit {
    pub fn new(widget: String, text: String) -> Self {
        let cursor = text.len();
        Self {
            widget,
            text,
            cursor,
            anchor: cursor,
            preedit: String::new(),
            preedit_cursor: None,
        }
    }
    pub fn range(&self) -> std::ops::Range<usize> {
        self.cursor.min(self.anchor)..self.cursor.max(self.anchor)
    }
    fn replace(&mut self, text: &str) {
        let range = self.range();
        let cursor = range.start + text.len();
        self.text.replace_range(range, text);
        self.cursor = cursor;
        self.anchor = cursor;
    }
    pub fn command(&mut self, command: &TextCommand) {
        // Composing text belongs to the IME until its done/commit batch arrives.
        if !self.preedit.is_empty() {
            return;
        }
        let previous = self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i);
        let next = self.text[self.cursor..]
            .chars()
            .next()
            .map_or(self.cursor, |c| self.cursor + c.len_utf8());
        match command {
            TextCommand::Insert(text) => self.replace(&single_line(text)),
            TextCommand::Backspace => {
                if self.cursor == self.anchor {
                    self.anchor = previous;
                }
                self.replace("");
            }
            TextCommand::Delete => {
                if self.cursor == self.anchor {
                    self.anchor = next;
                }
                self.replace("");
            }
            TextCommand::SelectAll => {
                self.anchor = 0;
                self.cursor = self.text.len();
            }
            TextCommand::Left { select } => self.move_to(
                if !select && self.cursor != self.anchor {
                    self.range().start
                } else {
                    previous
                },
                *select,
            ),
            TextCommand::Right { select } => self.move_to(
                if !select && self.cursor != self.anchor {
                    self.range().end
                } else {
                    next
                },
                *select,
            ),
            TextCommand::Home { select } => self.move_to(0, *select),
            TextCommand::End { select } => self.move_to(self.text.len(), *select),
            TextCommand::Accept | TextCommand::Cancel => {}
        }
    }
    fn move_to(&mut self, position: usize, select: bool) {
        self.cursor = position;
        if !select {
            self.anchor = position;
        }
    }
    pub fn ime(&mut self, update: TextUpdate) {
        self.preedit.clear();
        let selected = self.range();
        let start = selected.start.saturating_sub(update.delete_before as usize);
        let end = selected
            .end
            .saturating_add(update.delete_after as usize)
            .min(self.text.len());
        if self.text.is_char_boundary(start) && self.text.is_char_boundary(end) {
            self.text.replace_range(selected.end..end, "");
            self.text.replace_range(start..selected.start, "");
            let removed = selected.start - start;
            self.cursor -= removed;
            self.anchor -= removed;
        }
        if let Some(text) = update.commit {
            self.replace(&single_line(&text));
        }
        if !update.preedit.is_empty() {
            self.replace("");
        }
        self.preedit = single_line(&update.preedit);
        self.preedit_cursor = usize::try_from(update.preedit_cursor[0])
            .ok()
            .zip(usize::try_from(update.preedit_cursor[1]).ok())
            .and_then(|(start, end)| {
                (start <= end
                    && self.preedit.is_char_boundary(start)
                    && self.preedit.is_char_boundary(end))
                .then_some(start..end)
            });
    }
}
fn single_line(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

impl super::App {
    pub(super) fn input_command(&mut self, command: &TextCommand) {
        if self.refresh_edit.borrow().is_some() {
            match command {
                TextCommand::Cancel => {
                    self.finish_refresh_edit(false);
                }
                TextCommand::Accept => {
                    self.finish_refresh_edit(true);
                }
                _ => {
                    let changed = if let Some(edit) = self.refresh_edit.borrow_mut().as_mut() {
                        let before = edit.clone();
                        edit.command(command);
                        *edit != before
                    } else {
                        false
                    };
                    if changed {
                        self.update_refresh_input(false);
                    }
                }
            }
        } else {
            self.title_command(command);
        }
    }

    pub(super) fn title_command(&mut self, command: &TextCommand) {
        match command {
            TextCommand::Cancel => {
                self.finish_title_edit(false);
                self.surface_action(scorepeek_overlay_ui::editor_surface::SurfaceAction::Cancel);
            }
            TextCommand::Accept => {
                if self
                    .title_edit
                    .borrow()
                    .as_ref()
                    .is_some_and(|edit| edit.preedit.is_empty())
                {
                    self.finish_title_edit(true);
                }
            }
            _ => {
                let changed = if let Some(edit) = self.title_edit.borrow_mut().as_mut() {
                    let before = edit.clone();
                    edit.command(command);
                    *edit != before
                } else {
                    false
                };
                if changed {
                    self.update_title_input(false);
                }
            }
        }
    }
    pub(super) fn finish_title_edit(&mut self, accept: bool) {
        if accept
            && self
                .title_edit
                .borrow()
                .as_ref()
                .is_some_and(|edit| !edit.preedit.is_empty())
        {
            return;
        }
        let Some(edit) = self.title_edit.take() else {
            return;
        };
        self.shell.set_text_input(None);
        if accept {
            let before = self.undo_snapshot();
            let mut model = self.editor_model();
            model.title = Some(scorepeek_overlay_ui::editor_model::TitleDraft {
                canvas: self.canvas.id.clone(),
                widget: edit.widget,
                text: edit.text,
                composing: false,
            });
            model.action(&scorepeek_overlay_ui::editor::EditorAction::AcceptTitle);
            self.apply_editor_model(model);
            self.finish_draft_change(before);
        }
        crate::diagnostics::emit(
            "title_edit_finished",
            &serde_json::json!({"canvas_id":self.canvas.id,"status":if accept{"success"}else{"cancel"}}),
        );
    }
    pub(super) fn update_title_input(&mut self, from_ime: bool) {
        let Some(edit) = self.title_edit.borrow().clone() else {
            return;
        };
        let rectangle = {
            let doc = self.document.inner.borrow();
            doc.query_selector(".empty-title-edit")
                .ok()
                .flatten()
                .or_else(|| doc.query_selector(".empty-title-input").ok().flatten())
                .and_then(|node| doc.get_client_bounding_rect(node))
                .map_or([0, 0, 1, 1], |rect| {
                    [
                        super::snap_i32(rect.x),
                        super::snap_i32(rect.y),
                        super::snap_i32(rect.width).max(1),
                        super::snap_i32(rect.height).max(1),
                    ]
                })
        };
        self.shell
            .set_text_input(Some(scorepeek_overlay_handles::TextInputState {
                from_ime,
                text: edit.text,
                cursor: i32::try_from(edit.cursor).unwrap_or(i32::MAX),
                anchor: i32::try_from(edit.anchor).unwrap_or(i32::MAX),
                rectangle,
            }));
    }

    pub(super) fn finish_refresh_edit(&mut self, accept: bool) -> bool {
        let refresh = if accept {
            let edit = self.refresh_edit.borrow();
            let Some(edit) = edit.as_ref() else {
                return true;
            };
            match super::parse_refresh_rate(&edit.text) {
                Ok(refresh) => Some(refresh),
                Err(_) => return false,
            }
        } else {
            None
        };
        if self.refresh_edit.take().is_none() {
            return true;
        }
        self.shell.set_text_input(None);
        if let Some(refresh) = refresh {
            let before = self.undo_snapshot();
            self.set_refresh_rate_draft(refresh);
            self.finish_draft_change(before);
        }
        true
    }

    pub(super) fn update_refresh_input(&mut self, from_ime: bool) {
        let Some(edit) = self.refresh_edit.borrow().clone() else {
            return;
        };
        let rectangle = {
            let doc = self.document.inner.borrow();
            doc.query_selector(".refresh-rate-edit")
                .ok()
                .flatten()
                .or_else(|| doc.query_selector(".refresh-rate-input").ok().flatten())
                .and_then(|node| doc.get_client_bounding_rect(node))
                .map_or([0, 0, 1, 1], |rect| {
                    [
                        super::snap_i32(rect.x),
                        super::snap_i32(rect.y),
                        super::snap_i32(rect.width).max(1),
                        super::snap_i32(rect.height).max(1),
                    ]
                })
        };
        self.shell
            .set_text_input(Some(scorepeek_overlay_handles::TextInputState {
                from_ime,
                text: edit.text,
                cursor: i32::try_from(edit.cursor).unwrap_or(i32::MAX),
                anchor: i32::try_from(edit.anchor).unwrap_or(i32::MAX),
                rectangle,
            }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_title_selection_and_ime_replacement() {
        let mut edit = TitleEdit::new("camera".into(), "手元 CAMERA".into());
        edit.command(&TextCommand::SelectAll);
        edit.ime(TextUpdate {
            preedit: "上面".into(),
            ..TextUpdate::default()
        });
        edit.command(&TextCommand::Backspace);
        assert_eq!(edit.text, "");
        edit.ime(TextUpdate {
            commit: Some("上面 DP".into()),
            ..TextUpdate::default()
        });
        assert_eq!(edit.text, "上面 DP");
        edit.command(&TextCommand::Home { select: false });
        edit.command(&TextCommand::Delete);
        assert_eq!(edit.text, "面 DP");
        edit.command(&TextCommand::Right { select: true });
        edit.command(&TextCommand::Insert("手元".into()));
        assert_eq!(edit.text, "手元 DP");
    }
    #[test]
    fn ime_surrounding_deletion_uses_utf8_bytes() {
        let mut edit = TitleEdit::new("camera".into(), "SP 手元".into());
        edit.ime(TextUpdate {
            delete_before: 6,
            commit: Some("上面".into()),
            ..TextUpdate::default()
        });
        assert_eq!(edit.text, "SP 上面");
        edit.ime(TextUpdate {
            delete_before: 1,
            ..TextUpdate::default()
        });
        assert_eq!(edit.text, "SP 上面");
    }
    #[test]
    fn ime_deletes_around_forward_and_reverse_utf8_selections() {
        for (text, start, end, before, after, expected) in [
            ("abcDEFghi", 3, 6, 1, 1, "abXhi"),
            ("前日本後", 3, 9, 3, 3, "X"),
        ] {
            for (anchor, cursor) in [(start, end), (end, start)] {
                let mut edit = TitleEdit::new("camera".into(), text.into());
                edit.anchor = anchor;
                edit.cursor = cursor;
                edit.ime(TextUpdate {
                    delete_before: before,
                    delete_after: after,
                    commit: Some("X".into()),
                    ..TextUpdate::default()
                });
                assert_eq!(edit.text, expected);
                assert_eq!(edit.cursor, edit.anchor);
            }
        }
    }
}
