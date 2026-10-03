//! Cards the user marked as in progress. Marks are kept in a small file next
//! to the cache (see [`crate::store`]), so the web page and the TUI share
//! them and they survive restarts.

use std::collections::HashSet;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use pending_core::DashboardSnapshot;
use serde::{Deserialize, Serialize};

use crate::store::{Store, source_of, still_applies};

/// A marked card, with the source it came from and when it was marked: the
/// source's health and last refresh tell whether a missing card is finished
/// or just not loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Mark {
    id: String,
    source: String,
    marked_at: DateTime<Utc>,
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
    store: Store<Mark>,
}

impl Marks {
    /// Marks kept at `path` (in memory only with `None`); a missing or
    /// unreadable file starts empty.
    pub fn load(path: Option<PathBuf>) -> Self {
        Self {
            store: Store::new(path, "marks"),
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
        self.store.change(|marks| match source {
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
    /// that no longer apply (see [`still_applies`]): those of cards
    /// finished elsewhere, and old ones of sources no longer on the
    /// dashboard.
    pub fn apply(&self, snapshot: &mut DashboardSnapshot) {
        self.apply_hiding(snapshot, &HashSet::new());
    }

    /// [`Marks::apply`] for a dashboard whose stacks leave cards out
    /// (`exclude`). A card in `hidden` is absent from `snapshot` but its
    /// source still lists it, so it is not finished: its mark is kept, and
    /// stays in `snapshot.marked`, until the card is gone from the source
    /// as well.
    pub fn apply_hiding(&self, snapshot: &mut DashboardSnapshot, hidden: &HashSet<String>) {
        let now = Utc::now();
        let keep = |mark: &Mark| {
            still_applies(
                snapshot,
                hidden,
                &mark.source,
                &mark.id,
                mark.marked_at,
                now,
            )
        };

        let mut marks = self.store.current();
        if !marks.iter().all(keep) {
            // Decided again under the lock, on what the file holds then.
            self.store.change(|marks| marks.retain(keep));
            marks = self.store.current();
        }
        snapshot.marked = marks.into_iter().map(|mark| mark.id).collect();
    }
}
