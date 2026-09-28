use crate::{mail::Mail, theme, widgets::mail_card::MailCard};
use crossterm::event::{KeyCode, MouseEvent};
use rataflow::{
    Edge, FitViewOptions, Flow, FlowEvent, HandlePosition, Node, SelectionReveal, StepEdge,
    Sugiyama,
};
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders},
};

const CARD_WIDTH: usize = 40;
const CARD_HEIGHT: usize = 9;
const HORIZONTAL_LAYOUT_WIDTH: u16 = 104;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FlowLayout {
    Horizontal,
    Vertical,
}

pub struct Workspace {
    flow: Flow<MailCard, StepEdge>,
    mails: Vec<Mail>,
    selected_mail_id: String,
    layout: FlowLayout,
}

impl Workspace {
    pub fn new(mails: &[Mail], selected_mail_id: &str) -> Self {
        let layout = FlowLayout::Horizontal;
        Self {
            flow: conversation_flow(mails, selected_mail_id, layout),
            mails: mails.to_vec(),
            selected_mail_id: selected_mail_id.into(),
            layout,
        }
    }

    pub fn show_conversation(&mut self, mails: &[Mail], selected_mail_id: &str) {
        self.mails = mails.to_vec();
        self.selected_mail_id = selected_mail_id.into();
        self.flow = conversation_flow(&self.mails, &self.selected_mail_id, self.layout);
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
        let layout = if inner.width < HORIZONTAL_LAYOUT_WIDTH {
            FlowLayout::Vertical
        } else {
            FlowLayout::Horizontal
        };
        if layout != self.layout {
            self.layout = layout;
            self.flow = conversation_flow(&self.mails, &self.selected_mail_id, layout);
        }

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
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('l') => {
                self.flow.select_next_node();
            }
            KeyCode::Char('+' | '=') => self.flow.zoom_in(),
            KeyCode::Char('-' | '_') => self.flow.zoom_out(),
            KeyCode::Char('0') => self.flow.reset_zoom(),
            KeyCode::Char('f') => self.flow.request_fit_view(),
            KeyCode::Char('c') => self.flow.center_on_selected(),
            _ => {}
        }
        if let Some(mail_id) = self.flow.first_selected_node_id() {
            self.selected_mail_id = mail_id;
        }
    }

    pub fn selected_mail_id(&self) -> Option<String> {
        self.flow.first_selected_node_id()
    }
}

fn conversation_flow(
    mails: &[Mail],
    selected_mail_id: &str,
    layout: FlowLayout,
) -> Flow<MailCard, StepEdge> {
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
            Node::new(
                &mail.id,
                (0.0, 0.0),
                (CARD_WIDTH as f64, CARD_HEIGHT as f64),
                MailCard::new(mail, index + 1, conversation.len()),
            )
            .with_source_position(match layout {
                FlowLayout::Horizontal => HandlePosition::Right,
                FlowLayout::Vertical => HandlePosition::Bottom,
            })
            .with_target_position(match layout {
                FlowLayout::Horizontal => HandlePosition::Left,
                FlowLayout::Vertical => HandlePosition::Top,
            })
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
                .then(|| {
                    let edge = Edge::new(format!("reply-{}", mail.id), parent_id, &mail.id);
                    match layout {
                        FlowLayout::Horizontal => edge
                            .with_source_side(HandlePosition::Right)
                            .with_target_side(HandlePosition::Left),
                        FlowLayout::Vertical => edge
                            .with_source_side(HandlePosition::Bottom)
                            .with_target_side(HandlePosition::Top),
                    }
                })
        })
        .collect();

    let mut flow = Flow::with_graph(nodes, edges)
        .unwrap()
        .with_theme(theme::flow())
        .with_selection_reveal(SelectionReveal::EnsureVisible)
        .with_min_zoom(0.8)
        .with_max_zoom(1.25);
    let sugiyama = match layout {
        FlowLayout::Horizontal => Sugiyama::horizontal(),
        FlowLayout::Vertical => Sugiyama::vertical(),
    }
    .with_node_spacing(3.0)
    .with_rank_spacing(4.0)
    .with_margin(3.0);
    flow.apply_layout(sugiyama);
    flow.request_fit_view_with_options(
        FitViewOptions::default()
            .with_padding(2.0)
            .with_zoom_range(0.8, 1.0),
    );
    flow
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

    use super::{FlowLayout, conversation_flow};

    #[test]
    fn lays_out_conversations_horizontally() {
        let flow = conversation_flow(&demo_mails(), "project-2", FlowLayout::Horizontal);

        assert!(
            flow.node("project-1").unwrap().position.x < flow.node("project-2").unwrap().position.x
        );
        assert!(flow.node("project-2").unwrap().selected);
    }

    #[test]
    fn lays_out_conversations_vertically() {
        let flow = conversation_flow(&demo_mails(), "project-2", FlowLayout::Vertical);

        assert!(
            flow.node("project-1").unwrap().position.y < flow.node("project-2").unwrap().position.y
        );
    }
}
