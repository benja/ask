use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const INITIAL_DELAY: Duration = Duration::from_millis(150);
const FRAME_DELAY: Duration = Duration::from_millis(180);
const LONG_WAIT_TICK: usize = 32;
const DOTS: &[&str] = &["", ".", "..", "..."];

pub struct Spinner {
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Spinner {
    pub fn start(enabled: bool) -> Self {
        let running = Arc::new(AtomicBool::new(enabled));
        let worker = enabled.then(|| {
            let running = Arc::clone(&running);
            thread::spawn(move || {
                thread::park_timeout(INITIAL_DELAY);
                let mut tick = 0;
                while running.load(Ordering::Relaxed) {
                    let mut stderr = io::stderr().lock();
                    let _ = write!(stderr, "\r\x1b[2K{}", frame(tick));
                    let _ = stderr.flush();
                    drop(stderr);

                    tick = tick.saturating_add(1);
                    thread::park_timeout(FRAME_DELAY);
                }
            })
        });
        Self { running, worker }
    }

    pub fn stop(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.running.store(false, Ordering::Relaxed);
            worker.thread().unpark();
            let _ = worker.join();
            let mut stderr = io::stderr().lock();
            let _ = write!(stderr, "\r\x1b[2K\r");
            let _ = stderr.flush();
        }
    }
}

fn frame(tick: usize) -> String {
    let word = if tick < LONG_WAIT_TICK {
        "asking"
    } else {
        "still asking"
    };
    let dots = DOTS[tick % DOTS.len()];
    format!("{word}{dots}")
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn enabled() -> bool {
    io::stdout().is_terminal()
        && io::stderr().is_terminal()
        && std::env::var_os("ASK_NO_SPINNER").is_none()
        && std::env::var("TERM").is_ok_and(|term| term != "dumb")
}

#[cfg(test)]
mod tests {
    use super::frame;

    #[test]
    fn long_wait_changes_the_label_once() {
        assert_eq!(frame(0), "asking");
        assert_eq!(frame(31), "asking...");
        assert_eq!(frame(32), "still asking");
        assert_eq!(frame(999), "still asking...");
    }
}
