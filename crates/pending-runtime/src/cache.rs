//! The last good batch of every source, kept across restarts.

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use pending_core::SourceBatch;
use serde::{Deserialize, Serialize};
use tracing::warn;

const VERSION: u32 = 1;

/// A JSON file holding, per source name, its last successful batch.
#[derive(Debug, Clone)]
pub struct Cache {
    path: PathBuf,
}

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
        Self { path }
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

    /// Replaces the file with these entries. Written to a temporary file and
    /// renamed, so a reader never sees a partial write.
    pub fn save(&self, entries: &[(String, DateTime<Utc>, SourceBatch)]) -> std::io::Result<()> {
        let file = File {
            version: VERSION,
            sources: entries
                .iter()
                .map(|(name, refreshed_at, batch)| {
                    (
                        name.clone(),
                        Entry {
                            refreshed_at: *refreshed_at,
                            batch: batch.clone(),
                        },
                    )
                })
                .collect(),
        };
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self
            .path
            .with_extension(format!("tmp.{}", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec(&file)?)?;
        std::fs::rename(&tmp, &self.path)
    }
}
