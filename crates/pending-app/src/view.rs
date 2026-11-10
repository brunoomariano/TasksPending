//! Choices of the person looking at the dashboard that only this app
//! keeps: the board filter, collapsed stacks, the theme and how far along
//! each picked card is. Remembered in a small file; fine to lose. What is
//! shown and how often it refreshes is the config's, edited in Settings.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use pending_runtime::config::state_dir;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How far along a card in the Activity area is: the three columns of its
/// board, in order. A card arrives as picked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    #[default]
    Picked,
    /// Saved under its first name, Doing, which 0.7.0 wrote.
    #[serde(rename = "doing")]
    InProgress,
    /// Done here; still to be marked done at its source.
    Done,
}

impl Stage {
    /// The columns in order.
    pub const ALL: [Stage; 3] = [Stage::Picked, Stage::InProgress, Stage::Done];

    /// The column's name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Picked => "Picked",
            Self::InProgress => "In Progress",
            Self::Done => "Done / to mark",
        }
    }
}

/// Where the window's colours come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    /// The window's own palette: dark, or light when the system asks.
    #[default]
    TasksPending,
    /// The theme Omarchy has applied, followed as it changes.
    Omarchy,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Where the colours come from.
    pub theme: Theme,
    /// The stage of each picked card that moved on from [`Stage::Picked`],
    /// by card id.
    pub stages: BTreeMap<String, Stage>,
}

