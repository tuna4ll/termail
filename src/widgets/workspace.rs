use crate::mail::Mail;
use crossterm::event::MouseEvent;
use rataflow::{Edge, Flow, Node, StepEdge, TextContent};
use ratatui::{Frame, layout::Rect};

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
                (4.0 + index as f64 * 52.0, 3.0),
                format!(
                    "from: {}\nsubject: {}\n\n{}",
                    mail.from, mail.subject, mail.body
                ),
            )
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
