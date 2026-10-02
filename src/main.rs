mod mail;
mod mailbox;
mod theme;
mod widgets;

use mail::{Conversation, Mail, group_conversations};
use mailbox::{LoadError, LoadReport, MailboxSource};
use widgets::reader::Reader;
use widgets::workspace::Workspace;

use std::{ffi::OsString, io::stdout, path::PathBuf, time::Duration};

use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEvent,
        KeyEventKind, KeyModifiers,
    },
    execute,
};
use futures::StreamExt;
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, HighlightSpacing, List, ListItem, ListState, Paragraph, Wrap},
};
use ratatui_spinner::FluxSpinner;
use tokio::{sync::mpsc, time::interval};

const MIN_SPLIT_WIDTH: u16 = 78;
const MIN_DOCK_WIDTH: u16 = 112;
const INBOX_WIDTH: u16 = 28;
const READER_WIDTH: u16 = 44;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let Some(source) = parse_source(std::env::args_os().skip(1))? else {
        print_help();
        return Ok(());
    };

    let terminal = ratatui::init();

    execute!(stdout(), EnableMouseCapture)?;

    let result = App::new(source).run(terminal).await;

    execute!(stdout(), DisableMouseCapture)?;

    ratatui::restore();

    result
}

pub struct App {
    running: bool,
    event_stream: EventStream,
    selected: usize,
    mails: Vec<Mail>,
    conversations: Vec<Conversation>,
    workspace: Option<Workspace>,
    reader: Option<Reader>,
    focus: Pane,
    source: MailboxSource,
    mailbox_state: MailboxState,
    load_tx: mpsc::UnboundedSender<Result<LoadReport, LoadError>>,
    load_rx: mpsc::UnboundedReceiver<Result<LoadReport, LoadError>>,
    spinner_tick: u64,
    skipped: usize,
}

#[derive(Debug)]
enum MailboxState {
    Loading,
    Ready,
    Empty,
    Error(String),
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Pane {
    Inbox,
    Workspace,
}

impl App {
    pub fn new(source: MailboxSource) -> Self {
        let (load_tx, load_rx) = mpsc::unbounded_channel();
        let mut app = Self {
            running: false,
            event_stream: EventStream::new(),
            selected: 0,
            mails: Vec::new(),
            conversations: Vec::new(),
            workspace: None,
            reader: None,
            focus: Pane::Inbox,
            source,
            mailbox_state: MailboxState::Loading,
            load_tx,
            load_rx,
            spinner_tick: 0,
            skipped: 0,
        };
        app.reload();
        app
    }

    pub async fn run(mut self, mut terminal: DefaultTerminal) -> color_eyre::Result<()> {
        self.running = true;
        let mut animation = interval(Duration::from_millis(80));

        while self.running {
            terminal.draw(|frame| self.draw(frame))?;
            tokio::select! {
                _ = animation.tick() => {
                    if matches!(self.mailbox_state, MailboxState::Loading) {
                        self.spinner_tick = self.spinner_tick.wrapping_add(1);
                    }
                }
                result = self.load_rx.recv() => {
                    if let Some(result) = result {
                        self.apply_load(result);
                    }
                }
                event = self.event_stream.next() => {
                    if let Some(Ok(event)) = event {
                        self.handle_event(event);
                    }
                }
            }
        }

        Ok(())
    }

    fn draw(&mut self, frame: &mut Frame) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(frame.area());

        if !matches!(self.mailbox_state, MailboxState::Ready) {
            self.draw_mailbox_state(frame, rows[0]);
            frame.render_widget(
                Paragraph::new(status_line(&[("r", "retry"), ("q", "quit")])),
                rows[1],
            );
            return;
        }

        let items: Vec<ListItem> = self
            .conversations
            .iter()
            .map(|conversation| {
                let last_sender = conversation.last_sender(&self.mails).unwrap_or("unknown");
                ListItem::new(vec![
                    Line::from(format!(
                        "{} · {}",
                        conversation.subject,
                        conversation.mail_ids.len()
                    )),
                    Line::styled(format!("from {last_sender}"), theme::muted()),
                ])
            })
            .collect();

        let inbox_focused = self.reader.is_none() && self.focus == Pane::Inbox;
        let workspace_focused = self.reader.is_none() && self.focus == Pane::Workspace;
        let title = if self.skipped > 0 {
            format!(" inbox • {} skipped ", self.skipped)
        } else if inbox_focused {
            " inbox • active ".into()
        } else {
            " inbox ".into()
        };
        let list = List::new(items)
            .style(theme::text())
            .highlight_symbol("▌ ")
            .highlight_style(theme::selection())
            .highlight_spacing(HighlightSpacing::Always)
            .repeat_highlight_symbol(true)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(theme::border(inbox_focused))
                    .title_style(theme::title(inbox_focused))
                    .title(title),
            );
        let mut list_state = ListState::default().with_selected(Some(self.selected));

