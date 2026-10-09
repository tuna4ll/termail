use std::{
    fs,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub name: Option<String>,
    pub email: Option<String>,
    pub maildir: Option<PathBuf>,
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
        if let Some(maildir) = &config.maildir
            && maildir.is_relative()
        {
            config.maildir = Some(path.parent().unwrap_or(Path::new(".")).join(maildir));
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
        fs::write(&path, "email = 'tuna@example.com'\nmaildir = 'mail'\n").unwrap();

        let config = Config::load(Some(&path)).unwrap();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(config.maildir, Some(root.join("mail")));
        assert_eq!(config.identity().as_deref(), Some("tuna@example.com"));
    }
}
