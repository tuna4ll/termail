mod config;
mod doctor;
mod draft;
mod mail;
mod mailbox;
mod sender;
mod theme;
mod widgets;

use config::Config;
use mail::{Attachment, Conversation, Mail, group_conversations};
use mailbox::{LoadError, LoadReport, MailboxSource, MaildirChange};
use widgets::reader::Reader;
use widgets::workspace::Workspace;

use std::{
    ffi::OsString,
    fs,
    io::stdout,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEvent,
        KeyEventKind, KeyModifiers,
    },
    execute,
};
use directories::BaseDirs;
use futures::StreamExt;
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Stylize,
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph, Wrap,
    },
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
    let Some(options) = parse_options(std::env::args_os().skip(1))? else {
        print_help();
        return Ok(());
    };
    let config = Config::load(options.config_path.as_deref())?;
    if options.doctor {
        if !doctor::run(&config) {
            std::process::exit(1);
        }
        return Ok(());
    }
    let source = options
        .source
        .or_else(|| config.maildir.clone().map(MailboxSource::Maildir))
        .unwrap_or(MailboxSource::Demo);

    let terminal = ratatui::init();

    execute!(stdout(), EnableMouseCapture)?;

    let result = App::new(source, config).run(terminal).await;

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
    mailbox_fingerprint: u64,
    pending_editor: Option<ComposeState>,
    overlay: Option<Overlay>,
    config: Config,
    active_folder: String,
    account_label: String,
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

enum Overlay {
    Help {
        scroll: u16,
    },
    Command(String),
    DraftReady {
        state: ComposeState,
        summary: draft::Summary,
    },
    ComposeClose(ComposeState),
    Folders {
        selected: usize,
        items: Vec<FolderItem>,
    },
    Links {
        selected: usize,
        items: Vec<String>,
    },
    Attachments {
        selected: usize,
        items: Vec<Attachment>,
    },
    Accounts {
        selected: usize,
        items: Vec<AccountItem>,
    },
    Compose(ComposeState),
}

struct ComposeState {
    composer: draft::Composer,
    field: usize,
    error: Option<String>,
}

impl ComposeState {
    const FIELD_COUNT: usize = 6;
    const BODY_FIELD: usize = 5;

    fn value_mut(&mut self) -> Option<&mut String> {
        match self.field {
            0 => Some(&mut self.composer.to),
            1 => Some(&mut self.composer.cc),
            2 => Some(&mut self.composer.bcc),
            3 => Some(&mut self.composer.subject),
            4 => Some(&mut self.composer.attachments),
            _ => None,
        }
    }

    fn move_next(&mut self) {
        self.field = (self.field + 1) % Self::FIELD_COUNT;
        self.error = None;
    }

    fn move_previous(&mut self) {
        self.field = self.field.checked_sub(1).unwrap_or(Self::FIELD_COUNT - 1);
        self.error = None;
    }
}

#[derive(Clone)]
struct FolderItem {
    name: String,
    unread: usize,
    total: usize,
}

