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
use std::time::Duration;

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

    let (tx, rx) = mpsc::channel::<AppEvent>();

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

    // Per-target log pumps. Subscribe once and forward each line as a
    // LogLine event keyed by target name.
    for t in &targets {
        let name = t.name.clone();
        let rx_lines = manager.watch_log_lines(&name);
        let lines_tx = tx.clone();
        thread::Builder::new()
            .name(format!("tukituki-tui-pump-{name}"))
            .spawn(move || {
                while let Ok(line) = rx_lines.recv() {
                    if lines_tx
                        .send(AppEvent::LogLine {
                            target: name.clone(),
                            line,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            })?;
    }

    let mut app = App::new(targets, manager, run_dir, project_root);
    // Backfill ring-buffer history into the TUI buffer so the right pane
    // isn't empty on first paint.
    app.backfill_logs();

    let outcome = loop {
        terminal.draw(|f| view::render(f, &app))?;

        let ev = match rx.recv() {
            Ok(e) => e,
            Err(_) => break SessionOutcome { stop_all: false },
        };
        let cont = app.handle(ev);
        if !cont.continue_loop {
            break SessionOutcome {
                stop_all: cont.stop_all,
            };
        }
    };

    // Restore the terminal regardless of error paths.
    let _ = disable_raw_mode();
    let _ = execute!(std::io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
    drop(terminal);

    Ok(outcome)
}

fn input_loop(tx: mpsc::Sender<AppEvent>) {
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
    tx: mpsc::Sender<AppEvent>,
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
