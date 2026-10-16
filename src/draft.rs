use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::STANDARD};

use crate::mail::Mail;

#[derive(Clone, Debug)]
pub struct Summary {
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub attachments: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct Composer {
    pub from: String,
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub attachments: String,
    pub body_path: PathBuf,
    reply_to: Option<String>,
    references: Vec<String>,
    message_id: String,
}

#[derive(Debug)]
pub struct DraftError(String);

impl fmt::Display for DraftError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for DraftError {}

impl Composer {
    pub fn new(
        reply_to: Option<&Mail>,
        identity: Option<&str>,
        signature: Option<&str>,
    ) -> io::Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let body_path =
            std::env::temp_dir().join(format!("termail-body-{}-{nonce}.txt", std::process::id()));
        let body = signature
            .filter(|value| !value.trim().is_empty())
            .map(|value| format!("\n\n-- \n{}", value.trim()))
            .unwrap_or_default();
        fs::write(&body_path, body)?;
        let subject = reply_to
            .map(|mail| {
                if mail.subject.to_lowercase().starts_with("re:") {
                    mail.subject.clone()
                } else {
                    format!("Re: {}", mail.subject)
                }
            })
            .unwrap_or_default();
        let mut references = reply_to
            .map(|mail| mail.references.clone())
            .unwrap_or_default();
        if let Some(mail) = reply_to {
            references.push(mail.id.clone());
            references.dedup();
        }
        Ok(Self {
            from: identity.unwrap_or_default().into(),
            to: reply_to.map(|mail| mail.from.clone()).unwrap_or_default(),
            cc: String::new(),
            bcc: String::new(),
            subject,
            attachments: String::new(),
            body_path,
            reply_to: reply_to.map(|mail| mail.id.clone()),
            references,
            message_id: format!("termail-{}-{nonce}@localhost", std::process::id()),
        })
    }

    pub fn summary(&self, base: &Path) -> Result<Summary, DraftError> {
        inspect(self.source()?.as_bytes(), base)
    }

    pub fn message(&self, base: &Path) -> Result<Vec<u8>, DraftError> {
        prepare(self.source()?.as_bytes(), base)
    }

    pub fn draft_message(&self, base: &Path) -> Result<Vec<u8>, DraftError> {
        prepare_message(self.source()?.as_bytes(), base, false)
    }

    pub fn body_stats(&self) -> io::Result<(usize, usize)> {
        let body = fs::read_to_string(&self.body_path)?;
        Ok((body.lines().count(), body.chars().count()))
    }

    pub fn remove_body(&self) -> io::Result<()> {
        if self.body_path.exists() {
            fs::remove_file(&self.body_path)?;
        }
        Ok(())
    }

    fn source(&self) -> Result<String, DraftError> {
        let body = fs::read_to_string(&self.body_path).map_err(|error| {
            DraftError(format!(
                "Unable to read message body {}: {error}",
                self.body_path.display()
            ))
        })?;
        let mut headers = vec![
            format!("From: {}", one_line(&self.from)),
            format!("To: {}", one_line(&self.to)),
            format!("Cc: {}", one_line(&self.cc)),
            format!("Bcc: {}", one_line(&self.bcc)),
            format!("Subject: {}", one_line(&self.subject)),
        ];
        headers.extend(
            self.attachments
                .split(';')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| format!("Attach: {}", one_line(value))),
        );
        if let Some(reply_to) = &self.reply_to {
            headers.push(format!("In-Reply-To: <{}>", one_line(reply_to)));
        }
        if !self.references.is_empty() {
            headers.push(format!(
                "References: {}",
                self.references
                    .iter()
                    .map(|id| format!("<{}>", one_line(id)))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
        headers.push(format!("Message-ID: <{}>", self.message_id));
        headers.push("MIME-Version: 1.0".into());
        headers.push("Content-Type: text/plain; charset=utf-8".into());
        Ok(format!("{}\n\n{body}", headers.join("\n")))
    }
}

impl Drop for Composer {
    fn drop(&mut self) {
        let _ = self.remove_body();
    }
}

fn one_line(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

pub fn inspect(contents: &[u8], base: &Path) -> Result<Summary, DraftError> {
    inspect_message(contents, base, true)
}

fn inspect_message(
    contents: &[u8],
    base: &Path,
    require_recipient: bool,
) -> Result<Summary, DraftError> {
    let text = String::from_utf8_lossy(contents);
    let (headers, _) = split_message(&text);
    let to = header_values(headers, "to").join(", ");
    let cc = header_values(headers, "cc").join(", ");
    let bcc = header_values(headers, "bcc").join(", ");
    if require_recipient && to.trim().is_empty() && cc.trim().is_empty() && bcc.trim().is_empty() {
        return Err(DraftError(
            "Add at least one recipient to To, Cc, or Bcc.".into(),
        ));
    }
    let attachments: Vec<PathBuf> = header_values(headers, "attach")
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .map(|value| resolve_path(value.trim(), base))
        .collect();
    if let Some(path) = attachments.iter().find(|path| !path.is_file()) {
        return Err(DraftError(format!(
            "Attachment not found: {}. Fix the Attach line and try again.",
            path.display()
        )));
    }
    Ok(Summary {
        to,
        cc,
        bcc,
        subject: header_values(headers, "subject")
            .into_iter()
            .next()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "(no subject)".into()),
        attachments,
    })
}

pub fn prepare(contents: &[u8], base: &Path) -> Result<Vec<u8>, DraftError> {
    prepare_message(contents, base, true)
}

fn prepare_message(
    contents: &[u8],
    base: &Path,
    require_recipient: bool,
) -> Result<Vec<u8>, DraftError> {
    let summary = inspect_message(contents, base, require_recipient)?;
    if summary.attachments.is_empty() {
        let text = String::from_utf8_lossy(contents);
        let (headers, body) = split_message(&text);
        let headers = headers
            .lines()
            .filter(|line| !line.to_ascii_lowercase().starts_with("attach:"))
            .collect::<Vec<_>>()
            .join("\r\n");
        return Ok(format!("{headers}\r\n\r\n{body}").into_bytes());
    }
    let text = String::from_utf8_lossy(contents);
    let (headers, body) = split_message(&text);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let boundary = format!("termail-{nonce}");
    let mut message = String::new();
    for line in headers.lines() {
        let lower = line.to_ascii_lowercase();
        if !lower.starts_with("attach:")
            && !lower.starts_with("mime-version:")
            && !lower.starts_with("content-type:")
        {
            message.push_str(line.trim_end_matches('\r'));
            message.push_str("\r\n");
        }
    }
    message.push_str("MIME-Version: 1.0\r\n");
    message.push_str(&format!(
        "Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\r\n"
    ));
    message.push_str(&format!(
        "--{boundary}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{}\r\n",
        body.trim_start_matches(['\r', '\n'])
    ));
    for path in summary.attachments {
        let bytes = fs::read(&path).map_err(|error| {
            DraftError(format!(
                "Unable to read attachment {}: {error}",
                path.display()
            ))
        })?;
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .map(one_line)
            .unwrap_or_else(|| "attachment".into());
        let media_type = mime_guess::from_path(&path).first_or_octet_stream();
        message.push_str(&format!(
            "--{boundary}\r\nContent-Type: {media_type}; name=\"{name}\"\r\nContent-Disposition: attachment; filename=\"{name}\"\r\nContent-Transfer-Encoding: base64\r\n\r\n"
        ));
        let encoded = STANDARD.encode(bytes);
        for chunk in encoded.as_bytes().chunks(76) {
            message.push_str(&String::from_utf8_lossy(chunk));
            message.push_str("\r\n");
        }
    }
    message.push_str(&format!("--{boundary}--\r\n"));
    Ok(message.into_bytes())
}

fn split_message(message: &str) -> (&str, &str) {
    message
        .split_once("\r\n\r\n")
        .or_else(|| message.split_once("\n\n"))
        .unwrap_or((message, ""))
}

fn header_values(headers: &str, name: &str) -> Vec<String> {
    headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(key, _)| key.trim().eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().to_owned())
        .collect()
}

fn resolve_path(value: &str, base: &Path) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::mail::Mail;