#[derive(Clone)]
struct AccountItem {
    label: String,
    index: Option<usize>,
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
    pub fn new(source: MailboxSource, config: Config) -> Self {
        let (load_tx, load_rx) = mpsc::unbounded_channel();
        let account_label = config
            .accounts
            .get(config.active_account_index())
            .map(|account| account.label())
            .or_else(|| config.identity())
            .unwrap_or_else(|| "local".into());
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
            mailbox_fingerprint: 0,
            pending_editor: None,
            overlay: None,
            config,
            active_folder: "INBOX".into(),
            account_label,
        };
        app.reload();
        app
    }

    pub async fn run(mut self, mut terminal: DefaultTerminal) -> color_eyre::Result<()> {
        self.running = true;
        let mut animation = interval(Duration::from_millis(80));
        let mut mailbox_sync = interval(Duration::from_secs(2));

        while self.running {
            terminal.draw(|frame| self.draw(frame))?;
            tokio::select! {
                _ = animation.tick() => {
                    if matches!(self.mailbox_state, MailboxState::Loading) {
                        self.spinner_tick = self.spinner_tick.wrapping_add(1);
                    }
                }
                _ = mailbox_sync.tick() => self.check_mailbox_changes(),
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
            if let Some(state) = self.pending_editor.take() {
                terminal = self.edit_composer_body(state)?;
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
            let hints = if matches!(self.overlay, Some(Overlay::Help { .. })) {
                status_line(&[("j/k", "scroll"), ("esc", "close help")])
            } else if matches!(self.overlay, Some(Overlay::Command(_))) {
                status_line(&[("type", "command"), ("enter", "run"), ("esc", "cancel")])
            } else if matches!(self.overlay, Some(Overlay::DraftReady { .. })) {
                status_line(&[("s", "send"), ("e", "edit"), ("d", "save draft")])
            } else if matches!(self.overlay, Some(Overlay::ComposeClose(_))) {
                status_line(&[("s", "save draft"), ("d", "discard"), ("esc", "continue")])
            } else if matches!(
                self.overlay,
                Some(Overlay::Folders { .. } | Overlay::Links { .. } | Overlay::Accounts { .. })
            ) {
                status_line(&[("j/k", "select"), ("enter", "open"), ("esc", "cancel")])
            } else if matches!(self.overlay, Some(Overlay::Attachments { .. })) {
                status_line(&[("j/k", "select"), ("s", "save"), ("o", "open")])
            } else if matches!(self.overlay, Some(Overlay::Compose(_))) {
                status_line(&[
                    ("tab", "next field"),
                    ("ctrl+s", "review"),
                    ("esc", "close"),
                ])
            } else {
                status_line(&[
                    ("r", "retry"),
                    (":", "command"),
                    ("?", "help"),
                    ("q", "quit"),
                ])
            };
            frame.render_widget(Paragraph::new(hints), rows[1]);
            if let Some(overlay) = &self.overlay {
                draw_overlay(frame, overlay);
            }
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
                let account = self.mails.iter().find_map(|mail| {
                    conversation
                        .mail_ids
                        .contains(&mail.id)
                        .then_some(mail.account.as_deref())
                        .flatten()
                });
                let sender = match account {
                    Some(account) if matches!(self.source, MailboxSource::Unified(_)) => {
                        format!("from {last_sender} · {account}")
                    }
                    _ => format!("from {last_sender}"),
                };
                ListItem::new(vec![
                    Line::styled(subject, subject_style),
                    Line::styled(sender, sender_style),
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
            format!(
                " {} · {} • {} skipped ",
                self.account_label, self.active_folder, self.skipped
            )
        } else if inbox_focused {
            format!(" {} · {} • active ", self.account_label, self.active_folder)
        } else {
            format!(" {} · {} ", self.account_label, self.active_folder)
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
                        draw_no_matches(
                            frame,
                            rows[0],
                            &self.active_folder,
                            self.conversations.is_empty(),
                        );
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
                draw_no_matches(
                    frame,
                    areas[1],
                    &self.active_folder,
                    self.conversations.is_empty(),
                );
            } else if let Some(workspace) = &mut self.workspace {
                workspace.draw(frame, areas[1], workspace_focused);
            }
            if areas.len() == 3 {
                docked_reader_area = Some(areas[2]);
            }
        }

        let hints = if matches!(self.overlay, Some(Overlay::Help { .. })) {
            status_line(&[("j/k", "scroll"), ("esc", "close help")])
        } else if matches!(self.overlay, Some(Overlay::Command(_))) {
            status_line(&[("type", "command"), ("enter", "run"), ("esc", "cancel")])
        } else if matches!(self.overlay, Some(Overlay::DraftReady { .. })) {
            status_line(&[("s", "send"), ("e", "edit"), ("d", "save draft")])
        } else if matches!(self.overlay, Some(Overlay::ComposeClose(_))) {
            status_line(&[("s", "save draft"), ("d", "discard"), ("esc", "continue")])
        } else if matches!(
            self.overlay,
            Some(Overlay::Folders { .. } | Overlay::Links { .. } | Overlay::Accounts { .. })
        ) {
            status_line(&[("j/k", "select"), ("enter", "open"), ("esc", "cancel")])
        } else if matches!(self.overlay, Some(Overlay::Attachments { .. })) {
            status_line(&[("j/k", "select"), ("s", "save"), ("o", "open")])
        } else if matches!(self.overlay, Some(Overlay::Compose(_))) {
            status_line(&[
                ("tab", "next field"),
                ("ctrl+s", "review"),
                ("esc", "close"),
            ])
        } else if let Some(notice) = &self.notice {
            action_line(notice, self.undo.is_some())
        } else if self.searching {
            status_line(&[("type", "search"), ("enter", "apply"), ("esc", "clear")])
        } else if self.reader.is_some() {
            status_line(&[
                ("j/k", "scroll"),
                ("r", "reply"),
                ("l", "links"),
                ("v", "files"),
                ("m/s", "read/star"),
                ("a/d", "archive/trash"),
                ("esc", "close"),
            ])
        } else {
            match self.focus {
                Pane::Inbox => status_line(&[
                    ("tab", "thread map"),
                    ("g", "folders"),
                    ("p", "accounts"),
                    ("j/k", "select"),
                    ("/", "search"),
                    ("1/2/3", "filter"),
                    ("m/s", "read/star"),
                    ("a/d", "archive/trash"),
                    ("c/r", "compose/reply"),
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

        if let Some(overlay) = &self.overlay {
            draw_overlay(frame, overlay);
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
        if key.modifiers == KeyModifiers::CONTROL
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            self.running = false;
            return;
        }
        if self.handle_overlay_key(key) {
            return;
        }
        if !matches!(self.mailbox_state, MailboxState::Ready) {
            match (key.modifiers, key.code) {
                (_, KeyCode::Char('r')) => self.reload(),
                (_, KeyCode::Char('?')) => self.overlay = Some(Overlay::Help { scroll: 0 }),
                (_, KeyCode::Char(':')) => {
                    self.overlay = Some(Overlay::Command(String::new()));
                }
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

        if key.code == KeyCode::Esc && self.notice.take().is_some() {
            return;
        }

        match key.code {
            KeyCode::Char('?') => {
                self.overlay = Some(Overlay::Help { scroll: 0 });
                return;
            }
            KeyCode::Char(':') => {
                self.overlay = Some(Overlay::Command(String::new()));
                return;
            }
            KeyCode::Char('g') => {
                self.open_folders();
                return;
            }
            KeyCode::Char('p') => {
                self.open_accounts();
                return;
            }
            KeyCode::Char('x') if self.active_folder == "Outbox" => {
                self.retry_outbox();
                return;
            }
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
            KeyCode::Char('a') => {
                self.move_active_message(true);
                return;
            }
            KeyCode::Char('d') => {
                self.move_active_message(false);
                return;
            }
            KeyCode::Char('r') => {
                self.start_draft(true);
                return;
            }
            KeyCode::Char('c') if self.reader.is_some() || self.focus == Pane::Inbox => {
                self.start_draft(false);
                return;
            }
            KeyCode::Char('l') if self.reader.is_some() => {
                self.open_links();
                return;
            }
            KeyCode::Char('v') if self.reader.is_some() => {
                self.open_attachments();
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

            (_, KeyCode::Char('R')) => self.reload(),

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
            let active_mails: Vec<Mail> = self
                .mails
                .iter()
                .filter(|mail| mail_folder(mail) == self.active_folder)
                .cloned()
                .collect();
            workspace.show_conversation(&active_mails, &mail_id);
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
                let active_mail_id = self.active_mail_id();
                self.mails = report.mails;
                self.skipped = report.skipped.len();
                let folders = self.folder_items();
                if !folders
                    .iter()
                    .any(|folder| folder.name == self.active_folder)
                {
                    self.active_folder = folders
                        .first()
                        .map(|folder| folder.name.clone())
                        .unwrap_or_else(|| "INBOX".into());
                }
                self.rebuild_mailbox(active_mail_id.as_deref());
                if self.reader.as_ref().is_some_and(|reader| {
                    !self.mails.iter().any(|mail| mail.id == reader.mail_id())
                }) {
                    self.reader = None;
                }
                self.refresh_fingerprint();
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
                self.refresh_fingerprint();
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
                self.refresh_fingerprint();
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

    fn move_active_message(&mut self, archive: bool) {
        let Some(mail_id) = self.active_mail_id() else {
            return;
        };
        let Some(index) = self.mails.iter().position(|mail| mail.id == mail_id) else {
            return;
        };
        let snapshot = matches!(self.source, MailboxSource::Demo).then(|| self.mails.clone());
        let result = if archive {
            self.source.archive(&self.mails[index])
        } else {
            self.source.trash(&self.mails[index])
        };
        match result {
            Ok(change) => {
                if let Some(change) = &change {
                    self.mails[index].source_path = Some(change.after_path().to_path_buf());
                }
                self.mails[index].mailbox = Some(if archive { "Archive" } else { "Trash" }.into());
                self.reader = None;
                let label = if archive {
                    "Archived message"
                } else {
                    "Moved message to Trash"
                };
                self.undo = Some(UndoAction {
                    change,
                    demo_mails: snapshot,
                    label: label.into(),
                });
                self.notice = Some(label.into());
                self.refresh_fingerprint();
                self.rebuild_mailbox(None);
            }
            Err(error) => self.notice = Some(format!("Unable to move message: {error}")),
        }
    }

    fn rebuild_mailbox(&mut self, mail_id: Option<&str>) {
        let active_mails: Vec<Mail> = self
            .mails
            .iter()
            .filter(|mail| mail_folder(mail) == self.active_folder)
            .cloned()
            .collect();
        self.conversations = group_conversations(&active_mails);
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
            .map(|mail_id| Workspace::new(&active_mails, mail_id));
    }

    fn folder_items(&self) -> Vec<FolderItem> {
        let mut names: Vec<String> = self
            .mails
            .iter()
            .map(|mail| mail_folder(mail).into())
            .collect();
        if matches!(
            self.source,
            MailboxSource::Maildir(_) | MailboxSource::Unified(_)
        ) {
            names.extend(
                ["INBOX", "Drafts", "Sent", "Archive", "Trash", "Outbox"]
                    .into_iter()
                    .map(str::to_owned),
            );
        }
        names.sort();
        names.dedup();
        if let Some(position) = names.iter().position(|name| name == "INBOX") {
            names.swap(0, position);
        }
        names
            .into_iter()
            .map(|name| {
                let messages = self.mails.iter().filter(|mail| mail_folder(mail) == name);
                let total = messages.clone().count();
                let unread = messages.filter(|mail| mail.unread).count();
                FolderItem {
                    name,
                    unread,
                    total,
                }
            })
            .collect()
    }

    fn open_folders(&mut self) {
        let items = self.folder_items();
        let selected = items
            .iter()
            .position(|folder| folder.name == self.active_folder)
            .unwrap_or_default();
        self.overlay = Some(Overlay::Folders { selected, items });
    }

    fn open_links(&mut self) {
        let links = self
            .active_mail_id()
            .and_then(|id| self.mails.iter().find(|mail| mail.id == id))
            .map(|mail| mail.links.clone())
            .unwrap_or_default();
        if links.is_empty() {
            self.notice = Some("This message has no web links.".into());
        } else {
            self.overlay = Some(Overlay::Links {
                selected: 0,
                items: links,
            });
        }
    }

    fn open_attachments(&mut self) {
        let items = self
            .active_mail_id()
            .and_then(|id| self.mails.iter().find(|mail| mail.id == id))
            .map(|mail| mail.attachments.clone())
            .unwrap_or_default();
        if items.is_empty() {
            self.notice = Some("This message has no attachments.".into());
        } else {
            self.overlay = Some(Overlay::Attachments { selected: 0, items });
        }
    }

    fn open_accounts(&mut self) {
        if self.config.accounts.len() < 2 {
            self.notice = Some("Add another [[accounts]] entry to switch accounts.".into());
            return;
        }
        let mut items = vec![AccountItem {
            label: "All accounts".into(),
            index: None,
        }];
        items.extend(
            self.config
                .accounts
                .iter()
                .enumerate()
                .map(|(index, account)| AccountItem {
                    label: account.label(),
                    index: Some(index),
                }),
        );
        let selected = if matches!(self.source, MailboxSource::Unified(_)) {
            0
        } else {
            self.config.active_account_index() + 1
        };
        self.overlay = Some(Overlay::Accounts { selected, items });
    }

    fn select_account(&mut self, index: Option<usize>) {
        let source = match index {
            None => {
                let maildirs: Vec<(String, PathBuf)> = self
                    .config
                    .accounts
                    .iter()
                    .filter_map(|account| {
                        account
                            .maildir
                            .clone()
                            .map(|path| (account.id.clone(), path))
                    })
                    .collect();
                if maildirs.len() < 2 {
                    self.notice = Some("Configure Maildir paths for at least two accounts.".into());
                    return;
                }
                self.account_label = "all accounts".into();
                MailboxSource::Unified(maildirs)
            }
            Some(index) => {
                if !self.config.activate(index) {
                    return;
                }
                let Some(path) = self.config.maildir.clone() else {
                    self.notice = Some("Set maildir for this account in config.toml.".into());
                    return;
                };
                self.account_label = self.config.accounts[index].label();
                MailboxSource::Maildir(path)
            }
        };
        self.source = source;
        self.active_folder = "INBOX".into();
        self.filter = InboxFilter::All;
        self.query.clear();
        self.undo = None;
        self.reader = None;
        self.reload();
    }

    fn save_active_attachment(&mut self, index: usize, open: bool) {
        let Some(mail_id) = self.active_mail_id() else {
            return;
        };
        let Some(mail) = self.mails.iter().find(|mail| mail.id == mail_id) else {
            return;
        };
        match self
            .source
            .save_attachment(mail, index, &self.config.download_dir())
        {
            Ok(path) if open => {
                self.notice = match opener::open(&path) {
                    Ok(()) => Some(format!("Saved and opened {}", path.display())),
                    Err(error) => Some(format!(
                        "Saved to {}, but unable to open it: {error}",
                        path.display()
                    )),
                };
            }
            Ok(path) => self.notice = Some(format!("Saved attachment to {}", path.display())),
            Err(error) => {
                self.notice = Some(format!(
                    "Unable to save attachment: {error}. Check the download directory."
                ));
            }
        }
    }

    fn retry_outbox(&mut self) {
        let Some(mail_id) = self.active_mail_id() else {
            return;
        };
        let Some(index) = self.mails.iter().position(|mail| mail.id == mail_id) else {
            return;
        };
        let Some(path) = self.mails[index].source_path.as_ref() else {
            self.notice = Some("Queued message has no Maildir path.".into());
            return;
        };
        let contents = match fs::read(path) {
            Ok(contents) => contents,
            Err(error) => {
                self.notice = Some(format!("Unable to read queued message: {error}"));
                return;
            }
        };
        if let Err(error) = sender::send(&self.config.send_command, &contents) {
            self.notice = Some(format!("Send failed: {error}. Message remains in Outbox."));
            return;
        }
        match self.source.mark_sent(&self.mails[index]) {
            Ok(change) => {
                if let Some(change) = change {
                    self.mails[index].source_path = Some(change.after_path().to_path_buf());
                }
                self.mails[index].mailbox = Some("Sent".into());
                self.reader = None;
                self.notice = Some("Queued message sent".into());
                self.refresh_fingerprint();
                self.rebuild_mailbox(None);
            }
            Err(error) => {
                self.notice = Some(format!(
                    "Message sent, but unable to move it to Sent: {error}"
                ));
            }
        }
    }

    fn refresh_fingerprint(&mut self) {
        if let Ok(fingerprint) = self.source.fingerprint() {
            self.mailbox_fingerprint = fingerprint;
        }
    }

    fn check_mailbox_changes(&mut self) {
        if !matches!(self.mailbox_state, MailboxState::Ready) {
            return;
        }
        if let Ok(fingerprint) = self.source.fingerprint()
            && fingerprint != self.mailbox_fingerprint
        {
            self.notice = Some("Mailbox changed · refreshing".into());
            self.reload();
        }
    }

    fn handle_overlay_key(&mut self, key: KeyEvent) -> bool {
        let Some(overlay) = self.overlay.take() else {
            return false;
        };
        let mut overlay = match overlay {
            Overlay::Compose(state) => {
                self.handle_compose_key(state, key);
                return true;
            }
            Overlay::DraftReady { state, summary } => {
                if action_key(key, 's') {
                    self.finish_composer(state, true);
                } else if action_key(key, 'd') {
                    self.finish_composer(state, false);
                } else if action_key(key, 'e') || key.code == KeyCode::Esc {
                    self.overlay = Some(Overlay::Compose(state));
                } else {
                    self.overlay = Some(Overlay::DraftReady { state, summary });
                }
                return true;
            }
            Overlay::ComposeClose(state) => {
                if action_key(key, 's') {
                    self.finish_composer(state, false);
                } else if action_key(key, 'd') {
                    let _ = state.composer.remove_body();
                    self.notice = Some("Draft discarded".into());
                } else if key.code == KeyCode::Esc {
                    self.overlay = Some(Overlay::Compose(state));
                } else {
                    self.overlay = Some(Overlay::ComposeClose(state));
                }
                return true;
            }
            overlay => overlay,
        };
        match &mut overlay {
            Overlay::Help { scroll } => match key.code {
                KeyCode::Esc | KeyCode::Char('?') => {}
                KeyCode::Down | KeyCode::Char('j') => {
                    *scroll = scroll.saturating_add(1).min(14);
                    self.overlay = Some(overlay);
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    *scroll = scroll.saturating_sub(1);
                    self.overlay = Some(overlay);
                }
                _ => self.overlay = Some(overlay),
            },
            Overlay::Command(input) => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    let command = input.trim().to_owned();
                    self.run_command(&command);
                }
                KeyCode::Backspace => {
                    input.pop();
                    self.overlay = Some(overlay);
                }
                KeyCode::Char(character)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    input.push(character);
                    self.overlay = Some(overlay);
                }
                _ => self.overlay = Some(overlay),
            },
            Overlay::Folders { selected, items } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Down | KeyCode::Char('j') if *selected + 1 < items.len() => {
                    *selected += 1;
                    self.overlay = Some(overlay);
                }
                KeyCode::Up | KeyCode::Char('k') if *selected > 0 => {
                    *selected -= 1;
                    self.overlay = Some(overlay);
                }
                KeyCode::Enter => {
                    if let Some(folder) = items.get(*selected) {
                        self.active_folder = folder.name.clone();
                        self.reader = None;
                        self.rebuild_mailbox(None);
                    }
                }
                _ => self.overlay = Some(overlay),
            },
            Overlay::Links { selected, items } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Down | KeyCode::Char('j') if *selected + 1 < items.len() => {
                    *selected += 1;
                    self.overlay = Some(overlay);
                }
                KeyCode::Up | KeyCode::Char('k') if *selected > 0 => {
                    *selected -= 1;
                    self.overlay = Some(overlay);
                }
                KeyCode::Enter => {
                    if let Some(link) = items.get(*selected) {
                        self.notice = match opener::open(link) {
                            Ok(()) => Some("Link opened in the default browser.".into()),
                            Err(error) => Some(format!(
                                "Unable to open link: {error}. Copy it from the message."
                            )),
                        };
                    }
                }
                _ => self.overlay = Some(overlay),
            },
            Overlay::Attachments { selected, items } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Down | KeyCode::Char('j') if *selected + 1 < items.len() => {
                    *selected += 1;
                    self.overlay = Some(overlay);
                }
                KeyCode::Up | KeyCode::Char('k') if *selected > 0 => {
                    *selected -= 1;
                    self.overlay = Some(overlay);
                }
                KeyCode::Char('s') => self.save_active_attachment(*selected, false),
                KeyCode::Char('o') | KeyCode::Enter => self.save_active_attachment(*selected, true),
                _ => self.overlay = Some(overlay),
            },
            Overlay::Accounts { selected, items } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Down | KeyCode::Char('j') if *selected + 1 < items.len() => {
                    *selected += 1;
                    self.overlay = Some(overlay);
                }
                KeyCode::Up | KeyCode::Char('k') if *selected > 0 => {
                    *selected -= 1;
                    self.overlay = Some(overlay);
                }
                KeyCode::Enter => {
                    if let Some(item) = items.get(*selected) {
                        self.select_account(item.index);
                    }
                }
                _ => self.overlay = Some(overlay),
            },
            Overlay::DraftReady { .. } | Overlay::ComposeClose(_) | Overlay::Compose(_) => {
                unreachable!()
            }
        }
        true
    }

    fn run_command(&mut self, command: &str) {
        match command {
            "reload" => self.reload(),
            "filter all" => {
                self.filter = InboxFilter::All;
                self.refresh_inbox();
            }
            "filter unread" => {
                self.filter = InboxFilter::Unread;
                self.refresh_inbox();
            }
            "filter starred" => {
                self.filter = InboxFilter::Starred;
                self.refresh_inbox();
            }
            "compose" => self.start_draft(false),
            "reply" => self.start_draft(true),
            "help" => self.overlay = Some(Overlay::Help { scroll: 0 }),
            "quit" => self.running = false,
            _ if command.starts_with("open maildir ") => {
                let path = command.trim_start_matches("open maildir ").trim();
                if path.is_empty() {
                    self.notice = Some("Enter a Maildir path.".into());
                } else {
                    self.source = MailboxSource::Maildir(expand_path(path));
                    self.filter = InboxFilter::All;
                    self.query.clear();
                    self.undo = None;
                    self.mailbox_fingerprint = 0;
                    self.reload();
                }
            }
            "" => {}
            _ => {
                self.notice = Some(format!(
                    "Unknown command: {command}. Press ? to view commands."
                ));
            }
        }
    }

    fn start_draft(&mut self, reply: bool) {
        if !matches!(self.source, MailboxSource::Maildir(_)) {
            self.notice = Some(if matches!(self.source, MailboxSource::Unified(_)) {
                "Choose one account with p before composing.".into()
            } else {
                "Open a Maildir mailbox to save drafts.".into()
            });
            return;
        }
        let mail = reply
            .then(|| self.active_mail_id())
            .flatten()
            .and_then(|id| self.mails.iter().find(|mail| mail.id == id));
        if reply && mail.is_none() {
            self.notice = Some("Select a message to reply.".into());
            return;
        }
        match draft::Composer::new(
            mail,
            self.config.identity().as_deref(),
            self.config.signature.as_deref(),
        ) {
            Ok(composer) => {
                self.overlay = Some(Overlay::Compose(ComposeState {
                    field: if reply { ComposeState::BODY_FIELD } else { 0 },
                    composer,
                    error: None,
                }));
            }
            Err(error) => self.notice = Some(format!("Unable to create draft: {error}")),
        }
    }

    fn handle_compose_key(&mut self, mut state: ComposeState, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            match state.composer.summary(&base) {
                Ok(summary) => self.overlay = Some(Overlay::DraftReady { state, summary }),
                Err(error) => {
                    state.error = Some(error.to_string());
                    self.overlay = Some(Overlay::Compose(state));
                }
            }
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('d') {
            self.finish_composer(state, false);
            return;
        }
        match key.code {
            KeyCode::Esc => self.overlay = Some(Overlay::ComposeClose(state)),
            KeyCode::Tab | KeyCode::Down => {
                state.move_next();
                self.overlay = Some(Overlay::Compose(state));
            }
            KeyCode::BackTab | KeyCode::Up => {
                state.move_previous();
                self.overlay = Some(Overlay::Compose(state));
            }
            KeyCode::Enter if state.field == ComposeState::BODY_FIELD => {
                self.pending_editor = Some(state);
            }
            KeyCode::Enter => {
                state.move_next();
                self.overlay = Some(Overlay::Compose(state));
            }
            KeyCode::Backspace => {
                if let Some(value) = state.value_mut() {
                    value.pop();
                }
                state.error = None;
                self.overlay = Some(Overlay::Compose(state));
            }
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && state.field != ComposeState::BODY_FIELD =>
            {
                if let Some(value) = state.value_mut() {
                    value.push(character);
                }
                state.error = None;
                self.overlay = Some(Overlay::Compose(state));
            }
            _ => self.overlay = Some(Overlay::Compose(state)),
        }
    }

    fn edit_composer_body(
        &mut self,
        mut state: ComposeState,
    ) -> color_eyre::Result<DefaultTerminal> {
        execute!(stdout(), DisableMouseCapture)?;
        ratatui::restore();
        let result = self.run_editor(&state.composer.body_path);
        let terminal = ratatui::init();
        execute!(stdout(), EnableMouseCapture)?;

        if let Err(error) = result {
            state.error = Some(format!(
                "Unable to open $EDITOR: {error}. Check editor in config.toml."
            ));
        }
        self.overlay = Some(Overlay::Compose(state));
        Ok(terminal)
    }

    fn run_editor(&self, path: &Path) -> color_eyre::Result<()> {
        let command = self.config.editor_command();
        let (program, arguments) = command
            .split_first()
            .ok_or_else(|| color_eyre::eyre::eyre!("editor command is empty"))?;
        let status = Command::new(program).args(arguments).arg(path).status()?;
        if !status.success() {
            return Err(color_eyre::eyre::eyre!("editor exited with {status}"));
        }
        Ok(())
    }

    fn finish_composer(&mut self, mut state: ComposeState, send: bool) {
        let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let contents = if send {
            state.composer.message(&base)
        } else {
            state.composer.draft_message(&base)
        };
        let contents = match contents {
            Ok(contents) => contents,
            Err(error) => {
                state.error = Some(error.to_string());
                self.overlay = Some(Overlay::Compose(state));
                return;
            }
        };
        if send {
            match sender::send(&self.config.send_command, &contents) {
                Ok(()) => match self.source.save_sent(&contents) {
                    Ok(_) => {
                        let _ = state.composer.remove_body();
                        self.notice = Some("Message sent".into());
                    }
                    Err(error) => {
                        let _ = state.composer.remove_body();
                        self.notice = Some(format!(
                            "Message sent, but no Sent copy was saved: {error}. Check the Sent folder."
                        ));
                    }
                },
                Err(error) => match self.source.save_outbox(&contents) {
                    Ok(_) => {
                        let _ = state.composer.remove_body();
                        self.notice = Some(format!("Send failed: {error}. Queued in Outbox."));
                    }
                    Err(queue_error) => {
                        state.error = Some(format!(
                            "Send failed: {error}. Unable to queue: {queue_error}. Save the draft and retry."
                        ));
                        self.overlay = Some(Overlay::Compose(state));
                        return;
                    }
                },
            }
        } else {
            match self.source.save_draft(&contents) {
                Ok(_) => {
                    let _ = state.composer.remove_body();
                    self.notice = Some("Draft saved".into());
                }
                Err(error) => {
                    state.error = Some(format!(
                        "Unable to save draft: {error}. Check the Maildir and retry."
                    ));
                    self.overlay = Some(Overlay::Compose(state));
                    return;
                }
            }
        }
        self.refresh_fingerprint();
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
    spans.push(Span::styled(" esc ", theme::keycap()));
    spans.push(Span::styled(" dismiss", theme::muted()));
    Line::from(spans)
}

fn mail_folder(mail: &Mail) -> &str {
    mail.mailbox.as_deref().unwrap_or("INBOX")
}

fn draw_no_matches(frame: &mut Frame, area: Rect, folder: &str, folder_empty: bool) {
    let block = Block::bordered()
        .border_style(theme::border(false))
        .title_style(theme::title(false))
        .title(" workspace ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let message = if folder_empty {
        format!("No messages in {folder}\n\nPress g to choose another folder.")
    } else {
        "No matching conversations\n\nChange the filter or press esc to clear search.".into()
    };
    frame.render_widget(
        Paragraph::new(message)
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

fn draw_overlay(frame: &mut Frame, overlay: &Overlay) {
    let (width, height) = match overlay {
        Overlay::Help { .. } => (66, 23),
        Overlay::Command(_) => (64, 3),
        Overlay::DraftReady { .. } => (68, 11),
        Overlay::ComposeClose(_) => (58, 7),
        Overlay::Folders { items, .. } => (50, (items.len() as u16 + 2).min(18)),
        Overlay::Links { items, .. } => (72, (items.len() as u16 + 2).min(16)),
        Overlay::Attachments { items, .. } => (64, (items.len() as u16 + 2).min(16)),
        Overlay::Accounts { items, .. } => (52, (items.len() as u16 + 2).min(16)),
        Overlay::Compose(_) => (76, 17),
    };
    let screen = frame.area();
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width.min(screen.width)) / 2,
        screen.y + screen.height.saturating_sub(height.min(screen.height)) / 2,
        width.min(screen.width),
        height.min(screen.height),
    );
    frame.render_widget(Clear, area);
    match overlay {
        Overlay::Help { scroll } => {
            let text = [
                "navigation",
                "  j/k or arrows  select     tab  switch pane",
                "  enter          open       esc  close",
                "",
                "message actions",
                "  m  read/unread     s  star/unstar",
                "  a  archive         d  move to Trash",
                "  u  undo            r  reply       c  compose",
                "  l  open links       v  attachments",
                "  x  retry selected Outbox message",
                "",
                "inbox",
                "  /  search      g  folders      1/2/3  filters",
                "  p  accounts",
                "",
                "compose",
                "  tab  next field      shift+tab  previous field",
                "  enter on Body  $EDITOR    ctrl+s  review    ctrl+d  save",
                "",
                "commands",
                "  :reload                 :filter unread",
                "  :open maildir PATH      :compose  :reply  :quit",
            ]
            .join("\n");
            frame.render_widget(
                Paragraph::new(text)
                    .style(theme::text())
                    .block(
                        Block::bordered()
                            .border_style(theme::border(true))
                            .title_style(theme::title(true))
                            .title(" help · esc to close ")
                            .bg(theme::SURFACE),
                    )
                    .scroll((*scroll, 0))
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
        Overlay::Command(input) => {
            frame.render_widget(
                Paragraph::new(format!(":{input}_"))
                    .style(theme::text())
                    .block(
                        Block::bordered()
                            .border_style(theme::border(true))
                            .title_style(theme::title(true))
                            .title(" command ")
                            .bg(theme::SURFACE),
                    ),
                area,
            );
        }
        Overlay::DraftReady { summary, .. } => {
            let cc = if summary.cc.is_empty() {
                String::new()
            } else {
                format!("\nCc: {}", summary.cc)
            };
            let bcc = if summary.bcc.is_empty() {
                String::new()
            } else {
                format!("\nBcc: {}", summary.bcc)
            };
            frame.render_widget(
                Paragraph::new(format!(
                    "To: {}{cc}{bcc}\nSubject: {}\nAttachments: {}\n\n s  Send     e  Edit     d  Save draft     esc  Back",
                    summary.to,
                    summary.subject,
                    summary.attachments.len()
                ))
                .style(theme::text())
                .block(
                    Block::bordered()
                        .border_style(theme::border(true))
                        .title_style(theme::title(true))
                        .title(" choose an action ")
                        .bg(theme::SURFACE),
                )
                .wrap(Wrap { trim: true }),
                area,
            );
        }
        Overlay::ComposeClose(_) => {
            frame.render_widget(
                Paragraph::new(
                    "Close this message?\n\n s  Save draft     d  Discard message     esc  Continue editing",
                )
                .style(theme::text())
                .alignment(Alignment::Center)
                .block(
                    Block::bordered()
                        .border_style(theme::border(true))
                        .title_style(theme::title(true))
                        .title(" unsaved message ")
                        .bg(theme::SURFACE),
                )
                .wrap(Wrap { trim: true }),
                area,
            );
        }
        Overlay::Folders { selected, items } => {
            let rows: Vec<ListItem> = items
                .iter()
                .map(|folder| {
                    ListItem::new(format!(
                        "{:<20} {:>3} unread  {:>3} total",
                        folder.name, folder.unread, folder.total
                    ))
                })
                .collect();
            let list = List::new(rows)
                .style(theme::text())
                .highlight_symbol("▌ ")
                .highlight_style(theme::selection())
                .block(
                    Block::bordered()
                        .border_style(theme::border(true))
                        .title_style(theme::title(true))
                        .title(" folders ")
                        .bg(theme::SURFACE),
                );
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, area, &mut state);
        }
        Overlay::Links { selected, items } => {
            let rows: Vec<ListItem> = items
                .iter()
                .enumerate()
                .map(|(index, link)| ListItem::new(format!("{}. {link}", index + 1)))
                .collect();
            let list = List::new(rows)
                .style(theme::text())
                .highlight_symbol("▌ ")
                .highlight_style(theme::selection())
                .block(
                    Block::bordered()
                        .border_style(theme::border(true))
                        .title_style(theme::title(true))
                        .title(" links · enter to open ")
                        .bg(theme::SURFACE),
                );
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, area, &mut state);
        }
        Overlay::Attachments { selected, items } => {
            let rows: Vec<ListItem> = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    ListItem::new(format!(
                        "{}. {} · {} · {} bytes",
                        index + 1,
                        item.name,
                        item.media_type,
                        item.size
                    ))
                })
                .collect();
            let list = List::new(rows)
                .style(theme::text())
                .highlight_symbol("▌ ")
                .highlight_style(theme::selection())
                .block(
                    Block::bordered()
                        .border_style(theme::border(true))
                        .title_style(theme::title(true))
                        .title(" attachments · s save · o open ")
                        .bg(theme::SURFACE),
                );
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, area, &mut state);
        }
        Overlay::Accounts { selected, items } => {
            let rows: Vec<ListItem> = items
                .iter()
                .map(|item| ListItem::new(item.label.clone()))
                .collect();
            let list = List::new(rows)
                .style(theme::text())
                .highlight_symbol("▌ ")
                .highlight_style(theme::selection())
                .block(
                    Block::bordered()
                        .border_style(theme::border(true))
                        .title_style(theme::title(true))
                        .title(" accounts ")
                        .bg(theme::SURFACE),
                );
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, area, &mut state);
        }
        Overlay::Compose(state) => draw_compose(frame, area, state),
    }
}

fn draw_compose(frame: &mut Frame, area: Rect, state: &ComposeState) {
    let block = Block::bordered()
        .border_style(theme::border(true))
        .title_style(theme::title(true))
        .title(" compose ")
        .bg(theme::SURFACE);
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(7),
        Constraint::Length(2),
        Constraint::Min(1),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new(format!("From     {}", state.composer.from)).style(theme::muted()),
        rows[0],
    );
    let body = state.composer.body_stats().unwrap_or_default();
    let values = [
        ("To", state.composer.to.clone()),
        ("Cc", state.composer.cc.clone()),
        ("Bcc", state.composer.bcc.clone()),
        ("Subject", state.composer.subject.clone()),
        ("Files", state.composer.attachments.clone()),
        (
            "Body",
            format!("$EDITOR · {} lines · {} chars", body.0, body.1),
        ),
    ];
    let items: Vec<ListItem> = values
        .iter()
        .enumerate()
        .map(|(index, (label, value))| {
            let cursor = if index == state.field && index != ComposeState::BODY_FIELD {
                "_"
            } else {
                ""
            };
            ListItem::new(format!("{label:<8} {value}{cursor}"))
        })
        .collect();
    let list = List::new(items)
        .style(theme::text())
        .highlight_symbol("▌ ")
        .highlight_style(theme::selection());
    let mut selected = ListState::default().with_selected(Some(state.field));
    frame.render_stateful_widget(list, rows[1], &mut selected);
    let feedback = state
        .error
        .as_deref()
        .unwrap_or("Use ; between attachment paths.");
    frame.render_widget(
        Paragraph::new(feedback)
            .style(if state.error.is_some() {
                theme::title(true)
            } else {
                theme::muted()
            })
            .wrap(Wrap { trim: true }),
        rows[2],
    );
    frame.render_widget(
        Paragraph::new(
            "tab / shift+tab  move field    enter on Body  open $EDITOR\nctrl+s  review    ctrl+d  save draft    esc  close",
        )
        .style(theme::muted()),
        rows[3],
    );
}

