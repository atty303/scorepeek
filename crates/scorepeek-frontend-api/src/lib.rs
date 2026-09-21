//! Transport-neutral frontend protocol.

mod command;
mod error;
mod event;
mod protocol;
mod reply;
pub mod snapshot;

pub use command::{
    CaptureKind, ConfigAction, DiagnosticAction, FrontendCommand, OutputFormat, RunCommand,
    SkinAction, VulkanLayerAction,
};
pub use error::FrontendError;
pub use event::{FrontendEvent, OutputStream};
pub use protocol::{RequestId, Revision};
pub use reply::FrontendReply;
pub use snapshot::{ApplicationSnapshot, RunSnapshot};
