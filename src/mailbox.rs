use std::{fmt, io, path::PathBuf};

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

        let report = MailboxSource::Maildir(root.clone()).load().unwrap();
        fs::remove_dir_all(root).unwrap();

        assert_eq!(report.mails.len(), 1);
        assert_eq!(report.mails[0].id, "root@example.com");
        assert_eq!(report.mails[0].to, ["Tuna <tuna@example.com>"]);
        assert!(report.mails[0].starred);
        assert!(!report.mails[0].unread);
        assert_eq!(report.attempted, 2);
        assert_eq!(report.skipped.len(), 1);
    }
}
