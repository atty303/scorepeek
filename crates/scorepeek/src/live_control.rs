use signal_hook::consts::signal::{SIGINT, SIGTERM};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

pub struct SignalStopMonitor {
    stop: Arc<AtomicBool>,
    registrations: [signal_hook::SigId; 2],
}

impl SignalStopMonitor {
    pub fn start() -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let interrupt = signal_hook::flag::register(SIGINT, Arc::clone(&stop))
            .map_err(|error| format!("SIGINT handler registration failed: {error}"))?;
        let terminate = match signal_hook::flag::register(SIGTERM, Arc::clone(&stop)) {
            Ok(registration) => registration,
            Err(error) => {
                signal_hook::low_level::unregister(interrupt);
                return Err(format!("SIGTERM handler registration failed: {error}"));
            }
        };
        Ok(Self {
            stop,
            registrations: [interrupt, terminate],
        })
    }

    pub fn stop_token(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }
}

impl Drop for SignalStopMonitor {
    fn drop(&mut self) {
        for registration in self.registrations {
            signal_hook::low_level::unregister(registration);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use signal_hook::consts::signal::{SIGINT, SIGTERM};

    use super::SignalStopMonitor;

    #[test]
    fn signal_monitor_handles_interrupt_and_terminate_in_subprocesses() {
        if let Some(signal) = std::env::var_os("SCOREPEEK_SIGNAL_MONITOR_CHILD") {
            let signal = if signal == "INT" { SIGINT } else { SIGTERM };
            let monitor = SignalStopMonitor::start().unwrap();
            signal_hook::low_level::raise(signal).unwrap();
            let started = Instant::now();
            while !monitor.stop_token().load(Ordering::Acquire) {
                assert!(started.elapsed() < Duration::from_secs(1));
                std::thread::yield_now();
            }
            return;
        }

        for signal in ["INT", "TERM"] {
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "live_control::tests::signal_monitor_handles_interrupt_and_terminate_in_subprocesses",
                    "--nocapture",
                ])
                .env("SCOREPEEK_SIGNAL_MONITOR_CHILD", signal)
                .status()
                .unwrap();
            assert!(status.success());
        }
    }
}
