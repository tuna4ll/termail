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
