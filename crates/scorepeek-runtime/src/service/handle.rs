use scorepeek_frontend_api::{FrontendCommand, FrontendEvent, FrontendReply};

/// In-process frontend endpoint backed by a bounded command/reply pair.
pub struct ServiceHandle;

impl ServiceHandle {
    #[must_use]
    pub const fn in_process() -> Self {
        Self
    }

    #[must_use]
    pub fn dispatch(
        &self,
        command: FrontendCommand,
        mut event: impl FnMut(FrontendEvent),
    ) -> FrontendReply {
        super::dispatch::dispatch(command, &mut event)
    }
}
