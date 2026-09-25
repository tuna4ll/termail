#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mail {
    pub id: String,
    pub from: String,
    pub subject: String,
    pub body: String,
    pub reply_to: Option<String>,
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
