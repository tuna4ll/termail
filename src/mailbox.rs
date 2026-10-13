use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    fmt, fs,
    hash::{Hash, Hasher},
    io,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use mail_parser::{
    Address, MessageParser, MimeHeaders,
    decoders::html::html_to_text,
    mailbox::maildir::{Flag, FolderIterator},
};
use serde::{Deserialize, Serialize};

use crate::mail::{Attachment, Mail, demo_mails};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MailboxSource {
    Demo,
    Maildir(PathBuf),
}

#[derive(Debug)]
pub struct LoadReport {
    pub mails: Vec<Mail>,
    pub skipped: Vec<String>,
    pub attempted: usize,
}

#[derive(Debug)]
pub struct LoadError {
    message: String,
}

#[derive(Clone, Debug)]
pub struct MaildirChange {
    before: PathBuf,
    after: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CachedMessage {
    internal_date: u64,
    length: usize,
    digest: u64,
    mail: Mail,
}

impl MailboxSource {
    pub fn label(&self) -> String {
        match self {
            Self::Demo => "demo mailbox".into(),
            Self::Maildir(path) => path.display().to_string(),
        }
    }

    pub fn load(&self) -> Result<LoadReport, LoadError> {
        match self {
            Self::Demo => Ok(LoadReport {
                mails: demo_mails(),
                skipped: Vec::new(),
                attempted: 7,
            }),
            Self::Maildir(path) => load_maildir(path),
        }
    }

    pub fn fingerprint(&self) -> Result<u64, LoadError> {
        match self {
            Self::Demo => Ok(0),
            Self::Maildir(path) => mailbox_fingerprint(path).map_err(Into::into),
        }
    }

    pub fn set_seen(&self, mail: &Mail, seen: bool) -> Result<Option<MaildirChange>, LoadError> {
        self.set_flag(mail, 'S', seen)
    }

    pub fn set_starred(
        &self,
        mail: &Mail,
        starred: bool,
    ) -> Result<Option<MaildirChange>, LoadError> {
        self.set_flag(mail, 'F', starred)
    }

    pub fn archive(&self, mail: &Mail) -> Result<Option<MaildirChange>, LoadError> {
        self.move_to(mail, ".Archive")
    }

    pub fn trash(&self, mail: &Mail) -> Result<Option<MaildirChange>, LoadError> {
        self.move_to(mail, ".Trash")
    }

    pub fn mark_sent(&self, mail: &Mail) -> Result<Option<MaildirChange>, LoadError> {
        self.move_to(mail, ".Sent")
    }

    pub fn save_draft(&self, contents: &[u8]) -> Result<PathBuf, LoadError> {
        let Self::Maildir(root) = self else {
            return Err(LoadError {
                message: "Open a Maildir mailbox to save drafts.".into(),
            });
        };
        save_message(root, ".Drafts", "new", contents, "").map_err(Into::into)
    }

    pub fn save_outbox(&self, contents: &[u8]) -> Result<PathBuf, LoadError> {
        let Self::Maildir(root) = self else {
            return Err(LoadError {
                message: "Open a Maildir mailbox to queue messages.".into(),
            });
        };
        save_message(root, ".Outbox", "new", contents, "").map_err(Into::into)
    }

    pub fn save_sent(&self, contents: &[u8]) -> Result<PathBuf, LoadError> {
        let Self::Maildir(root) = self else {
            return Err(LoadError {
                message: "Open a Maildir mailbox to save sent messages.".into(),
            });
        };
        save_message(root, ".Sent", "cur", contents, "!2,S").map_err(Into::into)
    }