        let mut docked_reader_area = None;
        if rows[0].width < MIN_SPLIT_WIDTH {
            match self.focus {
                Pane::Inbox => frame.render_stateful_widget(list, rows[0], &mut list_state),
                Pane::Workspace => {
                    if let Some(workspace) = &mut self.workspace {
                        workspace.draw(frame, rows[0], workspace_focused);
                    }
                }
            }
        } else {
            let constraints = if reader_is_docked(rows[0].width, self.reader.is_some()) {
                vec![
                    Constraint::Length(INBOX_WIDTH),
                    Constraint::Min(0),
                    Constraint::Length(READER_WIDTH),
                ]
            } else {
                vec![Constraint::Length(30), Constraint::Min(0)]
            };
            let areas = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(constraints)
                .split(rows[0]);
            frame.render_stateful_widget(list, areas[0], &mut list_state);
            if let Some(workspace) = &mut self.workspace {
                workspace.draw(frame, areas[1], workspace_focused);
            }
            if areas.len() == 3 {
                docked_reader_area = Some(areas[2]);
            }
        }

        let hints = if self.reader.is_some() {
            status_line(&[("j/k", "scroll"), ("pgup/pgdn", "page"), ("esc", "close")])
        } else {
            match self.focus {
                Pane::Inbox => status_line(&[
                    ("tab", "thread map"),
                    ("j/k", "select"),
                    ("enter", "open"),
                    ("q", "quit"),
                ]),
                Pane::Workspace => status_line(&[
                    ("tab", "inbox"),
                    ("arrows", "select"),
                    ("enter", "open"),
                    ("f", "fit"),
                    ("+/-", "zoom"),
                    ("q", "quit"),
                ]),
            }
        };
        frame.render_widget(Paragraph::new(hints), rows[1]);

        if let Some(reader) = &mut self.reader
            && let Some(mail) = self.mails.iter().find(|mail| mail.id == reader.mail_id())
        {
            if let Some(area) = docked_reader_area {
                reader.draw_docked(frame, area, mail);
            } else {
                reader.draw(frame, frame.area(), mail);
            }
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.on_key_event(key),
            Event::Mouse(mouse) => {
                if self.reader.is_none()
                    && let Some(workspace) = &mut self.workspace
                    && let Some(mail_id) = workspace.handle_mouse(mouse)
                {
                    self.open_mail(&mail_id);
                }
            }
            _ => {}
        }
    }

    fn on_key_event(&mut self, key: KeyEvent) {
        if !matches!(self.mailbox_state, MailboxState::Ready) {
            match (key.modifiers, key.code) {
                (_, KeyCode::Char('r')) => self.reload(),
                (_, KeyCode::Char('q'))
                | (KeyModifiers::CONTROL, KeyCode::Char('c') | KeyCode::Char('C')) => {
                    self.running = false;
                }
                _ => {}
            }
            return;
        }

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

            (_, KeyCode::Char('r')) => self.reload(),

            (_, KeyCode::Down | KeyCode::Char('j'))
                if self.focus == Pane::Inbox && self.selected + 1 < self.conversations.len() =>
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

            (
                _,
                KeyCode::Left
                | KeyCode::Right
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Char('h' | 'j' | 'k' | 'l' | '+' | '=' | '-' | '_' | '0' | 'f' | 'c'),
            ) if self.focus == Pane::Workspace => {
                if let Some(workspace) = &mut self.workspace {
                    workspace.handle_key(key.code);
                }
                self.sync_inbox_selection();
            }

            (_, KeyCode::Enter | KeyCode::Char(' ')) => {
                let mail_id = match self.focus {
                    Pane::Inbox => self.conversations[self.selected]
                        .latest_mail_id()
                        .map(str::to_owned),
                    Pane::Workspace => self
                        .workspace
                        .as_ref()
                        .and_then(Workspace::selected_mail_id),
                };
                if let Some(mail_id) = mail_id {
                    self.open_mail(&mail_id);
                }
            }

            _ => {}
        }
    }

    fn sync_workspace(&mut self) {
        let Some(mail_id) = self.conversations[self.selected].latest_mail_id() else {
            return;
        };
        if let Some(workspace) = &mut self.workspace {
            workspace.show_conversation(&self.mails, mail_id);
        }
    }

    fn sync_inbox_selection(&mut self) {
        let Some(mail_id) = self
            .workspace
            .as_ref()
            .and_then(Workspace::selected_mail_id)
        else {
            return;
        };
        if let Some(index) = self
            .conversations
            .iter()
            .position(|conversation| conversation.mail_ids.iter().any(|id| id == &mail_id))
        {
            self.selected = index;
        }
    }

    fn open_mail(&mut self, mail_id: &str) {
        if let Some(index) = self
            .conversations
            .iter()
            .position(|conversation| conversation.mail_ids.iter().any(|id| id == mail_id))
        {
            self.selected = index;
            self.reader = Some(Reader::new(mail_id.into()));
        }
    }

    fn reload(&mut self) {
        self.mailbox_state = MailboxState::Loading;
        self.spinner_tick = 0;
        self.reader = None;
        let source = self.source.clone();
        let sender = self.load_tx.clone();
        tokio::task::spawn_blocking(move || {
            let _ = sender.send(source.load());
        });
    }

    fn apply_load(&mut self, result: Result<LoadReport, LoadError>) {
        match result {
            Ok(report) if report.mails.is_empty() && report.attempted > 0 => {
                self.clear_mailbox();
                self.skipped = report.skipped.len();
                self.mailbox_state = MailboxState::Error(
                    "No readable messages found. Check the message format, then retry.".into(),
                );
            }
            Ok(report) if report.mails.is_empty() => {
                self.clear_mailbox();
                self.mailbox_state = MailboxState::Empty;
            }
            Ok(report) => {
                self.mails = report.mails;
                self.conversations = group_conversations(&self.mails);
                self.selected = 0;
                self.skipped = report.skipped.len();
                self.workspace = self
                    .conversations
                    .first()
                    .and_then(Conversation::latest_mail_id)
                    .map(|mail_id| Workspace::new(&self.mails, mail_id));
                self.mailbox_state = MailboxState::Ready;
            }
            Err(error) => {
                self.clear_mailbox();
                self.mailbox_state = MailboxState::Error(error.to_string());
            }
        }
    }

    fn clear_mailbox(&mut self) {
        self.mails.clear();
        self.conversations.clear();
        self.workspace = None;
        self.reader = None;
        self.selected = 0;
        self.skipped = 0;
    }

    fn draw_mailbox_state(&self, frame: &mut Frame, area: Rect) {
        let block = Block::bordered()
            .border_style(theme::border(false))
            .title_style(theme::title(false))
            .title(" termail ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let width = inner.width.min(64);
        let content = Rect::new(
            inner.x + inner.width.saturating_sub(width) / 2,
            inner.y + inner.height.saturating_sub(5) / 2,
            width,
            5.min(inner.height),
        );

        match &self.mailbox_state {
            MailboxState::Loading => {
                let spinner_area = Rect::new(
                    content.x + content.width.saturating_sub(6) / 2,
                    content.y,
                    6.min(content.width),
                    1,
                );
                frame.render_widget(
                    FluxSpinner::new(self.spinner_tick)
                        .width(6)
                        .color(theme::ACCENT),
                    spinner_area,
                );
                frame.render_widget(
                    Paragraph::new(format!("Loading {}", self.source.label()))
                        .style(theme::text())
                        .alignment(Alignment::Center),
                    Rect::new(content.x, content.y.saturating_add(2), content.width, 1),
                );
            }
            MailboxState::Empty => {
                frame.render_widget(
                    Paragraph::new(
                        "No messages found\n\nAdd mail to cur/ or new/, then press r to retry.",
                    )
                    .style(theme::text())
                    .alignment(Alignment::Center)
                    .wrap(Wrap { trim: true }),
                    content,
                );
            }
            MailboxState::Error(error) => {
                frame.render_widget(
                    Paragraph::new(format!(
                        "Unable to load mailbox\n\n{error}\n\nCheck the path and permissions, then press r to retry."
                    ))
                    .style(theme::text())
                    .alignment(Alignment::Center)
                    .wrap(Wrap { trim: true }),
                    content,
                );
            }
            MailboxState::Ready => {}
        }
    }
}