fn reader_is_docked(width: u16, reader_open: bool) -> bool {
    reader_open && width >= MIN_DOCK_WIDTH
}

fn action_key(key: KeyEvent, expected: char) -> bool {
    matches!(key.code, KeyCode::Char(value) if value.eq_ignore_ascii_case(&expected))
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}

struct CliOptions {
    source: Option<MailboxSource>,
    config_path: Option<PathBuf>,
    doctor: bool,
}

fn parse_options(
    args: impl IntoIterator<Item = OsString>,
) -> color_eyre::Result<Option<CliOptions>> {
    let mut args = args.into_iter();
    let mut source = None;
    let mut config_path = None;
    let mut doctor = false;

    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--maildir") => {
                let path = args
                    .next()
                    .ok_or_else(|| color_eyre::eyre::eyre!("--maildir needs a path"))?;
                source = Some(MailboxSource::Maildir(PathBuf::from(path)));
            }
            Some("--config") => {
                let path = args
                    .next()
                    .ok_or_else(|| color_eyre::eyre::eyre!("--config needs a path"))?;
                config_path = Some(PathBuf::from(path));
            }
            Some("--demo") => source = Some(MailboxSource::Demo),
            Some("doctor") => doctor = true,
            Some("--help" | "-h") => return Ok(None),
            Some(value) => return Err(color_eyre::eyre::eyre!("unknown option: {value}")),
            None => return Err(color_eyre::eyre::eyre!("option is not valid utf-8")),
        }
    }

    Ok(Some(CliOptions {
        source,
        config_path,
        doctor,
    }))
}

