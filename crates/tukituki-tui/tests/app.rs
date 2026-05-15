//! Tests for the testable TUI surface: App.handle dispatch, key
//! handling, folder expand/collapse, restart-all log clear. We can't
//! drive a real terminal here, but the App is decoupled from
//! ratatui's frame so we can exercise its state machine directly.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use tukituki_config::RunTarget;
use tukituki_state::Status;
use tukituki_tui::{App, ManagerHandle};

#[derive(Default)]
struct FakeManager {
    statuses: Mutex<BTreeMap<String, Status>>,
    log_lines: Mutex<BTreeMap<String, Vec<String>>>,
    started: Mutex<Vec<String>>,
    stopped: Mutex<Vec<String>>,
    restarted: Mutex<Vec<String>>,
    dumped: Mutex<Vec<(String, PathBuf)>>,
    cleared: Mutex<Vec<String>>,
}

impl ManagerHandle for FakeManager {
    fn get_all_statuses(&self) -> BTreeMap<String, Status> {
        self.statuses.lock().unwrap().clone()
    }
    fn get_targets(&self) -> Vec<RunTarget> {
        Vec::new()
    }
    fn get_log_lines(&self, name: &str) -> Vec<String> {
        self.log_lines
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .unwrap_or_default()
    }
    fn watch_log_lines(&self, _name: &str) -> Receiver<String> {
        let (_tx, rx) = channel();
        rx
    }
    fn start(&self, name: &str) -> std::io::Result<()> {
        self.started.lock().unwrap().push(name.into());
        Ok(())
    }
    fn stop(&self, name: &str) -> std::io::Result<()> {
        self.stopped.lock().unwrap().push(name.into());
        Ok(())
    }
    fn restart(&self, name: &str) -> std::io::Result<()> {
        self.restarted.lock().unwrap().push(name.into());
        Ok(())
    }
    fn dump_log(&self, name: &str, dest: &std::path::Path) -> std::io::Result<()> {
        self.dumped
            .lock()
            .unwrap()
            .push((name.into(), dest.to_path_buf()));
        Ok(())
    }
    fn clear_log(&self, name: &str) -> std::io::Result<()> {
        self.cleared.lock().unwrap().push(name.into());
        Ok(())
    }
    fn stop_all(&self) -> std::io::Result<()> {
        Ok(())
    }
    fn update_targets(&self, _targets: Vec<RunTarget>) {}
    fn describe(&self, name: &str) -> String {
        format!("description of {name}")
    }
    fn ensure_otel_collector(&self) -> std::io::Result<()> {
        Ok(())
    }
    fn log_file_path(&self, _name: &str) -> Option<PathBuf> {
        None
    }
}

fn target(name: &str) -> RunTarget {
    RunTarget {
        name: name.into(),
        command: "true".into(),
        ..Default::default()
    }
}

fn grouped(name: &str, group: &str) -> RunTarget {
    RunTarget {
        name: name.into(),
        group: group.into(),
        command: "true".into(),
        ..Default::default()
    }
}

fn make_app(targets: Vec<RunTarget>) -> App<FakeManager> {
    App::new(
        targets,
        std::sync::Arc::new(FakeManager::default()),
        PathBuf::from("."),
        PathBuf::from("."),
    )
}

fn key(code: KeyCode) -> AppEventForTest {
    AppEventForTest::Key(KeyEvent {
        code,
        modifiers: KeyModifiers::empty(),
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::empty(),
    })
}

// `AppEvent` is private. We expose a tiny shim re-exporting the variants
// we need for tests by routing through App's public handle interface.
// Doing it via the public `handle_key` is cleanest — but `handle_key`
// is not public. Instead, we drive things through public methods on
// App directly (the ones that don't require crossterm input).
//
// To keep this test honest about wired-up bindings, we use a stripped
// AppEvent enum that mirrors the real one's Key/Tick/LogLine variants.
//
// We can't import AppEvent (private), so we cheat: route via a public
// helper exposed below for tests only.
pub enum AppEventForTest {
    Key(KeyEvent),
    Tick,
    LogLine { target: String, line: String },
}

