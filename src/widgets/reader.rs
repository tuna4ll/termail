use crate::mail::Mail;
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

pub struct Reader {
    mail_id: String,
}

impl Reader {
    pub fn new(mail_id: String) -> Self {
        Self { mail_id }
    }

    pub fn mail_id(&self) -> &str {
        &self.mail_id
    }

    pub fn draw(&self, frame: &mut Frame, area: Rect, mail: &Mail) {
        let area = reader_area(area);
        let content = format!(
            "from: {}\nsubject: {}\n\n{}",
            mail.from, mail.subject, mail.body
        );
        let reader = Paragraph::new(content)
            .block(Block::default().borders(Borders::ALL).title(" message "))
            .wrap(Wrap { trim: false });

        frame.render_widget(Clear, area);
        frame.render_widget(reader, area);
    }
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
