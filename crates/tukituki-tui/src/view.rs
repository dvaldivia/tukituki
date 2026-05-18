//! ratatui draw fns.
//!
//! Single entry point `render(frame, app)`. Layout: a single header
//! line + a horizontal split (sidebar | log pane). Help and Describe
//! overlays paint over the right pane when active.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use tukituki_state::Status;

use crate::app::App;
use crate::handle::ManagerHandle;
use crate::rows::Row;
use crate::theme;

pub fn render<H: ManagerHandle>(f: &mut Frame, app: &App<H>) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);
    let header_area = chunks[0];
    let body_area = chunks[1];

    render_header(f, header_area, app);

    if app.zoom_logs {
        render_log_pane(f, body_area, app);
    } else {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(theme::SIDEBAR_WIDTH),
                Constraint::Min(20),
            ])
            .split(body_area);
        render_sidebar(f, body[0], app);
        render_log_pane(f, body[1], app);
    }

    if app.help_visible {
        render_help_overlay(f, body_area);
    }
    if let Some(text) = &app.describe {
        render_text_overlay(f, body_area, "Describe", text);
    }
}

fn render_header<H: ManagerHandle>(f: &mut Frame, area: Rect, app: &App<H>) {
    let title = match app.selected_target() {
        Some(t) => format!(" tukituki — {} ", t.name),
        None => " tukituki ".to_string(),
    };
    let msg = if app.status_msg.is_empty() {
        " ? help  q detach  Q stop all & exit ".to_string()
    } else {
        format!(" {} ", app.status_msg)
    };
    let line = Line::from(vec![
        Span::styled(title, theme::header()),
        Span::raw(" "),
        Span::styled(msg, theme::header_hint()),
    ]);
    f.render_widget(Paragraph::new(line).style(theme::header()), area);
}

fn render_sidebar<H: ManagerHandle>(f: &mut Frame, area: Rect, app: &App<H>) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border());
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Reserve the bottom 5 lines for the key-binding legend.
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(5)])
        .split(inner);
    let rows_area = split[0];
    let hints_area = split[1];

    // Build target row lines.
    let mut lines: Vec<Line> = Vec::with_capacity(app.rows.len());
    for (i, r) in app.rows.iter().enumerate() {
        let is_sel = i == app.selected;
        lines.push(render_row(r, app, is_sel));
    }
    let widget = Paragraph::new(lines).style(theme::normal_item());
    f.render_widget(widget, rows_area);

    let hints = Paragraph::new(vec![
        Line::from(" r restart  s stop"),
        Line::from(" S start    d dump"),
        Line::from(" c clear    e edit"),
        Line::from(" q detach   Q exit"),
        Line::from(" ? help"),
    ])
    .style(theme::key_hint());
    f.render_widget(hints, hints_area);
}

fn render_row<H: ManagerHandle>(r: &Row, app: &App<H>, is_sel: bool) -> Line<'static> {
    match r {
        Row::Folder {
            group,
            expanded,
            count,
        } => {
            let arrow = if *expanded { "▼" } else { "▶" };
            let s = format!("{arrow} {group} ({count})");
            let mut style = theme::normal_item().add_modifier(Modifier::BOLD);
            if is_sel {
                style = theme::selected();
            }
            Line::from(Span::styled(
                pad_to(theme::SIDEBAR_WIDTH as usize - 2, &s),
                style,
            ))
        }
        Row::Target { target_idx, group } => {
            let Some(t) = app.targets.get(*target_idx) else {
                return Line::from("");
            };
            let status = app
                .statuses
                .get(&t.name)
                .copied()
                .unwrap_or(Status::Unknown);
            let (icon, icon_style) = theme::status_icon(status);
            let indent = if group.is_empty() { "" } else { "  " };
            let mut label_style = theme::normal_item();
            if is_sel {
                label_style = theme::selected();
            }
            let label = format!("{indent}{} ", t.name);
            let padded = pad_to(theme::SIDEBAR_WIDTH as usize - 4, &label);
            Line::from(vec![
                Span::raw(" "),
                Span::styled(icon.to_string(), icon_style),
                Span::raw(" "),
                Span::styled(padded, label_style),
            ])
        }
        Row::Separator { label } => {
            // Dim divider above the virtual-targets cluster — mirrors
            // the Go TUI's `  ─ collectors ─` line. Never selected
            // (move_selection skips separators), but render with the
            // selected style as a defensive fallback.
            let text = format!("  {label}");
            let style = if is_sel {
                theme::selected()
            } else {
                theme::key_hint()
            };
            Line::from(Span::styled(
                pad_to(theme::SIDEBAR_WIDTH as usize - 2, &text),
                style,
            ))
        }
    }
}

