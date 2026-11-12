//! The todos on disk: one SQLite file, `todos.sqlite3` in the data dir,
//! that TasksPending alone writes. Each change rewrites the list in one
//! transaction, so a write that failed is made good by the next one.
//!
//! A todo in Done keeps when it went there, so it is deleted on time
//! across restarts.
//!
//! The file says it is TasksPending's (its `application_id`) and which
//! schema it holds (its `user_version`). A file that is not ours, or not a
//! database at all, is left as it is; one written by a newer TasksPending
//! is read and never written.

use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::DateTime;
use rusqlite::{Connection, ErrorCode, OpenFlags, params};

use crate::session::Todo;
use crate::view::Stage;

/// What the file is called in the data dir.
pub const FILE_NAME: &str = "todos.sqlite3";

/// Marks the file as TasksPending's todos: "TPtd".
const APPLICATION_ID: i32 = 0x5450_7464;

/// How long a write waits for a lock someone else holds.
const BUSY_TIMEOUT: Duration = Duration::from_millis(500);

/// The schemas in order: the migration at index `n` takes the file from
/// version `n` to `n + 1`. The last one is what this build writes.
const MIGRATIONS: [&str; 2] = [
    // 1: the todos and the last id given.
    "CREATE TABLE todos (
        seq   INTEGER PRIMARY KEY CHECK (seq > 0),
        title TEXT NOT NULL CHECK (trim(title) <> ''),
        body  TEXT NOT NULL,
        stage TEXT NOT NULL CHECK (stage IN ('picked', 'doing', 'done'))
    ) STRICT;
    CREATE TABLE ids (
        one  INTEGER PRIMARY KEY CHECK (one = 1),
        last INTEGER NOT NULL CHECK (last >= 0)
    ) STRICT;
    INSERT INTO ids (one, last) VALUES (1, 0);",
    // 2: when a todo in Done went there, in seconds since the epoch.
    "ALTER TABLE todos ADD COLUMN done_at INTEGER;",
];

/// The schema this build writes.
const SCHEMA: i32 = MIGRATIONS.len() as i32;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{} is not a todo file of TasksPending", .0.display())]
    Foreign(PathBuf),
    #[error("could not create {}: {source}", .path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the todo file comes from a newer TasksPending")]
    Newer,
    #[error("a saved todo is unreadable: {0}")]
    Malformed(String),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

/// The todo file, open.
pub struct Store {
    conn: Connection,
    /// Off for a file from a newer TasksPending.
    writable: bool,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("path", &self.conn.path())
            .field("writable", &self.writable)
            .finish()
    }
}

/// What the file holds.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Saved {
    /// In the order they were written.
    pub todos: Vec<Todo>,
    /// The highest id ever given, deleted todos included.
    pub last: u64,
}

