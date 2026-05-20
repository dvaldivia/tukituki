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
            // Drop mouse capture in zoom mode so the terminal's
            // native text-selection (drag-to-select, double-click,
            // etc.) works again — copying log output is the whole
            // reason to be in zoom mode. Re-enable when zooming out.
            // The view-side change (no border) lives in view.rs.
            //
            // IsTerminal guard: in tests we drive `App::handle`
            // synthetically without a real terminal, so emitting
            // crossterm escape sequences here would pollute test
            // stdout. Skip when we aren't attached to a TTY.
            use std::io::IsTerminal;
            if std::io::stdout().is_terminal() {
                use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
                let _ = if app.zoom_logs {
                    crossterm::execute!(std::io::stdout(), DisableMouseCapture)
                } else {
                    crossterm::execute!(std::io::stdout(), EnableMouseCapture)
                };
            }
            app.flash(if app.zoom_logs {
                "zoom on (mouse off, select-to-copy enabled)"
            } else {
                "zoom off"
            });
        }
        KeyCode::Char('D') => action_describe(app),
        KeyCode::Char('E') => action_edit(app),
        _ => {}
    }

    Continuation::cont()
}

fn move_selection<H: ManagerHandle>(app: &mut App<H>, delta: i32) {
    use crate::rows::is_selectable;
    if app.rows.is_empty() {
        return;
    }
    let len = app.rows.len() as i32;
    let step = if delta >= 0 { 1 } else { -1 };
    let mut cur = app.selected as i32;
    // Walk in `step` steps over rows, skipping non-selectable ones
    // (separators), advancing `delta` "selectable" hops. Clamp at
    // either end.
    let total_hops = delta.unsigned_abs() as i32;
    let mut hops_done = 0;
    while hops_done < total_hops {
        let next = cur + step;
        if next < 0 || next >= len {
            break;
        }
        cur = next;
        if is_selectable(&app.rows[cur as usize]) {
            hops_done += 1;
        }
    }
    // If we ended up parked on an unselectable row (shouldn't happen
    // for the typical Up/Down case but is possible if we hit the
    // edge), scan one more step in the same direction to find a
    // selectable row; if nothing's there, scan the other way.
    if !is_selectable(&app.rows[cur as usize]) {
        let nudge = |start: i32, dir: i32| -> Option<i32> {
            let mut x = start;
            while x >= 0 && x < len {
                if is_selectable(&app.rows[x as usize]) {
                    return Some(x);
                }
                x += dir;
            }
            None
        };
        cur = nudge(cur + step, step)
            .or_else(|| nudge(cur - step, -step))
            .unwrap_or(cur);
    }
    let new = cur as usize;
    if new != app.selected {
        if app.search_mode {
            app.reset_search();
        }
        app.selected = new;
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
    let name_for_work = name.clone();
    app.spawn_op(format!("starting {name}"), move |mgr| {
        match mgr.start(&name_for_work) {
            Ok(_) => format!("started {name_for_work}"),
            Err(e) => format!("start {name_for_work}: {e}"),
        }
    });
}

fn action_stop<H: ManagerHandle>(app: &mut App<H>) {
    let Some(name) = app.selected_target_name() else {
        return;
    };
    let name_for_work = name.clone();
    app.spawn_op(format!("stopping {name}"), move |mgr| {
        match mgr.stop(&name_for_work) {
            Ok(_) => format!("stopped {name_for_work}"),
            Err(e) => format!("stop {name_for_work}: {e}"),
        }
    });
}

fn action_restart<H: ManagerHandle>(app: &mut App<H>) {
    let Some(name) = app.selected_target_name() else {
        return;
    };
    let name_for_work = name.clone();
    app.spawn_op(format!("restarting {name}"), move |mgr| {
        match mgr.restart(&name_for_work) {
            Ok(_) => format!("restarted {name_for_work}"),
            Err(e) => format!("restart {name_for_work}: {e}"),
        }
    });
}

fn action_restart_all<H: ManagerHandle>(app: &mut App<H>) {
    // Bulk restart honours `autorun: false`: a manual-only target is
    // restarted only if it's already running (the user started it on
    // purpose), otherwise it's left alone.
    let names: Vec<String> = app
        .targets
        .iter()
        .filter(|t| {
            t.autorun
                || app.statuses.get(&t.name).copied() == Some(tukituki_state::Status::Running)
        })
        .map(|t| t.name.clone())
        .collect();

    // Clear the per-target log buffers from the main thread — worker
    // threads can't touch App state, and clearing here gives the
    // restart a fresh visible scrollback right away (matches Go's R).
    for n in &names {
        if let Some(buf) = app.logs.get_mut(n) {
            buf.clear();
        }
    }

    let label = format!("restarting {} target(s)", names.len());
    app.spawn_op(label, move |mgr| {
        // Two-phase: stop+cleanup every target before starting any. A
        // naive per-target restart loop interleaves stop+start, so a
        // later target's cleanup command (e.g. a shared `pkill -f
        // node`) can kill an earlier target right after it just came
        // up.
        for n in &names {
            let _ = mgr.stop(n);
        }
        let mut ok = 0usize;
        let mut err = 0usize;
        for n in &names {
            if mgr.start(n).is_ok() {
                ok += 1;
            } else {
                err += 1;
            }
        }
        format!("restarted {ok} target(s), {err} error(s)")
    });
}

fn action_dump<H: ManagerHandle>(app: &mut App<H>) {
    let Some(name) = app.selected_target_name() else {
        return;
    };
    let stamp = Utc::now().format("%Y%m%d-%H%M%S");
    let dest = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join(format!("{name}-{stamp}.log"));
    let name_for_work = name.clone();
    let dest_for_work = dest.clone();
    app.spawn_op(format!("dumping {name}"), move |mgr| {
        match mgr.dump_log(&name_for_work, &dest_for_work) {
            Ok(_) => format!("dumped to {}", dest_for_work.display()),
            Err(e) => format!("dump {name_for_work}: {e}"),
        }
    });
}

fn action_clear<H: ManagerHandle>(app: &mut App<H>) {
    let Some(name) = app.selected_target_name() else {
        return;
    };
    if let Some(buf) = app.logs.get_mut(&name) {
        buf.clear();
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

    // Tell the input reader thread to stop consuming stdin before we
    // tear down raw mode — otherwise it and the editor child race for
    // every keystroke on the same PTY fd and the editor feels laggy.
    // Also drop mouse capture: `LeaveAlternateScreen` doesn't undo it
    // and the editor would otherwise see CSI mouse sequences as input.
    app.set_input_paused(true);
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
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
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);
    app.set_input_paused(false);
    // EnterAlternateScreen lands us on a freshly-cleared alt screen,
    // but ratatui still thinks its pre-editor frame is on the wire.
    // Without this nudge the next draw would only emit diffs and the
    // rest of the UI would stay blank.
    app.mark_full_redraw();

    match status {
        Ok(_) => app.flash(&format!("edited {source}")),
        Err(e) => app.flash(&format!("editor: {e}")),
    }
}