    use super::{Composer, inspect, prepare};

    #[test]
    fn keeps_headers_out_of_the_body_editor() {
        let mail = Mail::new(
            "first@example.com",
            "Ada <ada@example.com>",
            "Update",
            "hi",
            None,
        )
        .with_references(&["root@example.com"]);
        let mut composer =
            Composer::new(Some(&mail), Some("Tuna <tuna@example.com>"), Some("Tuna")).unwrap();
        composer.cc = "mert@example.com".into();
        let body = fs::read_to_string(&composer.body_path).unwrap();
        let message = String::from_utf8(composer.message(Path::new(".")).unwrap()).unwrap();

        assert!(!body.contains("Subject:"));
        assert!(body.contains("-- \nTuna"));
        assert!(message.contains("To: Ada <ada@example.com>"));
        assert!(message.contains("Cc: mert@example.com"));
        assert!(message.contains("Subject: Re: Update"));
        assert!(message.contains("In-Reply-To: <first@example.com>"));
        assert!(message.contains("<root@example.com> <first@example.com>"));
        composer.remove_body().unwrap();
    }

    #[test]
    fn validates_recipients_and_builds_attachments() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("termail-draft-test-{nonce}"));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("notes.txt"), "hello").unwrap();
        let contents = b"To: ada@example.com\nCc: mert@example.com\nBcc: hidden@example.com\nSubject: Report\nAttach: notes.txt\nMIME-Version: 1.0\nContent-Type: text/plain; charset=utf-8\n\nplease review";

        let summary = inspect(contents, &root).unwrap();
        assert_eq!(summary.to, "ada@example.com");
        assert_eq!(summary.attachments, [root.join("notes.txt")]);

        let message = String::from_utf8(prepare(contents, &root).unwrap()).unwrap();
        assert!(message.contains("Content-Type: multipart/mixed"));
        assert!(message.contains("filename=\"notes.txt\""));
        assert!(message.contains("aGVsbG8="));
        assert!(!message.contains("Attach: notes.txt"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_missing_recipients() {
        let error = inspect(b"To: \nSubject: hi\n\nbody", Path::new(".")).unwrap_err();
        assert!(error.to_string().contains("at least one recipient"));
    }

    #[test]
    fn removes_empty_attachment_headers() {
        let message = prepare(
            b"To: ada@example.com\nSubject: hi\nAttach: \n\nhello",
            Path::new("."),
        )
        .unwrap();
        let message = String::from_utf8(message).unwrap();

        assert!(!message.contains("Attach:"));
        assert!(message.ends_with("hello"));
    }

    #[test]
    fn saves_an_incomplete_structured_draft() {
        let composer = Composer::new(None, Some("tuna@example.com"), None).unwrap();
        let message = String::from_utf8(composer.draft_message(Path::new(".")).unwrap()).unwrap();

        assert!(message.contains("To: \r\n"));
        assert!(message.contains("Subject: \r\n"));
        composer.remove_body().unwrap();
    }
}
