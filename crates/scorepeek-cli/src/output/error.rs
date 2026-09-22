use scorepeek_frontend_api::{FrontendReply, OutputFormat};
use std::process::ExitCode;

pub fn render_reply(reply: FrontendReply) -> ExitCode {
    match reply {
        FrontendReply::Completed { exit_code, result } => {
            if let Some(result) = result {
                let rendered = match &result {
                    scorepeek_frontend_api::CommandResult::Config { format, .. }
                    | scorepeek_frontend_api::CommandResult::Doctor { format, .. }
                    | scorepeek_frontend_api::CommandResult::Skin { format, .. } => match format {
                        OutputFormat::Human => super::human::render(&result),
                        OutputFormat::Json => super::json::render(&result),
                    },
                    scorepeek_frontend_api::CommandResult::VulkanLayer { .. } => {
                        super::human::render(&result)
                    }
                };
                if let Err(error) = rendered {
                    eprintln!("scorepeek: output rendering failed: {error}");
                    return ExitCode::from(1);
                }
            }
            ExitCode::from(exit_code)
        }
        FrontendReply::Error { error } => {
            eprintln!("scorepeek: {}", error.message);
            ExitCode::from(1)
        }
        FrontendReply::Accepted { .. } | FrontendReply::Snapshot { .. } => ExitCode::SUCCESS,
    }
}