fn status_line(hints: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, (key, label)) in hints.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(format!(" {key} "), theme::keycap()));
        spans.push(Span::styled(format!(" {label}"), theme::muted()));
    }
    Line::from(spans)
}

fn reader_is_docked(width: u16, reader_open: bool) -> bool {
    reader_open && width >= MIN_DOCK_WIDTH
}

fn parse_source(
    args: impl IntoIterator<Item = OsString>,
) -> color_eyre::Result<Option<MailboxSource>> {
    let mut args = args.into_iter();
    let mut source = MailboxSource::Demo;

    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--maildir") => {
                let path = args
                    .next()
                    .ok_or_else(|| color_eyre::eyre::eyre!("--maildir needs a path"))?;
                source = MailboxSource::Maildir(PathBuf::from(path));
            }
            Some("--demo") => source = MailboxSource::Demo,
            Some("--help" | "-h") => return Ok(None),
            Some(value) => return Err(color_eyre::eyre::eyre!("unknown option: {value}")),
            None => return Err(color_eyre::eyre::eyre!("option is not valid utf-8")),
        }
    }

    Ok(Some(source))
}

fn print_help() {
    println!("termail [--demo | --maildir PATH]");
    println!();
    println!("  --demo          open the built-in mailbox");
    println!("  --maildir PATH  open a Maildir mailbox");
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, path::PathBuf};

    use crate::mailbox::MailboxSource;

    use super::{parse_source, reader_is_docked};

    #[test]
    fn docks_readers_only_when_they_fit() {
        assert!(reader_is_docked(120, true));
        assert!(!reader_is_docked(100, true));
        assert!(!reader_is_docked(120, false));
    }

    #[test]
    fn reads_maildir_source_from_arguments() {
        let source = parse_source([OsString::from("--maildir"), OsString::from("/tmp/mail")])
            .unwrap()
            .unwrap();

        assert_eq!(source, MailboxSource::Maildir(PathBuf::from("/tmp/mail")));
    }
}
