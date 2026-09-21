use scorepeek_frontend_api::{FrontendEvent, OutputStream};
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

    pub fn render_event(&mut self, event: FrontendEvent) {
        match event {
            FrontendEvent::Output { stream, text } => match stream {
                OutputStream::Stdout => super::json::write(&text),
                OutputStream::Stderr => {
                    let _ = std::io::stderr().lock().write_all(text.as_bytes());
                }
            },
            FrontendEvent::Snapshot { snapshot } => self.run.render(&snapshot),
        }
    }
}
