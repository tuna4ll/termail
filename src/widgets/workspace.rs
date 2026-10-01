use std::collections::HashSet;

use crate::{
    mail::{Mail, parent_id, root_id},
    theme,
    widgets::mail_card::MailCard,
};
use crossterm::event::{KeyCode, MouseEvent};
use rataflow::{
    Background, BackgroundVariant, Edge, EdgeStyle, FitViewOptions, Flow, FlowEvent,
    HandlePosition, MiniMap, MiniMapPosition, Node, SelectionReveal, StepEdge, Sugiyama,
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
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
        frame.render_widget(
            Background::new(&self.flow)
                .variant(BackgroundVariant::Dots)
                .gap(6, 3),
            inner,
        );
        frame.render_widget(&mut self.flow, inner);
        if self.needs_minimap(inner) {
            let map = MiniMap::new(&self.flow)
                .position(MiniMapPosition::BottomRight)
                .size(20, 7)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(theme::border(false))
                        .title_style(theme::muted())
                        .title(" map "),
                );
            frame.render_widget(map, inner);
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> Option<String> {
        let clicked =
            self.flow
                .handle_mouse_event(mouse)
                .into_events()
                .find_map(|event| match event {
                    FlowEvent::NodeClicked { node_id } => Some(node_id),
                    _ => None,
                });
        if let Some(mail_id) = &clicked {
            self.selected_mail_id = mail_id.clone();
            self.update_edge_styles();
        }
        clicked
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
            self.update_edge_styles();
        }
    }

    pub fn selected_mail_id(&self) -> Option<String> {
        self.flow.first_selected_node_id()
    }

    fn needs_minimap(&self, area: Rect) -> bool {
        if area.width < 48 || area.height < 12 {
            return false;
        }

        let bounds = self
            .flow
            .nodes()
            .map(Node::bounds)
            .reduce(|a, b| a.union(&b));
        let Some(bounds) = bounds else {
            return false;
        };
        let zoom = self.flow.viewport.zoom;

        bounds.width() * zoom > area.width.saturating_sub(4) as f64
            || bounds.height() * zoom > area.height.saturating_sub(4) as f64
    }

    fn update_edge_styles(&mut self) {
        let path = selected_path_ids(&self.mails, &self.selected_mail_id);
        for (edge_id, content) in self.flow.edges_content_mut() {
            let target_id = edge_id.strip_prefix("reply-").unwrap_or(edge_id);
            content.style = Some(reply_style(path.contains(target_id)));
        }
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
    let selected_path = selected_path_ids(mails, selected_mail_id);
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
            let parent_id = parent_id(mail, mails)?;
            conversation
                .iter()
                .any(|parent| parent.id == parent_id)
                .then(|| {
                    let edge = Edge::new(format!("reply-{}", mail.id), parent_id, &mail.id);
                    let edge = match layout {
                        FlowLayout::Horizontal => edge
                            .with_source_side(HandlePosition::Right)
                            .with_target_side(HandlePosition::Left),
                        FlowLayout::Vertical => edge
                            .with_source_side(HandlePosition::Bottom)
                            .with_target_side(HandlePosition::Top),
                    };
                    edge.with_content(
                        StepEdge::default()
                            .with_stem_length(2.0)
                            .with_style(reply_style(selected_path.contains(mail.id.as_str()))),
                    )
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

fn reply_style(active: bool) -> EdgeStyle {
    let style = Style::default().fg(if active { theme::ACCENT } else { theme::MUTED });
    let style = if active {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    };
    EdgeStyle::default().with_stroke_style(style)
}

fn selected_path_ids<'a>(mails: &'a [Mail], selected_mail_id: &str) -> HashSet<&'a str> {
    let mut ids = HashSet::new();
    let mut current = mails.iter().find(|mail| mail.id == selected_mail_id);

    while let Some(mail) = current {
        if !ids.insert(mail.id.as_str()) {
            break;
        }
        current = parent_id(mail, mails)
            .and_then(|parent_id| mails.iter().find(|candidate| candidate.id == parent_id));
    }

    ids
}

#[cfg(test)]
mod tests {
    use crate::mail::demo_mails;
    use ratatui::{Terminal, backend::TestBackend};

    use super::{FlowLayout, Workspace, conversation_flow, selected_path_ids};

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

    #[test]
    fn follows_the_selected_reply_path() {
        let mails = demo_mails();
        let path = selected_path_ids(&mails, "project-3");

        assert_eq!(path.len(), 3);
        assert!(path.contains("project-1"));
        assert!(path.contains("project-3"));
        assert!(!path.contains("weekend-1"));
    }

    #[test]
    fn renders_reference_branches() {
        let mut mails = demo_mails();
        mails.push(
            crate::mail::Mail::new(
                "branch",
                "mert@example.com",
                "re: project update",
                "ship it",
                None,
            )
            .with_references(&["project-1"]),
        );
        let flow = conversation_flow(&mails, "branch", FlowLayout::Horizontal);

        assert!(flow.edge("reply-branch").is_some());
        assert!(flow.node("project-1").is_some());
    }

    #[test]
    fn renders_mail_card_hierarchy() {
        let mails = demo_mails();
        let mut workspace = Workspace::new(&mails, "project-3");
        let backend = TestBackend::new(90, 28);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| workspace.draw(frame, frame.area(), true))
            .unwrap();
        terminal
            .draw(|frame| workspace.draw(frame, frame.area(), true))
            .unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(screen.contains("workspace"));
        assert!(screen.contains("project update"), "{screen}");
        assert!(screen.contains("ada@example.com"));
        assert!(screen.contains("↵ open"));
    }
}
