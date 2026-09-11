use blitz_dom::Document as _;
use scorepeek_overlay_handles::{TextCommand, TextUpdate};

impl super::App {
    pub(super) fn input_command(&mut self, command: &TextCommand) {
        dispatch_control_key(
            &mut self.document,
            command,
            matches!(self.text_composition, super::TextComposition::Active(_)),
        );
    }

    pub(super) fn input_composition(&mut self, update: TextUpdate) {
        if let Some(composing) = dispatch_control_composition(&mut self.document, update) {
            self.set_text_composing(composing);
        }
    }

    pub(super) fn set_text_composing(&mut self, composing: bool) {
        let field_key = if composing {
            focused_field_key(&self.document)
        } else {
            match &self.text_composition {
                super::TextComposition::Active(field_key) => Some(field_key.clone()),
                super::TextComposition::Idle => None,
            }
        };
        let Some(field_key) = field_key else {
            self.text_composition = super::TextComposition::Idle;
            return;
        };
        let composition = if composing {
            super::TextComposition::Active(field_key.clone())
        } else {
            super::TextComposition::Idle
        };
        if composition == self.text_composition {
            return;
        }
        self.text_composition = composition;
        let _ = self
            .coordinator
            .send(super::CoordinatorCommand::EditorInput {
                input: scorepeek_overlay_ui::editor_model::EditorInput::Action(
                    scorepeek_overlay_ui::editor::EditorAction::TextComposition {
                        field_key,
                        composing,
                    },
                ),
                correlation: None,
            });
    }
}

pub(super) fn dispatch_control_key(
    document: &mut dioxus_native_dom::DioxusDocument,
    command: &TextCommand,
    composing: bool,
) {
    use blitz_traits::events::{BlitzKeyEvent, KeyState, UiEvent};
    use dioxus::html::{Code, Key, Location, Modifiers};
    let (key, modifiers) = match command {
        TextCommand::Insert(text) => (Key::Character(text.clone()), Modifiers::empty()),
        TextCommand::Backspace => (Key::Backspace, Modifiers::empty()),
        TextCommand::Delete => (Key::Delete, Modifiers::empty()),
        TextCommand::Left { select } => (Key::ArrowLeft, shift(*select)),
        TextCommand::Right { select } => (Key::ArrowRight, shift(*select)),
        TextCommand::Up => (Key::ArrowUp, Modifiers::empty()),
        TextCommand::Down => (Key::ArrowDown, Modifiers::empty()),
        TextCommand::Tab { reverse } => (Key::Tab, shift(*reverse)),
        TextCommand::Home { select } => (Key::Home, shift(*select)),
        TextCommand::End { select } => (Key::End, shift(*select)),
        TextCommand::SelectAll => (Key::Character("a".into()), Modifiers::CONTROL),
        TextCommand::Accept => (Key::Enter, Modifiers::empty()),
        TextCommand::Cancel => (Key::Escape, Modifiers::empty()),
    };
    let text = match command {
        TextCommand::Insert(text) => Some(blitz_traits::SmolStr::new(text)),
        _ => None,
    };
    let event = |state| BlitzKeyEvent {
        key: key.clone(),
        code: Code::Unidentified,
        modifiers,
        location: Location::Standard,
        is_auto_repeating: false,
        is_composing: composing,
        state,
        text: text.clone(),
    };
    document.handle_ui_event(UiEvent::KeyDown(event(KeyState::Pressed)));
    document.handle_ui_event(UiEvent::KeyUp(event(KeyState::Released)));
}

