//! Cards the user marked as in progress. Marks are kept in a small file next
//! to the cache, so the web page and the TUI share them and they survive
//! restarts.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use pending_core::{DashboardSnapshot, SourceStatus};
use serde::{Deserialize, Serialize};
use tracing::warn;

const VERSION: u32 = 1;
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A marked card, with the source it came from: the source's health tells
/// whether a missing card is finished or just not loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Mark {
    id: String,
    source: String,
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    marks: Vec<Mark>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MarkError {
    #[error("no card with that id is on the dashboard")]
    UnknownCard,
    #[error("this dashboard cannot mark cards")]
    Unsupported,
}

/// The marked cards, in marking order.
pub struct Marks {
    /// `None` keeps the marks in memory only.
    path: Option<PathBuf>,
    marks: Mutex<Vec<Mark>>,
}

impl Marks {
    /// Reads the marks at `path`; a missing or unreadable file starts empty.
    pub fn load(path: Option<PathBuf>) -> Self {
        let marks = read(path.as_ref()).unwrap_or_default();
        Self {
            path,
            marks: Mutex::new(marks),
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
        let mut marks = self.lock();
        let before = marks.clone();
        if marked {
            let source = source_of(snapshot, id).ok_or(MarkError::UnknownCard)?;
            if !marks.iter().any(|mark| mark.id == id) {
                marks.push(Mark {
                    id: id.to_owned(),
                    source,
                });
            }
        } else {
            marks.retain(|mark| mark.id != id);
        }
        if *marks != before {
            self.save(&marks);
        }
        Ok(())
    }

    /// Lists the marked cards in `snapshot.marked`, first dropping the marks
    /// that no longer apply: those of sources removed from the config, and
    /// those of cards missing after a clean refresh of their source (the
    /// card was finished elsewhere). While a source is failing, refreshing
    /// or showing old data, its marks stay.
    pub fn apply(&self, snapshot: &mut DashboardSnapshot) {
        let mut marks = self.lock();
        let before = marks.len();
        marks.retain(|mark| {
            let Some(health) = snapshot.sources.iter().find(|s| s.name == mark.source) else {
                return false;
            };
            health.status != SourceStatus::Ready || source_of(snapshot, &mark.id).is_some()
        });
        if marks.len() != before {
            self.save(&marks);
        }
        snapshot.marked = marks.iter().map(|mark| mark.id.clone()).collect();
    }

    /// The marks, brought up to date with the file first: the daemon and
    /// the TUI are separate processes sharing it. An unreadable file keeps
    /// what is in memory.
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Mark>> {
        let mut marks = self.marks.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(on_disk) = read(self.path.as_ref()) {
            *marks = on_disk;
        }
        marks
    }

    /// Written to a private temporary file and renamed, so a reader never
    /// sees a partial write. A failure is logged; the marks stay in memory.
    fn save(&self, marks: &[Mark]) {
        let Some(path) = &self.path else {
            return;
        };
        let file = File {
            version: VERSION,
            marks: marks.to_vec(),
        };
        let tmp = path.with_extension(format!(
            "tmp.{}.{}",
            std::process::id(),
            TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let written = (|| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            options
                .open(&tmp)?
                .write_all(&serde_json::to_vec(&file).map_err(std::io::Error::other)?)?;
            std::fs::rename(&tmp, path)
        })();
        if let Err(error) = written {
            let _ = std::fs::remove_file(&tmp);
            warn!(%error, "marks not saved; they will be lost on restart");
        }
    }
}

/// The marks in the file at `path`, when it exists and is readable.
fn read(path: Option<&PathBuf>) -> Option<Vec<Mark>> {
    let bytes = std::fs::read(path?).ok()?;
    let file: File = serde_json::from_slice(&bytes).ok()?;
    (file.version == VERSION).then_some(file.marks)
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
