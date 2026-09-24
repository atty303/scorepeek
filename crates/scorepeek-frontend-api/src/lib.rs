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
pub use event::{
    FrontendEvent, InspectionHeader, InspectionRecord, OperationalWarning, OutputStream,
};
pub use protocol::{RequestId, Revision};
pub use reply::{
    CommandResult, ConfigResult, DoctorReport, FrontendReply, InstalledSkin, ModelDownload,
    SkinInstallResult, SkinResult, VulkanLayerResult,
};
pub use snapshot::{ApplicationSnapshot, RunSnapshot};
