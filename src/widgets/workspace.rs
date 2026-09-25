use crate::mail::Mail;
use crossterm::event::MouseEvent;
use rataflow::{Edge, Flow, Node, StepEdge, TextContent};
use ratatui::{Frame, layout::Rect};

const CARD_WIDTH: usize = 42;
const CARD_HEIGHT: usize = 10;
const CARD_GAP: usize = 6;

pub struct Workspace {
    flow: Flow<TextContent, StepEdge>,
}

impl Workspace {
    pub fn new(mails: &[Mail], selected_mail_id: &str) -> Self {
        Self {
            flow: conversation_flow(mails, selected_mail_id),
        }
    }

    pub fn show_conversation(&mut self, mails: &[Mail], selected_mail_id: &str) {
        self.flow = conversation_flow(mails, selected_mail_id);
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        frame.render_widget(&mut self.flow, area);
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        let _ = self.flow.handle_mouse_event(mouse);
    }
}

fn conversation_flow(mails: &[Mail], selected_mail_id: &str) -> Flow<TextContent, StepEdge> {
    let Some(selected) = mails.iter().find(|mail| mail.id == selected_mail_id) else {
        return Flow::new();
    };
    let selected_root_id = root_id(selected, mails);
    let conversation: Vec<&Mail> = mails
        .iter()
        .filter(|mail| root_id(mail, mails) == selected_root_id)
        .collect();
    let nodes = conversation
        .iter()
        .enumerate()
        .map(|(index, mail)| {
            Node::from_text(
                &mail.id,
                (4.0 + index as f64 * (CARD_WIDTH + CARD_GAP) as f64, 3.0),
                mail_preview(mail),
            )
            .with_dimensions(CARD_WIDTH as f64, CARD_HEIGHT as f64)
        })
        .collect();
    let edges: Vec<Edge<StepEdge>> = conversation
        .iter()
        .filter_map(|mail| {
            let parent_id = mail.reply_to.as_deref()?;
            conversation
                .iter()
                .any(|parent| parent.id == parent_id)
                .then(|| Edge::new(format!("reply-{}", mail.id), parent_id, &mail.id))
        })
        .collect();

    Flow::with_graph(nodes, edges).unwrap()
}

fn mail_preview(mail: &Mail) -> String {
    let line_width = CARD_WIDTH - 2;
    let from = clip_line(&format!("from: {}", mail.from), line_width);
    let subject = clip_line(&format!("subject: {}", mail.subject), line_width);
    let body = wrap_preview(&mail.body, line_width, CARD_HEIGHT - 5);

    format!("{from}\n{subject}\n\n{body}")
}

fn clip_line(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.into();
    }

    let mut clipped: String = value.chars().take(width - 1).collect();
    clipped.push('…');
    clipped
}

fn wrap_preview(value: &str, width: usize, max_lines: usize) -> String {
    let words: Vec<&str> = value.split_whitespace().collect();
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut truncated = false;

    for word in words {
        if current.is_empty() {
            current = clip_line(word, width);
            truncated |= word.chars().count() > width;
            continue;
        }

        let next_width = current.chars().count() + 1 + word.chars().count();
        if next_width <= width {
            current.push(' ');
            current.push_str(word);
        } else if lines.len() + 1 < max_lines {
            lines.push(current);
            current = clip_line(word, width);
            truncated |= word.chars().count() > width;
        } else {
            truncated = true;
            break;
        }
    }

    if !current.is_empty() && lines.len() < max_lines {
        lines.push(current);
    }
    if truncated && let Some(last) = lines.last_mut() {
        *last = clip_line(&format!("{last}…"), width);
    }

    lines.join("\n")
}

fn root_id<'a>(mail: &'a Mail, mails: &'a [Mail]) -> &'a str {
    let mut current = mail;

    while let Some(parent_id) = &current.reply_to {
        let Some(parent) = mails.iter().find(|mail| mail.id == *parent_id) else {
            break;
        };
        current = parent;
    }

    &current.id
}

#[cfg(test)]
mod tests {
    use super::{clip_line, wrap_preview};

    #[test]
    fn clips_long_lines() {
        assert_eq!(clip_line("123456", 5), "1234…");
    }

    #[test]
    fn wraps_and_marks_hidden_text() {
        assert_eq!(wrap_preview("one two three four", 7, 2), "one two\nthree…");
    }
}
