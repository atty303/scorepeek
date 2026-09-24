use scorepeek_frontend_api::{FrontendEvent, OperationalWarning, OutputStream};
use std::fmt::Write as _;
use std::io::Write as _;

pub struct Renderer {
    run: crate::tui::application::Application,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            run: crate::tui::application::Application::new(),
        }
    }

    pub fn render_event(&mut self, event: FrontendEvent) -> Result<(), String> {
        match event {
            FrontendEvent::ModelDownload { state } => {
                let text = match state {
                    scorepeek_frontend_api::ModelDownload::Started => {
                        "scorepeek: downloading PP-OCRv6-small model...\n"
                    }
                    scorepeek_frontend_api::ModelDownload::Completed => {
                        "scorepeek: PP-OCRv6-small model download complete\n"
                    }
                };
                std::io::stderr()
                    .lock()
                    .write_all(text.as_bytes())
                    .map_err(|error| error.to_string())?;
            }
            FrontendEvent::Output { stream, text } => match stream {
                OutputStream::Stdout => super::human::write(&text)?,
                OutputStream::Stderr => {
                    std::io::stderr()
                        .lock()
                        .write_all(text.as_bytes())
                        .map_err(|error| error.to_string())?;
                }
            },
            FrontendEvent::Snapshot { snapshot } => self.run.render(&snapshot),
            FrontendEvent::InspectionHeader { header } => super::human::write(&format!(
                "scorepeek diagnostic inspection\n  run: {}\n  active: {}\n  partial: {}\n  records: {}\nevents:\n",
                header.run_id,
                header.active,
                header.partial,
                header.next_sequence.saturating_sub(header.oldest_sequence)
            ))?,
            FrontendEvent::InspectionRecord { record } => {
                let mut text = format!("  {}: {}", record.sequence, record.operation);
                for key in ["stage", "status", "error_type"] {
                    if let Some(detail) = record.data[key].as_str() {
                        write!(text, " {key}={detail}").expect("writing to a String cannot fail");
                    }
                }
                text.push('\n');
                super::human::write(&text)?;
            }
            FrontendEvent::Warning { warning } => {
                let text = match warning {
                    OperationalWarning::ReplayTruncated { requested_seconds, available_us } => format!(
                        "scorepeek: requested {requested_seconds}s diagnostic replay, but the ring retains only {:.3}s; streaming the available suffix\n",
                        std::time::Duration::from_micros(available_us).as_secs_f64()
                    ),
                    OperationalWarning::DiagnosticPersistenceUnavailable { error } => format!("scorepeek: diagnostic persistence unavailable: {error}\n"),
                    OperationalWarning::DiagnosticPersistenceLagged => "scorepeek: diagnostic persistence degraded: writer lagged behind the ring\n".to_owned(),
                    OperationalWarning::DiagnosticPersistenceWriteFailed => "scorepeek: diagnostic persistence degraded: stream write failed\n".to_owned(),
                    OperationalWarning::DiagnosticSocketUnavailable { error } => format!("scorepeek: diagnostic socket unavailable: {error}\n"),
                    OperationalWarning::BackgroundCatalogWorkerStartFailed { error } => format!("scorepeek: background catalog update unavailable: {error}\n"),
                };
                std::io::stderr()
                    .lock()
                    .write_all(text.as_bytes())
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }
}
