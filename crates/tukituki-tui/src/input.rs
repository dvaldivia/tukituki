//! Key-event → action dispatcher.
//!
//! Mirrors `internal/tui/keymap.go` + the `Update` switch in
//! `internal/tui/model.go`. Every binding is wired here; actions that
//! talk to the manager run on the same thread (synchronous), which is
//! fine because Manager ops are themselves quick or fire-and-forget.

use chrono::Utc;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Continuation};
use crate::handle::ManagerHandle;
use crate::rows::Row;

pub fn handle_key<H: ManagerHandle>(app: &mut App<H>, k: KeyEvent) -> Continuation {
    // Modal: help overlay & describe overlay swallow most keys except
    // dismiss / quit.
    if app.help_visible {
        match k.code {
            KeyCode::Char('?') | KeyCode::Esc => app.help_visible = false,
            KeyCode::Char('Q') => return Continuation::kill_all(),
            KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                return Continuation::kill_all();
            }
            KeyCode::Char('q') => return Continuation::detach(),
            _ => {}
        }
        return Continuation::cont();
    }
    if app.describe.is_some() {
        match k.code {
            KeyCode::Char('D') | KeyCode::Esc => app.describe = None,
            KeyCode::Char('Q') => return Continuation::kill_all(),
            KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                return Continuation::kill_all();
            }
            KeyCode::Char('q') => return Continuation::detach(),
            _ => {}
        }
        return Continuation::cont();
    }

    // Search mode: every key feeds the query except for the few
    // controls below. Mirrors Go's `searchMode` branch in model.go.
    // Ctrl+C is the one universal escape hatch — even mid-search.
    if app.search_mode {
        if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
            return Continuation::kill_all();
        }
        match k.code {
            KeyCode::Esc => app.reset_search(),
            // Enter and `/` both cycle to the next match (wrap-around).
            KeyCode::Enter => app.next_search_match(),
            KeyCode::Char('/') => app.next_search_match(),
            KeyCode::Backspace => {
                app.search_query.pop();
                app.update_search_matches();
                if !app.search_matches.is_empty() {
                    app.jump_to_current_match();
                }
            }
            KeyCode::Char(c) => {
                app.search_query.push(c);
                app.update_search_matches();
                if !app.search_matches.is_empty() {
                    app.jump_to_current_match();
                }
            }
            _ => {}
        }
        return Continuation::cont();
    }

    // Quit handling that beats every other binding.
    if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
        return Continuation::kill_all();
    }
    if k.code == KeyCode::Char('Q') {
        return Continuation::kill_all();
    }
    if k.code == KeyCode::Char('q') {
        return Continuation::detach();
    }

    match k.code {
        KeyCode::Char('?') => app.help_visible = true,
        KeyCode::Char('/') => {
            app.search_mode = true;
            app.search_query.clear();
            app.search_matches.clear();
            app.search_match_idx = 0;
        }
        KeyCode::Up | KeyCode::Char('k') => move_selection(app, -1),
        KeyCode::Down | KeyCode::Char('j') => move_selection(app, 1),
        KeyCode::Tab => move_selection(app, 1),
        KeyCode::PageUp | KeyCode::Char('b') => app.scroll_log(-10),
        KeyCode::PageDown | KeyCode::Char('f') => app.scroll_log(10),
        KeyCode::Right | KeyCode::Char('l') => expand_folder(app),
        KeyCode::Left | KeyCode::Char('h') => collapse_folder(app),
        KeyCode::Enter | KeyCode::Char(' ') => toggle_folder(app),
        KeyCode::Char('r') => action_restart(app),
        KeyCode::Char('R') => action_restart_all(app),
        KeyCode::Char('s') => action_stop(app),
        KeyCode::Char('S') => action_start(app),
        KeyCode::Char('d') => action_dump(app),
        KeyCode::Char('c') => action_clear(app),
        KeyCode::Char('w') => {
            app.wrap_logs = !app.wrap_logs;
            app.flash(if app.wrap_logs { "wrap on" } else { "wrap off" });
        }
        KeyCode::Char('z') => {
            app.zoom_logs = !app.zoom_logs;
            app.flash(if app.zoom_logs { "zoom on" } else { "zoom off" });
        }
        KeyCode::Char('D') => action_describe(app),
        KeyCode::Char('E') => action_edit(app),
        _ => {}
    }

    Continuation::cont()
}

fn move_selection<H: ManagerHandle>(app: &mut App<H>, delta: i32) {
    if app.rows.is_empty() {
        return;
    }
    let len = app.rows.len() as i32;
    let new = (app.selected as i32 + delta).clamp(0, len - 1);
    if new as usize != app.selected {
        // Search matches are tied to the previously-selected target;
        // they're meaningless against the new buffer. Match Go's
        // implicit reset (it stores matches per-target via the
        // selectedTargetName lookup, which the Rust port collapsed
        // into a single Vec keyed by the active target).
        if app.search_mode {
            app.reset_search();
        }
        app.selected = new as usize;
    }
}

fn expand_folder<H: ManagerHandle>(app: &mut App<H>) {
    if let Some(Row::Folder { group, .. }) = app.rows.get(app.selected).cloned() {
        app.folder_expanded.insert(group, true);
        app.rebuild_rows();
    }
}