fn render_log_pane<H: ManagerHandle>(f: &mut Frame, area: Rect, app: &App<H>) {
    // Zoom mode hides the panel border and the title row so the user's
    // text selection covers nothing but real log content — copies
    // cleanly into the clipboard. The 'z' key handler also disables
    // mouse capture so the terminal owns selection.
    let inner = if app.zoom_logs {
        area
    } else {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme::border());
        let inner = block.inner(area);
        f.render_widget(block, area);
        inner
    };

    // Layout: title row (skipped in zoom) + log body + optional search bar.
    let mut constraints: Vec<Constraint> = Vec::new();
    if !app.zoom_logs {
        constraints.push(Constraint::Length(1)); // title
    }
    constraints.push(Constraint::Min(1)); // body
    if app.search_mode {
        constraints.push(Constraint::Length(1)); // search bar
    }
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);
    let (title_area, body_area, search_idx) = if app.zoom_logs {
        (None, split[0], 1usize)
    } else {
        (Some(split[0]), split[1], 2usize)
    };

    if let Some(title_area) = title_area {
        let title = match app.selected_target() {
            Some(t) => format!(" {} ", t.name),
            None => " (no target selected) ".into(),
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(title, theme::right_panel_title()))),
            title_area,
        );
    }

    let lines: Text<'static> = match app.selected_target_name().and_then(|n| app.logs.get(&n)) {
        Some(buf) => {
            let height = body_area.height as usize;
            let total = buf.lines.len();
            let end = total.saturating_sub(buf.scroll);
            let start = end.saturating_sub(height.max(1));

            if app.search_mode && !app.search_query.is_empty() {
                // Span-by-span rendering so search hits get a background
                // colour without losing ANSI fidelity on the rest of
                // the line. The line containing the active match is
                // tinted with the accent colour.
                let current_line_idx = app.search_matches.get(app.search_match_idx).copied();
                let mut out_lines: Vec<Line<'static>> = Vec::with_capacity(end - start);
                for (i, line) in buf.lines.iter().enumerate().skip(start).take(end - start) {
                    let is_current = current_line_idx == Some(i);
                    out_lines.push(highlight_line(line, &app.search_query, is_current));
                }
                Text::from(out_lines)
            } else {
                // Slice the pre-parsed Lines for the visible window.
                // ANSI escapes were parsed once when each line was
                // appended; here we just clone the resulting Spans.
                // This is the hot path on every frame — keeping it
                // free of `into_text()` calls is what eliminates the
                // 60fps × multi-millisecond parse tax on chatty
                // backends with structured (color-coded) logs.
                let out_lines: Vec<Line<'static>> = buf
                    .parsed
                    .iter()
                    .skip(start)
                    .take(end - start)
                    .cloned()
                    .collect();
                Text::from(out_lines)
            }
        }
        None => Text::raw(""),
    };

    let mut p = Paragraph::new(lines).style(Style::default());
    if app.wrap_logs {
        p = p.wrap(Wrap { trim: false });
    }
    f.render_widget(p, body_area);

    if app.search_mode {
        // `search_idx` was computed above and accounts for the
        // missing title row in zoom mode.
        let bar_area = split[search_idx];
        let count = if app.search_matches.is_empty() {
            if app.search_query.is_empty() {
                "0".to_string()
            } else {
                "no matches".to_string()
            }
        } else {
            format!("{}/{}", app.search_match_idx + 1, app.search_matches.len())
        };
        let bar = Line::from(Span::styled(
            format!(
                " /{}  [{count}]  (Enter=next, Esc=close) ",
                app.search_query
            ),
            theme::search_bar(),
        ));
        f.render_widget(Paragraph::new(bar), bar_area);
    }
}

