mod inventory;

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args_os();
    let _program = args.next();

    match (args.next(), args.next()) {
        (Some(command), None) if command == "doctor" => {
            println!("{}", inventory::collect().to_json());
            ExitCode::SUCCESS
        }
        (Some(flag), None) if flag == "--help" || flag == "-h" => {
            print_usage();
            ExitCode::SUCCESS
        }
        (Some(flag), None) if flag == "--version" || flag == "-V" => {
            println!("scorepeek {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: scorepeek doctor");
            ExitCode::from(2)
        }
    }
}

fn print_usage() {
    println!(
        "scorepeek {}\n\nUsage:\n  scorepeek doctor",
        env!("CARGO_PKG_VERSION")
    );
}
