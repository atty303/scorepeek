use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

pub struct InputStopMonitor {
    stop: Arc<AtomicBool>,
}

impl InputStopMonitor {
    pub fn start() -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        thread::Builder::new()
            .name("scorepeek-stop-input".to_owned())
            .spawn(move || monitor_input(&worker_stop))
            .map_err(|error| format!("live stop-input monitor thread unavailable: {error}"))?;
        Ok(Self { stop })
    }

    pub fn stop_token(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }
}

fn monitor_input(stop: &AtomicBool) {
    let stdin = io::stdin();
    monitor_reader(stdin.lock(), stop);
}

fn monitor_reader(mut reader: impl io::BufRead, stop: &AtomicBool) {
    let mut line = [0_u8; 6];
    let mut length = 0_usize;
    let mut oversized = false;
    loop {
        let available = match reader.fill_buf() {
            Ok([]) | Err(_) => return,
            Ok(available) => available,
        };
        let mut consumed = 0;
        for &byte in available {
            consumed += 1;
            if !oversized {
                if length < line.len() {
                    line[length] = byte;
                    length += 1;
                } else {
                    oversized = true;
                }
            }
            if byte == b'\n' {
                if !oversized && is_stop_line_bytes(&line[..length]) {
                    stop.store(true, Ordering::Release);
                    reader.consume(consumed);
                    return;
                }
                length = 0;
                oversized = false;
            }
        }
        reader.consume(consumed);
    }
}

fn is_stop_line_bytes(line: &[u8]) -> bool {
    matches!(line, b"stop\n" | b"stop\r\n")
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{is_stop_line_bytes, monitor_reader};

    #[test]
    fn only_exact_stop_control_line_requests_shutdown() {
        assert!(is_stop_line_bytes(b"stop\n"));
        assert!(is_stop_line_bytes(b"stop\r\n"));
        assert!(!is_stop_line_bytes(b"stop"));
        assert!(!is_stop_line_bytes(b"stop \n"));
        assert!(!is_stop_line_bytes(b" stop\n"));
        assert!(!is_stop_line_bytes(b""));
    }

    #[test]
    fn monitor_skips_other_and_oversized_lines_before_exact_stop() {
        let stop = AtomicBool::new(false);
        let input = format!("help\n{}\nstop\n", "x".repeat(4096));

        monitor_reader(Cursor::new(input), &stop);

        assert!(stop.load(Ordering::Acquire));
    }
}
