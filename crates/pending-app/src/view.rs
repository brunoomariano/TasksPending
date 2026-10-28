//! Choices of the person looking at the dashboard that only this app
//! keeps: the board filter, collapsed stacks, hidden items and the
//! auto-refresh interval. Remembered in a small file; fine to lose.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use pending_runtime::config::state_dir;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Intervals offered in Settings, in minutes; 0 is off.
pub const AUTO_REFRESH_CHOICES: [u32; 5] = [0, 1, 5, 15, 30];
pub const DEFAULT_AUTO_REFRESH_MINUTES: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    /// Only this board's groups; `None` shows every board.
    pub board: Option<String>,
    /// Groups (see [`group_key`]) showing their empty stacks.
    pub expanded: BTreeSet<String>,
    /// Stacks (see [`column_key`]) showing every card, not just the first
    /// ones.
    pub open_columns: BTreeSet<String>,
    /// Stacks collapsed to their header.
    pub collapsed: BTreeSet<String>,
    /// Boards, groups and stacks switched off (see [`hide_key`]).
    pub hidden: BTreeSet<String>,
    /// Minutes between automatic refreshes of every source; 0 is off.
    pub auto_refresh_minutes: u32,
}

impl Default for View {
    fn default() -> Self {
        Self {
            board: None,
            expanded: BTreeSet::new(),
            open_columns: BTreeSet::new(),
            collapsed: BTreeSet::new(),
            hidden: BTreeSet::new(),
            auto_refresh_minutes: DEFAULT_AUTO_REFRESH_MINUTES,
        }
    }
}

impl View {
    /// The view saved at `path`, or the default when there is none or it
    /// cannot be read. Each setting is read on its own, so one that is not
    /// understood falls back to its default without taking the others
    /// along; so does an interval that is not offered.
    pub fn load(path: &Path) -> Self {
        let mut view = Self::default();
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return view,
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "remembered view not read");
                return view;
            }
        };
        let saved: serde_json::Map<String, Value> = match serde_json::from_str(&text) {
            Ok(saved) => saved,
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "remembered view not understood");
                return view;
            }
        };
        let saved = Saved { path, saved };
        saved.read("board", &mut view.board);
        saved.read("expanded", &mut view.expanded);
        saved.read("open_columns", &mut view.open_columns);
        saved.read("collapsed", &mut view.collapsed);
        saved.read("hidden", &mut view.hidden);
        saved.read("auto_refresh_minutes", &mut view.auto_refresh_minutes);
        if !AUTO_REFRESH_CHOICES.contains(&view.auto_refresh_minutes) {
            view.auto_refresh_minutes = DEFAULT_AUTO_REFRESH_MINUTES;
        }
        view
    }

    /// Writes the view to `path`, creating its directory. A failure is
    /// logged, and the view is then remembered only while the window is
    /// open.
    pub fn save(&self, path: &Path) {
        let written = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| {
                let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
                fs::write(path, text)
            });
        if let Err(error) = written {
            tracing::warn!(path = %path.display(), %error, "view not remembered");
        }
    }

    /// Whether something is filtered or hidden.
    pub fn is_filtering(&self) -> bool {
        self.board.is_some() || !self.hidden.is_empty()
    }

    /// "Settings", plus what the settings currently change.
    pub fn settings_label(&self) -> String {
        let mut changes = Vec::new();
        if let Some(board) = &self.board {
            changes.push(format!("showing {board}"));
        }
        if !self.hidden.is_empty() {
            changes.push(format!("{} hidden", self.hidden.len()));
        }
        if changes.is_empty() {
            "Settings".to_owned()
        } else {
            format!("Settings ({})", changes.join(", "))
        }
    }
}

/// The settings in a saved view file, by name.
struct Saved<'a> {
    path: &'a Path,
    saved: serde_json::Map<String, Value>,
}

impl Saved<'_> {
    /// Sets `into` from the setting `name` when it is there and understood;
    /// otherwise leaves it as it is, saying why when it was there.
    fn read<T: DeserializeOwned>(&self, name: &str, into: &mut T) {
        let Some(value) = self.saved.get(name) else {
            return;
        };
        match T::deserialize(value) {
            Ok(read) => *into = read,
            Err(error) => tracing::warn!(
                path = %self.path.display(),
                setting = name,
                %error,
                "remembered setting not understood"
            ),
        }
    }
}

/// Where `env` keeps the view: `app-view.json` in the state directory
/// (`$XDG_STATE_HOME/tasks-pending`, else `~/.local/state/tasks-pending`).
/// `None` when the environment names neither, and the view is then not
/// remembered (rather than written into whatever directory the app was
/// started from).
pub fn view_path(env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    state_dir(env).map(|dir| dir.join("app-view.json"))
}

/// Adds `key` to `set`, or removes it if it was there.
pub fn flip(set: &mut BTreeSet<String>, key: &str) {
    if !set.remove(key) {
        set.insert(key.to_owned());
    }
}

