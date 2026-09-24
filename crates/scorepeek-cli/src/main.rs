mod command;
mod completion;
mod output;
mod tui;

use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    if let Some(exit) = scorepeek_runtime::dispatch_private_role(&arguments) {
        return exit;
    }
    match command::parse(arguments) {
        Ok(command::Action::Dispatch(command, format)) => {
            let mut renderer = output::Renderer::new();
            let reply = scorepeek_runtime::ServiceHandle::in_process()
                .dispatch(command, |event| renderer.render_event(event));
            drop(renderer);
            output::render_reply(reply, format)
        }
        Ok(command::Action::Complete(shell)) => {
            command::generate_completion(shell);
            ExitCode::SUCCESS
        }
        Ok(command::Action::Help) => {
            if let Err(error) = command::print_help() {
                eprintln!("scorepeek: help output failed: {error}");
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            ExitCode::from(code)
        }
    }
}
