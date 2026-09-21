use crate::bridge::data::Config;
use crate::bundle::embedded::Assets;

/// Serves only local embedded UI assets and display snapshots.
/// # Errors
/// Returns runtime, bind or worker errors.
pub fn run(config: Config, input: impl std::io::Read + Send + 'static) -> Result<(), String> {
    if Assets::get("index.html").is_none() {
        return Err("embedded index.html is missing".into());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(crate::server::router::serve(config, input))
}

#[cfg(test)]
mod browser_contract_tests {
    #[test]
    fn browser_runtime_keeps_validation_and_editor_revision_guards() {
        let browser = include_str!("../bundle/skin_browser.js");
        assert!(browser.contains("Number.isSafeInteger"));
        assert!(browser.contains("value.attributes === undefined"));
        assert!(browser.contains("value.children === undefined"));
        assert!(browser.contains("element.style.cssText = value"));
        assert!(browser.contains("scorepeek-editor-presentation"));
        assert!(browser.contains("awaitingEditorGeometry && !sameSpecification"));
        assert!(browser.contains("message.revision <= lastEditorRevision"));
        assert!(browser.contains("let spec = JSON.parse"));
        assert!(!browser.contains("const spec = JSON.parse"));
        assert!(browser.contains("spec = message.specification"));
    }
}
