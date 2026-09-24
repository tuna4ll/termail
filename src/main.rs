mod widgets;

use widgets::workspace::Workspace;

use crossterm::event::{
    Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use futures::{FutureExt, StreamExt};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout},
    style::Stylize,
    widgets::{Block, Borders, List, ListItem},
};

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let terminal = ratatui::init();
    let result = App::new().run(terminal).await;

    ratatui::restore();

    result
}

pub struct App {
    running: bool,
    event_stream: EventStream,
    selected: usize,
    mails: Vec<String>,
    workspace: Workspace,
}

impl Default for App {
    fn default() -> Self {
        Self {
            running: false,
            event_stream: EventStream::new(),
            selected: 0,
            mails: vec![
                "welcome to mailtui".into(),
                "test mail".into(),
                "hello from ratatui".into(),
            ],
            workspace: Workspace::new(),
        }
    }
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn run(
        mut self,
        mut terminal: DefaultTerminal,
    ) -> color_eyre::Result<()> {
        self.running = true;

        while self.running {
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_events().await?;
        }

        Ok(())
    }

    fn draw(&mut self, frame: &mut Frame) {
        let areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(35),
                Constraint::Percentage(65),
            ])
            .split(frame.area());

        let items: Vec<ListItem> = self
            .mails
            .iter()
            .enumerate()
            .map(|(i, mail)| {
                if i == self.selected {
                    ListItem::new(format!("> {mail}")).bold()
                } else {
                    ListItem::new(format!("  {mail}"))
                }
            })
            .collect();

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" inbox "),
        );

        frame.render_widget(list, areas[0]);

        self.workspace.draw(frame, areas[1]);
    }

    async fn handle_events(&mut self) -> color_eyre::Result<()> {
        let event = self.event_stream.next().fuse().await;

        if let Some(Ok(Event::Key(key))) = event {
            if key.kind == KeyEventKind::Press {
                self.on_key_event(key);
            }
        }

        Ok(())
    }

    fn on_key_event(&mut self, key: KeyEvent) {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc | KeyCode::Char('q'))
            | (
                KeyModifiers::CONTROL,
                KeyCode::Char('c') | KeyCode::Char('C'),
            ) => {
                self.running = false;
            }

            (_, KeyCode::Down | KeyCode::Char('j')) => {
                if self.selected + 1 < self.mails.len() {
                    self.selected += 1;
                }
            }

            (_, KeyCode::Up | KeyCode::Char('k')) => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
            }

            _ => {}
        }
    }
}