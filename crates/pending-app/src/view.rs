//! Choices of the person looking at the dashboard that only this app
//! keeps: the board filter, collapsed stacks, hidden items and the
//! auto-refresh interval. Remembered in a small file; fine to lose.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
    /// cannot be read. An interval that is not offered becomes the default.
    pub fn load(path: &Path) -> Self {
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        let mut view: Self = serde_json::from_str(&text).unwrap_or_default();
        if !AUTO_REFRESH_CHOICES.contains(&view.auto_refresh_minutes) {
            view.auto_refresh_minutes = DEFAULT_AUTO_REFRESH_MINUTES;
        }
        view
    }

    /// Writes the view to `path`, creating its directory. A failure is
    /// ignored: the view just isn't remembered.
    pub fn save(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, text);
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

/// Where the view is kept: `$XDG_STATE_HOME/tasks-pending/app-view.json`.
pub fn view_path(state_dir: &Path) -> PathBuf {
    state_dir.join("tasks-pending").join("app-view.json")
}

/// `$XDG_STATE_HOME`, or `~/.local/state`; the current directory as a last
/// resort, when the environment names neither.
pub fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map_or_else(
            || PathBuf::from("."),
            |home| PathBuf::from(home).join(".local/state"),
        )
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
        let path = view_path(&dir);
        assert_eq!(View::load(&path), View::default());

        let mut view = View {
            board: Some("Work".to_owned()),
            auto_refresh_minutes: 15,
            ..View::default()
        };
        view.collapsed.insert("Work/github/Drafts".to_owned());
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
