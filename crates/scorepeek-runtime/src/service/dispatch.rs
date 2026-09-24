use scorepeek_frontend_api::{FrontendCommand, FrontendError, FrontendEvent, FrontendReply};

#[path = "dispatch/application.rs"]
mod application;

pub(crate) use application::{dispatch_frontend, exit_for_result, frontend_event};

pub(super) fn dispatch(
    command: FrontendCommand,
    event: &mut impl FnMut(FrontendEvent) -> Result<(), String>,
) -> FrontendReply {
    let (commands, receiver) = std::sync::mpsc::sync_channel(1);
    let (replies, reply_receiver) = std::sync::mpsc::sync_channel(1);
    let (events, event_receiver) = std::sync::mpsc::sync_channel(64);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_stop = std::sync::Arc::clone(&stop);
    let worker = super::lifecycle::spawn(move || {
        if let Ok(command) = receiver.recv() {
            super::state::install(events, worker_stop);
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
            Ok((runtime_event, ack)) => {
                if let Err(error) = event(runtime_event) {
                    stop.store(true, std::sync::atomic::Ordering::Release);
                    let _ = ack.send(false);
                    drop(event_receiver);
                    super::shutdown::join(worker);
                    return service_error(&format!("frontend output failed: {error}"));
                }
                let _ = ack.send(true);
            }
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
    while let Ok((received, ack)) = event_receiver.try_recv() {
        if let Err(error) = event(received) {
            stop.store(true, std::sync::atomic::Ordering::Release);
            let _ = ack.send(false);
            drop(event_receiver);
            super::shutdown::join(worker);
            return service_error(&format!("frontend output failed: {error}"));
        }
        let _ = ack.send(true);
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