/// Identifies a group across refreshes.
pub fn group_key(board: &str, source: &str) -> String {
    format!("{board}/{source}")
}

/// Identifies a stack across refreshes.
pub fn column_key(board: &str, source: &str, column: &str) -> String {
    format!("{board}/{source}/{column}")
}

/// Keys of the things settings can hide.
pub mod hide_key {
    pub fn board(board: &str) -> String {
        format!("board:{board}")
    }

    pub fn group(board: &str, source: &str) -> String {
        format!("group:{board}/{source}")
    }

    pub fn column(board: &str, source: &str, column: &str) -> String {
        format!("column:{board}/{source}/{column}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("tasks-pending-view-{}-{name}", std::process::id()))
    }

    /// A saved view comes back as it was; a missing or broken file is the
    /// default; an interval that is not offered becomes the default.
    #[test]
    fn saves_loads_and_validates() {
        let dir = temp_path("roundtrip");
        let path = dir.join("tasks-pending").join("app-view.json");
        assert_eq!(View::load(&path), View::default());

        let mut view = View {
            board: Some("Work".to_owned()),
            auto_refresh_minutes: 15,
            ..View::default()
        };
        // Every setting, so one that loading forgets fails here.
        view.collapsed.insert("Work/github/Drafts".to_owned());
        view.expanded.insert("Work/github".to_owned());
        view.open_columns.insert("Work/github/Review".to_owned());
        view.hidden.insert("board:Personal".to_owned());
        view.save(&path);
        assert_eq!(View::load(&path), view);

        fs::write(
            &path,
            r#"{"auto_refresh_minutes": 7, "hidden": ["board:X"]}"#,
        )
        .unwrap();
        let loaded = View::load(&path);
        assert_eq!(loaded.auto_refresh_minutes, DEFAULT_AUTO_REFRESH_MINUTES);
        assert!(loaded.hidden.contains("board:X"));

        fs::write(&path, "not json").unwrap();
        assert_eq!(View::load(&path), View::default());
        let _ = fs::remove_dir_all(&dir);
    }

    /// A setting that cannot be read costs only itself: it falls back to
    /// its default, and the others are kept.
    #[test]
    fn keeps_the_settings_it_understands() {
        let dir = temp_path("tolerant");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app-view.json");
        fs::write(
            &path,
            r#"{"board": "Work", "hidden": 5, "auto_refresh_minutes": "often",
                "collapsed": ["Work/github/Drafts"], "later": true}"#,
        )
        .unwrap();
        let view = View::load(&path);
        assert_eq!(view.board.as_deref(), Some("Work"));
        assert!(view.hidden.is_empty());
        assert_eq!(view.auto_refresh_minutes, DEFAULT_AUTO_REFRESH_MINUTES);
        assert!(view.collapsed.contains("Work/github/Drafts"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// The view lives in the XDG state directory the environment names; a
    /// relative or empty value is ignored, and with neither there is none.
    #[test]
    fn keeps_the_view_in_the_state_directory() {
        let env = |pairs: &'static [(&str, &str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| OsString::from(value))
            }
        };
        assert_eq!(
            view_path(&env(&[("XDG_STATE_HOME", "/state"), ("HOME", "/home/me")])),
            Some(PathBuf::from("/state/tasks-pending/app-view.json"))
        );
        assert_eq!(
            view_path(&env(&[("XDG_STATE_HOME", "state"), ("HOME", "/home/me")])),
            Some(PathBuf::from(
                "/home/me/.local/state/tasks-pending/app-view.json"
            ))
        );
        assert_eq!(view_path(&env(&[("HOME", "")])), None);
    }

    /// The settings button says what the settings change.
    #[test]
    fn labels_the_settings_with_what_they_change() {
        let mut view = View::default();
        assert_eq!(view.settings_label(), "Settings");
        assert!(!view.is_filtering());
        view = View {
            board: Some("Work".to_owned()),
            ..view
        };
        flip(&mut view.hidden, "board:Personal");
        flip(&mut view.hidden, "group:Work/jira");
        assert_eq!(view.settings_label(), "Settings (showing Work, 2 hidden)");
        assert!(view.is_filtering());
        flip(&mut view.hidden, "group:Work/jira");
        assert_eq!(view.settings_label(), "Settings (showing Work, 1 hidden)");
    }

    #[test]
    fn keys_tell_things_apart() {
        assert_eq!(group_key("Work", "github"), "Work/github");
        assert_eq!(column_key("Work", "github", "Drafts"), "Work/github/Drafts");
        assert_eq!(hide_key::board("Work"), "board:Work");
        assert_eq!(hide_key::group("Work", "github"), "group:Work/github");
        assert_eq!(
            hide_key::column("Work", "github", "Drafts"),
            "column:Work/github/Drafts"
        );
    }
}
