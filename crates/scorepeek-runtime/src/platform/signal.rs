#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    reason = "internal signal adapter has fixed POSIX signal invariants and descriptive errors"
)]

use signal_hook::consts::signal::{SIGINT, SIGTERM};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub struct SignalStopMonitor {
    stop: Arc<AtomicBool>,
    signal: Arc<AtomicUsize>,
    registrations: [signal_hook::SigId; 4],
}

impl SignalStopMonitor {
    #[cfg(test)]
    pub fn start() -> Result<Self, String> {
        Self::start_with_stop(Arc::new(AtomicBool::new(false)))
    }

    pub fn start_with_stop(stop: Arc<AtomicBool>) -> Result<Self, String> {
        let signal = Arc::new(AtomicUsize::new(0));
        let interrupt = signal_hook::flag::register(SIGINT, Arc::clone(&stop))
            .map_err(|error| format!("SIGINT handler registration failed: {error}"))?;
        let interrupt_value = match signal_hook::flag::register_usize(
            SIGINT,
            Arc::clone(&signal),
            usize::try_from(SIGINT).expect("SIGINT is nonnegative"),
        ) {
            Ok(registration) => registration,
            Err(error) => {
                signal_hook::low_level::unregister(interrupt);
                return Err(format!("SIGINT value handler registration failed: {error}"));
            }
        };
        let terminate = match signal_hook::flag::register(SIGTERM, Arc::clone(&stop)) {
            Ok(registration) => registration,
            Err(error) => {
                signal_hook::low_level::unregister(interrupt);
                signal_hook::low_level::unregister(interrupt_value);
                return Err(format!("SIGTERM handler registration failed: {error}"));
            }
        };
        let terminate_value = match signal_hook::flag::register_usize(
            SIGTERM,
            Arc::clone(&signal),
            usize::try_from(SIGTERM).expect("SIGTERM is nonnegative"),
        ) {
            Ok(registration) => registration,
            Err(error) => {
                signal_hook::low_level::unregister(interrupt);
                signal_hook::low_level::unregister(interrupt_value);
                signal_hook::low_level::unregister(terminate);
                return Err(format!(
                    "SIGTERM value handler registration failed: {error}"
                ));
            }
        };
        Ok(Self {
            stop,
            signal,
            registrations: [interrupt, interrupt_value, terminate, terminate_value],
        })
    }

    #[must_use]
    pub fn stop_token(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }

    #[must_use]
    pub fn interrupted(&self) -> bool {
        self.signal.load(Ordering::Acquire)
            == usize::try_from(SIGINT).expect("SIGINT is nonnegative")
    }

    #[must_use]
    pub fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::Acquire)
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
            assert_eq!(monitor.interrupted(), signal == SIGINT);
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
