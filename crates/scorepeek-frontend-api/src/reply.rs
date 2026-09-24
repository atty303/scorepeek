use crate::{ApplicationSnapshot, FrontendError, Revision};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

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
    pub target_inventory: TargetInventory,
    pub numeric_model: NumericModelReport,
    pub catalog: CatalogReport,
    pub vulkan_layer: VulkanLayerReport,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TargetInventory {
    pub schema: String,
    pub os: BTreeMap<String, String>,
    pub observations: BTreeMap<String, ProbeObservation>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProbeObservation {
    Detected { value: String },
    Unavailable,
    Failed { exit_code: i32 },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NumericModelReport {
    Active {
        model_id: String,
        model_sha256: String,
        manifest_sha256: String,
        preprocessor_id: String,
    },
    Unavailable {
        reason: String,
        registered_manifest_sha256: String,
    },
}

impl NumericModelReport {
    #[must_use]
    pub const fn status(&self) -> &'static str {
        match self {
            Self::Active { .. } => "active",
            Self::Unavailable { .. } => "unavailable",
        }
    }

    #[must_use]
    pub fn model_id(&self) -> &str {
        match self {
            Self::Active { model_id, .. } => model_id,
            Self::Unavailable { .. } => "not available",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogStatus {
    Active,
    Unavailable,
}

impl CatalogStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CatalogReport {
    pub status: CatalogStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_catalog_sha256: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url_sha256: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_success_unix_seconds: Option<Option<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag_present: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified_present: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<Option<CatalogUpdateFailure>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CatalogUpdateFailure {
    pub source_url_sha256: String,
    pub failed_unix_seconds: u64,
    pub error_type: CatalogUpdateErrorType,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogUpdateErrorType {
    ConfigurationInvalid,
    TransportFailed,
    HttpStatus,
    ArtifactInvalid,
    ActivationFailed,
    StateFailed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VulkanLayerStatus {
    NotInstalled,
    MatchesEmbedded,
    DifferentPayload,
    Invalid,
}

impl VulkanLayerStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::MatchesEmbedded => "matches_embedded",
            Self::DifferentPayload => "different_payload",
            Self::Invalid => "invalid",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VulkanLayerReport {
    pub status: VulkanLayerStatus,
    pub manifest_path: PathBuf,
    pub library_path: PathBuf,
    pub embedded_manifest_sha256: Option<String>,
    pub embedded_library_sha256: Option<String>,
    pub installed_manifest_sha256: Option<String>,
    pub installed_library_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
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
    Doctor { report: Box<DoctorReport> },
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
