//! Native focus behavior that differs from the browser's built-in defaults.

use dioxus_native_dom::DioxusDocument;

pub use crate::input::keyboard::TextInputState;

pub(crate) fn focus_clicked_button(document: &mut DioxusDocument, point: [f32; 2]) {
    // Browsers focus an enabled button on primary click. Blitz currently only performs its
    // pointer-down focus default for text inputs, so keep this renderer difference here.
    let button = {
        let inner = document.inner.borrow();
        let mut candidate = inner.element_from_point(point[0], point[1]);
        loop {
            let Some(id) = candidate else { break None };
            let Some(node) = inner.get_node(id) else {
                break None;
            };
            if node.is_focussable()
                && node.element_data().is_some_and(|element| {
                    element.name.local.as_ref().eq_ignore_ascii_case("button")
                })
            {
                break Some(id);
            }
            candidate = node.parent;
        }
    };
    if let Some(button) = button {
        document.inner.borrow_mut().set_focus_to(button);
    }
}
