mod mail;
mod widgets;

use mail::{Mail, demo_mails};
use widgets::reader::Reader;
use widgets::workspace::Workspace;

use std::io::stdout;

use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEvent,
        KeyEventKind, KeyModifiers,
    },
    execute,
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

    execute!(stdout(), EnableMouseCapture)?;

    let result = App::new().run(terminal).await;

    execute!(stdout(), DisableMouseCapture)?;

    ratatui::restore();

    result
}

pub struct App {
    running: bool,
    event_stream: EventStream,
    selected: usize,
    mails: Vec<Mail>,
    workspace: Workspace,
    reader: Option<Reader>,
    focus: Pane,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Pane {
    Inbox,
    Workspace,
}

impl Default for App {
    fn default() -> Self {
        let mails = demo_mails();
        let workspace = Workspace::new(&mails, &mails[0].id);

        Self {
            running: false,
            event_stream: EventStream::new(),
            selected: 0,
            mails,
            workspace,
            reader: None,
            focus: Pane::Inbox,
        }
    }
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn run(mut self, mut terminal: DefaultTerminal) -> color_eyre::Result<()> {
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
            .constraints([Constraint::Length(30), Constraint::Min(0)])
            .split(frame.area());

        let items: Vec<ListItem> = self
            .mails
            .iter()
            .enumerate()
            .map(|(i, mail)| {
                if i == self.selected {
                    ListItem::new(format!("> {}", mail.subject)).bold()
                } else {
                    ListItem::new(format!("  {}", mail.subject))
                }
            })
            .collect();

        let title = if self.focus == Pane::Inbox {
            " inbox • active "
        } else {
            " inbox "
        };
        let list = List::new(items).block(Block::default().borders(Borders::ALL).title(title));

        frame.render_widget(list, areas[0]);

        self.workspace
            .draw(frame, areas[1], self.focus == Pane::Workspace);

        if let Some(reader) = &mut self.reader
            && let Some(mail) = self.mails.iter().find(|mail| mail.id == reader.mail_id())
        {
            reader.draw(frame, frame.area(), mail);
        }
    }

    async fn handle_events(&mut self) -> color_eyre::Result<()> {
        let event = self.event_stream.next().fuse().await;

        if let Some(Ok(event)) = event {
            match event {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    self.on_key_event(key);
                }

                Event::Mouse(mouse) => {
                    self.workspace.handle_mouse(mouse);
                }

                _ => {}
            }
        }

        Ok(())
    }

    fn on_key_event(&mut self, key: KeyEvent) {
        if self.reader.is_some() {
            match key.code {
                KeyCode::Esc => self.reader = None,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.reader.as_mut().unwrap().scroll_down(1);
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.reader.as_mut().unwrap().scroll_up(1);
                }
                KeyCode::PageDown => self.reader.as_mut().unwrap().scroll_down(5),
                KeyCode::PageUp => self.reader.as_mut().unwrap().scroll_up(5),
                _ => {}
            }
            return;
        }

        match (key.modifiers, key.code) {
            (_, KeyCode::Esc | KeyCode::Char('q'))
            | (KeyModifiers::CONTROL, KeyCode::Char('c') | KeyCode::Char('C')) => {
                self.running = false;
            }

            (_, KeyCode::Tab) => {
                self.focus = match self.focus {
                    Pane::Inbox => Pane::Workspace,
                    Pane::Workspace => Pane::Inbox,
                };
            }

            (_, KeyCode::Down | KeyCode::Char('j'))
                if self.focus == Pane::Inbox && self.selected + 1 < self.mails.len() =>
            {
                self.selected += 1;
                self.sync_workspace();
            }

            (_, KeyCode::Up | KeyCode::Char('k'))
                if self.focus == Pane::Inbox && self.selected > 0 =>
            {
                self.selected -= 1;
                self.sync_workspace();
            }

            (_, KeyCode::Enter) => {
                self.reader = Some(Reader::new(self.mails[self.selected].id.clone()));
            }

            _ => {}
        }
    }

    fn sync_workspace(&mut self) {
        self.workspace
            .show_conversation(&self.mails, &self.mails[self.selected].id);
    }
}
