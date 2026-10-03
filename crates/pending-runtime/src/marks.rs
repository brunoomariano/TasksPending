//! Cards the user marked as in progress. Marks are kept in a small file next
//! to the cache, so the web page and the TUI share them and they survive
//! restarts.
//!
//! The daemon and the TUI are separate processes that refresh on their own
//! schedules, so the file is the single source of truth: it is read before
//! every use, and changed only under a lock held across read-modify-write.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use chrono::{DateTime, Duration, Utc};
use pending_core::{DashboardSnapshot, SourceStatus};
use serde::{Deserialize, Serialize};
use tracing::warn;

const VERSION: u32 = 1;
/// Marks whose source is no longer on the dashboard are kept this long, in
/// case the source comes back.
const ABSENT_SOURCE_DAYS: i64 = 30;
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A marked card, with the source it came from and when it was marked: the
/// source's health and last refresh tell whether a missing card is finished
/// or just not loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Mark {
    id: String,
    source: String,
    marked_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    #[serde(default)]
    marks: Vec<Mark>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MarkError {
    #[error("no card with that id is on the dashboard")]
    UnknownCard,
    #[error("this dashboard cannot mark cards")]
    Unsupported,
}

/// What the marks file holds.
enum OnDisk {
    /// The marks; a missing or unreadable file has none.
    Marks(Vec<Mark>),
    /// Written by a newer version: left alone, never overwritten.
    Newer,
}

/// The marked cards, in marking order.
pub struct Marks {
    /// `None` keeps the marks in memory only.
    path: Option<PathBuf>,
    /// The marks when there is no file; also serializes this process's
    /// changes.
    memory: Mutex<Vec<Mark>>,
}

impl Marks {
    /// Marks kept at `path`; a missing or unreadable file starts empty.
    pub fn load(path: Option<PathBuf>) -> Self {
        Self {
            path,
            memory: Mutex::new(Vec::new()),
        }
    }

    /// Marks or unmarks the card `id`. Only a card on `snapshot` can be
    /// marked; unmarking a card that is not marked does nothing.
    pub fn set(
        &self,
        snapshot: &DashboardSnapshot,
        id: &str,
        marked: bool,
    ) -> Result<(), MarkError> {
        let source = match marked {
            true => Some(source_of(snapshot, id).ok_or(MarkError::UnknownCard)?),
            false => None,
        };
        self.change(|marks| match source {
            Some(source) if !marks.iter().any(|mark| mark.id == id) => marks.push(Mark {
                id: id.to_owned(),
                source,
                marked_at: Utc::now(),
            }),
            Some(_) => {}
            None => marks.retain(|mark| mark.id != id),
        });
        Ok(())
    }

    /// Lists the marked cards in `snapshot.marked`, first dropping the marks
    /// of finished cards: those missing after a clean refresh of their
    /// source that completed after the card was marked. An older refresh
    /// may simply not have the card yet (the daemon and the TUI fetch on
    /// their own schedules), and a failing, refreshing or stale source says
    /// nothing. A source absent from the dashboard keeps its marks too,
    /// until they are a month old.
    pub fn apply(&self, snapshot: &mut DashboardSnapshot) {
        let now = Utc::now();
        let keep = |mark: &Mark| {
            let Some(health) = snapshot.sources.iter().find(|s| s.name == mark.source) else {
                return now - mark.marked_at < Duration::days(ABSENT_SOURCE_DAYS);
            };
            let seen_since_marked = health.status == SourceStatus::Ready
                && health
                    .last_refresh_at
                    .is_some_and(|refreshed| refreshed > mark.marked_at);
            !seen_since_marked || source_of(snapshot, &mark.id).is_some()
        };

        let mut marks = self.current();
        if !marks.iter().all(keep) {
            // Decided again under the lock, on what the file holds then.
            self.change(|marks| marks.retain(keep));
            marks = self.current();
        }
        snapshot.marked = marks.into_iter().map(|mark| mark.id).collect();
    }

    /// The marks right now.
    fn current(&self) -> Vec<Mark> {
        match &self.path {
            None => self.memory().clone(),
            Some(path) => match read(path) {
                OnDisk::Marks(marks) => marks,
                OnDisk::Newer => Vec::new(),
            },
        }
    }

    /// Applies `edit` to the marks and saves them when they changed. With a
    /// file, the whole read-modify-write runs under a lock file, so two
    /// processes cannot overwrite each other's change.
    fn change(&self, edit: impl FnOnce(&mut Vec<Mark>)) {
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
            let OnDisk::Marks(mut marks) = read(path) else {
                warn!("marks file is from a newer version; leaving it unchanged");
                return Ok(());
            };
            let before = marks.clone();
            edit(&mut marks);
            if marks != before {
                save(path, &marks)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            warn!(%error, "marks not saved");
        }
    }

    fn memory(&self) -> std::sync::MutexGuard<'_, Vec<Mark>> {
        self.memory.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn read(path: &Path) -> OnDisk {
    let Ok(bytes) = std::fs::read(path) else {
        return OnDisk::Marks(Vec::new());
    };
    match serde_json::from_slice::<File>(&bytes) {
        Ok(file) if file.version == VERSION => OnDisk::Marks(file.marks),
        Ok(file) if file.version > VERSION => OnDisk::Newer,
        _ => OnDisk::Marks(Vec::new()),
    }
}

/// Written to a private temporary file and renamed, so a reader never sees a
/// partial write.
fn save(path: &Path, marks: &[Mark]) -> std::io::Result<()> {
    let file = File {
        version: VERSION,
        marks: marks.to_vec(),
    };
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

/// The source whose group holds the card `id`.
fn source_of(snapshot: &DashboardSnapshot, id: &str) -> Option<String> {
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
