use crate::mail::Mail;
use crossterm::event::{KeyCode, MouseEvent};
use rataflow::{Edge, Flow, FlowEvent, Node, StepEdge, TextContent};
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders},
};

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

    pub fn draw(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let title = if focused {
            " workspace • active "
        } else {
            " workspace "
        };
        let block = Block::default().borders(Borders::ALL).title(title);
        let inner = block.inner(area);

        frame.render_widget(block, area);
        frame.render_widget(&mut self.flow, inner);
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> Option<String> {
        self.flow
            .handle_mouse_event(mouse)
            .into_events()
            .find_map(|event| match event {
                FlowEvent::NodeClicked { node_id } => Some(node_id),
                _ => None,
            })
    }

    pub fn handle_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h') | KeyCode::Char('k') => {
                self.flow.select_prev_node();
                self.flow.center_on_selected();
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('l') => {
                self.flow.select_next_node();
                self.flow.center_on_selected();
            }
            KeyCode::Char('+' | '=') => self.flow.zoom_in(),
            KeyCode::Char('-' | '_') => self.flow.zoom_out(),
            KeyCode::Char('0') => self.flow.reset_zoom(),
            KeyCode::Char('f') => self.flow.request_fit_view(),
            KeyCode::Char('c') => self.flow.center_on_selected(),
            _ => {}
        }
    }

    pub fn selected_mail_id(&self) -> Option<String> {
        self.flow.first_selected_node_id()
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
    let selected_index = conversation
        .iter()
        .position(|mail| mail.id == selected_mail_id)
        .unwrap_or_default();
    let nodes = conversation
        .iter()
        .enumerate()
        .map(|(index, mail)| {
            let offset = index as isize - selected_index as isize;
            Node::from_text(
                &mail.id,
                (4.0 + offset as f64 * (CARD_WIDTH + CARD_GAP) as f64, 3.0),
                mail_preview(mail),
            )
            .with_dimensions(CARD_WIDTH as f64, CARD_HEIGHT as f64)
            .with_selected(mail.id == selected_mail_id)
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
    use crate::mail::demo_mails;

    use super::{clip_line, conversation_flow, wrap_preview};

    #[test]
    fn clips_long_lines() {
        assert_eq!(clip_line("123456", 5), "1234…");
    }

    #[test]
    fn wraps_and_marks_hidden_text() {
        assert_eq!(wrap_preview("one two three four", 7, 2), "one two\nthree…");
    }

    #[test]
    fn places_selected_mail_at_the_leading_edge() {
        let flow = conversation_flow(&demo_mails(), "project-2");

        assert_eq!(flow.node("project-2").unwrap().position.x, 4.0);
        assert!(flow.node("project-1").unwrap().position.x < 0.0);
    }
}
