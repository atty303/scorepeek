use std::io::Write as _;

use scorepeek_frontend_api::{
    CommandResult, ConfigResult, SkinInstallResult, SkinResult, VulkanLayerResult,
};

pub fn render(result: &CommandResult) -> Result<(), String> {
    let text = match result {
        CommandResult::Config { result, .. } => match result {
            ConfigResult::Path { path } => format!("{path}\n"),
            ConfigResult::Show {
                path,
                present,
                content,
            } => {
                if *present {
                    content.clone().unwrap_or_default()
                } else {
                    format!("config file is not present: {path}\n")
                }
            }
            ConfigResult::Check { path, present, .. } => {
                if *present {
                    format!("config is valid: {path}\n")
                } else {
                    format!("config file is not present (optional): {path}\n")
                }
            }
        },
        CommandResult::Doctor { report, .. } => format!(
            "scorepeek doctor\n  numeric model: {} ({})\n  catalog: {}\n  capture inventory: {}\n  Vulkan layer: {}\n",
            report.numeric_model["status"].as_str().unwrap_or("unknown"),
            report.numeric_model["model_id"]
                .as_str()
                .unwrap_or("not available"),
            report.catalog["status"].as_str().unwrap_or("unknown"),
            if report.target_inventory.is_object() {
                "available"
            } else {
                "unavailable"
            },
            report.vulkan_layer["status"].as_str().unwrap_or("unknown"),
        ),
        CommandResult::Skin { result, .. } => match result {
            SkinResult::Installed { outcome } => match outcome {
                SkinInstallResult::Installed => "installed\n".to_owned(),
                SkinInstallResult::Replaced { previous_release } => {
                    format!("replaced {previous_release}\n")
                }
                SkinInstallResult::Unchanged => "unchanged\n".to_owned(),
            },
            SkinResult::Uninstalled => "uninstalled\n".to_owned(),
            SkinResult::Listed { skins } => skins
                .iter()
                .map(|skin| format!("{}\t{}\t{}\n", skin.id, skin.release, skin.name))
                .collect(),
        },
        CommandResult::VulkanLayer { result } => match result {
            VulkanLayerResult::Installed => "installed\n",
            VulkanLayerResult::Updated => "updated\n",
            VulkanLayerResult::Unchanged => "unchanged\n",
            VulkanLayerResult::Uninstalled => "uninstalled\n",
            VulkanLayerResult::NotInstalled => "not installed\n",
        }
        .to_owned(),
    };
    write(&text)
}

pub(super) fn write(text: &str) -> Result<(), String> {
    std::io::stdout()
        .lock()
        .write_all(text.as_bytes())
        .map_err(|error| error.to_string())
}