    pub fn save_attachment(
        &self,
        mail: &Mail,
        index: usize,
        destination: &Path,
    ) -> Result<PathBuf, LoadError> {
        let Self::Maildir(root) = self else {
            return Err(LoadError {
                message: "Open a Maildir mailbox to save attachments.".into(),
            });
        };
        let source = mail.source_path.as_ref().ok_or_else(|| LoadError {
            message: "Message has no Maildir path.".into(),
        })?;
        if !source.starts_with(root) {
            return Err(LoadError {
                message: "Message is outside the active Maildir.".into(),
            });
        }
        let contents = fs::read(source)?;
        let parsed = MessageParser::default()
            .parse(&contents)
            .ok_or_else(|| LoadError {
                message: "Unable to parse the message attachment.".into(),
            })?;
        let part = parsed.attachments().nth(index).ok_or_else(|| LoadError {
            message: "Attachment is no longer available.".into(),
        })?;
        let fallback = format!("attachment-{}", index + 1);
        let name = safe_file_name(part.attachment_name().unwrap_or(&fallback));
        fs::create_dir_all(destination)?;
        let path = available_path(destination, &name);
        fs::write(&path, part.contents())?;
        Ok(path)
    }

    fn set_flag(
        &self,
        mail: &Mail,
        flag: char,
        enabled: bool,
    ) -> Result<Option<MaildirChange>, LoadError> {
        let Self::Maildir(root) = self else {
            return Ok(None);
        };
        let path = mail.source_path.as_ref().ok_or_else(|| LoadError {
            message: "Message has no Maildir path.".into(),
        })?;
        if !path.starts_with(root) {
            return Err(LoadError {
                message: "Message is outside the active Maildir.".into(),
            });
        }
        rename_with_flag(path, flag, enabled)
            .map(Some)
            .map_err(Into::into)
    }

    fn move_to(&self, mail: &Mail, folder: &str) -> Result<Option<MaildirChange>, LoadError> {
        let Self::Maildir(root) = self else {
            return Ok(None);
        };
        let path = mail.source_path.as_ref().ok_or_else(|| LoadError {
            message: "Message has no Maildir path.".into(),
        })?;
        if !path.starts_with(root) {
            return Err(LoadError {
                message: "Message is outside the active Maildir.".into(),
            });
        }
        move_to_folder(path, &root.join(folder))
            .map(Some)
            .map_err(Into::into)
    }
}

impl MaildirChange {
    pub fn after_path(&self) -> &Path {
        &self.after
    }

