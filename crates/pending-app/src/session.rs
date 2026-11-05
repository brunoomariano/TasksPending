//! What lasts as long as the window and no longer: the alerts set on
//! cards, and the todos written down in the window.

use crate::alert::Alerts;
use crate::view::Stage;

#[derive(Debug, Default)]
pub struct Session {
    pub alerts: Alerts,
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
/// source, that lives in the Activity area and is kept only while the
/// window is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Todo {
    pub id: String,
    pub title: String,
    pub body: String,
    /// Where it stands in the Activity area.
    pub stage: Stage,
}

/// The todos, in the order they were written.
#[derive(Debug, Default)]
pub struct Todos {
    list: Vec<Todo>,
    /// Ids count up and are never reused: a stage or an alert of a
    /// deleted todo must not land on a new one.
    next: u64,
}

impl Todos {
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
        true
    }

    /// Forgets the todo `id`.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.list.len();
        self.list.retain(|todo| todo.id != id);
        self.list.len() != before
    }

    /// Moves the todo `id` to `stage` of the Activity area.
    pub fn move_to(&mut self, id: &str, stage: Stage) -> bool {
        match self.list.iter_mut().find(|todo| todo.id == id) {
            Some(todo) => {
                todo.stage = stage;
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
