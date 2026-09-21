//! Compile-time embedded Web client assets.

#[derive(rust_embed::Embed)]
#[folder = "$SCOREPEEK_WEB_ASSET_DIR/"]
pub(crate) struct Assets;

pub(crate) const ASSET_VERSION: &str = env!("SCOREPEEK_OVERLAY_BUILD_ID");
