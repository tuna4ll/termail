use std::{
    collections::hash_map::DefaultHasher,
    fmt, fs,
    hash::{Hash, Hasher},
    io,
    path::{Path, PathBuf},
};

use mail_parser::{
    Address, MessageParser,
    mailbox::maildir::{Flag, FolderIterator},
};

use crate::mail::{Mail, demo_mails};

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

    for folder in FolderIterator::new(path, Some("."))? {
        let folder = folder?;
        let mailbox = folder.name().unwrap_or("INBOX").to_owned();
        if ["Archive", "Trash", "Drafts"].contains(&mailbox.as_str()) {
            continue;
        }
        for message in folder {
            attempted += 1;
            let message = message?;
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

            mails.push(Mail {
                id,
                from,
                to,
                cc,
                subject: parsed.subject().unwrap_or("(no subject)").into(),
                body: parsed.body_text(0).unwrap_or_default().into_owned(),
                date,
                timestamp,
                reply_to,
                references,
                unread,
                starred,
                attachment_count: parsed.attachments().count(),
                source_path: Some(message.path().to_path_buf()),
                mailbox: Some(mailbox.clone()),
            });
        }
    }

    mails.sort_by_key(|mail| mail.timestamp.unwrap_or_default());
    Ok(LoadReport {
        mails,
        skipped,
        attempted,
    })
}

fn rename_with_flag(path: &Path, flag: char, enabled: bool) -> io::Result<MaildirChange> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid Maildir filename."))?;
    let (base, current) = file_name.rsplit_once(":2,").unwrap_or((file_name, ""));
    let mut flags: Vec<char> = current.chars().filter(|value| *value != flag).collect();
    if enabled {
        flags.push(flag);
    }
    flags.sort_unstable();
    flags.dedup();
    let file_name = format!("{base}:2,{}", flags.into_iter().collect::<String>());
    let parent = if enabled && flag == 'S' && path.parent().is_some_and(|dir| dir.ends_with("new"))
    {
        path.parent().unwrap().parent().unwrap().join("cur")
    } else {
        path.parent().unwrap().to_path_buf()
    };
    let after = parent.join(file_name);
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
        let unique = format!(
            "{}.termail-{}",
            file_name.to_string_lossy(),
            std::process::id()
        );
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

fn fingerprint_dir(path: &Path, fingerprint: &mut u64) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            if ["tmp", ".Archive", ".Trash", ".Drafts"].contains(&name) {
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
            root.join("cur/message:2,FS"),
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
        assert!(source.load().unwrap().mails.is_empty());
        change.undo().unwrap();
        assert_eq!(source.load().unwrap().mails.len(), 1);

        fs::remove_dir_all(root).unwrap();
    }
}