fn collapse_folder<H: ManagerHandle>(app: &mut App<H>) {
    // If a folder header is selected, collapse it. If a target inside a
    // folder is selected, jump up to the header and collapse it —
    // matches Go's behaviour.
    match app.rows.get(app.selected).cloned() {
        Some(Row::Folder { group, .. }) => {
            app.folder_expanded.insert(group, false);
            app.rebuild_rows();
        }
        Some(Row::Target { group, .. }) if !group.is_empty() => {
            app.folder_expanded.insert(group.clone(), false);
            app.rebuild_rows();
            // Walk back to the folder header (now collapsed).
            for (i, r) in app.rows.iter().enumerate() {
                if matches!(r, Row::Folder { group: g, .. } if g == &group) {
                    app.selected = i;
                    return;
                }
            }
        }
        _ => {}
    }
}

fn toggle_folder<H: ManagerHandle>(app: &mut App<H>) {
    if let Some(Row::Folder {
        group, expanded, ..
    }) = app.rows.get(app.selected).cloned()
    {
        app.folder_expanded.insert(group, !expanded);
        app.rebuild_rows();
    }
}

fn action_start<H: ManagerHandle>(app: &mut App<H>) {
    let Some(name) = app.selected_target_name() else {
        return;
    };
    let msg = match app.manager.start(&name) {
        Ok(_) => format!("started {name}"),
        Err(e) => format!("start {name}: {e}"),
    };
    app.flash(&msg);
}

fn action_stop<H: ManagerHandle>(app: &mut App<H>) {
    let Some(name) = app.selected_target_name() else {
        return;
    };
    let msg = match app.manager.stop(&name) {
        Ok(_) => format!("stopped {name}"),
        Err(e) => format!("stop {name}: {e}"),
    };
    app.flash(&msg);
}

fn action_restart<H: ManagerHandle>(app: &mut App<H>) {
    let Some(name) = app.selected_target_name() else {
        return;
    };
    let msg = match app.manager.restart(&name) {
        Ok(_) => format!("restarted {name}"),
        Err(e) => format!("restart {name}: {e}"),
    };
    app.flash(&msg);
}

fn action_restart_all<H: ManagerHandle>(app: &mut App<H>) {
    let names: Vec<String> = app.targets.iter().map(|t| t.name.clone()).collect();
    let mut ok = 0usize;
    let mut err = 0usize;
    for n in &names {
        // Clear the per-target log buffer so restart-all gives a fresh
        // scrollback (matches Go's R behaviour).
        if let Some(buf) = app.logs.get_mut(n) {
            buf.lines.clear();
            buf.scroll = 0;
            buf.at_bottom = true;
        }
        if app.manager.restart(n).is_ok() {
            ok += 1;
        } else {
            err += 1;
        }
    }
    app.flash(&format!("restarted {ok} target(s), {err} error(s)"));
}

fn action_dump<H: ManagerHandle>(app: &mut App<H>) {
    let Some(name) = app.selected_target_name() else {
        return;
    };
    let stamp = Utc::now().format("%Y%m%d-%H%M%S");
    let dest = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join(format!("{name}-{stamp}.log"));
    match app.manager.dump_log(&name, &dest) {
        Ok(_) => app.flash(&format!("dumped to {}", dest.display())),
        Err(e) => app.flash(&format!("dump {name}: {e}")),
    }
}

fn action_clear<H: ManagerHandle>(app: &mut App<H>) {
    let Some(name) = app.selected_target_name() else {
        return;
    };
    if let Some(buf) = app.logs.get_mut(&name) {
        buf.lines.clear();
        buf.scroll = 0;
        buf.at_bottom = true;
    }
    // Match indices point into the buffer we just emptied; drop them
    // so any subsequent jump doesn't land on a nonexistent line.
    app.search_matches.clear();
    app.search_match_idx = 0;
    let msg = match app.manager.clear_log(&name) {
        Ok(_) => format!("cleared {name} logs"),
        Err(e) => format!("clear {name}: {e}"),
    };
    app.flash(&msg);
}

fn action_describe<H: ManagerHandle>(app: &mut App<H>) {
    let Some(name) = app.selected_target_name() else {
        return;
    };
    let body = app.manager.describe(&name);
    if body.is_empty() {
        app.flash(&format!("no description for {name}"));
    } else {
        app.describe = Some(body);
    }
}

fn action_edit<H: ManagerHandle>(app: &mut App<H>) {
    let Some(t) = app.selected_target() else {
        return;
    };
    let source = t.source_file.clone();
    if source.is_empty() {
        app.flash("no source file for this target");
        return;
    }
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
    // Leave alt screen / raw mode, run editor synchronously, restore.
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);

    let parts = shell_words::split(&editor).unwrap_or_else(|_| vec![editor.clone()]);
    let mut cmd = std::process::Command::new(&parts[0]);
    if parts.len() > 1 {
        cmd.args(&parts[1..]);
    }
    cmd.arg(&source);
    let status = cmd.status();

    let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen);
    let _ = crossterm::terminal::enable_raw_mode();

    match status {
        Ok(_) => app.flash(&format!("edited {source}")),
        Err(e) => app.flash(&format!("editor: {e}")),
    }
}
