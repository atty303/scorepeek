//! Browser viewport observation and resize subscription.

use dioxus::prelude::Callback;
use scorepeek_overlay::editor_model::{EditorEffect, EditorInput};
use wasm_bindgen::{JsCast as _, closure::Closure};

pub(crate) fn viewport() -> [u32; 2] {
    web_sys::window()
        .and_then(|window| window.document())
        .and_then(|doc| doc.document_element())
        .map_or([1920, 1080], |element| {
            [
                u32::try_from(element.client_width()).unwrap_or(1920),
                u32::try_from(element.client_height()).unwrap_or(1080),
            ]
        })
}

pub(crate) struct ResizeListener(Closure<dyn FnMut(web_sys::Event)>);

impl ResizeListener {
    pub(crate) fn new(dispatch: Callback<EditorInput, Vec<EditorEffect>>) -> Self {
        let callback = Closure::wrap(Box::new(move |_: web_sys::Event| {
            let _ = dispatch.call(EditorInput::Resize {
                output: "obs-output".into(),
                logical_size: viewport(),
            });
        }) as Box<dyn FnMut(_)>);
        if let Some(window) = web_sys::window() {
            let _ = window
                .add_event_listener_with_callback("resize", callback.as_ref().unchecked_ref());
        }
        Self(callback)
    }
}

impl Drop for ResizeListener {
    fn drop(&mut self) {
        if let Some(window) = web_sys::window() {
            let _ = window
                .remove_event_listener_with_callback("resize", self.0.as_ref().unchecked_ref());
        }
    }
}
