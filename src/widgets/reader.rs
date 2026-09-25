use crate::mail::Mail;
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    widgets::{
        Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
};

pub struct Reader {
    mail_id: String,
    scroll: u16,
    max_scroll: u16,
}

impl Reader {
    pub fn new(mail_id: String) -> Self {
        Self {
            mail_id,
            scroll: 0,
            max_scroll: 0,
        }
    }

    pub fn mail_id(&self) -> &str {
        &self.mail_id
    }

    pub fn scroll_down(&mut self, amount: u16) {
        self.scroll = self.scroll.saturating_add(amount).min(self.max_scroll);
    }

    pub fn scroll_up(&mut self, amount: u16) {
        self.scroll = self.scroll.saturating_sub(amount);
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect, mail: &Mail) {
        let area = reader_area(area);
        let content = format!(
            "from: {}\nsubject: {}\n\n{}",
            mail.from, mail.subject, mail.body
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" message ")
            .title_bottom(" j/k scroll • esc close ");
        let reader = Paragraph::new(content.as_str()).wrap(Wrap { trim: false });
        let viewport_height = area.height.saturating_sub(2) as usize;
        let content_height = wrapped_line_count(&content, area.width.saturating_sub(2));
        self.max_scroll = content_height.saturating_sub(viewport_height) as u16;
        self.scroll = self.scroll.min(self.max_scroll);
        let reader = reader.block(block).scroll((self.scroll, 0));

        frame.render_widget(Clear, area);
        frame.render_widget(reader, area);

        if self.max_scroll > 0 {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None);
            let mut state = ScrollbarState::new(content_height)
                .position(self.scroll as usize)
                .viewport_content_length(viewport_height);
            frame.render_stateful_widget(
                scrollbar,
                area.inner(Margin {
                    vertical: 1,
                    horizontal: 0,
                }),
                &mut state,
            );
        }
    }
}

fn wrapped_line_count(value: &str, width: u16) -> usize {
    let width = width as usize;
    if width == 0 {
        return 0;
    }

    value
        .split('\n')
        .map(|line| {
            let mut count = 1;
            let mut used = 0;

            for word in line.split_whitespace() {
                let word_width = word.chars().count();
                if used == 0 {
                    count += word_width.saturating_sub(1) / width;
                    used = word_width % width;
                } else if used + 1 + word_width <= width {
                    used += 1 + word_width;
                } else {
                    count += 1 + word_width.saturating_sub(1) / width;
                    used = word_width % width;
                }
                if used == 0 {
                    used = width;
                }
            }

            count
        })
        .sum()
}

fn reader_area(area: Rect) -> Rect {
    let width = area.width.saturating_sub(4).min(76);
    let height = area.height.saturating_sub(4);

    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}
