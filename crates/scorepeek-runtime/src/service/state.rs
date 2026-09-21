use scorepeek_frontend_api::{FrontendEvent, OutputStream};

thread_local! {
    static FRONTEND_OUTPUT: std::cell::RefCell<Option<std::sync::mpsc::SyncSender<FrontendEvent>>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn install(sender: std::sync::mpsc::SyncSender<FrontendEvent>) {
    FRONTEND_OUTPUT.with(|slot| *slot.borrow_mut() = Some(sender));
}

pub(super) fn remove() {
    FRONTEND_OUTPUT.with(|slot| *slot.borrow_mut() = None);
}

pub(crate) fn event(event: FrontendEvent) -> bool {
    FRONTEND_OUTPUT.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|sender| sender.send(event).is_ok())
    })
}

pub(crate) fn output(stream: OutputStream, text: String) -> bool {
    event(FrontendEvent::Output { stream, text })
}
