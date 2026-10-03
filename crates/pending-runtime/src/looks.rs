//! When the user last looked at the dashboard, to flag what changed since.
//!
//! "Looking" is any activity on the web page or in the TUI. Activity after
//! a while away starts a new sitting: cards updated after the previous
//! sitting ended are flagged, and stay flagged for the whole sitting. The
//! times are kept in a small file next to the cache (see [`crate::store`]),
//! shared by every browser and the TUI.

use std::collections::HashSet;
use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use pending_core::DashboardSnapshot;
use serde::{Deserialize, Serialize};

use crate::store::Store;

/// Minutes without activity after which the user counts as away.
const AWAY_MINUTES: i64 = 10;
/// Activity is recorded at most this often, to spare the disk.
const RECORD_EVERY_SECONDS: i64 = 30;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Look {
    /// Cards updated after this are flagged: the end of the previous
    /// sitting.
    since: DateTime<Utc>,
    /// The latest activity.
    last: DateTime<Utc>,
}

pub struct Looks {
    /// At most one entry.
    store: Store<Look>,
}

impl Looks {
    /// Times kept at `path` (in memory only with `None`).
    pub fn load(path: Option<PathBuf>) -> Self {
        Self {
            store: Store::new(path, "looks"),
        }
    }

    /// Records that the user is looking now.
    pub fn look(&self) {
        self.look_at(Utc::now());
    }

    /// Records activity at `now`. After [`AWAY_MINUTES`] without any, a new
    /// sitting starts and flags count from the previous activity.
    pub fn look_at(&self, now: DateTime<Utc>) {
        let next = |look: Option<&Look>| match look {
            // The first look ever: nothing to compare with.
            None => Look {
                since: now,
                last: now,
            },
            Some(look) if now - look.last >= Duration::minutes(AWAY_MINUTES) => Look {
                since: look.last,
                last: now,
            },
            Some(look) => Look {
                since: look.since,
                last: now.max(look.last),
            },
        };
        let current = self.store.current();
        let current = current.first();
        let same_sitting = current.is_some_and(|look| next(Some(look)).since == look.since);
        let recorded_lately =
            current.is_some_and(|look| now - look.last < Duration::seconds(RECORD_EVERY_SECONDS));
        if same_sitting && recorded_lately {
            return;
        }
        // Decided again under the lock, on what the file holds then.
        // A failed write is logged; the next look records it again.
        let _ = self.store.change(|looks| {
            let look = next(looks.first());
            looks.clear();
            looks.push(look);
        });
    }

    /// Lists in `snapshot.changed` the cards on the boards updated since
    /// the previous sitting ended; none before the first look.
    pub fn apply(&self, snapshot: &mut DashboardSnapshot) {
        let Some(look) = self.store.current().into_iter().next() else {
            snapshot.changed = Vec::new();
            return;
        };
        let mut seen = HashSet::new();
        snapshot.changed = snapshot
            .boards
            .iter()
            .flat_map(|board| &board.groups)
            .flat_map(|group| &group.columns)
            .flat_map(|column| &column.cards)
            .filter(|card| card.updated_at > look.since && seen.insert(card.id.as_str()))
            .map(|card| card.id.clone())
            .collect();
    }
}
