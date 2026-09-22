use std::io::{IsTerminal as _, Write as _};

use ratatui::Terminal as RatatuiTerminal;
use ratatui::backend::CrosstermBackend;
use scorepeek_frontend_api::RunSnapshot;

pub struct Terminal {
    output: Output,
}

enum Output {
    Interactive(RatatuiTerminal<CrosstermBackend<std::io::Stdout>>),
    Plain { last_line: Option<String> },
}

impl Terminal {
    pub fn new() -> Self {
        if !std::io::stdout().is_terminal() {
            return Self {
                output: Output::Plain { last_line: None },
            };
        }

        let mut stdout = std::io::stdout();
        if let Err(error) = stdout
            .write_all(b"\x1b[?1049h\x1b[?25l")
            .and_then(|()| stdout.flush())
        {
            let _ = stdout.write_all(b"\x1b[?25h\x1b[?1049l");
            let _ = stdout.flush();
            eprintln!("scorepeek: terminal could not enter TUI mode: {error}");
            return Self {
                output: Output::Plain { last_line: None },
            };
        }

        match RatatuiTerminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => Self {
                output: Output::Interactive(terminal),
            },
            Err(error) => {
                let mut restore = std::io::stdout();
                let _ = restore.write_all(b"\x1b[?25h\x1b[?1049l");
                let _ = restore.flush();
                eprintln!("scorepeek: terminal could not initialize TUI rendering: {error}");
                Self {
                    output: Output::Plain { last_line: None },
                }
            }
        }
    }

    pub fn draw(&mut self, run: &RunSnapshot) {
        match &mut self.output {
            Output::Interactive(terminal) => {
                if let Err(error) = terminal.draw(|frame| super::render::render(frame, run)) {
                    eprintln!("scorepeek: TUI output failed: {error}");
                }
            }
            Output::Plain { last_line } => {
                let line = super::render::plain_status_line(run);
                if last_line.as_deref() != Some(&line) {
                    let mut output = std::io::stdout().lock();
                    let _ = writeln!(output, "{line}");
                    let _ = output.flush();
                    *last_line = Some(line);
                }
            }
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if matches!(self.output, Output::Interactive(_)) {
            let mut output = std::io::stdout();
            let _ = output.write_all(b"\x1b[?25h\x1b[?1049l");
            let _ = output.flush();
        }
    }
}
