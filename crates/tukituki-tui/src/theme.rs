//! Colors + style helpers — ratatui equivalents of `internal/tui/styles.go`.

use ratatui::style::{Color, Modifier, Style};

pub const SIDEBAR_WIDTH: u16 = 24;

pub fn header() -> Style {
    Style::default()
        .bg(Color::Rgb(0x26, 0x32, 0x38))
        .fg(Color::Rgb(0xEC, 0xEF, 0xF1))
        .add_modifier(Modifier::BOLD)
}

pub fn header_hint() -> Style {
    Style::default()
        .bg(Color::Rgb(0x26, 0x32, 0x38))
        .fg(Color::Rgb(0x54, 0x6E, 0x7A))
}

pub fn border() -> Style {
    Style::default().fg(Color::Rgb(0x37, 0x47, 0x4F))
}

pub fn selected() -> Style {
    Style::default()
        .bg(Color::Rgb(0x15, 0x65, 0xC0))
        .fg(Color::Rgb(0xFF, 0xFF, 0xFF))
        .add_modifier(Modifier::BOLD)
}

pub fn normal_item() -> Style {
    Style::default().fg(Color::Rgb(0xB0, 0xBE, 0xC5))
}

pub fn key_hint() -> Style {
    Style::default().fg(Color::Rgb(0x54, 0x6E, 0x7A))
}

pub fn status_msg() -> Style {
    Style::default()
        .fg(Color::Rgb(0x54, 0x6E, 0x7A))
        .add_modifier(Modifier::ITALIC)
}

pub fn right_panel_title() -> Style {
    Style::default()
        .fg(Color::Rgb(0xEC, 0xEF, 0xF1))
        .add_modifier(Modifier::BOLD)
}

pub fn icon_running() -> Style {
    Style::default().fg(Color::Rgb(0x00, 0xE6, 0x76))
}
pub fn icon_stopped() -> Style {
    Style::default().fg(Color::Rgb(0xFF, 0xD6, 0x00))
}
pub fn icon_failed() -> Style {
    Style::default().fg(Color::Rgb(0xFF, 0x17, 0x44))
}
pub fn icon_unknown() -> Style {
    Style::default().fg(Color::Rgb(0x78, 0x90, 0x9C))
}

pub fn status_icon(s: tukituki_state::Status) -> (&'static str, Style) {
    use tukituki_state::Status as S;
    match s {
        S::Running => ("●", icon_running()),
        S::Stopped => ("○", icon_stopped()),
        S::Failed => ("✗", icon_failed()),
        S::Unknown => ("?", icon_unknown()),
    }
}
