use crate::{ApplicationSnapshot, FrontendError, Revision};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelDownload {
    Started,
    Completed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InstalledSkin {
    pub id: String,
    pub release: String,
    pub name: String,
    pub path: OsString,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ConfigResult {
    Path {
        path: OsString,
    },
    Show {
        path: OsString,
        present: bool,
        content: Option<String>,
    },
    Check {
        path: OsString,
        present: bool,
        valid: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DoctorReport {
    pub schema: String,
    pub target_inventory: serde_json::Value,
    pub numeric_model: serde_json::Value,
    pub catalog: serde_json::Value,
    pub vulkan_layer: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum SkinInstallResult {
    Installed,
    Replaced { previous_release: String },
    Unchanged,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum SkinResult {
    Installed { outcome: SkinInstallResult },
    Uninstalled,
    Listed { skins: Vec<InstalledSkin> },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VulkanLayerResult {
    Installed,
    Updated,
    Unchanged,
    Uninstalled,
    NotInstalled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum CommandResult {
    Config { result: ConfigResult },
    Doctor { report: DoctorReport },
    Skin { result: SkinResult },
    VulkanLayer { result: VulkanLayerResult },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FrontendReply {
    Accepted {
        revision: Revision,
    },
    Snapshot {
        snapshot: Box<ApplicationSnapshot>,
    },
    Completed {
        exit_code: u8,
        result: Option<CommandResult>,
    },
    Error {
        error: FrontendError,
    },
}
