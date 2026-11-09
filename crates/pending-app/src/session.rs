//! What the window holds beyond the dashboard: the alerts set on cards and
//! those missed, which last as long as the window, and the todos written
//! down in it, which are kept in the todo file.

use std::path::Path;

use crate::alert::{Alerts, Missed};
use crate::todo_store::{Saved, Store};
use crate::view::Stage;

#[derive(Debug, Default)]
pub struct Session {
    pub alerts: Alerts,
    /// The alerts that rang with the window away, for the icon's dot.
    pub missed: Missed,
    pub todos: Todos,
}

impl Session {
    /// Rewrites the todo `id`, and the alert set on it rings with the new
    /// title. Says whether anything changed.
    pub fn edit_todo(&mut self, id: &str, title: &str, body: &str) -> bool {
        if !self.todos.edit(id, title, body) {
            return false;
        }
        if let Some(todo) = self.todos.get(id) {
            self.alerts.retitle(id, &todo.title);
        }
        true
    }

    /// Forgets the todo `id` and the alert set on it: a todo deleted on
    /// purpose must not ring later, with no card left to cancel it from.
    pub fn forget_todo(&mut self, id: &str) -> bool {
        if !self.todos.remove(id) {
            return false;
        }
        self.alerts.cancel(id);
        true
    }
}

/// A todo written down in the window: a card of the window's own, on no
/// source, that lives in the Activity area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Todo {
    pub id: String,
    pub title: String,
    pub body: String,
    /// Where it stands in the Activity area.
    pub stage: Stage,
}

/// The todos, in the order they were written. Each change goes to the
/// store, when there is one; the list here is what the window shows,
/// saved or not.
#[derive(Debug, Default)]
pub struct Todos {
    list: Vec<Todo>,
    /// Ids count up and are never reused: a stage or an alert of a
    /// deleted todo must not land on a new one.
    next: u64,
    store: Option<Store>,
    /// The todos outlast the window: the store is a file.
    kept: bool,
    /// Why the todos are not being saved, while they are not.
    unsaved: Option<String>,
}

