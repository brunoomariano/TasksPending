//! What lasts as long as the window and no longer: the alerts set on
//! cards, and the todos written down in the window.

use crate::alert::Alerts;
use crate::view::Stage;

#[derive(Debug, Default)]
pub struct Session {
    pub alerts: Alerts,
    pub todos: Todos,
}

/// A todo written down in the window: a card of the window's own, on no
/// source, kept only while the window is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Todo {
    pub id: String,
    pub title: String,
    pub body: String,
    /// Where it stands in the Activity area, once picked.
    pub stage: Option<Stage>,
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
    /// Writes down a todo and gives its id; none for a blank title. Title
    /// and body are kept without the blanks around them.
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
            stage: None,
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

    /// Puts the todo `id` at `stage` of the Activity area, or takes it out
    /// with `None`.
    pub fn pick(&mut self, id: &str, stage: Option<Stage>) -> bool {
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
    use super::*;

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
                stage: None,
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

    /// Picking a todo puts it at a stage of the Activity area, and
    /// unpicking takes it out; a todo that is not there cannot be picked.
    #[test]
    fn picks_todos_into_the_activity() {
        let mut todos = Todos::default();
        let id = todos.add("Call the bank", "").unwrap();
        assert_eq!(todos.get(&id).and_then(|t| t.stage), None);
        assert!(todos.pick(&id, Some(Stage::Doing)));
        assert_eq!(todos.get(&id).and_then(|t| t.stage), Some(Stage::Doing));
        assert!(todos.pick(&id, None));
        assert_eq!(todos.get(&id).and_then(|t| t.stage), None);
        assert!(!todos.pick("todo:9", Some(Stage::Picked)));
    }
}
