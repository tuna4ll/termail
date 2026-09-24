use crossterm::event::{
    Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use futures::{FutureExt, StreamExt};
use ratatui::{
    DefaultTerminal, Frame,
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

#[derive(Debug, Default)]
pub struct App {
    running: bool,
    event_stream: EventStream,
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
        let mails = vec![
            ListItem::new("> welcome to mailtui").bold(),
            ListItem::new("  test mail"),
            ListItem::new("  hello from ratatui"),
        ];

        let list = List::new(mails).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" inbox "),
        );

        frame.render_widget(list, frame.area());
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

            _ => {}
        }
    }
}