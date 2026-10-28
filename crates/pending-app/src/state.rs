//! What the window shows: loading, the dashboard, or an error, with the
//! last good dashboard kept while the daemon is unreachable.

use chrono::{DateTime, Local};
use pending_core::DashboardSnapshot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewState {
    Loading,
    Ready {
        snapshot: DashboardSnapshot,
        fetched_at: DateTime<Local>,
    },
    Unavailable {
        error: String,
    },
    /// The daemon failed after an earlier success; `snapshot` is from
    /// `fetched_at`.
    Stale {
        snapshot: DashboardSnapshot,
        fetched_at: DateTime<Local>,
        error: String,
    },
}

impl ViewState {
    /// The dashboard on screen, if any.
    pub fn snapshot(&self) -> Option<&DashboardSnapshot> {
        match self {
            Self::Ready { snapshot, .. } | Self::Stale { snapshot, .. } => Some(snapshot),
            Self::Loading | Self::Unavailable { .. } => None,
        }
    }

    /// The state after one load.
    pub fn next(self, result: Result<DashboardSnapshot, String>, now: DateTime<Local>) -> Self {
        match (result, self) {
            (Ok(snapshot), _) => Self::Ready {
                snapshot,
                fetched_at: now,
            },
            (
                Err(error),
                Self::Ready {
                    snapshot,
                    fetched_at,
                }
                | Self::Stale {
                    snapshot,
                    fetched_at,
                    ..
                },
            ) => Self::Stale {
                snapshot,
                fetched_at,
                error,
            },
            (Err(error), Self::Loading | Self::Unavailable { .. }) => Self::Unavailable { error },
        }
    }
}

#[cfg(test)]
mod tests {
    use pending_core::sandbox_snapshot;

    use super::*;

    /// A failure keeps the last good dashboard (stale), and only a failure
    /// without one is "unavailable".
    #[test]
    fn keeps_the_last_good_snapshot_while_the_daemon_fails() {
        let now = Local::now();
        let state = ViewState::Loading.next(Err("refused".to_owned()), now);
        assert_eq!(
            state,
            ViewState::Unavailable {
                error: "refused".to_owned()
            }
        );

        let state = state.next(Ok(sandbox_snapshot()), now);
        assert!(matches!(state, ViewState::Ready { .. }));
        let state = state.next(Err("timed out".to_owned()), now);
        let ViewState::Stale {
            snapshot,
            fetched_at,
            error,
        } = &state
        else {
            panic!("expected stale, got {state:?}");
        };
        assert_eq!(*snapshot, sandbox_snapshot());
        assert_eq!(*fetched_at, now);
        assert_eq!(error, "timed out");
        assert_eq!(state.snapshot(), Some(&sandbox_snapshot()));
    }
}
