use scorepeek_overlay_wayland::native::{VisualDebugScenario, run_visual_debug};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let scenario = args
        .next()
        .ok_or("usage: visual_native SCENARIO.json OUTPUT_DIR")?;
    let output = args
        .next()
        .ok_or("usage: visual_native SCENARIO.json OUTPUT_DIR")?;
    if args.next().is_some() {
        return Err("usage: visual_native SCENARIO.json OUTPUT_DIR".into());
    }
    let scenario: VisualDebugScenario = serde_json::from_slice(&std::fs::read(scenario)?)?;
    run_visual_debug(&scenario, std::path::Path::new(&output))?;
    Ok(())
}
