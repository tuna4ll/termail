mod mail;
mod mailbox;
mod theme;
mod widgets;

use mail::{Conversation, Mail, group_conversations};
use mailbox::{LoadError, LoadReport, MailboxSource, MaildirChange};
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
    query: String,
    searching: bool,
    filter: InboxFilter,
    undo: Option<UndoAction>,
    notice: Option<String>,
}

struct UndoAction {
    change: Option<MaildirChange>,
    demo_mails: Option<Vec<Mail>>,
    label: String,
}

#[derive(Debug)]
enum MailboxState {
    Loading,
    Ready,
    Empty,
    Error(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InboxFilter {
    All,
    Unread,
    Starred,
}

impl InboxFilter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Unread => "unread",
            Self::Starred => "starred",
        }
    }
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
            query: String::new(),
            searching: false,
            filter: InboxFilter::All,
            undo: None,
            notice: None,
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
            draw_mailbox_state(
                frame,
                rows[0],
                &self.mailbox_state,
                &self.source,
                self.spinner_tick,
            );
            frame.render_widget(
                Paragraph::new(status_line(&[("r", "retry"), ("q", "quit")])),
                rows[1],
            );
            return;
        }

        let visible = self.visible_conversations();
        let items: Vec<ListItem> = visible
            .iter()
            .map(|index| {
                let conversation = &self.conversations[*index];
                let last_sender = conversation.last_sender(&self.mails).unwrap_or("unknown");
                let unread = conversation.is_unread(&self.mails);
                let starred = conversation.is_starred(&self.mails);
                let marker = match (unread, starred) {
                    (true, true) => "●★",
                    (true, false) => "● ",
                    (false, true) => " ★",
                    (false, false) => "  ",
                };
                let subject = format!(
                    "{marker} {} · {}",
                    conversation.subject,
                    conversation.mail_ids.len()
                );
                let subject_style = if !self.query.is_empty()
                    && conversation
                        .subject
                        .to_lowercase()
                        .contains(&self.query.to_lowercase())
                {
                    theme::title(true)
                } else {
                    theme::text()
                };
                let lowered_query = self.query.to_lowercase();
                let sender_query = lowered_query
                    .strip_prefix("from:")
                    .unwrap_or(&lowered_query)
                    .trim();
                let sender_style = if !sender_query.is_empty()
                    && last_sender.to_lowercase().contains(sender_query)
                {
                    theme::title(true)
                } else {
                    theme::muted()
                };
                ListItem::new(vec![
                    Line::styled(subject, subject_style),
                    Line::styled(format!("from {last_sender}"), sender_style),
                ])
            })
            .collect();

        let inbox_focused = self.reader.is_none() && self.focus == Pane::Inbox;
        let workspace_focused = self.reader.is_none() && self.focus == Pane::Workspace;
        let title = if self.searching {
            format!(" search: {}_ ", self.query)
        } else if !self.query.is_empty() {
            format!(" inbox • {} • /{} ", self.filter.label(), self.query)
        } else if self.filter != InboxFilter::All {
            format!(" inbox • {} ", self.filter.label())
        } else if self.skipped > 0 {
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
                    if visible.is_empty() {
                        draw_no_matches(frame, rows[0]);
                    } else if let Some(workspace) = &mut self.workspace {
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
            if visible.is_empty() {
                draw_no_matches(frame, areas[1]);
            } else if let Some(workspace) = &mut self.workspace {
                workspace.draw(frame, areas[1], workspace_focused);
            }
            if areas.len() == 3 {
                docked_reader_area = Some(areas[2]);
            }
        }

        let hints = if let Some(notice) = &self.notice {
            action_line(notice, self.undo.is_some())
        } else if self.searching {
            status_line(&[("type", "search"), ("enter", "apply"), ("esc", "clear")])
        } else if self.reader.is_some() {
            status_line(&[("j/k", "scroll"), ("pgup/pgdn", "page"), ("esc", "close")])
        } else {
            match self.focus {
                Pane::Inbox => status_line(&[
                    ("tab", "thread map"),
                    ("j/k", "select"),
                    ("/", "search"),
                    ("1/2/3", "filter"),
                    ("m/s", "read/star"),
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
                    && !self.visible_conversations().is_empty()
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

        if self.searching {
            match key.code {
                KeyCode::Enter => self.searching = false,
                KeyCode::Esc => {
                    self.searching = false;
                    self.query.clear();
                    self.refresh_inbox();
                }
                KeyCode::Backspace => {
                    self.query.pop();
                    self.refresh_inbox();
                }
                KeyCode::Char(character)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    self.query.push(character);
                    self.refresh_inbox();
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('m') => {
                self.toggle_read();
                return;
            }
            KeyCode::Char('s') => {
                self.toggle_starred();
                return;
            }
            KeyCode::Char('u') if self.undo.is_some() => {
                self.undo_last();
                return;
            }
            _ => {}
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
            (_, KeyCode::Esc) if !self.query.is_empty() => {
                self.query.clear();
                self.refresh_inbox();
            }

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

            (_, KeyCode::Char('/')) if self.focus == Pane::Inbox => self.searching = true,

            (_, KeyCode::Char('1')) if self.focus == Pane::Inbox => {
                self.filter = InboxFilter::All;
                self.refresh_inbox();
            }

            (_, KeyCode::Char('2')) if self.focus == Pane::Inbox => {
                self.filter = InboxFilter::Unread;
                self.refresh_inbox();
            }

            (_, KeyCode::Char('3')) if self.focus == Pane::Inbox => {
                self.filter = InboxFilter::Starred;
                self.refresh_inbox();
            }

            (_, KeyCode::Down | KeyCode::Char('j'))
                if self.focus == Pane::Inbox
                    && self.selected + 1 < self.visible_conversations().len() =>
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
                    Pane::Inbox => self
                        .selected_conversation()
                        .and_then(Conversation::latest_mail_id)
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
        let Some(mail_id) = self
            .selected_conversation()
            .and_then(Conversation::latest_mail_id)
            .map(str::to_owned)
        else {
            return;
        };
        if let Some(workspace) = &mut self.workspace {
            workspace.show_conversation(&self.mails, &mail_id);
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
        if let Some(index) = self.visible_conversations().iter().position(|index| {
            self.conversations[*index]
                .mail_ids
                .iter()
                .any(|id| id == &mail_id)
        }) {
            self.selected = index;
        }
    }

    fn open_mail(&mut self, mail_id: &str) {
        if let Some(index) = self.visible_conversations().iter().position(|index| {
            self.conversations[*index]
                .mail_ids
                .iter()
                .any(|id| id == mail_id)
        }) {
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

    fn active_mail_id(&self) -> Option<String> {
        self.reader
            .as_ref()
            .map(|reader| reader.mail_id().to_owned())
            .or_else(|| {
                self.workspace
                    .as_ref()
                    .and_then(Workspace::selected_mail_id)
            })
            .or_else(|| {
                self.selected_conversation()
                    .and_then(Conversation::latest_mail_id)
                    .map(str::to_owned)
            })
    }

    fn toggle_read(&mut self) {
        let Some(mail_id) = self.active_mail_id() else {
            return;
        };
        let Some(index) = self.mails.iter().position(|mail| mail.id == mail_id) else {
            return;
        };
        let target_seen = self.mails[index].unread;
        let snapshot = matches!(self.source, MailboxSource::Demo).then(|| self.mails.clone());
        match self.source.set_seen(&self.mails[index], target_seen) {
            Ok(change) => {
                if let Some(change) = &change {
                    self.mails[index].source_path = Some(change.after_path().to_path_buf());
                }
                self.mails[index].unread = !target_seen;
                let label = if target_seen {
                    "Marked read"
                } else {
                    "Marked unread"
                };
                self.undo = Some(UndoAction {
                    change,
                    demo_mails: snapshot,
                    label: label.into(),
                });
                self.notice = Some(label.into());
                self.rebuild_mailbox(Some(&mail_id));
            }
            Err(error) => self.notice = Some(format!("Unable to update message: {error}")),
        }
    }

    fn toggle_starred(&mut self) {
        let Some(mail_id) = self.active_mail_id() else {
            return;
        };
        let Some(index) = self.mails.iter().position(|mail| mail.id == mail_id) else {
            return;
        };
        let target = !self.mails[index].starred;
        let snapshot = matches!(self.source, MailboxSource::Demo).then(|| self.mails.clone());
        match self.source.set_starred(&self.mails[index], target) {
            Ok(change) => {
                if let Some(change) = &change {
                    self.mails[index].source_path = Some(change.after_path().to_path_buf());
                }
                self.mails[index].starred = target;
                let label = if target { "Starred" } else { "Removed star" };
                self.undo = Some(UndoAction {
                    change,
                    demo_mails: snapshot,
                    label: label.into(),
                });
                self.notice = Some(label.into());
                self.rebuild_mailbox(Some(&mail_id));
            }
            Err(error) => self.notice = Some(format!("Unable to update message: {error}")),
        }
    }

    fn undo_last(&mut self) {
        let Some(undo) = self.undo.take() else {
            return;
        };
        if let Some(mails) = undo.demo_mails {
            self.mails = mails;
            self.rebuild_mailbox(None);
            self.notice = Some(format!("Undid {}", undo.label.to_lowercase()));
        } else if let Some(change) = undo.change {
            match change.undo() {
                Ok(()) => {
                    self.notice = Some(format!("Undid {}", undo.label.to_lowercase()));
                    self.reload();
                }
                Err(error) => self.notice = Some(format!("Unable to undo: {error}")),
            }
        }
    }

    fn rebuild_mailbox(&mut self, mail_id: Option<&str>) {
        self.conversations = group_conversations(&self.mails);
        let visible = self.visible_conversations();
        self.selected = mail_id
            .and_then(|mail_id| {
                visible.iter().position(|index| {
                    self.conversations[*index]
                        .mail_ids
                        .iter()
                        .any(|id| id == mail_id)
                })
            })
            .unwrap_or_default()
            .min(visible.len().saturating_sub(1));
        self.workspace = self
            .selected_conversation()
            .and_then(Conversation::latest_mail_id)
            .map(|mail_id| Workspace::new(&self.mails, mail_id));
    }

    fn visible_conversations(&self) -> Vec<usize> {
        self.conversations
            .iter()
            .enumerate()
            .filter(|(_, conversation)| match self.filter {
                InboxFilter::All => true,
                InboxFilter::Unread => conversation.is_unread(&self.mails),
                InboxFilter::Starred => conversation.is_starred(&self.mails),
            })
            .filter(|(_, conversation)| conversation.matches(&self.mails, &self.query))
            .map(|(index, _)| index)
            .collect()
    }

    fn selected_conversation(&self) -> Option<&Conversation> {
        let index = *self.visible_conversations().get(self.selected)?;
        self.conversations.get(index)
    }

    fn refresh_inbox(&mut self) {
        let count = self.visible_conversations().len();
        self.selected = self.selected.min(count.saturating_sub(1));
        if count > 0 {
            self.sync_workspace();
        }
    }
}

fn draw_mailbox_state(
    frame: &mut Frame,
    area: Rect,
    state: &MailboxState,
    source: &MailboxSource,
    spinner_tick: u64,
) {
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

    match state {
        MailboxState::Loading => {
            let spinner_area = Rect::new(
                content.x + content.width.saturating_sub(6) / 2,
                content.y,
                6.min(content.width),
                1,
            );
            frame.render_widget(
                FluxSpinner::new(spinner_tick).width(6).color(theme::ACCENT),
                spinner_area,
            );
            frame.render_widget(
                Paragraph::new(format!("Loading {}", source.label()))
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

fn action_line(message: &str, undo: bool) -> Line<'static> {
    let mut spans = vec![Span::styled(format!(" {message} "), theme::title(true))];
    if undo {
        spans.push(Span::styled(" u ", theme::keycap()));
        spans.push(Span::styled(" undo", theme::muted()));
    }
    Line::from(spans)
}

fn draw_no_matches(frame: &mut Frame, area: Rect) {
    let block = Block::bordered()
        .border_style(theme::border(false))
        .title_style(theme::title(false))
        .title(" workspace ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(
            "No matching conversations\n\nChange the filter or press esc to clear search.",
        )
        .style(theme::muted())
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true }),
        Rect::new(
            inner.x,
            inner.y + inner.height.saturating_sub(3) / 2,
            inner.width,
            3.min(inner.height),
        ),
    );
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

    use ratatui::{Terminal, backend::TestBackend};

    use crate::mailbox::MailboxSource;

    use super::{MailboxState, draw_mailbox_state, parse_source, reader_is_docked};

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

    #[test]
    fn renders_mailbox_states_with_recovery() {
        assert!(render_state(&MailboxState::Loading).contains("Loading demo mailbox"));
        assert!(render_state(&MailboxState::Empty).contains("No messages found"));

        let screen = render_state(&MailboxState::Error("permission denied".into()));
        assert!(screen.contains("Unable to load mailbox"));
        assert!(screen.contains("press r to retry"));
    }

    fn render_state(state: &MailboxState) -> String {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_mailbox_state(frame, frame.area(), state, &MailboxSource::Demo, 2))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }
}
