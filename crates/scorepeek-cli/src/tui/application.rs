use scorepeek_frontend_api::ApplicationSnapshot;

pub struct Application {
    terminal: Option<super::terminal::Terminal>,
}

impl Application {
    pub fn new() -> Self {
        Self { terminal: None }
    }

    pub fn render(&mut self, snapshot: &ApplicationSnapshot) {
        if let Some(run) = super::view::active_run(snapshot) {
            self.terminal
                .get_or_insert_with(super::terminal::Terminal::new)
                .draw(run);
        }
    }
}