    pub fn undo(self) -> Result<(), LoadError> {
        fs::rename(self.after, self.before).map_err(Into::into)
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LoadError {}

impl From<io::Error> for LoadError {
    fn from(error: io::Error) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

fn load_maildir(path: &PathBuf) -> Result<LoadReport, LoadError> {
    let mut mails = Vec::new();
    let mut skipped = Vec::new();
    let mut attempted = 0;
    let cache = load_cache(path).unwrap_or_default();
    let mut next_cache = BTreeMap::new();

    for folder in FolderIterator::new(path, Some("."))? {
        let folder = folder?;
        let mailbox = folder.name().unwrap_or("INBOX").to_owned();
        for message in folder {
            attempted += 1;
            let message = message?;
            let cache_key = message.path().to_string_lossy().into_owned();
            if let Some(cached) = cache.get(&cache_key)
                && cached.internal_date == message.internal_date()
                && cached.length == message.contents().len()
                && cached.digest == content_digest(message.contents())
            {
                mails.push(cached.mail.clone());
                next_cache.insert(cache_key, cached.clone());
                continue;
            }
            let Some(parsed) = MessageParser::default().parse(message.contents()) else {
                skipped.push(message.path().display().to_string());
                continue;
            };
            let fallback_id = message.path().display().to_string();
            let id = parsed.message_id().map(clean_id).unwrap_or(fallback_id);
            let from = parsed
                .from()
                .and_then(Address::first)
                .map(format_address)
                .unwrap_or_else(|| "unknown sender".into());
            let to = parsed.to().map(format_addresses).unwrap_or_default();
            let cc = parsed.cc().map(format_addresses).unwrap_or_default();
            let date = parsed.date().map(|date| date.to_rfc3339());
            let timestamp = parsed
                .date()
                .map(|date| date.to_timestamp())
                .or_else(|| Some(message.internal_date() as i64));
            let reply_to = parsed.in_reply_to().as_text().map(clean_id);
            let references = parsed
                .references()
                .as_text_list()
                .unwrap_or_default()
                .iter()
                .map(|value| clean_id(value))
                .collect();
            let unread = !message.flags().contains(&Flag::Seen);
            let starred = message.flags().contains(&Flag::Flagged);
            let html = parsed.html_body_count() > 0;
            let html_body = parsed.body_html(0);
            let body = parsed
                .body_text(0)
                .map(|body| body.into_owned())
                .or_else(|| html_body.as_deref().map(html_to_text))
                .unwrap_or_default();
            let links = html_body.as_deref().map(extract_links).unwrap_or_default();
            let attachments: Vec<Attachment> = parsed
                .attachments()
                .enumerate()
                .map(|(index, part)| {
                    let media_type = part
                        .content_type()
                        .map(|kind| {
                            let subtype = kind.c_subtype.as_deref().unwrap_or("octet-stream");
                            format!("{}/{subtype}", kind.c_type)
                        })
                        .unwrap_or_else(|| "application/octet-stream".into());
                    Attachment {
                        name: part
                            .attachment_name()
                            .map(str::to_owned)
                            .unwrap_or_else(|| format!("attachment-{}", index + 1)),
                        media_type,
                        size: part.contents().len(),
                    }
                })
                .collect();

            let mail = Mail {
                id,
                from,
                to,
                cc,
                subject: parsed.subject().unwrap_or("(no subject)").into(),
                body,
                date,
                timestamp,
                reply_to,
                references,
                unread,
                starred,
                attachment_count: attachments.len(),
                attachments,
                links,
                html,
                account: None,
                source_path: Some(message.path().to_path_buf()),
                mailbox: Some(mailbox.clone()),
            };
            next_cache.insert(
                cache_key,
                CachedMessage {
                    internal_date: message.internal_date(),
                    length: message.contents().len(),
                    digest: content_digest(message.contents()),
                    mail: mail.clone(),
                },
            );
            mails.push(mail);
        }
    }

    mails.sort_by_key(|mail| mail.timestamp.unwrap_or_default());
    let _ = save_cache(path, &next_cache);
    Ok(LoadReport {
        mails,
        skipped,
        attempted,
    })
}

fn cache_path(maildir: &Path) -> Option<PathBuf> {
    let mut hasher = DefaultHasher::new();
    maildir.hash(&mut hasher);
    ProjectDirs::from("com", "tunakilic", "termail").map(|dirs| {
        dirs.cache_dir()
            .join(format!("maildir-{:016x}.json", hasher.finish()))
    })
}

fn load_cache(maildir: &Path) -> io::Result<BTreeMap<String, CachedMessage>> {
    let Some(path) = cache_path(maildir) else {
        return Ok(BTreeMap::new());
    };
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let contents = fs::read(path)?;
    serde_json::from_slice(&contents)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn save_cache(maildir: &Path, cache: &BTreeMap<String, CachedMessage>) -> io::Result<()> {
    let Some(path) = cache_path(maildir) else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    let contents = serde_json::to_vec(cache)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    fs::write(&temporary, contents)?;
    if path.exists() {
        fs::remove_file(&path)?;
    }
    fs::rename(temporary, path)
}

fn content_digest(contents: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    contents.hash(&mut hasher);
    hasher.finish()
}

fn rename_with_flag(path: &Path, flag: char, enabled: bool) -> io::Result<MaildirChange> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid Maildir filename."))?;
    let (base, current, separator) = if let Some((base, flags)) = file_name.rsplit_once(":2,") {
        (base, flags, ":2,")
    } else if let Some((base, flags)) = file_name.rsplit_once("!2,") {
        (base, flags, "!2,")
    } else if cfg!(target_os = "windows") {
        (file_name, "", "!2,")
    } else {
        (file_name, "", ":2,")
    };
    let mut flags: Vec<char> = current.chars().filter(|value| *value != flag).collect();
    if enabled {
        flags.push(flag);
    }
    flags.sort_unstable();
    flags.dedup();
    let file_name = format!("{base}{separator}{}", flags.into_iter().collect::<String>());
    let parent = if enabled && flag == 'S' && path.parent().is_some_and(|dir| dir.ends_with("new"))
    {
        path.parent().unwrap().parent().unwrap().join("cur")
    } else {
        path.parent().unwrap().to_path_buf()
    };
    let after = parent.join(file_name);
    if after != path && after.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "A message with the target flags already exists.",
        ));
    }
    fs::rename(path, &after)?;
    Ok(MaildirChange {
        before: path.to_path_buf(),
        after,
    })
}

fn move_to_folder(path: &Path, folder: &Path) -> io::Result<MaildirChange> {
    for child in ["cur", "new", "tmp"] {
        fs::create_dir_all(folder.join(child))?;
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid Maildir filename."))?;
    let mut after = folder.join("cur").join(file_name);
    if after.exists() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let unique = format!("{}.termail-{nonce}", file_name.to_string_lossy());
        after = folder.join("cur").join(unique);
    }
    fs::rename(path, &after)?;
    Ok(MaildirChange {
        before: path.to_path_buf(),
        after,
    })
}

fn mailbox_fingerprint(path: &Path) -> io::Result<u64> {
    let mut fingerprint = 0;
    fingerprint_dir(path, &mut fingerprint)?;
    Ok(fingerprint)
}

fn save_message(
    root: &Path,
    folder: &str,
    location: &str,
    contents: &[u8],
    suffix: &str,
) -> io::Result<PathBuf> {
    let folder = root.join(folder);
    for child in ["cur", "new", "tmp"] {
        fs::create_dir_all(folder.join(child))?;
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!("{nonce}.{}.termail{suffix}", std::process::id());
    let temporary = folder.join("tmp").join(&name);
    let destination = folder.join(location).join(name);
    fs::write(&temporary, contents)?;
    fs::rename(temporary, &destination)?;
    Ok(destination)
}

fn safe_file_name(value: &str) -> String {
    let mut name: String = value
        .chars()
        .map(|character| {
            if character.is_control() || "<>:\"/\\|?*".contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    name = name.trim_matches([' ', '.']).to_owned();
    if name.is_empty() {
        return "attachment".into();
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    ) {
        name.insert(0, '_');
    }
    name
}

fn available_path(directory: &Path, name: &str) -> PathBuf {
    let path = directory.join(name);
    if !path.exists() {
        return path;
    }
    let name = Path::new(name);
    let stem = name
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("attachment");
    let extension = name.extension().and_then(|value| value.to_str());
    for number in 2.. {
        let candidate = match extension {
            Some(extension) => directory.join(format!("{stem} ({number}).{extension}")),
            None => directory.join(format!("{stem} ({number})")),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

fn fingerprint_dir(path: &Path, fingerprint: &mut u64) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            if name == "tmp" {
                continue;
            }
            fingerprint_dir(&path, fingerprint)?;
        } else if path.is_file() {
            let metadata = entry.metadata()?;
            let mut hasher = DefaultHasher::new();
            path.hash(&mut hasher);
            metadata.len().hash(&mut hasher);
            metadata.modified()?.hash(&mut hasher);
            *fingerprint ^= hasher.finish();
        }
    }
    Ok(())
}

fn clean_id(value: &str) -> String {
    value
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .into()
}

fn format_addresses(addresses: &Address<'_>) -> Vec<String> {
    addresses.iter().map(format_address).collect()
}

fn format_address(address: &mail_parser::Addr<'_>) -> String {
    match (address.name.as_deref(), address.address.as_deref()) {
        (Some(name), Some(email)) => format!("{name} <{email}>"),
        (None, Some(email)) => email.into(),
        (Some(name), None) => name.into(),
        (None, None) => "unknown".into(),
    }
}

fn extract_links(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut links = Vec::new();
    let mut offset = 0;

    while let Some(position) = lower[offset..].find("href") {
        let start = offset + position + 4;
        let Some(equal) = html[start..].find('=') else {
            break;
        };
        let value_start = start + equal + 1;
        let value = html[value_start..].trim_start();
        let skipped = html[value_start..].len() - value.len();
        let Some(first) = value.chars().next() else {
            break;
        };
        let (link, consumed) = if matches!(first, '\'' | '"') {
            let rest = &value[first.len_utf8()..];
            let end = rest.find(first).unwrap_or(rest.len());
            (&rest[..end], end + first.len_utf8() * 2)
        } else {
            let end = value
                .find(|character: char| character.is_whitespace() || character == '>')
                .unwrap_or(value.len());
            (&value[..end], end)
        };
        if (link.starts_with("https://") || link.starts_with("http://"))
            && !links.iter().any(|existing| existing == link)
        {
            links.push(link.to_owned());
        }
        offset = value_start + skipped + consumed;
    }
    links
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::MailboxSource;

    #[test]
    fn loads_maildir_headers_and_flags() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("termail-maildir-{nonce}"));
        fs::create_dir_all(root.join("cur")).unwrap();
        fs::create_dir_all(root.join("new")).unwrap();
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::write(
            root.join("cur/message!2,FS"),
            b"From: Ada <ada@example.com>\r\nTo: Tuna <tuna@example.com>\r\nSubject: Build ready\r\nMessage-ID: <root@example.com>\r\nDate: Thu, 1 Oct 2026 09:00:00 +0300\r\n\r\nPlease review it.",
        )
        .unwrap();
        fs::write(root.join("new/broken"), []).unwrap();

        let source = MailboxSource::Maildir(root.clone());
        let report = source.load().unwrap();

        assert_eq!(report.mails.len(), 1);
        assert_eq!(report.mails[0].id, "root@example.com");
        assert_eq!(report.mails[0].to, ["Tuna <tuna@example.com>"]);
        assert!(report.mails[0].starred);
        assert!(!report.mails[0].unread);
        assert_eq!(report.attempted, 2);
        assert_eq!(report.skipped.len(), 1);

        let fingerprint = source.fingerprint().unwrap();
        let change = source.set_seen(&report.mails[0], false).unwrap().unwrap();
        assert_ne!(source.fingerprint().unwrap(), fingerprint);
        assert!(source.load().unwrap().mails[0].unread);
        change.undo().unwrap();
        assert!(!source.load().unwrap().mails[0].unread);

        let change = source.archive(&report.mails[0]).unwrap().unwrap();
        assert_eq!(
            source.load().unwrap().mails[0].mailbox.as_deref(),
            Some("Archive")
        );
        change.undo().unwrap();
        assert_eq!(source.load().unwrap().mails.len(), 1);

        let draft = source.save_draft(b"To: ada@example.com\n\nhello").unwrap();
        assert!(draft.starts_with(root.join(".Drafts/new")));
        assert_eq!(fs::read(draft).unwrap(), b"To: ada@example.com\n\nhello");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_html_links_and_attachments() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("termail-rich-{nonce}"));
        for child in ["cur", "new", "tmp"] {
            fs::create_dir_all(root.join(child)).unwrap();
        }
        fs::write(
            root.join("new/message"),
            b"From: ada@example.com\r\nSubject: Report\r\nMessage-ID: <rich@example.com>\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Read <a href=\"https://example.com/report\">the report</a>.</p>\r\n--x\r\nContent-Type: text/plain; name=notes.txt\r\nContent-Disposition: attachment; filename=notes.txt\r\n\r\nhello\r\n--x--\r\n",
        )
        .unwrap();

        let source = MailboxSource::Maildir(root.clone());
        let mail = source.load().unwrap().mails.remove(0);

        assert!(mail.html);
        assert!(mail.body.contains("Read the report"));
        assert_eq!(mail.links, ["https://example.com/report"]);
        assert_eq!(mail.attachments[0].name, "notes.txt");
        let saved = source
            .save_attachment(&mail, 0, &root.join("downloads"))
            .unwrap();
        assert_eq!(saved.file_name().unwrap(), "notes.txt");
        assert_eq!(fs::read_to_string(saved).unwrap().trim(), "hello");
        fs::remove_dir_all(root).unwrap();
    }
}
