use scorepeek_frontend_api::FrontendReply;
use std::process::ExitCode;

pub fn render_reply(reply: FrontendReply) -> ExitCode {
    match reply {
        FrontendReply::Completed { exit_code } => ExitCode::from(exit_code),
        FrontendReply::Error { error } => {
            eprintln!("scorepeek: {}", error.message);
            ExitCode::from(1)
        }
        FrontendReply::Accepted { .. } | FrontendReply::Snapshot { .. } => ExitCode::SUCCESS,
    }
}
