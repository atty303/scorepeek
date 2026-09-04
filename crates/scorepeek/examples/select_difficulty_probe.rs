//! Read-only production marker evaluation; RGB stream input consists of canonical frames.
use scorepeek::recognition::ScreenClass;
use scorepeek::recognition::{
    ScreenRgb8Crops, observe_music_select_difficulty, route_screen_rgb8_crops,
};
use std::io::{BufRead, Read};
fn report(pixels: &[u8], id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let ScreenRgb8Crops::MusicSelect(crops) =
        route_screen_rgb8_crops(pixels, ScreenClass::MusicSelect)?
    else {
        unreachable!()
    };
    println!(
        "{}",
        serde_json::json!({"id": id, "observation": observe_music_select_difficulty(&crops.difficulty_markers)})
    );
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return Err("usage: select_difficulty_probe FILE.qoi ... | --rgb-stream".into());
    }
    if args.as_slice() == ["--rgb-stream"] {
        let mut input = std::io::stdin().lock();
        let mut pixels = vec![0; 1920 * 1080 * 3];
        let mut index = 0;
        while !input.fill_buf()?.is_empty() {
            input.read_exact(&mut pixels)?;
            report(&pixels, &index.to_string())?;
            index += 1;
        }
    } else {
        for path in args {
            let (header, pixels) = qoi::decode_to_vec(std::fs::read(&path)?)?;
            if header.width != 1920
                || header.height != 1080
                || header.channels != qoi::Channels::Rgb
            {
                return Err("expected canonical RGB8 1920x1080 QOI".into());
            }
            report(&pixels, &path)?;
        }
    }
    Ok(())
}