impl View {
    /// The view saved at `path`, or the default when there is none or it
    /// cannot be read. Each setting is read on its own, so one that is not
    /// understood falls back to its default without taking the others
    /// along. Settings of earlier versions (the hidden boards, groups and
    /// stacks, and the auto-refresh interval, now the config's) are left
    /// out.
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
        saved.read("theme", &mut view.theme);
        saved.read("stages", &mut view.stages);
        view
    }

    /// The stage of the picked card `id`: where it was moved to, else
    /// picked.
    pub fn stage(&self, id: &str) -> Stage {
        self.stages.get(id).copied().unwrap_or_default()
    }

    /// Moves the picked card `id` to `stage`; back to picked is forgetting
    /// the move.
    pub fn set_stage(&mut self, id: &str, stage: Stage) {
        if stage == Stage::Picked {
            self.stages.remove(id);
        } else {
            self.stages.insert(id.to_owned(), stage);
        }
    }

    /// Forgets the stages of cards no longer among `picked` (unpicked, or
    /// finished at their source): one picked again later starts over.
    /// Says whether anything was forgotten.
    pub fn keep_stages_of(&mut self, picked: &[String]) -> bool {
        let before = self.stages.len();
        self.stages.retain(|id, _| picked.contains(id));
        self.stages.len() != before
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

    /// Whether only one board is shown.
    pub fn is_filtering(&self) -> bool {
        self.board.is_some()
    }

    /// What the settings currently change, for the settings button.
    pub fn changes(&self) -> Vec<String> {
        self.board
            .iter()
            .map(|board| format!("showing {board}"))
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("tasks-pending-view-{}-{name}", std::process::id()))
    }

    /// A saved view comes back as it was; a missing or broken file is the
    /// default; the settings of earlier versions are left out.
    #[test]
    fn saves_loads_and_validates() {
        let dir = temp_path("roundtrip");
        let path = dir.join("tasks-pending").join("app-view.json");
        assert_eq!(View::load(&path), View::default());

        let mut view = View {
            board: Some("Work".to_owned()),
            theme: Theme::Omarchy,
            ..View::default()
        };
        // Every setting, so one that loading forgets fails here.
        view.collapsed.insert("Work/github/Drafts".to_owned());
        view.expanded.insert("Work/github".to_owned());
        view.open_columns.insert("Work/github/Review".to_owned());
        view.set_stage("gh:2", Stage::InProgress);
        view.save(&path);
        assert_eq!(View::load(&path), view);

        fs::write(
            &path,
            r#"{"board": "Work", "auto_refresh_minutes": 5, "hidden": ["board:X"]}"#,
        )
        .unwrap();
        let loaded = View::load(&path);
        assert_eq!(loaded.board.as_deref(), Some("Work"));
        loaded.save(&path);
        let saved = fs::read_to_string(&path).unwrap();
        assert!(
            !saved.contains("hidden") && !saved.contains("auto_refresh"),
            "{saved}"
        );

        fs::write(&path, "not json").unwrap();
        assert_eq!(View::load(&path), View::default());
        let _ = fs::remove_dir_all(&dir);
    }

    /// A picked card starts in the Picked column and keeps the column it
    /// was moved to; moving it back to Picked forgets the move. The stages
    /// of cards that are no longer picked are forgotten together, so a
    /// card picked again later starts over; those still picked are kept.
    #[test]
    fn remembers_how_far_along_each_picked_card_is() {
        let mut view = View::default();
        assert_eq!(view.stage("gh:1"), Stage::Picked);
        view.set_stage("gh:1", Stage::InProgress);
        view.set_stage("gh:2", Stage::Done);
        assert_eq!(view.stage("gh:1"), Stage::InProgress);
        assert_eq!(view.stage("gh:2"), Stage::Done);
        view.set_stage("gh:1", Stage::Picked);
        assert_eq!(view.stage("gh:1"), Stage::Picked);
        assert!(
            !view.stages.contains_key("gh:1"),
            "back to picked is forgotten"
        );

        assert!(!view.keep_stages_of(&["gh:2".to_owned(), "gh:3".to_owned()]));
        assert_eq!(view.stage("gh:2"), Stage::Done, "still picked: kept");
        assert!(view.keep_stages_of(&["gh:3".to_owned()]));
        assert_eq!(view.stage("gh:2"), Stage::Picked);
        assert_eq!(
            Stage::ALL.map(Stage::name),
            ["Picked", "In Progress", "Done / to mark"]
        );
    }

    /// A stage saved under a name this version does not know costs only
    /// the stages: the other settings are kept.
    #[test]
    fn forgets_stages_it_does_not_understand() {
        let dir = temp_path("stages");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app-view.json");
        fs::write(
            &path,
            r#"{"board": "Work", "stages": {"gh:1": "doing", "gh:2": "shipped"}}"#,
        )
        .unwrap();
        let view = View::load(&path);
        assert_eq!(view.board.as_deref(), Some("Work"));
        assert!(view.stages.is_empty());

        // In Progress was called Doing, and is still saved under that name.
        fs::write(&path, r#"{"stages": {"gh:1": "doing"}}"#).unwrap();
        let view = View::load(&path);
        assert_eq!(view.stage("gh:1"), Stage::InProgress);
        view.save(&path);
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains(r#""gh:1": "doing""#)
        );
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
            r#"{"board": "Work", "expanded": 5,
                "collapsed": ["Work/github/Drafts"], "theme": "neon", "later": true}"#,
        )
        .unwrap();
        let view = View::load(&path);
        assert_eq!(view.board.as_deref(), Some("Work"));
        assert!(view.expanded.is_empty());
        assert_eq!(view.theme, Theme::TasksPending);
        assert!(view.collapsed.contains("Work/github/Drafts"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// The window keeps its own colours until told to follow Omarchy, and
    /// the choice is saved by name.
    #[test]
    fn keeps_its_own_colours_unless_told_otherwise() {
        assert_eq!(View::default().theme, Theme::TasksPending);
        for (theme, name) in [
            (Theme::TasksPending, "tasks-pending"),
            (Theme::Omarchy, "omarchy"),
        ] {
            assert_eq!(serde_json::to_value(theme).unwrap(), name);
        }
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
    fn says_what_the_settings_change() {
        let mut view = View::default();
        assert!(view.changes().is_empty());
        assert!(!view.is_filtering());
        view.board = Some("Work".to_owned());
        assert_eq!(view.changes(), ["showing Work"]);
        assert!(view.is_filtering());
    }

    #[test]
    fn keys_tell_things_apart() {
        assert_eq!(group_key("Work", "github"), "Work/github");
        assert_eq!(column_key("Work", "github", "Drafts"), "Work/github/Drafts");
    }
}
