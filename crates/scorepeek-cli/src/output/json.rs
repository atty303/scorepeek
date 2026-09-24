use scorepeek_frontend_api::{CommandResult, ConfigResult, SkinResult};
use serde::Serialize;
use std::ffi::OsStr;

pub fn render(result: &CommandResult) -> Result<(), String> {
    let mut text = format(result)?;
    text.push('\n');
    super::human::write(&text)
}

fn format(result: &CommandResult) -> Result<String, String> {
    let text = match result {
        CommandResult::Config { result } => match result {
            ConfigResult::Path { path } => serde_json::to_string(&serde_json::json!({
                "path": config_path(path)?,
            })),
            ConfigResult::Show {
                path,
                present,
                content,
            } => serde_json::to_string(&serde_json::json!({
                "path": config_path(path)?,
                "present": present,
                "content": content,
            })),
            ConfigResult::Check {
                path,
                present,
                valid,
            } => serde_json::to_string(&serde_json::json!({
                "path": config_path(path)?,
                "present": present,
                "valid": valid,
            })),
        },
        CommandResult::Doctor { report } => serde_json::to_string(report),
        CommandResult::Skin { result } => match result {
            SkinResult::Listed { skins } => {
                #[derive(Serialize)]
                struct JsonSkin<'a> {
                    id: &'a str,
                    release: &'a str,
                    name: &'a str,
                    path: &'a str,
                }

                let skins = skins.iter().map(|skin| {
                    Ok(JsonSkin {
                        id: &skin.id,
                        release: &skin.release,
                        name: &skin.name,
                        path: skin.path.to_str().ok_or_else(|| {
                            "skin list serialization failed: path contains invalid UTF-8 characters".to_owned()
                        })?,
                    })
                }).collect::<Result<Vec<_>, String>>()?;
                serde_json::to_string(&skins)
            }
            _ => serde_json::to_string(result),
        },
        CommandResult::VulkanLayer { result } => serde_json::to_string(result),
    };
    text.map_err(|error| error.to_string())
}

fn config_path(path: &OsStr) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| "config path must be UTF-8 for JSON output".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use scorepeek_frontend_api::{
        CatalogReport, CatalogStatus, DoctorReport, InstalledSkin, NumericModelReport,
        TargetInventory, VulkanLayerReport, VulkanLayerStatus,
    };
    use std::collections::BTreeMap;
    use std::os::unix::ffi::OsStringExt as _;

    #[test]
    fn config_json_preserves_public_shapes() {
        let path: std::ffi::OsString = "/tmp/config.toml".into();
        let results = [
            (
                ConfigResult::Path { path: path.clone() },
                r#"{"path":"/tmp/config.toml"}"#,
            ),
            (
                ConfigResult::Show {
                    path: path.clone(),
                    present: false,
                    content: None,
                },
                r#"{"content":null,"path":"/tmp/config.toml","present":false}"#,
            ),
            (
                ConfigResult::Check {
                    path,
                    present: true,
                    valid: true,
                },
                r#"{"path":"/tmp/config.toml","present":true,"valid":true}"#,
            ),
        ];
        for (result, expected) in results {
            assert_eq!(format(&CommandResult::Config { result }).unwrap(), expected);
        }
    }

    #[test]
    fn doctor_and_skin_json_preserve_public_shapes() {
        let report = DoctorReport {
            schema: "scorepeek-doctor-v5".to_owned(),
            target_inventory: TargetInventory {
                schema: "scorepeek-target-inventory-v1".to_owned(),
                os: BTreeMap::new(),
                observations: BTreeMap::new(),
            },
            numeric_model: NumericModelReport::Unavailable {
                reason: "missing".to_owned(),
                registered_manifest_sha256: "hash".to_owned(),
            },
            catalog: CatalogReport {
                status: CatalogStatus::Unavailable,
                reason: Some("missing".to_owned()),
                active_catalog_sha256: None,
                source_url_sha256: None,
                last_success_unix_seconds: None,
                etag_present: None,
                last_modified_present: None,
                last_failure: None,
            },
            vulkan_layer: VulkanLayerReport {
                status: VulkanLayerStatus::NotInstalled,
                manifest_path: "/tmp/manifest".into(),
                library_path: "/tmp/library".into(),
                embedded_manifest_sha256: None,
                embedded_library_sha256: None,
                installed_manifest_sha256: None,
                installed_library_sha256: None,
                reason: None,
            },
        };
        assert_eq!(
            format(&CommandResult::Doctor {
                report: Box::new(report)
            })
            .unwrap(),
            r#"{"schema":"scorepeek-doctor-v5","target_inventory":{"schema":"scorepeek-target-inventory-v1","os":{},"observations":{}},"numeric_model":{"status":"unavailable","reason":"missing","registered_manifest_sha256":"hash"},"catalog":{"status":"unavailable","reason":"missing"},"vulkan_layer":{"status":"not_installed","manifest_path":"/tmp/manifest","library_path":"/tmp/library","embedded_manifest_sha256":null,"embedded_library_sha256":null,"installed_manifest_sha256":null,"installed_library_sha256":null}}"#
        );
        let result = SkinResult::Listed {
            skins: vec![InstalledSkin {
                id: "cyan".to_owned(),
                release: "1".to_owned(),
                name: "Cyan".to_owned(),
                path: "/tmp/cyan.zip".into(),
            }],
        };
        assert_eq!(
            format(&CommandResult::Skin { result }).unwrap(),
            r#"[{"id":"cyan","release":"1","name":"Cyan","path":"/tmp/cyan.zip"}]"#
        );
    }

    #[test]
    fn non_utf8_paths_fail_only_at_json_rendering() {
        let path = std::ffi::OsString::from_vec(b"/tmp/scorepeek-\xff".to_vec());
        let result = CommandResult::Config {
            result: ConfigResult::Path { path: path.clone() },
        };
        assert_eq!(
            format(&result).unwrap_err(),
            "config path must be UTF-8 for JSON output"
        );
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
        assert_eq!(
            format(&result).unwrap_err(),
            "skin list serialization failed: path contains invalid UTF-8 characters"
        );
    }
}
