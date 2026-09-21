//! Blitz DOM polling and native event adaptation.

use super::dioxus_dom::{FrameWorkProfile, text};
use crate::host::event_loop::Event;
use blitz_dom::Document as _;
use dioxus_native_dom::DioxusDocument;
use std::task::{Context as TaskContext, Waker};
use std::time::Instant;

pub(crate) fn poll_native_document(document: &mut DioxusDocument, waker: &Waker) -> bool {
    // Browser DOM reconciliation retains the focused control's selection. Blitz currently resets
    // it while applying an otherwise unrelated Dioxus rebuild, so preserve that renderer fact at
    // the native DOM adapter boundary without interpreting the editor field or its value.
    let selection = text::focused_selection(document);
    let changed = document.poll(Some(TaskContext::from_waker(waker)));
    if changed && let Some(selection) = selection {
        text::restore_focused_selection(document, &selection);
    }
    changed
}

pub(crate) fn poll_native_document_for_frame(
    document: &mut DioxusDocument,
    waker: &Waker,
    full_layout_pending: &mut bool,
    work: &mut FrameWorkProfile,
) -> bool {
    let started = Instant::now();
    let mut changed = false;
    while poll_native_document(document, waker) {
        changed = true;
    }
    *full_layout_pending |= changed;
    work.record("dioxus_poll", started.elapsed());
    changed
}

#[derive(Clone, Copy, Default)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct NativeEventOutcome {
    pub(crate) frame: bool,
    pub(crate) configured: bool,
    pub(crate) input_damage: bool,
    pub(crate) closed: bool,
}

pub(crate) trait NativeEventConsumer {
    fn configure_event(
        &mut self,
        logical: [u32; 2],
        physical: [u32; 2],
        scale_120: u32,
    ) -> Result<(), String>;
    fn pointer_motion_event(&mut self, point: [f64; 2]);
    fn pointer_button_event(&mut self, button: u32, pressed: bool, point: [f64; 2]);
    fn pointer_scroll_event(&mut self, delta: [f64; 2], point: [f64; 2]);
    fn text_event(&mut self, command: &scorepeek_overlay_wayland_handles::TextCommand);
    fn ime_event(&mut self, update: scorepeek_overlay_wayland_handles::TextUpdate);
    fn keyboard_focus_event(&mut self, focused: bool);
}

pub(crate) fn dispatch_native_event(
    consumer: &mut impl NativeEventConsumer,
    event: Event,
) -> Result<NativeEventOutcome, String> {
    let mut outcome = NativeEventOutcome::default();
    match event {
        Event::Configure {
            logical,
            physical,
            scale_120,
        } => {
            consumer.configure_event(logical, physical, scale_120)?;
            outcome.configured = true;
        }
        Event::Wake => {}
        Event::PointerMotion { x, y } => {
            consumer.pointer_motion_event([x, y]);
            outcome.input_damage = true;
        }
        Event::PointerButton {
            button,
            pressed,
            x,
            y,
        } => {
            consumer.pointer_button_event(button, pressed, [x, y]);
            outcome.input_damage = true;
        }
        Event::PointerScroll { dx, dy, x, y } => {
            consumer.pointer_scroll_event([dx, dy], [x, y]);
            outcome.input_damage = true;
        }
        Event::Text(command) => consumer.text_event(&command),
        Event::Ime(update) => consumer.ime_event(update),
        Event::KeyboardFocus(focused) => consumer.keyboard_focus_event(focused),
        Event::Frame => outcome.frame = true,
        Event::Closed => outcome.closed = true,
    }
    Ok(outcome)
}