fn dispatch<H: ManagerHandle>(app: &mut App<H>, ev: AppEventForTest) -> bool {
    let real = match ev {
        AppEventForTest::Key(k) => tukituki_tui::test_support::key(k),
        AppEventForTest::Tick => tukituki_tui::test_support::tick(),
        AppEventForTest::LogLine { target, line } => {
            tukituki_tui::test_support::log_line(target, line)
        }
    };
    app.handle(real).continue_loop
}

#[test]
fn down_arrow_moves_selection() {
    let mut app = make_app(vec![target("a"), target("b"), target("c")]);
    assert_eq!(app.selected, 0);
    dispatch(&mut app, key(KeyCode::Down));
    assert_eq!(app.selected, 1);
    dispatch(&mut app, key(KeyCode::Char('j')));
    assert_eq!(app.selected, 2);
    // Past-end clamps.
    dispatch(&mut app, key(KeyCode::Down));
    assert_eq!(app.selected, 2);
}

#[test]
fn up_arrow_moves_selection() {
    let mut app = make_app(vec![target("a"), target("b"), target("c")]);
    app.selected = 2;
    dispatch(&mut app, key(KeyCode::Up));
    assert_eq!(app.selected, 1);
    dispatch(&mut app, key(KeyCode::Char('k')));
    assert_eq!(app.selected, 0);
    // Past-start clamps.
    dispatch(&mut app, key(KeyCode::Up));
    assert_eq!(app.selected, 0);
}

#[test]
fn folder_expand_collapse_reshapes_rows() {
    let mut app = make_app(vec![
        target("api"),
        grouped("kb-a", "kb"),
        grouped("kb-b", "kb"),
    ]);
    // Initial: top-level row, then a single folder header (collapsed).
    assert_eq!(app.rows.len(), 2);

    // Select the folder header and expand it.
    app.selected = 1;
    dispatch(&mut app, key(KeyCode::Right));
    assert_eq!(app.rows.len(), 4, "expanded folder should show its members");

    // Collapse.
    dispatch(&mut app, key(KeyCode::Char('h')));
    assert_eq!(app.rows.len(), 2);
}

#[test]
fn enter_toggles_selected_folder() {
    let mut app = make_app(vec![grouped("kb-a", "kb")]);
    app.selected = 0; // the folder header (kb-a is grouped, so no top-level rows)
    assert_eq!(app.rows.len(), 1);
    dispatch(&mut app, key(KeyCode::Enter));
    assert_eq!(app.rows.len(), 2, "Enter should expand");
    dispatch(&mut app, key(KeyCode::Enter));
    assert_eq!(app.rows.len(), 1, "Enter should re-collapse");
}

#[test]
fn detach_quits_loop_without_stop_all() {
    let mut app = make_app(vec![target("a")]);
    let cont = app.handle(tukituki_tui::test_support::key(KeyEvent {
        code: KeyCode::Char('q'),
        modifiers: KeyModifiers::empty(),
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::empty(),
    }));
    assert!(!cont.continue_loop, "q should end the loop");
    assert!(!cont.stop_all, "q should NOT request stop_all");
}

#[test]
fn shift_q_quits_with_stop_all() {
    let mut app = make_app(vec![target("a")]);
    let cont = app.handle(tukituki_tui::test_support::key(KeyEvent {
        code: KeyCode::Char('Q'),
        modifiers: KeyModifiers::SHIFT,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::empty(),
    }));
    assert!(!cont.continue_loop);
    assert!(cont.stop_all, "Q should request stop_all");
}

#[test]
fn ctrl_c_quits_with_stop_all() {
    let mut app = make_app(vec![target("a")]);
    let cont = app.handle(tukituki_tui::test_support::key(KeyEvent {
        code: KeyCode::Char('c'),
        modifiers: KeyModifiers::CONTROL,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::empty(),
    }));
    assert!(!cont.continue_loop);
    assert!(cont.stop_all);
}

#[test]
fn start_key_calls_manager_start() {
    let mgr = std::sync::Arc::new(FakeManager::default());
    let mut app = App::new(
        vec![target("alpha")],
        mgr.clone(),
        PathBuf::from("."),
        PathBuf::from("."),
    );
    app.selected = 0;
    dispatch(&mut app, key(KeyCode::Char('S')));
    let started = mgr.started.lock().unwrap();
    assert_eq!(*started, vec!["alpha".to_string()]);
}

