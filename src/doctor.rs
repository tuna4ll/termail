use std::{env, path::Path};

use crate::config::Config;

pub struct Check {
    pub ok: bool,
    pub label: &'static str,
    pub detail: String,
}

pub fn run(config: &Config) -> bool {
    let checks = checks(config);
    println!("termail doctor\n");
    for check in &checks {
        let mark = if check.ok { "ok" } else { "fail" };
        println!("[{mark}] {:<12} {}", check.label, check.detail);
    }
    let healthy = checks.iter().all(|check| check.ok);
    println!();
    if healthy {
        println!("Ready to read and send mail.");
    } else {
        println!("Fix the failed checks, then run termail doctor again.");
    }
    healthy
}

pub fn checks(config: &Config) -> Vec<Check> {
    let mut checks = Vec::new();
    checks.push(Check {
        ok: config.identity().is_some(),
        label: "identity",
        detail: config
            .identity()
            .unwrap_or_else(|| "Set email in config.toml.".into()),
    });
    checks.push(match &config.maildir {
        Some(path) if path.is_dir() => Check {
            ok: ["cur", "new", "tmp"]
                .into_iter()
                .all(|child| path.join(child).is_dir()),
            label: "maildir",
            detail: if ["cur", "new", "tmp"]
                .into_iter()
                .all(|child| path.join(child).is_dir())
            {
                path.display().to_string()
            } else {
                format!("{} needs cur, new, and tmp directories.", path.display())
            },
        },
        Some(path) => Check {
            ok: false,
            label: "maildir",
            detail: format!("{} does not exist.", path.display()),
        },
        None => Check {
            ok: false,
            label: "maildir",
            detail: "Set maildir in config.toml.".into(),
        },
    });
    checks.push(command_check("editor", &config.editor_command()));
    checks.push(
        if config
            .send_command
            .first()
            .is_some_and(|value| value == "termail-test-sink")
        {
            Check {
                ok: config.send_command.len() > 1,
                label: "sender",
                detail: if config.send_command.len() > 1 {
                    "local test sink".into()
                } else {
                    "termail-test-sink needs an output path.".into()
                },
            }
        } else {
            command_check("sender", &config.send_command)
        },
    );
    let downloads = config.download_dir();
    checks.push(Check {
        ok: downloads.is_dir(),
        label: "downloads",
        detail: if downloads.is_dir() {
            downloads.display().to_string()
        } else {
            format!("Create {} or set download_dir.", downloads.display())
        },
    });
    checks
}

fn command_check(label: &'static str, command: &[String]) -> Check {
    let Some(program) = command.first() else {
        return Check {
            ok: false,
            label,
            detail: format!("Set {label} command in config.toml."),
        };
    };
    Check {
        ok: command_exists(program),
        label,
        detail: if command_exists(program) {
            program.clone()
        } else {
            format!("{program} was not found in PATH.")
        },
    }
}

fn command_exists(program: &str) -> bool {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path.is_file();
    }
    let extensions: Vec<String> = if cfg!(windows) {
        env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .map(str::to_owned)
            .collect()
    } else {
        vec![String::new()]
    };
    env::var_os("PATH").is_some_and(|paths| {
        env::split_paths(&paths).any(|directory| {
            extensions.iter().any(|extension| {
                let candidate =
                    if extension.is_empty() || program.to_ascii_uppercase().ends_with(extension) {
                        directory.join(program)
                    } else {
                        directory.join(format!("{program}{extension}"))
                    };
                candidate.is_file()
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::checks;
    use crate::config::Config;

    #[test]
    fn explains_missing_setup() {
        let checks = checks(&Config::default());

        assert!(
            checks
                .iter()
                .any(|check| check.label == "identity" && !check.ok)
        );
        assert!(
            checks
                .iter()
                .any(|check| check.label == "maildir" && !check.ok)
        );
        assert!(
            checks
                .iter()
                .any(|check| check.label == "sender" && !check.ok)
        );
    }
}