impl Todos {
    /// The todos kept in the file at `path`. A file that cannot be opened
    /// or read gives none, kept for the run only, saying why.
    pub fn open(path: &Path) -> Todos {
        let opened = Store::open(path).and_then(|store| {
            let saved = store.load()?;
            Ok((store, saved))
        });
        match opened {
            Ok((store, Saved { todos, last })) => Todos {
                unsaved: (!store.writable()).then(|| {
                    format!(
                        "{} comes from a newer TasksPending; changes last until it quits",
                        path.display()
                    )
                }),
                list: todos,
                next: last,
                store: Some(store),
                kept: true,
            },
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "opening the todos");
                Todos {
                    unsaved: Some(error.to_string()),
                    ..Todos::default()
                }
            }
        }
    }

    /// Todos for the sandbox: the same store, in memory.
    pub fn in_memory() -> Todos {
        Todos {
            store: Store::in_memory()
                .inspect_err(|error| tracing::warn!(%error, "opening the sandbox's todos"))
                .ok(),
            ..Todos::default()
        }
    }

    /// Todos for a screenshot: in memory, but said to be kept, since the
    /// picture shows the real window.
    pub fn picture() -> Todos {
        Todos {
            kept: true,
            ..Todos::in_memory()
        }
    }

    /// Whether the todos outlast the window, as the window says.
    pub fn kept(&self) -> bool {
        self.kept
    }

    /// Why the todos are not being saved, while they are not.
    pub fn unsaved(&self) -> Option<&str> {
        self.unsaved.as_deref()
    }

    /// Writes the list to the store. The whole list goes each time, so a
    /// save that works makes good every one that failed before it.
    fn save(&mut self) {
        let Some(store) = self.store.as_mut().filter(|store| store.writable()) else {
            return;
        };
        match store.save(&self.list, self.next) {
            Ok(()) => self.unsaved = None,
            Err(error) => {
                tracing::warn!(%error, "saving the todos");
                self.unsaved = Some(format!("Could not save the todos ({error})"));
            }
        }
    }

    /// Writes down a todo in Picked and gives its id; none for a blank
    /// title. Title and body are kept without the blanks around them.
    pub fn add(&mut self, title: &str, body: &str) -> Option<String> {
        let title = title.trim();
        if title.is_empty() {
            return None;
        }
        self.next += 1;
        let id = format!("todo:{}", self.next);
        self.list.push(Todo {
            id: id.clone(),
            title: title.to_owned(),
            body: body.trim().to_owned(),
            stage: Stage::Picked,
        });
        self.save();
        Some(id)
    }

    /// Rewrites the todo `id`; a blank title changes nothing. Says whether
    /// it did.
    pub fn edit(&mut self, id: &str, title: &str, body: &str) -> bool {
        let title = title.trim();
        if title.is_empty() {
            return false;
        }
        let Some(todo) = self.list.iter_mut().find(|todo| todo.id == id) else {
            return false;
        };
        todo.title = title.to_owned();
        todo.body = body.trim().to_owned();
        self.save();
        true
    }

    /// Forgets the todo `id`.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.list.len();
        self.list.retain(|todo| todo.id != id);
        if self.list.len() == before {
            return false;
        }
        self.save();
        true
    }

    /// Moves the todo `id` to `stage` of the Activity area.
    pub fn move_to(&mut self, id: &str, stage: Stage) -> bool {
        match self.list.iter_mut().find(|todo| todo.id == id) {
            Some(todo) => {
                todo.stage = stage;
                self.save();
                true
            }
            None => false,
        }
    }

    pub fn get(&self, id: &str) -> Option<&Todo> {
        self.list.iter().find(|todo| todo.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Todo> {
        self.list.iter()
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Local, TimeZone};

    use super::*;
    use crate::alert::Alert;

    /// An alert set on a todo goes with it: deleted, nothing rings about
    /// it; rewritten, the alert rings with the new title.
    #[test]
    fn a_todo_takes_its_alert_along() {
        let mut session = Session::default();
        let id = session.todos.add("Call the bank", "").unwrap();
        let at = Local.with_ymd_and_hms(2026, 10, 7, 16, 0, 0).unwrap();
        session.alerts.set(
            &id,
            Alert {
                at,
                title: "Call the bank".to_owned(),
                source: "Todo".to_owned(),
            },
        );

        assert!(session.edit_todo(&id, "Call the bank about the card", ""));
        assert_eq!(
            session.alerts.cancel(&id).map(|alert| alert.title),
            Some("Call the bank about the card".to_owned())
        );
        session.alerts.set(
            &id,
            Alert {
                at,
                title: "Call the bank about the card".to_owned(),
                source: "Todo".to_owned(),
            },
        );
        assert!(
            !session.edit_todo(&id, " ", ""),
            "a blank title changes nothing"
        );

        assert!(session.forget_todo(&id));
        assert_eq!(session.todos.get(&id), None);
        assert!(session.alerts.due(at).is_empty(), "nothing rings about it");
        assert!(!session.forget_todo(&id));
    }

    /// A todo needs a title; it keeps the title and the details without
    /// the blanks around them, can be rewritten and forgotten, and ids are
    /// never given twice.
    #[test]
    fn writes_down_rewrites_and_forgets_todos() {
        let mut todos = Todos::default();
        assert_eq!(todos.add("  ", "details"), None);
        let first = todos
            .add("  Write the release notes ", " for 0.7 ")
            .unwrap();
        assert_eq!(first, "todo:1");
        assert_eq!(
            todos.get(&first),
            Some(&Todo {
                id: "todo:1".to_owned(),
                title: "Write the release notes".to_owned(),
                body: "for 0.7".to_owned(),
                stage: Stage::Picked,
            })
        );
        assert!(todos.edit(&first, "Write the notes", ""));
        assert!(
            !todos.edit(&first, " ", "kept"),
            "a blank title changes nothing"
        );
        assert_eq!(
            todos.get(&first).map(|t| t.title.as_str()),
            Some("Write the notes")
        );
        assert_eq!(todos.get(&first).map(|t| t.body.as_str()), Some(""));
        assert!(!todos.edit("todo:9", "x", ""));

        assert!(todos.remove(&first));
        assert!(!todos.remove(&first));
        assert_eq!(
            todos.add("Again", ""),
            Some("todo:2".to_owned()),
            "ids are not reused"
        );
        assert_eq!(todos.iter().count(), 1);
    }

    /// A fresh folder for one test, gone with its files when it ends.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!(
                "tasks-pending-session-{}-{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn file(&self) -> std::path::PathBuf {
            self.0.join("tasks-pending").join("todos.sqlite3")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn titles(todos: &Todos) -> Vec<(&str, Stage)> {
        todos
            .iter()
            .map(|todo| (todo.title.as_str(), todo.stage))
            .collect()
    }

    /// Todos written down, moved, rewritten and deleted are there as they
    /// were after a restart, and an id given before it is not given again.
    #[test]
    fn todos_outlast_the_window() {
        let scratch = Scratch::new("outlast");
        let path = scratch.file();
        let mut todos = Todos::open(&path);
        assert!(todos.kept());
        assert_eq!(todos.unsaved(), None);
        let bank = todos.add("Call the bank", "").unwrap();
        let notes = todos.add("Write the notes", "for 0.9").unwrap();
        let gone = todos.add("Forget me", "").unwrap();
        assert!(todos.move_to(&bank, Stage::InProgress));
        assert!(todos.edit(&notes, "Write the release notes", "for 0.9"));
        assert!(todos.remove(&gone));
        drop(todos);

        let mut todos = Todos::open(&path);
        assert_eq!(
            titles(&todos),
            [
                ("Call the bank", Stage::InProgress),
                ("Write the release notes", Stage::Picked),
            ]
        );
        assert_eq!(todos.get(&notes).map(|t| t.body.as_str()), Some("for 0.9"));
        assert_eq!(todos.add("Next", ""), Some("todo:4".to_owned()));
        assert_eq!(todos.unsaved(), None);
    }

    /// A todo file that is not TasksPending's is left alone: the window
    /// keeps its todos for the run, as before 0.9, and says they are not
    /// saved.
    #[test]
    fn says_when_todos_cannot_be_saved() {
        let scratch = Scratch::new("foreign");
        let path = scratch.file();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"my notes").unwrap();

        let mut todos = Todos::open(&path);
        assert!(!todos.kept());
        let unsaved = todos.unsaved().unwrap().to_owned();
        assert!(
            unsaved.contains("is not a todo file of TasksPending"),
            "{unsaved}"
        );
        assert_eq!(todos.add("Still works", ""), Some("todo:1".to_owned()));
        assert_eq!(todos.iter().count(), 1);
        assert_eq!(std::fs::read(&path).unwrap(), b"my notes");
    }

    /// A todo file from a newer TasksPending shows its todos, and changes
    /// to them last only for the run.
    #[test]
    fn shows_a_newer_file_without_saving_to_it() {
        let scratch = Scratch::new("newer");
        let path = scratch.file();
        let mut todos = Todos::open(&path);
        todos.add("From the newer one", "").unwrap();
        drop(todos);
        rusqlite::Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();

        let mut todos = Todos::open(&path);
        assert_eq!(titles(&todos), [("From the newer one", Stage::Picked)]);
        assert!(todos.unsaved().unwrap().contains("newer TasksPending"));
        todos.add("Only for now", "").unwrap();
        drop(todos);
        assert_eq!(Todos::open(&path).iter().count(), 1);
    }

    /// A write that fails says so, and the todo stays in the window.
    #[test]
    fn says_when_a_save_fails() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let scratch = Scratch::new("readonly");
        let path = scratch.file();
        drop(Todos::open(&path));
        if std::fs::metadata(&path).unwrap().uid() == 0 {
            return; // root writes to a read-only file all the same
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();

        let mut todos = Todos::open(&path);
        assert_eq!(todos.unsaved(), None);
        assert!(todos.add("Kept for now", "").is_some());
        assert!(
            todos
                .unsaved()
                .unwrap()
                .starts_with("Could not save the todos")
        );
        assert_eq!(todos.iter().count(), 1);
    }

    /// A save that fails for a while (another program holds the file)
    /// is made good by the next one that works: the whole list is
    /// written, and the Activity stops saying the todos are not saved.
    #[test]
    fn a_save_that_works_makes_good_one_that_failed() {
        let scratch = Scratch::new("busy");
        let path = scratch.file();
        let mut todos = Todos::open(&path);

        let other = rusqlite::Connection::open(&path).unwrap();
        other.execute_batch("BEGIN EXCLUSIVE").unwrap();
        assert!(todos.add("Written while busy", "").is_some());
        assert!(todos.unsaved().is_some(), "the file was held");

        other.execute_batch("COMMIT").unwrap();
        drop(other);
        assert!(todos.add("Written after", "").is_some());
        assert_eq!(todos.unsaved(), None);

        let reopened = Todos::open(&path);
        let titles: Vec<_> = reopened.iter().map(|todo| todo.title.as_str()).collect();
        assert_eq!(titles, ["Written while busy", "Written after"]);
    }

    /// The sandbox's todos go through the same store, in memory, and are
    /// not kept.
    #[test]
    fn sandbox_todos_are_not_kept() {
        let mut todos = Todos::in_memory();
        assert!(!todos.kept());
        assert_eq!(todos.unsaved(), None);
        assert!(todos.add("Try it", "").is_some());
        assert_eq!(todos.unsaved(), None);
    }

    /// A screenshot keeps its todos in memory too, but pictures the real
    /// window, whose todos are kept.
    #[test]
    fn a_screenshot_pictures_kept_todos() {
        let mut todos = Todos::picture();
        assert!(todos.kept());
        assert!(todos.add("Pictured", "").is_some());
        assert_eq!(todos.unsaved(), None);
    }

    /// A todo is written down in Picked, the first stage of the Activity
    /// area, and moves along the stages; one that is not there cannot be
    /// moved.
    #[test]
    fn writes_todos_down_in_picked() {
        let mut todos = Todos::default();
        let id = todos.add("Call the bank", "").unwrap();
        assert_eq!(todos.get(&id).map(|t| t.stage), Some(Stage::Picked));
        assert!(todos.move_to(&id, Stage::InProgress));
        assert_eq!(todos.get(&id).map(|t| t.stage), Some(Stage::InProgress));
        assert!(!todos.move_to("todo:9", Stage::Picked));
    }
}
