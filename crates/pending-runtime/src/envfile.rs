//! The env file: the tokens and addresses the sources read from their
//! environment, one `KEY=value` per line, in the syntax systemd reads for
//! the user service (`EnvironmentFile=`). The daemon reads it too, so a
//! value changed in Settings applies without a restart.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use crate::live::Env;

/// Where the env file is: `$XDG_CONFIG_HOME/tasks-pending/env`, else
/// `$HOME/.config/tasks-pending/env`, as systemd resolves `%E` for the
/// user service. `TASKS_PENDING_CONFIG` moves the config file, never this
/// one. Relative values are ignored, per the XDG spec.
pub fn env_path(env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |key: &str| {
        env(key)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let base = absolute("XDG_CONFIG_HOME")
        .or_else(|| absolute("HOME").map(|home| home.join(".config")))?;
    Some(base.join("tasks-pending").join("env"))
}

/// The env file as written: its assignments, and every other line kept as
/// it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnvFile {
    lines: Vec<Line>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Line {
    Set {
        key: String,
        value: String,
        raw: String,
    },
    Other(String),
}

impl EnvFile {
    /// Reads `text` as systemd does: blank lines and those starting with
    /// `#` or `;` are comments, then `KEY=value`, with the spacing around
    /// the value dropped and `'…'` or `"…"` quotes taken off. A line it
    /// cannot read is kept, and ignored, as systemd ignores it.
    pub fn parse(text: &str) -> Self {
        let lines = text
            .lines()
            .map(|raw| match assignment(raw) {
                Some((key, value)) => Line::Set {
                    key,
                    value,
                    raw: raw.to_owned(),
                },
                None => Line::Other(raw.to_owned()),
            })
            .collect();
        Self { lines }
    }

    /// The value of `key`; the last assignment wins, as in systemd.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.lines.iter().rev().find_map(|line| match line {
            Line::Set {
                key: name, value, ..
            } if name == key => Some(value.as_str()),
            _ => None,
        })
    }

    /// Every variable the file sets.
    pub fn keys(&self) -> BTreeSet<String> {
        self.lines
            .iter()
            .filter_map(|line| match line {
                Line::Set { key, .. } => Some(key.clone()),
                Line::Other(_) => None,
            })
            .collect()
    }

    /// Sets `key` to `value`: in place of its line, or of the commented-out
    /// example of it (`# KEY=`), or at the end.
    pub fn set(&mut self, key: &str, value: &str) {
        let line = Line::Set {
            key: key.to_owned(),
            value: value.to_owned(),
            raw: format!("{key}={}", quote(value)),
        };
        let at = self
            .lines
            .iter()
            .position(|line| matches!(line, Line::Set { key: name, .. } if name == key))
            .or_else(|| {
                self.lines.iter().position(|line| match line {
                    Line::Other(raw) => is_example_of(raw, key),
                    Line::Set { .. } => false,
                })
            });
        match at {
            Some(at) => {
                self.lines[at] = line;
                let mut seen = 0;
                self.lines.retain(|line| match line {
                    Line::Set { key: name, .. } if name == key => {
                        seen += 1;
                        seen == 1
                    }
                    _ => true,
                });
            }
            None => self.lines.push(line),
        }
    }

    /// Removes every assignment of `key`.
    pub fn remove(&mut self, key: &str) {
        self.lines
            .retain(|line| !matches!(line, Line::Set { key: name, .. } if name == key));
    }

    /// The file's text, a line break after each line.
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|line| match line {
                Line::Set { raw, .. } | Line::Other(raw) => format!("{raw}\n"),
            })
            .collect()
    }
}

/// The environment the sources see: the env file's values over `base`,
/// and nothing for a variable that the file set when TasksPending started
/// (`started`) but no longer does: systemd put that one in `base`, from
/// the file.
pub fn overlay(base: Env, file: EnvFile, started: Arc<BTreeSet<String>>) -> Env {
    Arc::new(move |key| match file.get(key) {
        Some(value) => Some(OsString::from(value)),
        None if started.contains(key) => None,
        None => base(key),
    })
}

/// `KEY=value` read from `raw`, if it is one.
fn assignment(raw: &str) -> Option<(String, String)> {
    let line = raw.trim_start();
    if line.starts_with('#') || line.starts_with(';') {
        return None;
    }
    let (key, value) = line.split_once('=')?;
    let key = key.trim_end();
    let mut chars = key.chars();
    let starts = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if !starts || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some((
        key.to_owned(),
        unquote(value.trim_matches([' ', '\t', '\r'])),
    ))
}

/// A value as systemd reads it: inside `'…'` verbatim; inside `"…"` with a
/// backslash before any of ``"\`$`` standing for that character; bare,
/// with a backslash standing for the character after it.
fn unquote(value: &str) -> String {
    let quoted =
        |quote: char| value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote);
    if quoted('\'') {
        return value[1..value.len() - 1].to_owned();
    }
    let (body, escapes): (&str, &dyn Fn(char) -> bool) = if quoted('"') {
        (&value[1..value.len() - 1], &|c| {
            matches!(c, '"' | '\\' | '`' | '$')
        })
    } else {
        (value, &|_| true)
    };
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match (c, chars.clone().next()) {
            ('\\', Some(next)) if escapes(next) => {
                out.push(next);
                chars.next();
            }
            _ => out.push(c),
        }
    }
    out
}