#[test]
fn stop_key_calls_manager_stop() {
    let mgr = std::sync::Arc::new(FakeManager::default());
    let mut app = App::new(
        vec![target("alpha")],
        mgr.clone(),
        PathBuf::from("."),
        PathBuf::from("."),
    );
    dispatch(&mut app, key(KeyCode::Char('s')));
    let stopped = mgr.stopped.lock().unwrap();
    assert_eq!(*stopped, vec!["alpha".to_string()]);
}

#[test]
fn restart_key_calls_manager_restart() {
    let mgr = std::sync::Arc::new(FakeManager::default());
    let mut app = App::new(
        vec![target("alpha")],
        mgr.clone(),
        PathBuf::from("."),
        PathBuf::from("."),
    );
    dispatch(&mut app, key(KeyCode::Char('r')));
    assert_eq!(*mgr.restarted.lock().unwrap(), vec!["alpha".to_string()]);
}

#[test]
fn clear_key_calls_manager_clear_and_empties_buffer() {
    let mgr = std::sync::Arc::new(FakeManager::default());
    let mut app = App::new(
        vec![target("alpha")],
        mgr.clone(),
        PathBuf::from("."),
        PathBuf::from("."),
    );
    // Pre-load some lines.
    dispatch(
        &mut app,
        AppEventForTest::LogLine {
            target: "alpha".into(),
            line: "old line".into(),
        },
    );
    dispatch(&mut app, key(KeyCode::Char('c')));
    let buf = app.logs.get("alpha").unwrap();
    assert!(buf.lines.is_empty(), "buffer should be cleared");
    assert_eq!(*mgr.cleared.lock().unwrap(), vec!["alpha".to_string()]);
}

#[test]
fn help_overlay_toggles_visibility() {
    let mut app = make_app(vec![target("a")]);
    assert!(!app.help_visible);
    dispatch(&mut app, key(KeyCode::Char('?')));
    assert!(app.help_visible);
    dispatch(&mut app, key(KeyCode::Char('?')));
    assert!(!app.help_visible);
}

#[test]
fn describe_overlay_populated_from_manager() {
    let mgr = std::sync::Arc::new(FakeManager::default());
    let mut app = App::new(
        vec![target("alpha")],
        mgr.clone(),
        PathBuf::from("."),
        PathBuf::from("."),
    );
    dispatch(&mut app, key(KeyCode::Char('D')));
    assert!(app.describe.as_deref().is_some_and(|s| s.contains("alpha")));
    // Esc / D toggles closed.
    dispatch(&mut app, key(KeyCode::Esc));
    assert!(app.describe.is_none());
}

#[test]
fn wrap_and_zoom_toggle() {
    let mut app = make_app(vec![target("a")]);
    assert!(!app.wrap_logs);
    assert!(!app.zoom_logs);
    dispatch(&mut app, key(KeyCode::Char('w')));
    assert!(app.wrap_logs);
    dispatch(&mut app, key(KeyCode::Char('z')));
    assert!(app.zoom_logs);
}

#[test]
fn log_line_event_appends_to_buffer() {
    let mut app = make_app(vec![target("a")]);
    dispatch(
        &mut app,
        AppEventForTest::LogLine {
            target: "a".into(),
            line: "hello".into(),
        },
    );
    let buf = app.logs.get("a").unwrap();
    assert_eq!(buf.lines.back().map(String::as_str), Some("hello"));
}

#[test]
fn restart_all_clears_per_target_buffers() {
    let mgr = std::sync::Arc::new(FakeManager::default());
    let mut app = App::new(
        vec![target("a"), target("b")],
        mgr.clone(),
        PathBuf::from("."),
        PathBuf::from("."),
    );
    dispatch(
        &mut app,
        AppEventForTest::LogLine {
            target: "a".into(),
            line: "before".into(),
        },
    );
    dispatch(&mut app, key(KeyCode::Char('R')));
    assert!(app.logs.get("a").unwrap().lines.is_empty());
    assert_eq!(*mgr.restarted.lock().unwrap(), vec!["a", "b"]);
}
