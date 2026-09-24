use crate::RequestId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureKind {
    Pipewire,
    VulkanLayer,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    #[default]
    Human,
    Json,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "paired transport flags preserve explicit enable and disable overrides"
)]
pub struct RunCommand {
    pub capture: Option<CaptureKind>,
    pub node_name: Option<String>,
    pub crop_left: Option<u32>,
    pub crop_top: Option<u32>,
    pub crop_right: Option<u32>,
    pub crop_bottom: Option<u32>,
    pub scores_db: Option<String>,
    pub no_scores: bool,
    pub scores: bool,
    pub record: bool,
    pub record_all: bool,
    pub no_record: bool,
    pub record_memory_mib: Option<usize>,
    pub overlay_wayland: bool,
    pub no_overlay_wayland: bool,
    pub overlay_wayland_edit: bool,
    pub no_overlay_wayland_edit: bool,
    pub overlay_obs: bool,
    pub no_overlay_obs: bool,
    pub overlay_config: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ConfigAction {
    Path,
    Show,
    Check,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum DiagnosticAction {
    Observe {
        replay_seconds: Option<u64>,
    },
    Inspect {
        run_id: Option<String>,
        format: OutputFormat,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SkinAction {
    Install {
        package: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        force: Option<bool>,
    },
    Uninstall {
        id: String,
    },
    List,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VulkanLayerAction {
    Install,
    Uninstall,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum FrontendCommand {
    Run {
        request_id: RequestId,
        config: Option<String>,
        command: RunCommand,
    },
    Doctor {
        request_id: RequestId,
    },
    Config {
        request_id: RequestId,
        config: Option<String>,
        action: ConfigAction,
    },
    Diagnostic {
        request_id: RequestId,
        action: DiagnosticAction,
    },
    Skin {
        request_id: RequestId,
        action: SkinAction,
    },
    VulkanLayer {
        request_id: RequestId,
        action: VulkanLayerAction,
    },
}

#[cfg(test)]
mod tests {
    use super::SkinAction;

    #[test]
    fn skin_install_without_force_keeps_the_previous_wire_shape() {
        let previous = serde_json::json!({"action":"install","package":"skin.zip"});
        let action: SkinAction = serde_json::from_value(previous.clone()).unwrap();
        assert_eq!(
            action,
            SkinAction::Install {
                package: "skin.zip".into(),
                force: None,
            }
        );
        assert_eq!(serde_json::to_value(action).unwrap(), previous);
    }
}
