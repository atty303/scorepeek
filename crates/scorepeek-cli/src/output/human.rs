use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use scorepeek_frontend_api::{
    CommandResult, ConfigResult, SkinInstallResult, SkinResult, VulkanLayerResult,
};

pub fn render(result: &CommandResult) -> Result<(), String> {
    write(&format(result))
}

fn format(result: &CommandResult) -> String {
    match result {
        CommandResult::Config { result, .. } => match result {
            ConfigResult::Path { path } => format!("{}\n", Path::new(path).display()),
            ConfigResult::Show {
                path,
                present,
                content,
            } => {
                if *present {
                    content.clone().unwrap_or_default()
                } else {
                    format!(
                        "config file is not present: {}\n",
                        Path::new(path).display()
                    )
                }
            }
            ConfigResult::Check { path, present, .. } => {
                if *present {
                    format!("config is valid: {}\n", Path::new(path).display())
                } else {
                    format!(
                        "config file is not present (optional): {}\n",
                        Path::new(path).display()
                    )
                }
            }
        },
        CommandResult::Doctor { report, .. } => format!(
            "scorepeek doctor\n  numeric model: {} ({})\n  catalog: {}\n  capture inventory: {}\n  Vulkan layer: {}\n",
            report.numeric_model.status(),
            report.numeric_model.model_id(),
            report.catalog.status.as_str(),
            "available",
            report.vulkan_layer.status.as_str(),
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
            SkinResult::Listed { skins } => {
                let mut text = String::new();
                for skin in skins {
                    writeln!(&mut text, "{}\t{}\t{}", skin.id, skin.release, skin.name)
                        .expect("writing to a String cannot fail");
                }
                text
            }
        },
        CommandResult::VulkanLayer { result } => match result {
            VulkanLayerResult::Installed => "installed\n",
            VulkanLayerResult::Updated => "updated\n",
            VulkanLayerResult::Unchanged => "unchanged\n",
            VulkanLayerResult::Uninstalled => "uninstalled\n",
            VulkanLayerResult::NotInstalled => "not installed\n",
        }
        .to_owned(),
    }
}

pub(super) fn write(text: &str) -> Result<(), String> {
    std::io::stdout()
        .lock()
        .write_all(text.as_bytes())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use scorepeek_frontend_api::InstalledSkin;
    use std::os::unix::ffi::OsStringExt as _;

    #[test]
    fn non_utf8_paths_remain_displayable() {
        let path = std::ffi::OsString::from_vec(b"/tmp/scorepeek-\xff".to_vec());
        let result = CommandResult::Config {
            result: ConfigResult::Path { path: path.clone() },
        };
        assert_eq!(format(&result), "/tmp/scorepeek-�\n");

        let result = CommandResult::Skin {
            result: SkinResult::Listed {
                skins: vec![InstalledSkin {
                    id: "cyan".to_owned(),
                    release: "1".to_owned(),
                    name: "Cyan".to_owned(),
                    path,
                }],
            },
        };
        assert_eq!(format(&result), "cyan\t1\tCyan\n");
    }
}
