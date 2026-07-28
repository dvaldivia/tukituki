//! Handshake between the input-reader thread and handlers that hand
//! the terminal to a child process (`$EDITOR`).
//!
//! A plain "paused" flag isn't enough. `crossterm::event::poll` pulls
//! bytes off the tty into crossterm's own parser buffer as soon as the
//! fd is readable, so a reader already sitting inside `poll` when the
//! flag flips still steals the next keystroke — one the child was meant
//! to receive. The keystroke isn't lost, it's *buffered*: it replays
//! into the TUI the moment the reader resumes. That's the editor
//! feeling laggy (keys vanish) and the stray command waiting on exit
//! (typing `/foo` in vim leaves the TUI's search prompt open).
//!
//! So the pause is acknowledged rather than announced: the requester
//! blocks until the reader confirms it is parked *outside* `poll`, and
//! only then touches the terminal.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// How long a parked reader sleeps between checks. Bounds how quickly
/// the first keystroke after the editor exits reaches the TUI.
const PARKED_SLEEP: Duration = Duration::from_millis(50);

/// Poll interval used while waiting for the reader's acknowledgement.
/// Short because this runs exactly once per editor launch and the user
/// is staring at a frozen frame until it clears.
const ACK_POLL: Duration = Duration::from_millis(1);

#[derive(Debug, Default)]
pub struct ReaderGate {
    /// Requested state, written by the main thread.
    pause: AtomicBool,
    /// Observed state, written by the reader thread. True only while
    /// the reader is provably not inside `poll`/`read`.
    parked: AtomicBool,
}

impl ReaderGate {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- requester side (main thread) ----

    /// Ask the reader to yield stdin, then wait for it to actually do
    /// so. Returns false if the reader didn't acknowledge within
    /// `timeout` — callers proceed anyway, since a wedged reader must
    /// not stop the user from opening their editor.
    pub fn pause_and_wait(&self, timeout: Duration) -> bool {
        self.pause.store(true, Ordering::Release);
        let deadline = Instant::now() + timeout;
        while !self.parked.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(ACK_POLL);
        }
        true
    }

    /// Hand stdin back to the reader.
    pub fn resume(&self) {
        self.pause.store(false, Ordering::Release);
    }

    // ---- reader side ----

    pub fn is_paused(&self) -> bool {
        self.pause.load(Ordering::Acquire)
    }

    /// Acknowledge the pause and sleep. Call only from the point in the
    /// reader loop where no crossterm read is in flight.
    pub fn park_and_sleep(&self) {
        self.parked.store(true, Ordering::Release);
        thread::sleep(PARKED_SLEEP);
    }

    /// Mark the reader as owning stdin again. Returns false if a pause
    /// was requested in the meantime, in which case the caller must go
    /// back and park instead of entering `poll` — otherwise the
    /// requester would wait out a full poll timeout believing we're
    /// still running.
    pub fn try_unpark(&self) -> bool {
        self.parked.store(false, Ordering::Release);
        if self.is_paused() {
            self.parked.store(true, Ordering::Release);
            return false;
        }
        true
    }

    #[cfg(test)]
    pub fn is_parked(&self) -> bool {
        self.parked.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn pause_waits_for_the_reader_to_acknowledge() {
        let gate = Arc::new(ReaderGate::new());
        let reader = gate.clone();
        // Reader is "inside poll" for 60ms, so the ack can't be
        // instantaneous — pause_and_wait has to actually block.
        let h = thread::spawn(move || {
            thread::sleep(Duration::from_millis(60));
            while reader.is_paused() {
                reader.park_and_sleep();
            }
        });
        let start = Instant::now();
        assert!(gate.pause_and_wait(Duration::from_secs(2)));
        assert!(start.elapsed() >= Duration::from_millis(60));
        assert!(gate.is_parked());
        gate.resume();
        h.join().unwrap();
    }

    #[test]
    fn pause_times_out_when_the_reader_never_parks() {
        let gate = ReaderGate::new();
        let start = Instant::now();
        assert!(!gate.pause_and_wait(Duration::from_millis(30)));
        assert!(start.elapsed() >= Duration::from_millis(30));
        // The request stands even though nobody acknowledged it.
        assert!(gate.is_paused());
    }

    #[test]
    fn unpark_refuses_when_a_pause_is_pending() {
        let gate = ReaderGate::new();
        gate.pause.store(true, Ordering::Release);
        assert!(!gate.try_unpark());
        // Refusing still counts as parked, so a waiter isn't stranded.
        assert!(gate.is_parked());

        gate.resume();
        assert!(gate.try_unpark());
        assert!(!gate.is_parked());
    }
}