/// Wrap every case-insensitive occurrence of `query` in `line` with
/// the appropriate highlight style. The current-match line uses the
/// accent colour; other matched lines use the regular highlight.
fn highlight_line(line: &str, query: &str, is_current: bool) -> Line<'static> {
    if query.is_empty() {
        return Line::from(line.to_string());
    }
    let lower_line = line.to_lowercase();
    let lower_q = query.to_lowercase();
    let qlen = lower_q.len();

    let style = if is_current {
        theme::search_current_match()
    } else {
        theme::search_match()
    };

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut offset = 0usize;
    while offset < line.len() {
        match lower_line[offset..].find(&lower_q) {
            None => {
                spans.push(Span::raw(line[offset..].to_string()));
                break;
            }
            Some(rel) => {
                let abs = offset + rel;
                if abs > offset {
                    spans.push(Span::raw(line[offset..abs].to_string()));
                }
                // Original-case slice for the highlighted span so users
                // see the actual log content, not a lowercased copy.
                let end = abs + qlen;
                spans.push(Span::styled(line[abs..end].to_string(), style));
                offset = end;
            }
        }
    }
    Line::from(spans)
}

fn render_help_overlay(f: &mut Frame, area: Rect) {
    let body = vec![
        Line::from(""),
        Line::from(" Navigation:"),
        Line::from("   ↑/↓ or j/k     move selection"),
        Line::from("   Tab            cycle to next row"),
        Line::from("   →/l            expand folder"),
        Line::from("   ←/h            collapse folder"),
        Line::from("   Enter / Space  toggle folder"),
        Line::from(""),
        Line::from(" Process actions:"),
        Line::from("   S              start selected"),
        Line::from("   s              stop selected"),
        Line::from("   r              restart selected"),
        Line::from("   R              restart all (clears logs)"),
        Line::from("   d              dump logs to file"),
        Line::from("   c              clear logs"),
        Line::from("   E              edit run file in $EDITOR"),
        Line::from("   D              describe launch"),
        Line::from(""),
        Line::from(" Log viewer:"),
        Line::from("   PgUp / b       scroll up"),
        Line::from("   PgDn / f       scroll down"),
        Line::from("   w              toggle line wrap"),
        Line::from("   z              zoom (full-width logs)"),
        Line::from("   /              search logs (Enter cycles next, Esc closes)"),
        Line::from(""),
        Line::from(" Exit:"),
        Line::from("   q              detach (leave procs running)"),
        Line::from("   Q / Ctrl+C     stop all + exit"),
        Line::from(""),
        Line::from(" Press ? or Esc to dismiss."),
    ];
    render_text_overlay_lines(f, area, "Help", body);
}

fn render_text_overlay(f: &mut Frame, area: Rect, title: &str, body: &str) {
    let lines: Vec<Line> = body.lines().map(|l| Line::from(l.to_string())).collect();
    render_text_overlay_lines(f, area, title, lines);
}

fn render_text_overlay_lines(f: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>) {
    let w = area.width.saturating_sub(4).max(20);
    let h = area.height.saturating_sub(2).max(5);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let overlay = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    f.render_widget(Clear, overlay);
    let block = Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_style(theme::border());
    let inner = block.inner(overlay);
    f.render_widget(block, overlay);
    f.render_widget(
        Paragraph::new(lines).style(theme::status_msg().remove_modifier(Modifier::ITALIC)),
        inner,
    );
}

fn pad_to(width: usize, s: &str) -> String {
    if s.chars().count() >= width {
        s.chars().take(width).collect()
    } else {
        let mut out = s.to_string();
        for _ in 0..(width - s.chars().count()) {
            out.push(' ');
        }
        out
    }
}
