pub mod build;
pub mod verify;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

pub fn run(arguments: impl IntoIterator<Item = impl Into<std::ffi::OsString> + Clone>) -> ExitCode {
    let cli = match Cli::try_parse_from(arguments) {
        Ok(cli) => cli,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            return ExitCode::from(code);
        }
    };
    let result = match cli.command {
        Command::Build(options) => build::run(options),
        Command::Verify(options) => verify::run(&options),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("scorepeek catalog publisher failed: {error}");
            ExitCode::from(1)
        }
    }
}

#[derive(Parser)]
#[command(name = "scorepeek-catalog-publisher", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Build(build::BuildOptions),
    Verify(verify::VerifyOptions),
}

pub(crate) fn absolute_path(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if !path.is_absolute() || path.as_os_str().is_empty() {
        return Err("path must be absolute and non-empty".to_owned());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {

    #[test]
    fn distribution_notices_retain_each_source_policy() {
        let notices = include_str!("../../../../THIRD_PARTY_NOTICES.md");
        for required in [
            "Tachi IIDX seeds",
            "Unlicense",
            "Textage",
            "textage.cc/score/readme.html",
            "dqn/iidxapi",
            "License: ISC",
        ] {
            assert!(notices.contains(required), "missing notice: {required}");
        }
    }
}