fn print_help() {
    println!("termail [doctor] [--config PATH] [--demo | --maildir PATH]");
    println!();
    println!("  --demo          open the built-in mailbox");
    println!("  --maildir PATH  open a Maildir mailbox");
    println!("  --config PATH   load a config file");
    println!("  doctor          check account, Maildir, editor, and sender setup");
}

fn expand_path(value: &str) -> PathBuf {
    if let Some(relative) = value.strip_prefix("~/")
        && let Some(base) = BaseDirs::new()
    {
        return base.home_dir().join(relative);
    }
    PathBuf::from(value)
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, path::PathBuf};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    use crate::{draft::Composer, mailbox::MailboxSource};

    use super::{
        ComposeState, MailboxState, Overlay, action_key, draw_mailbox_state, draw_overlay,
        parse_options, reader_is_docked,
    };

    #[test]
    fn docks_readers_only_when_they_fit() {
        assert!(reader_is_docked(120, true));
        assert!(!reader_is_docked(100, true));
        assert!(!reader_is_docked(120, false));
    }

    #[test]
    fn accepts_shifted_action_keys() {
        assert!(action_key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
            's'
        ));
        assert!(action_key(
            KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT),
            's'
        ));
        assert!(!action_key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            's'
        ));
    }

    #[test]
    fn reads_maildir_source_from_arguments() {
        let options = parse_options([OsString::from("--maildir"), OsString::from("/tmp/mail")])
            .unwrap()
            .unwrap();

        assert_eq!(
            options.source,
            Some(MailboxSource::Maildir(PathBuf::from("/tmp/mail")))
        );
    }

    #[test]
    fn reads_doctor_command() {
        let options = parse_options([OsString::from("doctor")]).unwrap().unwrap();

        assert!(options.doctor);
    }

    #[test]
    fn renders_mailbox_states_with_recovery() {
        assert!(render_state(&MailboxState::Loading).contains("Loading demo mailbox"));
        assert!(render_state(&MailboxState::Empty).contains("No messages found"));

        let screen = render_state(&MailboxState::Error("permission denied".into()));
        assert!(screen.contains("Unable to load mailbox"));
        assert!(screen.contains("press r to retry"));
    }

    #[test]
    fn renders_help_and_command_overlays() {
        assert!(render_overlay(&Overlay::Help { scroll: 0 }).contains("message actions"));
        assert!(
            render_overlay(&Overlay::Command("filter unread".into())).contains(":filter unread_")
        );
    }

    #[test]
    fn renders_the_builtin_compose_form() {
        let composer = Composer::new(None, Some("Tuna <tuna@example.com>"), None).unwrap();
        let screen = render_overlay(&Overlay::Compose(ComposeState {
            composer,
            field: 0,
            error: None,
        }));

        assert!(screen.contains("From"));
        assert!(screen.contains("To       _"));
        assert!(screen.contains("$EDITOR"));
        assert!(screen.contains("ctrl+s"));
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

    fn render_overlay(overlay: &Overlay) -> String {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw_overlay(frame, overlay)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }
}