impl Store {
    /// Opens the todo file at `path`, creating it (and its folder) for the
    /// user alone when it is not there, and bringing an older schema up to
    /// date.
    pub fn open(path: &Path) -> Result<Store, StoreError> {
        create_private(path)?;
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| foreign(path, error))?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Store::start(conn, path)
    }

    /// A store that lives in memory, for the sandbox.
    pub fn in_memory() -> Result<Store, StoreError> {
        Store::start(Connection::open_in_memory()?, Path::new(":memory:"))
    }

    /// Reads what `conn` holds and brings it up to date.
    fn start(mut conn: Connection, path: &Path) -> Result<Store, StoreError> {
        let id: i32 = conn
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .map_err(|error| foreign(path, error))?;
        let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if id != APPLICATION_ID {
            let tables: i64 =
                conn.query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))?;
            if id != 0 || version != 0 || tables != 0 {
                return Err(StoreError::Foreign(path.to_owned()));
            }
        }
        if version > SCHEMA {
            return Ok(Store {
                conn,
                writable: false,
            });
        }
        let pending = MIGRATIONS
            .get(version.max(0) as usize..)
            .unwrap_or_default();
        if !pending.is_empty() {
            let tx = conn.transaction()?;
            for migration in pending {
                tx.execute_batch(migration)?;
            }
            tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            tx.pragma_update(None, "user_version", SCHEMA)?;
            tx.commit()?;
        }
        Ok(Store {
            conn,
            writable: true,
        })
    }

    /// Whether changes are written: not to a file from a newer TasksPending.
    pub fn writable(&self) -> bool {
        self.writable
    }

    /// The todos in the file.
    pub fn load(&self) -> Result<Saved, StoreError> {
        let mut statement = self
            .conn
            .prepare("SELECT seq, title, body, stage, done_at FROM todos ORDER BY seq")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })?;
        let mut todos = Vec::new();
        for row in rows {
            let (seq, title, body, stage, done_at) = row?;
            let stage = parse_stage(&stage)
                .ok_or_else(|| StoreError::Malformed(format!("todo:{seq} is at {stage:?}")))?;
            let done_at = done_at
                .map(|secs| {
                    DateTime::from_timestamp(secs, 0).ok_or_else(|| {
                        StoreError::Malformed(format!("todo:{seq} went to Done at {secs}"))
                    })
                })
                .transpose()?;
            todos.push(Todo {
                id: format!("todo:{seq}"),
                title,
                body,
                stage,
                done_at,
            });
        }
        let last: i64 = self
            .conn
            .query_row("SELECT last FROM ids WHERE one = 1", [], |row| row.get(0))?;
        Ok(Saved {
            todos,
            last: u64::try_from(last).unwrap_or_default(),
        })
    }

    /// Rewrites the file to hold `todos`, `last` being the highest id
    /// given. All of it or none: a todo refused leaves the file as it was.
    pub fn save(&mut self, todos: &[Todo], last: u64) -> Result<(), StoreError> {
        if !self.writable {
            return Err(StoreError::Newer);
        }
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM todos", [])?;
        let mut highest = last;
        {
            let mut insert = tx.prepare(
                "INSERT INTO todos (seq, title, body, stage, done_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for todo in todos {
                let n = seq(&todo.id)
                    .filter(|n| *n > 0)
                    .ok_or_else(|| StoreError::Malformed(format!("{:?} is no todo id", todo.id)))?;
                insert.execute(params![
                    i64::try_from(n).map_err(|_| StoreError::Malformed(todo.id.clone()))?,
                    todo.title,
                    todo.body,
                    stage_name(todo.stage),
                    todo.done_at.map(|at| at.timestamp()),
                ])?;
                highest = highest.max(n);
            }
        }
        tx.execute(
            "UPDATE ids SET last = ?1 WHERE one = 1",
            params![
                i64::try_from(highest).map_err(|_| StoreError::Malformed(highest.to_string()))?
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
}

/// Makes the folders up to `path` and an empty `path`, for the user
/// alone; what is already there keeps its mode.
fn create_private(path: &Path) -> Result<(), StoreError> {
    let failed = |source| StoreError::Create {
        path: path.to_owned(),
        source,
    };
    if let Some(dir) = path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(failed)?;
    }
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(failed(error)),
    }
}

/// `error` as the refusal of a file that is not a database.
fn foreign(path: &Path, error: rusqlite::Error) -> StoreError {
    match error.sqlite_error_code() {
        Some(ErrorCode::NotADatabase) => StoreError::Foreign(path.to_owned()),
        _ => StoreError::Sqlite(error),
    }
}

/// How a stage is written in the file: as `app-view.json` writes it.
fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::Picked => "picked",
        Stage::InProgress => "doing",
        Stage::Done => "done",
    }
}

fn parse_stage(name: &str) -> Option<Stage> {
    Stage::ALL
        .into_iter()
        .find(|stage| stage_name(*stage) == name)
}

