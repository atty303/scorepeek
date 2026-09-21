use scorepeek_overlay_wayland::skin::{InstallOutcome, StoreRoot};
use std::path::PathBuf;

fn main() -> Result<(), String> {
    let packages = std::env::args_os().skip(1).map(PathBuf::from);
    let store = StoreRoot::discover();
    for package in packages {
        match store.install(&package)? {
            InstallOutcome::Installed => println!("installed"),
            InstallOutcome::Replaced { previous_release } => {
                println!("replaced {previous_release}");
            }
            InstallOutcome::Unchanged => println!("unchanged"),
        }
    }
    Ok(())
}
