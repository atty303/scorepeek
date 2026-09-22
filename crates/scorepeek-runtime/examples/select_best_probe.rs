//! Offline, read-only evaluation using only the two registered recognition bundles.
use scorepeek_core::recognition::music_select::{MusicSelectBestCrops, resolve_music_select_best};
use scorepeek_core::recognition::result::numeric::RegisteredNumericRuntime;
use scorepeek_core::recognition::title::RegisteredDynamicTitleRuntime;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let text_root = args
        .next()
        .ok_or("expected registered text bundle directory")?;
    let mut numeric = RegisteredNumericRuntime::load_embedded()?;
    let mut text = RegisteredDynamicTitleRuntime::load(std::path::Path::new(&text_root))?;
    for name in args {
        let (_, pixels) = qoi::decode_to_vec(std::fs::read(&name)?)?;
        let crops = MusicSelectBestCrops::extract(&pixels)?;
        let numeric = numeric.observe_music_select_best(&crops)?;
        let header = text.observe_open_text(&crops.header)?.open_text;
        let clear = text.observe_open_text(&crops.clear_type)?.open_text;
        println!(
            "{}",
            serde_json::json!({
                "schema": "scorepeek-music-select-best-probe-v1", "file": name,
                "observation": resolve_music_select_best(header, clear, numeric)
            })
        );
    }
    Ok(())
}
