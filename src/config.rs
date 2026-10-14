use std::{
    fs,
    path::{Path, PathBuf},
};

use directories::{ProjectDirs, UserDirs};
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub name: Option<String>,
    pub email: Option<String>,
    pub signature: Option<String>,
    pub maildir: Option<PathBuf>,
    pub download_dir: Option<PathBuf>,
    pub editor: Vec<String>,
    pub send_command: Vec<String>,
    pub default_account: Option<String>,
    pub accounts: Vec<Account>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Account {
    pub id: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub signature: Option<String>,
    pub maildir: Option<PathBuf>,
    pub download_dir: Option<PathBuf>,
    pub editor: Vec<String>,
    pub send_command: Vec<String>,
}

impl Config {
    pub fn load(path: Option<&Path>) -> color_eyre::Result<Self> {
        let explicit = path
            .map(Path::to_path_buf)
            .or_else(|| std::env::var_os("TERMAIL_CONFIG").map(PathBuf::from));
        let path = explicit.clone().or_else(default_path);
        let Some(path) = path else {
            return Ok(Self::default());
        };
        if !path.exists() && explicit.is_none() {
            return Ok(Self::default());
        }
        let contents = fs::read_to_string(&path)?;
        let mut config: Self = toml::from_str(&contents)?;
        let base = path.parent().unwrap_or(Path::new("."));
        resolve_path(&mut config.maildir, base);
        resolve_path(&mut config.download_dir, base);
        for account in &mut config.accounts {
            resolve_path(&mut account.maildir, base);
            resolve_path(&mut account.download_dir, base);
        }
        if !config.accounts.is_empty() {
            let selected = config
                .default_account
                .as_deref()
                .and_then(|id| config.accounts.iter().position(|account| account.id == id))
                .unwrap_or_default();
            config.activate(selected);
        }
        Ok(config)
    }

    pub fn identity(&self) -> Option<String> {
        match (&self.name, &self.email) {
            (Some(name), Some(email)) => Some(format!("{name} <{email}>")),
            (None, Some(email)) => Some(email.clone()),
            _ => None,
        }
    }

    pub fn editor_command(&self) -> Vec<String> {
        if !self.editor.is_empty() {
            return self.editor.clone();
        }
        if let Ok(editor) = std::env::var("EDITOR") {
            let command: Vec<String> = editor.split_whitespace().map(Into::into).collect();
            if !command.is_empty() {
                return command;
            }
        }
        if cfg!(target_os = "windows") {
            vec!["notepad".into()]
        } else if cfg!(target_os = "macos") {
            vec!["open".into(), "-W".into(), "-a".into(), "TextEdit".into()]
        } else {
            vec!["vi".into()]
        }
    }

    pub fn download_dir(&self) -> PathBuf {
        self.download_dir
            .clone()
            .or_else(|| UserDirs::new().and_then(|dirs| dirs.download_dir().map(Path::to_path_buf)))
            .unwrap_or_else(std::env::temp_dir)
    }

    pub fn activate(&mut self, index: usize) -> bool {
        let Some(account) = self.accounts.get(index).cloned() else {
            return false;
        };
        self.name = account.name;
        self.email = account.email;
        self.signature = account.signature;
        self.maildir = account.maildir;
        self.download_dir = account.download_dir;
        self.editor = account.editor;
        self.send_command = account.send_command;
        true
    }

    pub fn active_account_index(&self) -> usize {
        self.email
            .as_deref()
            .and_then(|email| {
                self.accounts
                    .iter()
                    .position(|account| account.email.as_deref() == Some(email))
            })
            .unwrap_or_default()
    }
}

impl Account {
    pub fn label(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.email.clone())
            .unwrap_or_else(|| self.id.clone())
    }
}

fn resolve_path(path: &mut Option<PathBuf>, base: &Path) {
    if path.as_ref().is_some_and(|path| path.is_relative()) {
        *path = path.take().map(|path| base.join(path));
    }
}

pub fn default_path() -> Option<PathBuf> {
    ProjectDirs::from("com", "tunakilic", "termail")
        .map(|dirs| dirs.config_dir().join("config.toml"))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::Config;

    #[test]
    fn resolves_relative_maildir_from_config() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("termail-config-{nonce}"));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("config.toml");
        fs::write(
            &path,
            "email = 'tuna@example.com'\nmaildir = 'mail'\ndownload_dir = 'downloads'\n",
        )
        .unwrap();

        let config = Config::load(Some(&path)).unwrap();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(config.maildir, Some(root.join("mail")));
        assert_eq!(config.download_dir, Some(root.join("downloads")));
        assert_eq!(config.identity().as_deref(), Some("tuna@example.com"));
    }

    #[test]
    fn selects_the_default_account() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("termail-accounts-{nonce}"));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("config.toml");
        fs::write(
            &path,
            "default_account = 'work'\n[[accounts]]\nid = 'personal'\nemail = 'me@example.com'\nmaildir = 'personal'\n[[accounts]]\nid = 'work'\nname = 'Work'\nemail = 'me@work.test'\nmaildir = 'work'\n",
        )
        .unwrap();

        let mut config = Config::load(Some(&path)).unwrap();

        assert_eq!(config.identity().as_deref(), Some("Work <me@work.test>"));
        assert_eq!(config.maildir, Some(root.join("work")));
        assert_eq!(config.active_account_index(), 1);
        assert!(config.activate(0));
        assert_eq!(config.email.as_deref(), Some("me@example.com"));
        fs::remove_dir_all(root).unwrap();
    }
}