pub(super) fn dispatch_control_composition(
    document: &mut dioxus_native_dom::DioxusDocument,
    update: TextUpdate,
) -> Option<bool> {
    use blitz_traits::events::{BlitzImeEvent, UiEvent};
    let changed = update.preedit.is_some()
        || update.commit.is_some()
        || update.delete_before != 0
        || update.delete_after != 0;
    if !changed {
        return None;
    }

    // text-input-v3 applies one `done` batch by replacing the old preedit first, then deleting
    // surrounding committed text, committing new text, and finally installing the new preedit.
    // Blitz does not yet implement DeleteSurrounding, so normalize that renderer gap through the
    // focused standard DOM input and ordinary key/input events. No editor field is identified here.
    document.handle_ui_event(UiEvent::Ime(BlitzImeEvent::Preedit(String::new(), None)));
    if update.delete_before != 0 || update.delete_after != 0 {
        replace_surrounding(document, update.delete_before, update.delete_after);
    }
    if let Some(commit) = update.commit {
        document.handle_ui_event(UiEvent::Ime(BlitzImeEvent::Commit(commit)));
    }
    let composing = update
        .preedit
        .as_ref()
        .is_some_and(|preedit| !preedit.is_empty());
    if let Some(preedit) = update.preedit {
        let cursor = usize::try_from(update.preedit_cursor[0])
            .ok()
            .zip(usize::try_from(update.preedit_cursor[1]).ok());
        document.handle_ui_event(UiEvent::Ime(BlitzImeEvent::Preedit(preedit, cursor)));
    }
    Some(composing)
}

fn replace_surrounding(document: &mut dioxus_native_dom::DioxusDocument, before: u32, after: u32) {
    let Some((text, selection)) = focused_text(document) else {
        return;
    };
    let before = usize::try_from(before).unwrap_or(usize::MAX);
    let after = usize::try_from(after).unwrap_or(usize::MAX);
    let Some(start) = selection.start.checked_sub(before) else {
        return;
    };
    let Some(end) = selection.end.checked_add(after) else {
        return;
    };
    if end > text.len() || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
        return;
    }
    let mut replacement = String::with_capacity(text.len() - before - after);
    replacement.push_str(&text[..start]);
    replacement.push_str(&text[selection.clone()]);
    replacement.push_str(&text[end..]);
    let selected_start = start;
    let selected_end = selected_start + selection.len();

    dispatch_control_key(document, &TextCommand::SelectAll, false);
    if replacement.is_empty() {
        dispatch_control_key(document, &TextCommand::Backspace, false);
    } else {
        dispatch_control_key(document, &TextCommand::Insert(replacement), false);
    }
    dispatch_control_key(document, &TextCommand::Home { select: false }, false);
    move_focus_to(document, selected_start, false);
    move_focus_to(document, selected_end, true);
}

fn move_focus_to(document: &mut dioxus_native_dom::DioxusDocument, target: usize, select: bool) {
    loop {
        let Some((_, selection)) = focused_text(document) else {
            return;
        };
        let current = if select {
            selection.end
        } else {
            selection.start
        };
        if current >= target {
            return;
        }
        dispatch_control_key(document, &TextCommand::Right { select }, false);
        let Some((_, next)) = focused_text(document) else {
            return;
        };
        let next = if select { next.end } else { next.start };
        if next <= current {
            return;
        }
    }
}

fn focused_text(
    document: &dioxus_native_dom::DioxusDocument,
) -> Option<(String, std::ops::Range<usize>)> {
    let document = document.inner.borrow();
    let node = document.get_focussed_node_id()?;
    let input = document.get_node(node)?.element_data()?.text_input_data()?;
    Some((
        input.editor.raw_text().to_owned(),
        input.editor.raw_selection().text_range(),
    ))
}

pub(super) fn focused_field_key(document: &dioxus_native_dom::DioxusDocument) -> Option<String> {
    let document = document.inner.borrow();
    let node = document.get_focussed_node_id()?;
    document
        .get_node(node)?
        .element_data()?
        .id
        .as_ref()
        .map(ToString::to_string)
}

fn shift(enabled: bool) -> dioxus::html::Modifiers {
    if enabled {
        dioxus::html::Modifiers::SHIFT
    } else {
        dioxus::html::Modifiers::empty()
    }
}
