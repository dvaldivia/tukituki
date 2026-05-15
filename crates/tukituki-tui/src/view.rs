//! ratatui draw fns.
//!
//! Single entry point `render(frame, app)`. Layout: a single header
//! line + a horizontal split (sidebar | log pane). Help and Describe
//! overlays paint over the right pane when active.

use ansi_to_tui::IntoText;
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
    f.render_widget(Paragraph::new(line), area);
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
    }
}

fn render_log_pane<H: ManagerHandle>(f: &mut Frame, area: Rect, app: &App<H>) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border());
    let inner = block.inner(area);
    f.render_widget(block, area);

    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);
    let title_area = split[0];
    let body_area = split[1];

    let title = match app.selected_target() {
        Some(t) => format!(" {} ", t.name),
        None => " (no target selected) ".into(),
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(title, theme::right_panel_title()))),
        title_area,
    );

    // Pull the buffer and slice the visible window. The slice is the
    // *bottom* `body_area.height` lines minus the scroll offset.
    let lines: Text<'static> = match app.selected_target_name().and_then(|n| app.logs.get(&n)) {
        Some(buf) => {
            let height = body_area.height as usize;
            let total = buf.lines.len();
            // When buf.scroll == 0 the user is pinned at the bottom;
            // otherwise scroll counts how many lines from the bottom
            // we've scrolled up.
            let end = total.saturating_sub(buf.scroll);
            let start = end.saturating_sub(height.max(1));
            // Concatenate into a single ANSI-parsed Text so colours
            // survive into the render.
            let joined = buf
                .lines
                .iter()
                .skip(start)
                .take(end - start)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            joined.into_text().unwrap_or_else(|_| Text::raw(joined))
        }
        None => Text::raw(""),
    };

    let mut p = Paragraph::new(lines).style(Style::default());
    if app.wrap_logs {
        p = p.wrap(Wrap { trim: false });
    }
    f.render_widget(p, body_area);
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