/// `value` as written in the file: bare when systemd reads it back the
/// same, else in double quotes.
fn quote(value: &str) -> String {
    let special = |c: char| matches!(c, '"' | '\'' | '\\' | '`' | '$') || c.is_control();
    let bare = !value.chars().any(special) && value.trim() == value;
    if bare {
        return value.to_owned();
    }
    let mut out = String::from("\"");
    for c in value.chars() {
        if matches!(c, '"' | '\\' | '`' | '$') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Whether `raw` is the commented-out example of `key`, as `# KEY=` in the
/// file the setup writes.
fn is_example_of(raw: &str, key: &str) -> bool {
    raw.trim_start()
        .strip_prefix('#')
        .map(str::trim_start)
        .and_then(|rest| rest.strip_prefix(key))
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "# Tokens.\n\
                        GITHUB_TOKEN=ghp_one\n\
                        \n\
                        ; Plane\n\
                        # PLANE_TOKEN=\n\
                        PLANE_BASE_URL = https://plane.example.com  \n\
                        JIRA_EMAIL='me@example.com'\n\
                        TODOIST_API_TOKEN=\"t\\\"o\\\\k\\$\"\n\
                        not an assignment\n\
                        2BAD=x\n\
                        GITHUB_TOKEN=ghp_two\n";

    fn env(pairs: &'static [(&'static str, &'static str)]) -> Env {
        Arc::new(move |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| OsString::from(*value))
        })
    }

    /// Values read as systemd reads them; the last one wins; comments and
    /// lines that are not assignments set nothing.
    #[test]
    fn reads_the_file_as_systemd_does() {
        let file = EnvFile::parse(FILE);
        assert_eq!(file.get("GITHUB_TOKEN"), Some("ghp_two"));
        assert_eq!(
            file.get("PLANE_BASE_URL"),
            Some("https://plane.example.com")
        );
        assert_eq!(file.get("JIRA_EMAIL"), Some("me@example.com"));
        assert_eq!(file.get("TODOIST_API_TOKEN"), Some("t\"o\\k$"));
        assert_eq!(file.get("PLANE_TOKEN"), None, "commented out");
        assert_eq!(file.get("2BAD"), None, "not a name");
        assert_eq!(
            file.keys().into_iter().collect::<Vec<_>>(),
            [
                "GITHUB_TOKEN",
                "JIRA_EMAIL",
                "PLANE_BASE_URL",
                "TODOIST_API_TOKEN"
            ]
        );
        assert_eq!(file.text(), FILE, "written back as it was");
        assert_eq!(unquote("a\\ b"), "a b", "bare: a backslash keeps the next");
        assert_eq!(unquote("\"a\\nb\""), "a\\nb", "quoted: only some escapes");
    }

    /// A value set replaces its line (dropping repeats), or the example
    /// line of it, or goes at the end; one removed goes everywhere; values
    /// systemd would read differently are quoted.
    #[test]
    fn changes_one_variable_and_keeps_the_rest() {
        let mut file = EnvFile::parse(FILE);
        file.set("GITHUB_TOKEN", "ghp_new");
        file.set("PLANE_TOKEN", "pl an\"$");
        file.set("LINEAR_API_KEY", "lin");
        file.remove("JIRA_EMAIL");
        let text = file.text();
        assert_eq!(
            text,
            "# Tokens.\n\
             GITHUB_TOKEN=ghp_new\n\
             \n\
             ; Plane\n\
             PLANE_TOKEN=\"pl an\\\"\\$\"\n\
             PLANE_BASE_URL = https://plane.example.com  \n\
             TODOIST_API_TOKEN=\"t\\\"o\\\\k\\$\"\n\
             not an assignment\n\
             2BAD=x\n\
             LINEAR_API_KEY=lin\n"
        );
        let read = EnvFile::parse(&text);
        assert_eq!(
            read.get("PLANE_TOKEN"),
            Some("pl an\"$"),
            "read back the same"
        );
        assert_eq!(read.get("GITHUB_TOKEN"), Some("ghp_new"));
        assert_eq!(read.get("JIRA_EMAIL"), None);
        assert_eq!(EnvFile::parse("").text(), "");
        let mut empty = EnvFile::default();
        empty.set("A", " padded ");
        assert_eq!(empty.text(), "A=\" padded \"\n");
        assert_eq!(EnvFile::parse(&empty.text()).get("A"), Some(" padded "));
    }

    /// The file's values win over the process's; a variable the file held
    /// at start and dropped since is gone, though systemd left it in the
    /// process; the rest comes from the process.
    #[test]
    fn puts_the_file_over_the_environment() {
        let base = env(&[
            ("GITHUB_TOKEN", "old"),
            ("HOME", "/home/me"),
            ("PLANE_TOKEN", "p"),
        ]);
        let started = Arc::new(BTreeSet::from(["GITHUB_TOKEN".to_owned()]));
        let file = EnvFile::parse("PLANE_TOKEN=new\n");
        let seen = overlay(base, file, started);
        assert_eq!(seen("PLANE_TOKEN"), Some(OsString::from("new")));
        assert_eq!(seen("GITHUB_TOKEN"), None, "dropped from the file");
        assert_eq!(seen("HOME"), Some(OsString::from("/home/me")));
        assert_eq!(seen("NOPE"), None);
    }

    /// Where systemd finds it for the user service.
    #[test]
    fn finds_the_file() {
        assert_eq!(
            env_path(&*env(&[("XDG_CONFIG_HOME", "/x"), ("HOME", "/home/me")])),
            Some(PathBuf::from("/x/tasks-pending/env"))
        );
        assert_eq!(
            env_path(&*env(&[("XDG_CONFIG_HOME", "rel"), ("HOME", "/home/me")])),
            Some(PathBuf::from("/home/me/.config/tasks-pending/env"))
        );
        assert_eq!(
            env_path(&*env(&[
                ("TASKS_PENDING_CONFIG", "/c.toml"),
                ("HOME", "/h")
            ])),
            Some(PathBuf::from("/h/.config/tasks-pending/env")),
            "the config's variable does not move it"
        );
        assert_eq!(env_path(&*env(&[])), None);
    }
}
