//! Native pointer events adapted to the Blitz/Dioxus document.

use blitz_dom::Document as _;
use dioxus_native_dom::DioxusDocument;
use std::sync::Arc;

use super::focus::focus_clicked_button;

#[derive(Default)]
pub(crate) struct PointerInput {
    buttons: blitz_traits::events::MouseEventButtons,
}

impl PointerInput {
    #[cfg(test)]
    pub(crate) const fn buttons(&self) -> blitz_traits::events::MouseEventButtons {
        self.buttons
    }

    pub(crate) fn dispatch_blitz(
        &mut self,
        document: &mut DioxusDocument,
        point: [f64; 2],
        button: u32,
        pressed: Option<bool>,
    ) {
        use blitz_traits::events::{
            BlitzPointerEvent, BlitzPointerId, MouseEventButton, Point, PointerCoords,
            PointerDetails, UiEvent,
        };
        let point = dioxus::html::geometry::ClientPoint::new(point[0], point[1]).to_f32();
        let (x, y) = (point.x, point.y);
        let button = if button == 0x111 {
            MouseEventButton::Secondary
        } else {
            MouseEventButton::Main
        };
        if let Some(pressed) = pressed {
            self.buttons.set(button.into(), pressed);
        }
        let event = BlitzPointerEvent {
            id: BlitzPointerId::Mouse,
            is_primary: true,
            coords: PointerCoords {
                page_x: x,
                page_y: y,
                screen_x: x,
                screen_y: y,
                client_x: x,
                client_y: y,
            },
            button,
            buttons: self.buttons,
            mods: dioxus::html::Modifiers::default(),
            details: PointerDetails::default(),
            element: Point::default(),
            active_pointers: Arc::default(),
        };
        document.handle_ui_event(match pressed {
            Some(true) => UiEvent::PointerDown(event),
            Some(false) => UiEvent::PointerUp(event),
            None => UiEvent::PointerMove(event),
        });
    }

    pub(crate) fn dispatch(
        &mut self,
        document: &mut DioxusDocument,
        point: [f64; 2],
        button: u32,
        pressed: Option<bool>,
    ) {
        self.dispatch_blitz(document, point, button, pressed);
        if pressed == Some(false) && button == 0x110 {
            let point = dioxus::html::geometry::ClientPoint::new(point[0], point[1]).to_f32();
            focus_clicked_button(document, [point.x, point.y]);
        }
    }
    pub(crate) fn click(&mut self, document: &mut DioxusDocument, point: [f64; 2]) {
        self.dispatch(document, point, 0x110, None);
        self.dispatch(document, point, 0x110, Some(true));
        self.dispatch(document, point, 0x110, Some(false));
    }

    pub(crate) fn wheel(
        &mut self,
        document: &mut DioxusDocument,
        point: [f64; 2],
        delta: [f64; 2],
    ) {
        use blitz_traits::events::{
            BlitzWheelDelta, BlitzWheelEvent, Point, PointerCoords, UiEvent,
        };
        // Browser wheel targeting is resolved from the wheel event's coordinates. Blitz currently
        // routes the default scroll action through its previously stored hover chain, so reproduce
        // the browser contract at this adapter boundary without synthesizing a Dioxus pointermove.
        let point = dioxus::html::geometry::ClientPoint::new(point[0], point[1]).to_f32();
        document.inner.borrow_mut().set_hover_to(point.x, point.y);
        document.handle_ui_event(UiEvent::Wheel(BlitzWheelEvent {
            delta: BlitzWheelDelta::Pixels(delta[0], delta[1]),
            coords: PointerCoords {
                page_x: point.x,
                page_y: point.y,
                screen_x: point.x,
                screen_y: point.y,
                client_x: point.x,
                client_y: point.y,
            },
            buttons: self.buttons,
            mods: dioxus::html::Modifiers::default(),
            element: Point::default(),
        }));
    }
}
