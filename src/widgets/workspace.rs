use crossterm::event::MouseEvent;
use rataflow::{Edge, Flow, Node, StepEdge, TextContent};
use ratatui::{Frame, layout::Rect};

pub struct Workspace {
    flow: Flow<TextContent, StepEdge>,
}

impl Workspace {
    pub fn new() -> Self {
        let nodes = vec![
            Node::from_text(
                "mail-1",
                (10.0, 5.0),
                "hello from mailtui",
            ),
        ];

        let edges: Vec<Edge<StepEdge>> = vec![];

        let flow = Flow::with_graph(nodes, edges).unwrap();

        Self { flow }
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        frame.render_widget(&mut self.flow, area);
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        let _ = self.flow.handle_mouse_event(mouse);
    }
}