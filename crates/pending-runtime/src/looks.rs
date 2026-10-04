//! When the user last looked at the dashboard, to flag what changed since.
//!
//! "Looking" is any activity on the web page or in the TUI. Activity after
//! a while away starts a new sitting: cards updated after the previous
//! sitting ended are flagged, and stay flagged for the whole sitting. The
//! times are kept in a small file next to the cache (see [`crate::store`]),
//! shared by every browser and the TUI.
//!
//! A change is on screen only once its source was refreshed, so each look
//! also records the refresh every source was showing. A card changed just
//! before the user left, but fetched after, was never seen: it counts from
//! the refresh that was on screen, not from the moment the user left.

use std::collections::{BTreeMap, HashSet};
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
    /// Per source, the refresh its cards came from at `last`.
    #[serde(default)]
    shown: Refreshes,
    /// Per source, the refresh its cards came from when the previous
    /// sitting ended: its cards updated after that were never on screen.
    #[serde(default)]
    since_shown: Refreshes,
}

/// When each source's cards on screen were fetched, by source name.
type Refreshes = BTreeMap<String, DateTime<Utc>>;

fn refreshes(snapshot: &DashboardSnapshot) -> Refreshes {
    snapshot
        .sources
        .iter()
        .filter_map(|source| Some((source.name.clone(), source.last_refresh_at?)))
        .collect()
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

    /// Records that the user is looking now at `snapshot`.
    pub fn look(&self, snapshot: &DashboardSnapshot) {
        self.look_at_board(Utc::now(), snapshot);
    }

    /// Records activity at `now`, without saying what was on screen.
    pub fn look_at(&self, now: DateTime<Utc>) {
        self.record(now, &Refreshes::new());
    }

    /// Records activity at `now` on a dashboard showing `snapshot`.
    pub fn look_at_board(&self, now: DateTime<Utc>, snapshot: &DashboardSnapshot) {
        self.record(now, &refreshes(snapshot));
    }

    /// After [`AWAY_MINUTES`] without activity, a new sitting starts and
    /// flags count from the previous activity. A clock that went back is
    /// not time away, and never moves the latest activity back.
    fn record(&self, now: DateTime<Utc>, shown: &Refreshes) {
        let next = |look: Option<&Look>| match look {
            // The first look ever: nothing to compare with.
            None => Look {
                since: now,
                last: now,
                shown: shown.clone(),
                since_shown: shown.clone(),
            },
            Some(look) if now - look.last >= Duration::minutes(AWAY_MINUTES) => Look {
                since: look.last,
                last: now,
                shown: shown.clone(),
                since_shown: look.shown.clone(),
            },
            Some(look) => Look {
                since: look.since,
                last: now.max(look.last),
                shown: shown.clone(),
                since_shown: look.since_shown.clone(),
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
    /// the previous sitting ended (or, when earlier, since the refresh
    /// their source was showing then); none before the first look.
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
            .flat_map(|group| {
                let since = look
                    .since_shown
                    .get(&group.source)
                    .map_or(look.since, |shown| look.since.min(*shown));
                group
                    .columns
                    .iter()
                    .flat_map(|column| &column.cards)
                    .filter(move |card| card.updated_at > since)
            })
            .filter(|card| seen.insert(card.id.as_str()))
            .map(|card| card.id.clone())
            .collect();
    }
}
