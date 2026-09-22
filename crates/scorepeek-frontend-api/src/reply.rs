use crate::{ApplicationSnapshot, FrontendError, Revision};
use serde::{Deserialize, Serialize};

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
    pub path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ConfigResult {
    Path {
        path: String,
    },
    Show {
        path: String,
        present: bool,
        content: Option<String>,
    },
    Check {
        path: String,
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
    Config {
        format: crate::OutputFormat,
        result: ConfigResult,
    },
    Doctor {
        format: crate::OutputFormat,
        report: DoctorReport,
    },
    Skin {
        format: crate::OutputFormat,
        result: SkinResult,
    },
    VulkanLayer {
        result: VulkanLayerResult,
    },
}

impl CommandResult {
    /// Serializes a typed command result for a frontend-selected JSON presentation.
    ///
    /// # Errors
    /// Returns the serializer error when the protocol value cannot be encoded.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        match self {
            Self::Config { result, .. } => match result {
                ConfigResult::Path { path } => {
                    serde_json::to_string(&serde_json::json!({"path": path}))
                }
                ConfigResult::Show {
                    path,
                    present,
                    content,
                } => serde_json::to_string(&serde_json::json!({
                    "path": path,
                    "present": present,
                    "content": content,
                })),
                ConfigResult::Check {
                    path,
                    present,
                    valid,
                } => serde_json::to_string(&serde_json::json!({
                    "path": path,
                    "present": present,
                    "valid": valid,
                })),
            },
            Self::Doctor { report, .. } => serde_json::to_string(report),
            Self::Skin { result, .. } => match result {
                SkinResult::Listed { skins } => serde_json::to_string(skins),
                _ => serde_json::to_string(result),
            },
            Self::VulkanLayer { result } => serde_json::to_string(result),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_json_preserves_the_public_document_shapes() {
        let path = CommandResult::Config {
            format: crate::OutputFormat::Json,
            result: ConfigResult::Path {
                path: "/tmp/config.toml".to_owned(),
            },
        };
        assert_eq!(path.to_json().unwrap(), r#"{"path":"/tmp/config.toml"}"#);

        let show = CommandResult::Config {
            format: crate::OutputFormat::Json,
            result: ConfigResult::Show {
                path: "/tmp/config.toml".to_owned(),
                present: false,
                content: None,
            },
        };
        assert_eq!(
            show.to_json().unwrap(),
            r#"{"content":null,"path":"/tmp/config.toml","present":false}"#
        );

        let check = CommandResult::Config {
            format: crate::OutputFormat::Json,
            result: ConfigResult::Check {
                path: "/tmp/config.toml".to_owned(),
                present: true,
                valid: true,
            },
        };
        assert_eq!(
            check.to_json().unwrap(),
            r#"{"path":"/tmp/config.toml","present":true,"valid":true}"#
        );
    }
}
