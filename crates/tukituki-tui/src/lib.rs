//! ratatui-based TUI for tukituki.
//!
//! Mirrors the Go `internal/tui` package: sidebar of folder/target rows,
//! right-pane log viewport, status tick, file-watching reload, external
//! editor, and the keybinding surface from the README. Reads + writes
//! every long-lived I/O through std (threads + mpsc) — no async runtime
//! needed.

mod app;
mod event;
mod handle;
mod input;
mod rows;
mod theme;
mod view;

pub use app::App;
pub use handle::ManagerHandle;

/// Helpers that tests use to construct private `AppEvent` variants.
/// Not intended for production callers; the module is public only so
/// integration tests in `tests/` can route through it.
pub mod test_support {
    use crossterm::event::KeyEvent;

    use crate::event::AppEvent;

    pub fn key(k: KeyEvent) -> AppEvent {
        AppEvent::Key(k)
    }
    pub fn tick() -> AppEvent {
        AppEvent::Tick
    }
    pub fn log_line(target: String, line: String) -> AppEvent {
        AppEvent::LogLine { target, line }
    }
}

use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Max number of in-flight events to/from the App's main loop.
///
/// Each event is small (a String + a few bytes) so 16 KiB events
/// ≈ low single-digit MB at worst. Big enough to absorb bursty
/// stdout from chatty backends; small enough that memory stays
/// sane under pathological load.
const APP_CHANNEL_CAPACITY: usize = 16_384;

/// How long the main loop will keep draining events between renders
/// before forcing a repaint. Targets ~60fps under heavy log load —
/// a burst of N LogLines is one render, not N. Also caps the worst-
/// case latency between a key press and a re-render.
const FRAME_BUDGET: Duration = Duration::from_millis(16);

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event as CtEvent, KeyEventKind, MouseEvent,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use tukituki_config::RunTarget;

use crate::event::AppEvent;

/// Outcome of a TUI session — does the caller need to stop_all on exit?
#[derive(Debug, Clone, Copy)]
pub struct SessionOutcome {
    pub stop_all: bool,
}

