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
    /// Mouse wheel scroll (positive = down, negative = up).
    ScrollLog(i32),
    /// External editor exited.
    EditorDone(std::io::Result<()>),
}