/// The number in a todo's id, `todo:<n>`.
fn seq(id: &str) -> Option<u64> {
    id.strip_prefix("todo:")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// A fresh folder for one test, gone with its files when it ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir()
                .join(format!("tasks-pending-todos-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn file(&self) -> PathBuf {
            self.0.join("data").join("tasks-pending").join(FILE_NAME)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn todo(n: u64, title: &str, body: &str, stage: Stage) -> Todo {
        Todo {
            id: format!("todo:{n}"),
            title: title.to_owned(),
            body: body.to_owned(),
            stage,
            done_at: None,
        }
    }

    /// A moment, as the file keeps it: to the second.
    fn moment(secs: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(secs, 0).unwrap()
    }

    /// The file holds the todos in the order they were written, each with
    /// its stage, and the highest id given even once that todo is gone.
    #[test]
    fn keeps_the_todos_across_runs() {
        let scratch = Scratch::new("roundtrip");
        let path = scratch.file();
        let mut store = Store::open(&path).unwrap();
        assert!(store.writable());
        assert_eq!(store.load().unwrap(), Saved::default());

        let todos = vec![
            todo(2, "Call the bank", "about the card", Stage::InProgress),
            todo(5, "Write the notes", "", Stage::Picked),
            todo(7, "Ship 0.9", "", Stage::Done),
        ];
        store.save(&todos, 9).unwrap();
        drop(store);

        let store = Store::open(&path).unwrap();
        assert_eq!(store.load().unwrap(), Saved { todos, last: 9 });
    }

    /// Each save replaces what was there: a todo left out is gone, and an
    /// id never goes back below one already given.
    #[test]
    fn a_save_replaces_the_list() {
        let scratch = Scratch::new("replace");
        let mut store = Store::open(&scratch.file()).unwrap();
        store
            .save(
                &[
                    todo(1, "One", "", Stage::Picked),
                    todo(2, "Two", "", Stage::Picked),
                ],
                2,
            )
            .unwrap();
        store.save(&[todo(2, "Two", "", Stage::Done)], 2).unwrap();
        assert_eq!(
            store.load().unwrap(),
            Saved {
                todos: vec![todo(2, "Two", "", Stage::Done)],
                last: 2,
            }
        );
        // A last below the highest id saved would hand that id out again.
        store
            .save(&[todo(4, "Four", "", Stage::Picked)], 1)
            .unwrap();
        assert_eq!(store.load().unwrap().last, 4);
    }

    /// A todo the window would never write is refused, and the file keeps
    /// what it had.
    #[test]
    fn refuses_a_todo_it_cannot_keep() {
        let scratch = Scratch::new("refuse");
        let mut store = Store::open(&scratch.file()).unwrap();
        let kept = [todo(1, "Kept", "", Stage::Picked)];
        store.save(&kept, 1).unwrap();
        assert!(store.save(&[todo(2, "  ", "", Stage::Picked)], 2).is_err());
        let mut bad = todo(3, "Bad id", "", Stage::Picked);
        bad.id = "card:3".to_owned();
        assert!(store.save(&[bad], 3).is_err());
        assert_eq!(
            store.load().unwrap(),
            Saved {
                todos: kept.to_vec(),
                last: 1,
            }
        );
    }

    /// A stage is written as the view file writes it, so the two never
    /// disagree on a name.
    #[test]
    fn writes_stages_as_the_view_does() {
        for stage in Stage::ALL {
            assert_eq!(
                serde_json::to_value(stage).unwrap(),
                serde_json::Value::from(stage_name(stage))
            );
            assert_eq!(parse_stage(stage_name(stage)), Some(stage));
        }
        assert_eq!(parse_stage("review"), None);
    }

    /// The file and the folder made for it are the user's alone; a folder
    /// that was already there keeps its mode.
    #[test]
    fn creates_the_file_for_the_user_alone() {
        let scratch = Scratch::new("modes");
        let path = scratch.file();
        Store::open(&path).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(mode(path.parent().unwrap().parent().unwrap()), 0o700);
        assert_ne!(mode(&scratch.0), 0o700, "an existing folder is left alone");
    }

    /// A file that is not a database, or a database of something else, is
    /// refused and left byte for byte as it was.
    #[test]
    fn leaves_a_file_that_is_not_its_own() {
        let scratch = Scratch::new("foreign");
        let path = scratch.file();
        fs::create_dir_all(path.parent().unwrap()).unwrap();

        fs::write(&path, b"not a database, just some notes\n").unwrap();
        assert!(matches!(Store::open(&path), Err(StoreError::Foreign(_))));
        assert_eq!(
            fs::read(&path).unwrap(),
            b"not a database, just some notes\n"
        );

        fs::remove_file(&path).unwrap();
        let other = Connection::open(&path).unwrap();
        other
            .execute_batch("CREATE TABLE notes (text TEXT); INSERT INTO notes VALUES ('mine');")
            .unwrap();
        drop(other);
        let before = fs::read(&path).unwrap();
        assert!(matches!(Store::open(&path), Err(StoreError::Foreign(_))));
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    /// A file from a newer TasksPending is read but never written: this
    /// build would drop what it does not know.
    #[test]
    fn reads_a_newer_file_without_writing_it() {
        let scratch = Scratch::new("newer");
        let path = scratch.file();
        let mut store = Store::open(&path).unwrap();
        store
            .save(&[todo(1, "From before", "", Stage::Picked)], 1)
            .unwrap();
        drop(store);
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", SCHEMA + 1)
            .unwrap();
        let before = fs::read(&path).unwrap();

        let mut store = Store::open(&path).unwrap();
        assert!(!store.writable());
        assert_eq!(store.load().unwrap().todos.len(), 1);
        assert!(matches!(store.save(&[], 1), Err(StoreError::Newer)));
        drop(store);
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    /// An empty file, as a crash right after creating it leaves, becomes a
    /// todo file at the current schema.
    #[test]
    fn brings_an_empty_file_up_to_date() {
        let scratch = Scratch::new("empty");
        let path = scratch.file();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"").unwrap();
        let store = Store::open(&path).unwrap();
        assert_eq!(store.load().unwrap(), Saved::default());
        let conn = Connection::open(&path).unwrap();
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let id: i32 = conn
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .unwrap();
        assert_eq!((version, id), (SCHEMA, APPLICATION_ID));
    }

    /// When a todo went to Done is kept with it, and one in Done from
    /// before the time was kept stays without one.
    #[test]
    fn keeps_when_a_todo_went_to_done() {
        let scratch = Scratch::new("done-at");
        let path = scratch.file();
        let mut store = Store::open(&path).unwrap();
        let done = Todo {
            done_at: Some(moment(1_794_500_000)),
            ..todo(1, "Call the bank", "", Stage::Done)
        };
        let undated = todo(2, "From 0.10.0", "", Stage::Done);
        store.save(&[done.clone(), undated.clone()], 2).unwrap();
        drop(store);
        assert_eq!(
            Store::open(&path).unwrap().load().unwrap().todos,
            [done, undated]
        );
    }

    /// A file of the first schema, as 0.9 and 0.10.0 write it, is brought
    /// up to date with its todos, those in Done without a time yet.
    #[test]
    fn brings_a_first_schema_file_up_to_date() {
        let scratch = Scratch::new("first");
        let path = scratch.file();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let old = Connection::open(&path).unwrap();
        old.execute_batch(MIGRATIONS[0]).unwrap();
        old.execute_batch(
            "INSERT INTO todos (seq, title, body, stage) VALUES (3, 'Ship 0.10', '', 'done');
             UPDATE ids SET last = 3;",
        )
        .unwrap();
        old.pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        old.pragma_update(None, "user_version", 1).unwrap();
        drop(old);

        let mut store = Store::open(&path).unwrap();
        assert!(store.writable());
        let saved = store.load().unwrap();
        assert_eq!(saved.todos, [todo(3, "Ship 0.10", "", Stage::Done)]);
        assert_eq!(saved.last, 3);
        let dated = Todo {
            done_at: Some(moment(1_794_500_000)),
            ..todo(3, "Ship 0.10", "", Stage::Done)
        };
        store.save(std::slice::from_ref(&dated), 3).unwrap();
        drop(store);
        let version: i32 = Connection::open(&path)
            .unwrap()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2);
        assert_eq!(Store::open(&path).unwrap().load().unwrap().todos, [dated]);
    }

    /// The sandbox's store works the same, and leaves nothing on disk.
    #[test]
    fn keeps_todos_in_memory_for_the_sandbox() {
        let mut store = Store::in_memory().unwrap();
        let todos = [todo(1, "Try it", "", Stage::Picked)];
        store.save(&todos, 1).unwrap();
        assert_eq!(
            store.load().unwrap(),
            Saved {
                todos: todos.to_vec(),
                last: 1,
            }
        );
    }
}