/// Run the TUI until the user detaches (`q`) or quits-and-kills (`Q` /
/// `Ctrl+C`).  Blocks the caller; returns `stop_all=true` when the
/// caller should walk the target list and stop everything.
pub fn start<H: ManagerHandle + Send + Sync + 'static>(
    targets: Vec<RunTarget>,
    manager: H,
    run_dir: PathBuf,
    project_root: PathBuf,
) -> io::Result<SessionOutcome> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Bounded channel. Producers that matter for UX (input, ticks,
    // file changes) use blocking `send` so they never get dropped.
    // Log-line pumps use `try_send` and drop lines on overflow —
    // see the pump spawn below for the rationale.
    let (tx, rx) = mpsc::sync_channel::<AppEvent>(APP_CHANNEL_CAPACITY);

    let manager: std::sync::Arc<H> = std::sync::Arc::new(manager);

    // Spawn the input reader thread. It blocks on crossterm::event::read
    // and forwards keys/resizes/mouse to the app loop.
    let input_tx = tx.clone();
    let _input_handle = thread::Builder::new()
        .name("tukituki-tui-input".into())
        .spawn(move || input_loop(input_tx))?;

    // Status tick every second so PID liveness + reaper-driven status
    // changes propagate to the sidebar without a key press.
    let tick_tx = tx.clone();
    let _tick_handle = thread::Builder::new()
        .name("tukituki-tui-tick".into())
        .spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(1));
                if tick_tx.send(AppEvent::Tick).is_err() {
                    return;
                }
            }
        })?;

    // File-watcher: any *.yaml/*.yml change under run_dir triggers a
    // reload event. Debounced inside notify-debouncer-mini.
    let reload_tx = tx.clone();
    let _watcher = spawn_fs_watcher(&run_dir, reload_tx).ok();

    // Per-target log pumps. Forward each line from the manager's
    // subscriber channel as an `AppEvent::LogLine`.
    //
    // `try_send` + drop on Full is deliberate: under a log storm, a
    // chatty target can't backpressure the input thread (whose send
    // we want to be near-instant). Dropped lines are still reachable
    // via the manager's 1000-line ring buffer and the on-disk log
    // file — the user can run `tukituki logs <name>` from another
    // terminal to see anything missed in real-time here.
    for t in &targets {
        let name = t.name.clone();
        let rx_lines = manager.watch_log_lines(&name);
        let lines_tx = tx.clone();
        thread::Builder::new()
            .name(format!("tukituki-tui-pump-{name}"))
            .spawn(move || {
                while let Ok(line) = rx_lines.recv() {
                    let ev = AppEvent::LogLine {
                        target: name.clone(),
                        line,
                    };
                    match lines_tx.try_send(ev) {
                        Ok(_) => {}
                        Err(mpsc::TrySendError::Full(_)) => {
                            // TUI saturated. Drop the line.
                        }
                        Err(mpsc::TrySendError::Disconnected(_)) => return,
                    }
                }
            })?;
    }

    let mut app = App::new(targets, manager, run_dir, project_root);
    // Backfill ring-buffer history into the TUI buffer so the right pane
    // isn't empty on first paint.
    app.backfill_logs();

    // Event loop. Two coordinating mechanisms:
    //
    //   * `app.is_dirty()` — set by handlers when something the user
    //     can actually see has changed. Renders only happen when
    //     dirty, so LogLine events for *non-selected* targets buffer
    //     silently without touching the terminal at all. Huge win
    //     for projects with many concurrent chatty targets.
    //
    //   * Drain prioritisation — non-LogLine events (keys, ticks,
    //     file changes, etc.) break out of the drain pass so the
    //     repaint that follows reflects them immediately rather than
    //     waiting for the rest of any queued log flood. Switching
    //     targets during a log storm now feels instant: the Key
    //     handler short-circuits the drain and we render right away.
    //
    //   * `next_frame_at` keeps the actual render rate under
    //     1/FRAME_BUDGET. Coalesces bursts on the selected target.
    let mut next_frame_at = Instant::now();
    let outcome = 'outer: loop {
        let now = Instant::now();
        if app.is_dirty() && now >= next_frame_at {
            terminal.draw(|f| view::render(f, &app))?;
            app.clear_dirty();
            next_frame_at = now + FRAME_BUDGET;
        }

        // Wait for the next event. When dirty, time-bound so we wake
        // up to render at the next frame boundary; when clean,
        // a long block is fine (Tick will arrive every second to
        // refresh status icons regardless).
        let wait_for = if app.is_dirty() {
            next_frame_at.saturating_duration_since(Instant::now())
        } else {
            Duration::from_secs(1)
        };
        let first = match rx.recv_timeout(wait_for.max(Duration::from_millis(1))) {
            Ok(e) => e,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break SessionOutcome { stop_all: false };
            }
        };
        let cont = app.handle(first);
        if !cont.continue_loop {
            break SessionOutcome {
                stop_all: cont.stop_all,
            };
        }

        // Drain follow-up events with two breakouts: time budget OR
        // a high-priority (non-LogLine) event. The latter ensures
        // key presses don't wait out the budget while a log flood
        // queues up.
        let drain_deadline = Instant::now() + FRAME_BUDGET;
        while Instant::now() < drain_deadline {
            match rx.try_recv() {
                Ok(ev) => {
                    let is_hi_pri = !matches!(&ev, AppEvent::LogLine { .. });
                    let cont = app.handle(ev);
                    if !cont.continue_loop {
                        break 'outer SessionOutcome {
                            stop_all: cont.stop_all,
                        };
                    }
                    if is_hi_pri {
                        break;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    break 'outer SessionOutcome { stop_all: false };
                }
            }
        }
    };

    // Restore the terminal regardless of error paths.
    let _ = disable_raw_mode();
    let _ = execute!(std::io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
    drop(terminal);

    Ok(outcome)
}

fn input_loop(tx: mpsc::SyncSender<AppEvent>) {
    loop {
        match crossterm::event::read() {
            Ok(CtEvent::Key(k)) if k.kind == KeyEventKind::Press => {
                if tx.send(AppEvent::Key(k)).is_err() {
                    return;
                }
            }
            Ok(CtEvent::Resize(w, h)) => {
                if tx.send(AppEvent::Resize(w, h)).is_err() {
                    return;
                }
            }
            Ok(CtEvent::Mouse(MouseEvent { kind, .. })) => {
                use crossterm::event::MouseEventKind as Mk;
                let scroll = match kind {
                    Mk::ScrollUp => Some(-3),
                    Mk::ScrollDown => Some(3),
                    _ => None,
                };
                if let Some(d) = scroll
                    && tx.send(AppEvent::ScrollLog(d)).is_err()
                {
                    return;
                }
            }
            Ok(_) => {}
            Err(_) => return,
        }
    }
}

fn spawn_fs_watcher(
    run_dir: &std::path::Path,
    tx: mpsc::SyncSender<AppEvent>,
) -> notify::Result<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>> {
    let mut debouncer = notify_debouncer_mini::new_debouncer(
        Duration::from_millis(200),
        move |events: notify_debouncer_mini::DebounceEventResult| {
            if let Ok(events) = events {
                let interesting = events.iter().any(|e| {
                    let ext = e.path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    ext == "yaml" || ext == "yml"
                });
                if interesting {
                    let _ = tx.send(AppEvent::FileChange);
                }
            }
        },
    )?;
    debouncer
        .watcher()
        .watch(run_dir, notify::RecursiveMode::Recursive)?;
    Ok(debouncer)
}
