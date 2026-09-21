use scorepeek_frontend_api::ApplicationSnapshot;

pub struct Application {
    terminal: super::terminal::Terminal,
}

impl Application {
    pub fn new() -> Self {
        Self {
            terminal: super::terminal::Terminal::new(),
        }
    }

    pub fn render(&mut self, snapshot: &ApplicationSnapshot) {
        if let Some(run) = super::view::active_run(snapshot) {
            self.terminal.draw(&super::render::lines(run));
        }
    }
}
