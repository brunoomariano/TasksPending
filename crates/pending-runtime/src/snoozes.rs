//! Cards the user snoozed: hidden from the boards until a chosen time or
//! until the item changes, whichever comes first. Nothing is written to the
//! providers; snoozes are kept in a small file next to the cache (see
//! [`crate::store`]), shared by the web page and the TUI.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use pending_core::{DashboardSnapshot, PendingCard, SnoozedCard};
use serde::{Deserialize, Serialize};

use crate::store::{Store, source_of, still_applies};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Snooze {
    id: String,
    source: String,
    snoozed_at: DateTime<Utc>,
    /// When the card comes back by itself; `None` waits for a change.
    until: Option<DateTime<Utc>>,
    /// The item's `updated_at` when it was snoozed; a later one means the
    /// item changed.
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SnoozeError {
    #[error("no card with that id is on the dashboard")]
    UnknownCard,
    #[error("the snooze time is in the past")]
    PastTime,
    #[error("this dashboard cannot snooze cards")]
    Unsupported,
}

pub struct Snoozes {
    store: Store<Snooze>,
}

impl Snoozes {
    /// Snoozes kept at `path` (in memory only with `None`); a missing or
    /// unreadable file starts empty.
    pub fn load(path: Option<PathBuf>) -> Self {
        Self {
            store: Store::new(path, "snoozes"),
        }
    }

    /// Hides the card `id` until `until`, or until the item changes when
    /// `None` (it also comes back early if the item changes). Only a card
    /// on `snapshot` can be snoozed. Snoozing again replaces the time.
    pub fn snooze(
        &self,
        snapshot: &DashboardSnapshot,
        id: &str,
        until: Option<DateTime<Utc>>,
    ) -> Result<(), SnoozeError> {
        let now = Utc::now();
        if until.is_some_and(|until| until <= now) {
            return Err(SnoozeError::PastTime);
        }
        let source = source_of(snapshot, id).ok_or(SnoozeError::UnknownCard)?;
        let updated_at = cards(snapshot)
            .find(|card| card.id == id)
            .map(|card| card.updated_at)
            .ok_or(SnoozeError::UnknownCard)?;
        self.store.change(|snoozes| {
            snoozes.retain(|snooze| snooze.id != id);
            snoozes.push(Snooze {
                id: id.to_owned(),
                source,
                snoozed_at: now,
                until,
                updated_at,
            });
        });
        Ok(())
    }

    /// Brings the card `id` back now; does nothing if it is not snoozed.
    pub fn wake(&self, id: &str) {
        self.store
            .change(|snoozes| snoozes.retain(|snooze| snooze.id != id));
    }

    /// Takes the snoozed cards out of the boards and lists them in
    /// `snapshot.snoozed`, first forgetting the snoozes that ended: time
    /// passed, the item changed, the card was finished elsewhere, or its
    /// source left the dashboard long ago (see [`still_applies`]).
    pub fn apply(&self, snapshot: &mut DashboardSnapshot) {
        self.apply_hiding(snapshot, &HashSet::new());
    }

    /// [`Snoozes::apply`] for a dashboard whose stacks leave cards out
    /// (`exclude`): a card in `hidden` is still listed by its source, so
    /// its snooze is not forgotten as finished.
    pub fn apply_hiding(&self, snapshot: &mut DashboardSnapshot, hidden: &HashSet<String>) {
        let now = Utc::now();
        let on_board: HashMap<String, DateTime<Utc>> = cards(snapshot)
            .map(|card| (card.id.clone(), card.updated_at))
            .collect();
        let keep = |snooze: &Snooze| {
            snooze.until.is_none_or(|until| now < until)
                && on_board
                    .get(&snooze.id)
                    .is_none_or(|updated_at| *updated_at <= snooze.updated_at)
                && still_applies(
                    snapshot,
                    hidden,
                    &snooze.source,
                    &snooze.id,
                    snooze.snoozed_at,
                    now,
                )
        };

        let mut snoozes = self.store.current();
        if !snoozes.iter().all(keep) {
            // Decided again under the lock, on what the file holds then.
            self.store.change(|snoozes| snoozes.retain(keep));
            snoozes = self.store.current();
        }

        // A card the source did not return (it is failing) stays snoozed
        // but has nothing to list.
        let mut hidden: HashMap<&str, Option<PendingCard>> = snoozes
            .iter()
            .filter(|snooze| on_board.contains_key(&snooze.id))
            .map(|snooze| (snooze.id.as_str(), None))
            .collect();
        for board in &mut snapshot.boards {
            for group in &mut board.groups {
                for column in &mut group.columns {
                    column
                        .cards
                        .retain(|card| match hidden.get_mut(card.id.as_str()) {
                            Some(taken) => {
                                taken.get_or_insert_with(|| card.clone());
                                false
                            }
                            None => true,
                        });
                }
            }
        }
        snapshot.snoozed = snoozes
            .iter()
            .filter_map(|snooze| {
                Some(SnoozedCard {
                    card: hidden.remove(snooze.id.as_str())??,
                    source: snooze.source.clone(),
                    until: snooze.until,
                })
            })
            .collect();
    }
}

fn cards(snapshot: &DashboardSnapshot) -> impl Iterator<Item = &PendingCard> {
    snapshot
        .boards
        .iter()
        .flat_map(|board| &board.groups)
        .flat_map(|group| &group.columns)
        .flat_map(|column| &column.cards)
}
