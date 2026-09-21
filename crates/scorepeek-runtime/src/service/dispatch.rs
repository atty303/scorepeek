use scorepeek_frontend_api::{FrontendCommand, FrontendError, FrontendEvent, FrontendReply};

#[path = "dispatch/application.rs"]
mod application;

pub use application::dev_operation_main;
pub(crate) use application::{dispatch_frontend, exit_for_result, frontend_event};

pub(super) fn dispatch(
    command: FrontendCommand,
    event: &mut impl FnMut(FrontendEvent),
) -> FrontendReply {
    let (commands, receiver) = std::sync::mpsc::sync_channel(1);
    let (replies, reply_receiver) = std::sync::mpsc::sync_channel(1);
    let (events, event_receiver) = std::sync::mpsc::sync_channel(64);
    let worker = super::lifecycle::spawn(move || {
        if let Ok(command) = receiver.recv() {
            super::state::install(events);
            let _ = replies.send(dispatch_frontend(command));
            super::state::remove();
        }
    });
    let Ok(worker) = worker else {
        return service_error("runtime service thread could not be started");
    };
    if commands.send(command).is_err() {
        super::shutdown::join(worker);
        return service_error("runtime service command channel closed");
    }
    let reply = loop {
        match event_receiver.recv_timeout(std::time::Duration::from_millis(25)) {
            Ok(runtime_event) => event(runtime_event),
            Err(
                std::sync::mpsc::RecvTimeoutError::Disconnected
                | std::sync::mpsc::RecvTimeoutError::Timeout,
            ) => {}
        }
        match reply_receiver.try_recv() {
            Ok(reply) => break reply,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                break service_error("runtime service reply channel closed");
            }
        }
    };
    while let Ok(received) = event_receiver.try_recv() {
        event(received);
    }
    super::shutdown::join(worker);
    reply
}

fn service_error(message: &str) -> FrontendReply {
    FrontendReply::Error {
        error: FrontendError {
            error_type: "service_unavailable".to_owned(),
            message: message.to_owned(),
        },
    }
}
