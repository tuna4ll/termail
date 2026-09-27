use std::collections::HashSet;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mail {
    pub id: String,
    pub from: String,
    pub subject: String,
    pub body: String,
    pub reply_to: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Conversation {
    pub root_id: String,
    pub subject: String,
    pub mail_ids: Vec<String>,
}

impl Conversation {
    pub fn new(root_id: &str, subject: &str) -> Self {
        Self {
            root_id: root_id.into(),
            subject: subject.into(),
            mail_ids: Vec::new(),
        }
    }

    pub fn last_sender<'a>(&self, mails: &'a [Mail]) -> Option<&'a str> {
        let mail_id = self.mail_ids.last()?;
        mails
            .iter()
            .find(|mail| mail.id == *mail_id)
            .map(|mail| mail.from.as_str())
    }
}

pub fn group_conversations(mails: &[Mail]) -> Vec<Conversation> {
    let mut conversations = Vec::new();

    for mail in mails {
        let root = root_mail(mail, mails);
        let index = conversations
            .iter()
            .position(|conversation: &Conversation| conversation.root_id == root.id);
        let conversation = match index {
            Some(index) => &mut conversations[index],
            None => {
                conversations.push(Conversation::new(&root.id, &root.subject));
                conversations.last_mut().unwrap()
            }
        };
        conversation.mail_ids.push(mail.id.clone());
    }

    conversations
}

fn root_mail<'a>(mail: &'a Mail, mails: &'a [Mail]) -> &'a Mail {
    let mut current = mail;
    let mut visited = HashSet::new();

    while visited.insert(current.id.as_str()) {
        let Some(parent_id) = &current.reply_to else {
            return current;
        };
        let Some(parent) = mails.iter().find(|mail| mail.id == *parent_id) else {
            return current;
        };
        current = parent;
    }

    mail
}

impl Mail {
    pub fn new(id: &str, from: &str, subject: &str, body: &str, reply_to: Option<&str>) -> Self {
        Self {
            id: id.into(),
            from: from.into(),
            subject: subject.into(),
            body: body.into(),
            reply_to: reply_to.map(Into::into),
        }
    }
}

pub fn demo_mails() -> Vec<Mail> {
    vec![
        Mail::new(
            "project-1",
            "ada@example.com",
            "project update",
            "hey,\nthe new build is ready.\ncan you review it?",
            None,
        ),
        Mail::new(
            "project-2",
            "tuna@tunakilic.com",
            "re: project update",
            "sure, i'll check it tonight.",
            Some("project-1"),
        ),
        Mail::new(
            "project-3",
            "ada@example.com",
            "re: project update",
            "great, i added the release notes too.",
            Some("project-2"),
        ),
        Mail::new(
            "weekend-1",
            "mert@example.com",
            "weekend plans",
            "coffee on saturday?",
            None,
        ),
        Mail::new(
            "weekend-2",
            "tuna@tunakilic.com",
            "re: weekend plans",
            "sounds good. same place at two?",
            Some("weekend-1"),
        ),
        Mail::new(
            "pull-1",
            "github@github.com",
            "review requested",
            "ada requested your review on pull request #42.",
            None,
        ),
        Mail::new(
            "pull-2",
            "github@github.com",
            "pull request merged",
            "pull request #42 has been merged.",
            Some("pull-1"),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::{Mail, demo_mails, group_conversations};

    #[test]
    fn groups_replies_with_their_root_mail() {
        let conversations = group_conversations(&demo_mails());

        assert_eq!(conversations.len(), 3);
        assert_eq!(conversations[0].root_id, "project-1");
        assert_eq!(conversations[0].subject, "project update");
        assert_eq!(
            conversations[0].mail_ids,
            ["project-1", "project-2", "project-3"]
        );
    }

    #[test]
    fn keeps_orphan_replies_in_their_own_conversation() {
        let mails = vec![Mail::new(
            "orphan",
            "ada@example.com",
            "missing parent",
            "hello",
            Some("missing"),
        )];
        let conversations = group_conversations(&mails);

        assert_eq!(conversations[0].root_id, "orphan");
    }
}
