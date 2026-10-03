//! A small list kept in a JSON file next to the cache, shared by the daemon
//! and the TUI.
//!
//! They are separate processes, so the file is the single source of truth:
//! it is read before every use, and changed only under a lock file held
//! across read-modify-write.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tracing::warn;

const VERSION: u64 = 1;
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// What the file holds.
enum OnDisk<T> {
    /// The entries; a missing or unreadable file has none.
    Entries(Vec<T>),
    /// Written by a newer version: left alone, never overwritten.
    Newer,
}

pub(crate) struct Store<T> {
    /// `None` keeps the entries in memory only.
    path: Option<PathBuf>,
    /// Name of the list inside the file, e.g. `marks`.
    field: &'static str,
    /// The entries when there is no file; also serializes this process's
    /// changes.
    memory: Mutex<Vec<T>>,
}

impl<T: Clone + PartialEq + Serialize + DeserializeOwned> Store<T> {
    pub(crate) fn new(path: Option<PathBuf>, field: &'static str) -> Self {
        Self {
            path,
            field,
            memory: Mutex::new(Vec::new()),
        }
    }

    /// The entries right now.
    pub(crate) fn current(&self) -> Vec<T> {
        match &self.path {
            None => self.memory().clone(),
            Some(path) => match self.read(path) {
                OnDisk::Entries(entries) => entries,
                OnDisk::Newer => Vec::new(),
            },
        }
    }

    /// Applies `edit` to the entries and saves them when they changed. With
    /// a file, the whole read-modify-write runs under a lock file, so two
    /// processes cannot overwrite each other's change.
    pub(crate) fn change(&self, edit: impl FnOnce(&mut Vec<T>)) {
        let mut memory = self.memory();
        let Some(path) = &self.path else {
            edit(&mut memory);
            return;
        };
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let lock = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(path.with_extension("lock"))?;
            lock.lock()?;
            let OnDisk::Entries(mut entries) = self.read(path) else {
                warn!(file = %path.display(), "file is from a newer version; leaving it unchanged");
                return Ok(());
            };
            let before = entries.clone();
            edit(&mut entries);
            if entries != before {
                self.save(path, &entries)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            warn!(%error, file = %path.display(), "not saved");
        }
    }

    fn memory(&self) -> std::sync::MutexGuard<'_, Vec<T>> {
        self.memory.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn read(&self, path: &Path) -> OnDisk<T> {
        let empty = || OnDisk::Entries(Vec::new());
        let Ok(bytes) = std::fs::read(path) else {
            return empty();
        };
        let Ok(mut file) = serde_json::from_slice::<Value>(&bytes) else {
            return empty();
        };
        match file.get("version").and_then(Value::as_u64) {
            Some(VERSION) => {}
            Some(version) if version > VERSION => return OnDisk::Newer,
            _ => return empty(),
        }
        let entries = file
            .get_mut(self.field)
            .map(Value::take)
            .unwrap_or_default();
        match serde_json::from_value(entries) {
            Ok(entries) => OnDisk::Entries(entries),
            Err(_) => empty(),
        }
    }

    /// Written to a private temporary file and renamed, so a reader never
    /// sees a partial write.
    fn save(&self, path: &Path, entries: &[T]) -> std::io::Result<()> {
        let file = json!({ "version": VERSION, self.field: entries });
        let tmp = path.with_extension(format!(
            "tmp.{}.{}",
            std::process::id(),
            TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options
            .open(&tmp)
            .and_then(|mut out| {
                out.write_all(&serde_json::to_vec(&file).map_err(std::io::Error::other)?)
            })
            .and_then(|()| std::fs::rename(&tmp, path))
            .inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })
    }
}

/// Days an entry is kept after its source left the dashboard, in case the
/// source comes back.
pub(crate) const ABSENT_SOURCE_DAYS: i64 = 30;

/// Whether an entry about the card `id` of `source`, made at `since`, still
/// applies to `snapshot`. It no longer does when the card is missing after
/// a clean refresh of its source that completed after `since` (the item was
/// finished elsewhere). An older refresh may simply not have the card yet
/// (the daemon and the TUI fetch on their own schedules), and a failing,
/// refreshing or stale source says nothing. A source absent from the
/// dashboard keeps its entries for [`ABSENT_SOURCE_DAYS`]. A card in
/// `hidden` is left out of the boards by an `exclude` pattern but its
/// source still lists it, so it is not finished.
pub(crate) fn still_applies(
    snapshot: &pending_core::DashboardSnapshot,
    hidden: &std::collections::HashSet<String>,
    source: &str,
    id: &str,
    since: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    let Some(health) = snapshot.sources.iter().find(|s| s.name == source) else {
        return now - since < chrono::Duration::days(ABSENT_SOURCE_DAYS);
    };
    let seen_since = health.status == pending_core::SourceStatus::Ready
        && health
            .last_refresh_at
            .is_some_and(|refreshed| refreshed > since);
    !seen_since || source_of(snapshot, id).is_some() || hidden.contains(id)
}

/// The source whose group holds the card `id`.
pub(crate) fn source_of(snapshot: &pending_core::DashboardSnapshot, id: &str) -> Option<String> {
    snapshot
        .boards
        .iter()
        .flat_map(|board| &board.groups)
        .find(|group| {
            group
                .columns
                .iter()
                .any(|column| column.cards.iter().any(|card| card.id == id))
        })
        .map(|group| group.source.clone())
}
