use rataflow::{Palette, Theme};
use ratatui::style::{Color, Modifier, Style};

pub const CANVAS: Color = Color::Indexed(234);
pub const SURFACE: Color = Color::Indexed(236);
pub const MUTED: Color = Color::Indexed(245);
pub const SUBTLE: Color = Color::Indexed(238);
pub const ACCENT: Color = Color::Indexed(81);
pub const TEXT: Color = Color::Indexed(252);

pub const fn flow() -> Theme {
    Theme::Custom(Palette {
        canvas_bg: CANVAS,
        surface: SURFACE,
        muted: MUTED,
        subtle: SUBTLE,
        accent: ACCENT,
        text: TEXT,
        success: Color::Indexed(78),
        error: Color::Indexed(203),
    })
}

pub fn border(active: bool) -> Style {
    Style::default().fg(if active { ACCENT } else { SUBTLE })
}

pub fn title(active: bool) -> Style {
    let style = Style::default().fg(if active { ACCENT } else { MUTED });
    if active {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

pub fn text() -> Style {
    Style::default().fg(TEXT)
}

pub fn muted() -> Style {
    Style::default().fg(MUTED)
}

pub fn selection() -> Style {
    Style::default()
        .fg(TEXT)
        .bg(SUBTLE)
        .add_modifier(Modifier::BOLD)
}
