//! Browser pointer-capture adaptation.

use dioxus::prelude::PointerEvent;
use dioxus_web::WebEventExt as _;
use wasm_bindgen::JsCast as _;

/// Captures subsequent browser pointer events on the element that began the gesture.
pub fn capture(event: &PointerEvent) {
    let raw = event.data().as_web_event();
    if let Some(target) = raw
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
    {
        let _ = target.set_pointer_capture(raw.pointer_id());
    }
}
