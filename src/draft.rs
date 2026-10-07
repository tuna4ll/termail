use std::{
    fs, io,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::mail::Mail;

pub fn create(reply_to: Option<&Mail>) -> io::Result<PathBuf> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("termail-draft-{}-{nonce}.eml", std::process::id()));
    fs::write(&path, template(reply_to))?;
    Ok(path)
}

pub fn template(reply_to: Option<&Mail>) -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut headers = Vec::new();

    if let Some(mail) = reply_to {
        let subject = if mail.subject.to_lowercase().starts_with("re:") {
            mail.subject.clone()
        } else {
            format!("Re: {}", mail.subject)
        };
        let mut references = mail.references.clone();
        references.push(mail.id.clone());
        references.dedup();
        headers.push(format!("To: {}", one_line(&mail.from)));
        headers.push(format!("Subject: {}", one_line(&subject)));
        headers.push(format!("In-Reply-To: <{}>", one_line(&mail.id)));
        headers.push(format!(
            "References: {}",
            references
                .iter()
                .map(|id| format!("<{}>", one_line(id)))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    } else {
        headers.push("To: ".into());
        headers.push("Subject: ".into());
    }

    headers.push(format!(
        "Message-ID: <termail-{}-{nonce}@localhost>",
        std::process::id()
    ));
    headers.push("MIME-Version: 1.0".into());
    headers.push("Content-Type: text/plain; charset=utf-8".into());
    format!("{}\n\n", headers.join("\n"))
}

fn one_line(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use crate::mail::Mail;

    use super::template;

    #[test]
    fn builds_reply_headers() {
        let mail = Mail::new(
            "first@example.com",
            "Ada <ada@example.com>",
            "Update",
            "hi",
            None,
        )
        .with_references(&["root@example.com"]);
        let draft = template(Some(&mail));

        assert!(draft.contains("To: Ada <ada@example.com>"));
        assert!(draft.contains("Subject: Re: Update"));
        assert!(draft.contains("In-Reply-To: <first@example.com>"));
        assert!(draft.contains("<root@example.com> <first@example.com>"));
    }
}
