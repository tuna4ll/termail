use crate::mail::Mail;
use rataflow::{NodeContent, NodeRenderContext};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Paragraph, Widget, Wrap},
};

#[derive(Clone, Debug)]
pub struct MailCard {
    from: String,
    subject: String,
    body: String,
    date: Option<String>,
    unread: bool,
    starred: bool,
    attachment_count: usize,
    position: usize,
    total: usize,
}

impl MailCard {
    pub fn new(mail: &Mail, position: usize, total: usize) -> Self {
        Self {
            from: mail.from.clone(),
            subject: mail.subject.clone(),
            body: mail.body.clone(),
            date: mail.date.clone(),
            unread: mail.unread,
            starred: mail.starred,
            attachment_count: mail.attachment_count,
            position,
            total,
        }
    }
}

impl NodeContent for MailCard {
    fn render(&self, ctx: &NodeRenderContext, buf: &mut Buffer) {
        let palette = ctx.theme.palette();
        let border = if ctx.selected {
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.muted)
        };
        let mut block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border)
            .bg(palette.surface);
        if ctx.selected {
            block = block.title(Line::styled(" ● selected ", border));
        }
        let inner = block.inner(ctx.area);
        block.render(ctx.area, buf);

        if inner.height == 0 {
            return;
        }

        let markers = match (self.unread, self.starred) {
            (true, true) => "● ★ ",
            (true, false) => "● ",
            (false, true) => "★ ",
            (false, false) => "",
        };
        let subject = clip_line(&format!("{markers}{}", self.subject), inner.width as usize);
        Paragraph::new(subject)
            .style(
                Style::default()
                    .fg(palette.text)
                    .add_modifier(Modifier::BOLD),
            )
            .render(Rect::new(inner.x, inner.y, inner.width, 1), buf);

        if inner.height < 2 {
            return;
        }

        let sender = Line::from(vec![
            Span::styled("from ", Style::default().fg(palette.muted)),
            Span::styled(
                self.from.as_str(),
                Style::default()
                    .fg(palette.text)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        Paragraph::new(sender).render(Rect::new(inner.x, inner.y + 1, inner.width, 1), buf);

        if inner.height > 3 {
            Paragraph::new(Text::from(self.body.as_str()))
                .style(Style::default().fg(palette.text))
                .wrap(Wrap { trim: false })
                .render(
                    Rect::new(
                        inner.x,
                        inner.y + 3,
                        inner.width,
                        inner.height.saturating_sub(4),
                    ),
                    buf,
                );
        }

        if inner.height > 1 {
            let mut details = Vec::new();
            if let Some(date) = &self.date {
                details.push(date.chars().take(10).collect());
            }
            if self.attachment_count > 0 {
                details.push(format!("{} file", self.attachment_count));
            }
            let details = if details.is_empty() {
                String::new()
            } else {
                format!("{}  ", details.join(" · "))
            };
            let footer = format!("{details}{}/{}  ↵ open", self.position, self.total);
            let style = Style::default().fg(if ctx.selected {
                palette.accent
            } else {
                palette.muted
            });
            Paragraph::new(footer)
                .style(style)
                .render(Rect::new(inner.x, inner.bottom() - 1, inner.width, 1), buf);
        }
    }
}

fn clip_line(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.into();
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }

    let mut clipped: String = value.chars().take(width - 1).collect();
    clipped.push('…');
    clipped
}

#[cfg(test)]
mod tests {
    use super::clip_line;

    #[test]
    fn clips_card_titles() {
        assert_eq!(clip_line("123456", 5), "1234…");
        assert_eq!(clip_line("123", 5), "123");
    }
}
