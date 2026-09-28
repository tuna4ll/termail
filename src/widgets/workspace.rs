use crate::{mail::Mail, theme, widgets::mail_card::MailCard};
use crossterm::event::{KeyCode, MouseEvent};
use rataflow::{Edge, Flow, FlowEvent, Node, StepEdge};
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders},
};

const CARD_WIDTH: usize = 40;
const CARD_HEIGHT: usize = 9;
const CARD_GAP: usize = 4;

pub struct Workspace {
    flow: Flow<MailCard, StepEdge>,
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
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme::border(focused))
            .title_style(theme::title(focused))
            .title(title);
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

fn conversation_flow(mails: &[Mail], selected_mail_id: &str) -> Flow<MailCard, StepEdge> {
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
            Node::new(
                &mail.id,
                (4.0 + offset as f64 * (CARD_WIDTH + CARD_GAP) as f64, 3.0),
                (CARD_WIDTH as f64, CARD_HEIGHT as f64),
                MailCard::new(mail, index + 1, conversation.len()),
            )
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

    Flow::with_graph(nodes, edges)
        .unwrap()
        .with_theme(theme::flow())
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

    use super::conversation_flow;

    #[test]
    fn places_selected_mail_at_the_leading_edge() {
        let flow = conversation_flow(&demo_mails(), "project-2");

        assert_eq!(flow.node("project-2").unwrap().position.x, 4.0);
        assert!(flow.node("project-1").unwrap().position.x < 0.0);
    }
}
