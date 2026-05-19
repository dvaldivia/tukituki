//! Event enum dispatched into the App's update loop.

use crossterm::event::KeyEvent;

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Resize(u16, u16),
    /// 1s heartbeat that drives status reconciliation.
    Tick,
    /// One new log line for a target.
    LogLine {
        target: String,
        line: String,
    },
    /// Run-directory file change (debounced 200ms) — triggers reload.
    FileChange,
    /// State-file change (debounced 200ms) — an external `tukituki
    /// start/stop/restart` updated `state.json`, so we re-read it and
    /// resync tailers. Without this we'd keep checking liveness against
    /// the pre-restart PID and flag the service as crashed.
    StateFileChange,
    /// Mouse wheel scroll (positive = down, negative = up).
    ScrollLog(i32),
    /// External editor exited.
    EditorDone(std::io::Result<()>),
    /// A background operation (start/stop/restart/restart-all/dump)
    /// finished. The main loop removes `id` from the in-flight map
    /// and flashes `summary` in the header. Sent from worker threads
    /// spawned by the input-action handlers.
    OpDone {
        id: u64,
        summary: String,
    },
}
