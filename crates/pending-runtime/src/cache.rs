//! The last good batch of every source, kept across restarts.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use pending_core::SourceBatch;
use serde::{Deserialize, Serialize};
use tracing::warn;

const VERSION: u32 = 1;

/// Entries not refreshed for this long are dropped on save (sources removed
/// from every config).
const MAX_AGE: chrono::Duration = chrono::Duration::days(30);

/// A JSON file holding, per source name, its last successful batch. Several
/// processes (API, TUI) may share it: each save merges its own entries into
/// what is on disk.
pub struct Cache {
    path: PathBuf,
    /// Serializes saves inside this process.
    lock: Mutex<()>,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache").field("path", &self.path).finish()
    }
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    sources: HashMap<String, Entry>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    refreshed_at: DateTime<Utc>,
    batch: SourceBatch,
}

impl Cache {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    /// Entries by source name. A missing, unreadable or incompatible file
    /// means an empty cache; it is rewritten on the next save.
    pub fn load(&self) -> HashMap<String, (DateTime<Utc>, SourceBatch)> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return HashMap::new(),
            Err(error) => {
                warn!(path = %self.path.display(), %error, "ignoring unreadable cache");
                return HashMap::new();
            }
        };
        match serde_json::from_str::<File>(&text) {
            Ok(file) if file.version == VERSION => file
                .sources
                .into_iter()
                .map(|(name, entry)| (name, (entry.refreshed_at, entry.batch)))
                .collect(),
            Ok(_) => HashMap::new(),
            Err(error) => {
                warn!(path = %self.path.display(), %error, "ignoring corrupt cache");
                HashMap::new()
            }
        }
    }

    /// Merges these entries into the file, replacing entries with the same
    /// source name and keeping the others (another process's sources).
    /// Written to a private temporary file and renamed, so a reader never sees
    /// a partial write.
    pub fn save(&self, entries: &[(String, DateTime<Utc>, SourceBatch)]) -> std::io::Result<()> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let mut merged = self.load();
        for (name, refreshed_at, batch) in entries {
            merged.insert(name.clone(), (*refreshed_at, batch.clone()));
        }
        let oldest = Utc::now() - MAX_AGE;
        let file = File {
            version: VERSION,
            sources: merged
                .into_iter()
                .filter(|(_, (refreshed_at, _))| *refreshed_at >= oldest)
                .map(|(name, (refreshed_at, batch))| {
                    (
                        name,
                        Entry {
                            refreshed_at,
                            batch,
                        },
                    )
                })
                .collect(),
        };

        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension(format!(
            "tmp.{}.{}",
            std::process::id(),
            TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut out = options.open(&tmp)?;
        out.write_all(&serde_json::to_vec(&file)?)?;
        drop(out);
        std::fs::rename(&tmp, &self.path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }
}
