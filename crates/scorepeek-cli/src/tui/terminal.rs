use std::io::{IsTerminal as _, Write as _};

pub struct Terminal {
    interactive: bool,
    last_plain: Option<String>,
}

impl Terminal {
    pub fn new() -> Self {
        let interactive = std::io::stdout().is_terminal();
        if interactive {
            let _ = std::io::stdout().lock().write_all(b"\x1b[?1049h\x1b[?25l");
        }
        Self {
            interactive,
            last_plain: None,
        }
    }

    pub fn draw(&mut self, lines: &[String]) {
        let mut output = std::io::stdout().lock();
        if self.interactive {
            let _ = output.write_all(b"\x1b[H\x1b[2J");
            for line in lines {
                let _ = writeln!(output, "{line}");
            }
            let _ = output.flush();
        } else if let Some(line) = lines.first()
            && self.last_plain.as_ref() != Some(line)
        {
            let _ = writeln!(output, "{line}");
            self.last_plain = Some(line.clone());
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if self.interactive {
            let _ = std::io::stdout().lock().write_all(b"\x1b[?25h\x1b[?1049l");
        }
    }
}
