use super::{Event, Platform, Shell};
use smithay_client_toolkit::shell::WaylandSurface as _;
use smithay_client_toolkit::{
    reexports::calloop::LoopHandle,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
    },
};
use wayland_client::{
    Connection, QueueHandle,
    globals::GlobalList,
    protocol::{wl_keyboard::WlKeyboard, wl_seat::WlSeat, wl_surface::WlSurface},
};
use wayland_protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3::ZwpTextInputManagerV3,
    zwp_text_input_v3::{self, ZwpTextInputV3},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextCommand {
    Insert(String),
    Backspace,
    Delete,
    Left { select: bool },
    Right { select: bool },
    Home { select: bool },
    End { select: bool },
    SelectAll,
    Accept,
    Cancel,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextUpdate {
    pub commit: Option<String>,
    pub preedit: String,
    pub preedit_cursor: [i32; 2],
    pub delete_before: u32,
    pub delete_after: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextInputState {
    pub from_ime: bool,
    pub text: String,
    /// UTF-8 byte offsets into text.
    pub cursor: i32,
    pub anchor: i32,
    /// Surface-local logical rectangle for the candidate popup.
    pub rectangle: [i32; 4],
}

#[derive(Default)]
struct ImeSerial {
    commits: u32,
    deferred: bool,
}
impl ImeSerial {
    fn commit(&mut self) {
        self.commits = self.commits.wrapping_add(1);
    }
    fn acknowledge(&mut self, serial: u32) {
        self.deferred = serial != self.commits;
    }
}

pub(super) struct Input {
    seats: SeatState,
    seat: Option<WlSeat>,
    keyboard: Option<WlKeyboard>,
    loop_handle: LoopHandle<'static, Platform>,
    modifiers: Modifiers,
    manager: Option<ZwpTextInputManagerV3>,
    protocol: Option<ZwpTextInputV3>,
    entered: bool,
    focused: bool,
    editing: Option<TextInputState>,
    serial: ImeSerial,
    surrounding_supported: bool,
    pending: TextUpdate,
}
impl Input {
    pub(super) fn new(
        globals: &GlobalList,
        qh: &QueueHandle<Platform>,
        loop_handle: LoopHandle<'static, Platform>,
    ) -> Self {
        Self {
            seats: SeatState::new(globals, qh),
            seat: None,
            keyboard: None,
            loop_handle,
            modifiers: Modifiers::default(),
            manager: globals.bind(qh, 1..=1, ()).ok(),
            protocol: None,
            entered: false,
            focused: false,
            editing: None,
            serial: ImeSerial::default(),
            surrounding_supported: false,
            pending: TextUpdate::default(),
        }
    }
    fn commit(&mut self) {
        if let Some(input) = &self.protocol {
            input.commit();
            self.serial.commit();
        }
    }
    fn send_state(&mut self, enable: bool) {
        if !self.entered || self.serial.deferred {
            return;
        }
        let (Some(input), Some(state)) = (&self.protocol, &self.editing) else {
            return;
        };
        let surrounding = surrounding_text(state);
        let supports = surrounding.is_some();
        if enable || self.surrounding_supported != supports {
            input.enable();
        }
        self.surrounding_supported = supports;
        input.set_content_type(
            zwp_text_input_v3::ContentHint::None,
            zwp_text_input_v3::ContentPurpose::Normal,
        );
        if let Some((text, cursor, anchor)) = surrounding {
            input.set_surrounding_text(text, cursor, anchor);
        }
        let [x, y, w, h] = state.rectangle;
        input.set_cursor_rectangle(x, y, w, h);
        input.set_text_change_cause(if state.from_ime {
            zwp_text_input_v3::ChangeCause::InputMethod
        } else {
            zwp_text_input_v3::ChangeCause::Other
        });
        self.commit();
    }
    fn disable(&mut self) {
        if self.entered
            && let Some(input) = &self.protocol
        {
            input.disable();
            self.commit();
        }
        self.pending = TextUpdate::default();
        self.serial.deferred = false;
    }
}
fn surrounding_text(state: &TextInputState) -> Option<(String, i32, i32)> {
    let cursor = usize::try_from(state.cursor).ok()?;
    let anchor = usize::try_from(state.anchor).ok()?;
    if !state.text.is_char_boundary(cursor) || !state.text.is_char_boundary(anchor) {
        return None;
    }
    let selection = cursor.min(anchor)..cursor.max(anchor);
    let padding = 4000usize.checked_sub(selection.len())? / 2;
    let mut start = selection.start.saturating_sub(padding);
    let mut end = (selection.end + padding).min(state.text.len());
    while !state.text.is_char_boundary(start) {
        start += 1;
    }
    while !state.text.is_char_boundary(end) {
        end -= 1;
    }
    Some((
        state.text[start..end].to_owned(),
        i32::try_from(cursor - start).ok()?,
        i32::try_from(anchor - start).ok()?,
    ))
}

impl Shell {
    /// Captures keys only for an explicit title edit; None restores normal overlay focus policy.
    pub fn set_text_input(&mut self, state: Option<TextInputState>) {
        let was_editing = self.state.input.editing.is_some();
        let editing = state.is_some();
        if !editing {
            self.state.input.focused = false;
            self.state.input.disable();
        }
        self.state.input.editing = state;
        if editing {
            self.state.input.send_state(!was_editing);
        }
        if was_editing != editing {
            self.owner.layer.set_keyboard_interactivity(if editing {
                smithay_client_toolkit::shell::wlr_layer::KeyboardInteractivity::Exclusive
            } else {
                smithay_client_toolkit::shell::wlr_layer::KeyboardInteractivity::None
            });
            self.owner.layer.commit();
        }
    }
}
impl SeatHandler for Platform {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.input.seats
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        if self
            .input
            .seat
            .as_ref()
            .is_some_and(|selected| selected != &seat)
        {
            return;
        }
        self.input.seat = Some(seat.clone());
        match capability {
            Capability::Keyboard if self.input.keyboard.is_none() => {
                match self.input.seats.get_keyboard_with_repeat(
                    qh,
                    &seat,
                    None,
                    self.input.loop_handle.clone(),
                    Box::new(|state, _, event| state.text_key(event)),
                ) {
                    Ok(keyboard) => self.input.keyboard = Some(keyboard),
                    Err(error) => self.failure = Some(format!("keyboard_initialization: {error}")),
                }
                if let Some(manager) = &self.input.manager {
                    self.input.protocol = Some(manager.get_text_input(&seat, qh, ()));
                    self.input.serial = ImeSerial::default();
                }
            }
            Capability::Pointer if self.pointer.is_none() => {
                let pointer = seat.get_pointer(qh, ());
                self.cursor_device = self
                    .cursor_manager
                    .as_ref()
                    .map(|manager| manager.get_pointer(&pointer, qh, ()));
                self.pointer = Some(pointer);
            }
            _ => {}
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        if self.input.seat.as_ref() != Some(&seat) {
            return;
        }
        match capability {
            Capability::Keyboard => {
                self.input.disable();
                if let Some(input) = self.input.protocol.take() {
                    input.destroy();
                }
                if let Some(keyboard) = self.input.keyboard.take() {
                    keyboard.release();
                }
                self.input.entered = false;
                self.input.focused = false;
                self.events.push(Event::KeyboardFocus(false));
            }
            Capability::Pointer => {
                if let Some(device) = self.cursor_device.take() {
                    device.destroy();
                }
                if let Some(pointer) = self.pointer.take() {
                    pointer.release();
                }
                self.pointer_enter_serial = None;
            }
            _ => {}
        }
    }
    fn remove_seat(&mut self, conn: &Connection, qh: &QueueHandle<Self>, seat: WlSeat) {
        self.remove_capability(conn, qh, seat.clone(), Capability::Keyboard);
        self.remove_capability(conn, qh, seat.clone(), Capability::Pointer);
        if self.input.seat.as_ref() == Some(&seat) {
            self.input.seat = None;
        }
    }
}
impl Platform {
    fn text_key(&mut self, event: KeyEvent) {
        if !self.input.focused || self.input.editing.is_none() {
            return;
        }
        let m = self.input.modifiers;
        if let Some(command) = key_command(event.keysym, event.utf8, m) {
            self.events.push(Event::Text(command));
        }
    }
}
fn key_command(key: Keysym, utf8: Option<String>, m: Modifiers) -> Option<TextCommand> {
    if m.alt || m.logo {
        return None;
    }
    Some(match key {
        Keysym::Escape => TextCommand::Cancel,
        Keysym::Return | Keysym::KP_Enter => TextCommand::Accept,
        Keysym::BackSpace => TextCommand::Backspace,
        Keysym::Delete => TextCommand::Delete,
        Keysym::Left => TextCommand::Left { select: m.shift },
        Keysym::Right => TextCommand::Right { select: m.shift },
        Keysym::Home => TextCommand::Home { select: m.shift },
        Keysym::End => TextCommand::End { select: m.shift },
        Keysym::a | Keysym::A if m.ctrl => TextCommand::SelectAll,
        _ if !m.ctrl => TextCommand::Insert(
            utf8.filter(|text| !text.is_empty() && !text.chars().any(char::is_control))?,
        ),
        _ => return None,
    })
}
impl KeyboardHandler for Platform {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        surface: &WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        use smithay_client_toolkit::shell::WaylandSurface as _;
        self.input.focused = self
            .owner
            .as_ref()
            .is_some_and(|owner| owner.layer.wl_surface() == surface);
        self.events.push(Event::KeyboardFocus(self.input.focused));
    }
    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: &WlSurface,
        _: u32,
    ) {
        let was_focused = std::mem::replace(&mut self.input.focused, false);
        self.input.modifiers = Modifiers::default();
        if was_focused {
            self.events.push(Event::KeyboardFocus(false));
        }
    }
    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.text_key(event);
    }
    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.text_key(event);
    }
    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }
    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        self.input.modifiers = modifiers;
    }
}
wayland_client::delegate_noop!(Platform: ignore ZwpTextInputManagerV3);
impl wayland_client::Dispatch<ZwpTextInputV3, ()> for Platform {
    fn event(
        state: &mut Self,
        input: &ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use smithay_client_toolkit::shell::WaylandSurface as _;
        if state.input.protocol.as_ref() != Some(input) {
            return;
        }
        match event {
            zwp_text_input_v3::Event::Enter { surface } => {
                state.input.entered = state
                    .owner
                    .as_ref()
                    .is_some_and(|owner| owner.layer.wl_surface() == &surface);
                state.input.send_state(true);
            }
            zwp_text_input_v3::Event::Leave { .. } => {
                state.input.entered = false;
                state.input.pending = TextUpdate::default();
            }
            zwp_text_input_v3::Event::PreeditString {
                text,
                cursor_begin,
                cursor_end,
            } => {
                state.input.pending.preedit = text.unwrap_or_default();
                state.input.pending.preedit_cursor = [cursor_begin, cursor_end];
            }
            zwp_text_input_v3::Event::CommitString { text } => state.input.pending.commit = text,
            zwp_text_input_v3::Event::DeleteSurroundingText {
                before_length,
                after_length,
            } => {
                state.input.pending.delete_before = before_length;
                state.input.pending.delete_after = after_length;
            }
            zwp_text_input_v3::Event::Done { serial } => {
                let pending = std::mem::take(&mut state.input.pending);
                state.input.serial.acknowledge(serial);
                if state.input.entered && state.input.editing.is_some() {
                    state.events.push(Event::Ime(pending));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ime_responses_defer_protocol_state_until_the_current_commit_is_acknowledged() {
        let mut serial = ImeSerial::default();
        serial.commit();
        serial.commit();
        serial.acknowledge(1);
        assert!(serial.deferred);
        serial.acknowledge(2);
        assert!(!serial.deferred);
        serial.commit();
        assert!(!serial.deferred);
    }
    #[test]
    fn shortcuts_and_translated_text_stay_distinct() {
        assert_eq!(
            key_command(
                Keysym::a,
                Some("a".into()),
                Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                }
            ),
            Some(TextCommand::SelectAll)
        );
        assert_eq!(
            key_command(
                Keysym::a,
                Some("A".into()),
                Modifiers {
                    shift: true,
                    ..Modifiers::default()
                }
            ),
            Some(TextCommand::Insert("A".into()))
        );
        assert_eq!(
            key_command(
                Keysym::a,
                Some("a".into()),
                Modifiers {
                    logo: true,
                    ..Modifiers::default()
                }
            ),
            None
        );
        assert_eq!(
            key_command(Keysym::Return, Some("\r".into()), Modifiers::default()),
            Some(TextCommand::Accept)
        );
    }
    #[test]
    fn surrounding_window_keeps_complete_multibyte_selection() {
        let text = "日本語".repeat(1000);
        let state = TextInputState {
            from_ime: false,
            text,
            cursor: 4500,
            anchor: 4512,
            rectangle: [0, 0, 1, 1],
        };
        let (text, cursor, anchor) = surrounding_text(&state).unwrap();
        assert!(text.len() <= 4000);
        assert_eq!(
            &text[usize::try_from(cursor).unwrap()..usize::try_from(anchor).unwrap()],
            &state.text[4500..4512]
        );
        assert!(surrounding_text(&TextInputState { anchor: 0, ..state }).is_none());
    }
}
