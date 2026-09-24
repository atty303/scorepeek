use crate::command::OutputFormat;
use scorepeek_frontend_api::FrontendReply;
use std::io::Write as _;
use std::process::ExitCode;

pub fn render_reply(reply: FrontendReply, format: OutputFormat) -> ExitCode {
    match reply {
        FrontendReply::Completed { exit_code, result } => {
            if let Some(result) = result {
                let rendered = match format {
                    OutputFormat::Human => super::human::render(&result),
                    OutputFormat::Json => super::json::render(&result),
                };
                if let Err(error) = rendered {
                    let _ = writeln!(
                        std::io::stderr().lock(),
                        "scorepeek: output rendering failed: {error}"
                    );
                    return ExitCode::from(1);
                }
            }
            ExitCode::from(exit_code)
        }
        FrontendReply::Error { error } => {
            let _ = writeln!(std::io::stderr().lock(), "scorepeek: {}", error.message);
            ExitCode::from(1)
        }
        FrontendReply::Accepted { .. } | FrontendReply::Snapshot { .. } => ExitCode::SUCCESS,
    }
}
