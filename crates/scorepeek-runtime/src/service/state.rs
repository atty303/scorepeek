use scorepeek_frontend_api::{FrontendEvent, OutputStream};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

type EventEnvelope = (FrontendEvent, std::sync::mpsc::SyncSender<bool>);
type EventSender = std::sync::mpsc::SyncSender<EventEnvelope>;

struct EventState {
    sender: EventSender,
    stop: Arc<AtomicBool>,
}

thread_local! {
    static FRONTEND_OUTPUT: std::cell::RefCell<Option<EventState>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn install(sender: EventSender, stop: Arc<AtomicBool>) {
    FRONTEND_OUTPUT.with(|slot| *slot.borrow_mut() = Some(EventState { sender, stop }));
}

pub(super) fn remove() {
    FRONTEND_OUTPUT.with(|slot| *slot.borrow_mut() = None);
}

pub(crate) fn event(event: FrontendEvent) -> bool {
    FRONTEND_OUTPUT.with(|slot| {
        slot.borrow().as_ref().is_some_and(|state| {
            let (ack, received) = std::sync::mpsc::sync_channel(0);
            state.sender.send((event, ack)).is_ok() && received.recv().unwrap_or(false)
        })
    })
}

pub(crate) fn stop_token() -> Arc<AtomicBool> {
    FRONTEND_OUTPUT.with(|slot| {
        slot.borrow().as_ref().map_or_else(
            || Arc::new(AtomicBool::new(false)),
            |state| Arc::clone(&state.stop),
        )
    })
}

pub(crate) fn output(stream: OutputStream, text: String) -> bool {
    event(FrontendEvent::Output { stream, text })
}
