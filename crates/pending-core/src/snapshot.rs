use std::cmp::Reverse;
use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Utc};

use crate::config::StackSort;

use crate::model::{Board, Column, DashboardSnapshot, Group, Icon, SourceHealth, SourceStatus};
use crate::source::{SourceBatch, SourceError};

/// The current state of one configured source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceReport {
    pub name: String,
    /// Board (tab) from the source's configuration.
    pub board: String,
    /// The source's columns in display order.
    pub columns: Vec<String>,
    pub icon: Option<Icon>,
    /// Card order per column name; columns not listed are newest first.
    pub sorts: BTreeMap<String, StackSort>,
    pub outcome: SourceOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceOutcome {
    /// The first refresh has not finished yet.
    Pending,
    /// The latest refresh succeeded. `refreshed_at` is when it completed.
    Fresh {
        batch: SourceBatch,
        refreshed_at: DateTime<Utc>,
    },
    /// The latest refresh failed and there is no earlier success to show.
    Failed(SourceError),
    /// The latest refresh failed; `batch` is the last success, from `refreshed_at`.
    Stale {
        batch: SourceBatch,
        refreshed_at: DateTime<Utc>,
        error: SourceError,
    },
    /// Nothing refreshed yet in this run; `batch` is the last success of a
    /// previous run, from `refreshed_at`.
    Cached {
        batch: SourceBatch,
        refreshed_at: DateTime<Utc>,
    },
}

/// Builds the dashboard from every source's report. A bad source never takes
/// the dashboard down.
///
/// - Each source becomes a group of columns in its board; boards and groups
///   keep report order, and declared columns show even when empty. A column
///   a source uses without declaring it is appended.
/// - A pending source shows as `Refreshing`; a failed one as `Failed` with its
///   error; both contribute empty columns.
/// - A stale source keeps its last known cards and shows as `Degraded`,
///   explaining the failure.
/// - A cached source shows the previous run's cards and stays `Refreshing`
///   until its first refresh in this run.
/// - A card may appear in several columns. Within one column, a repeated id
///   or a non-http(s) url drops the card and degrades its source, naming it.
/// - Source warnings also make the source `Degraded`.
/// - Cards inside a column are sorted by severity (critical first), then
///   cards with a `due_at`, soonest first, then by most recent `updated_at`;
///   a column sorted [`StackSort::Oldest`] puts the least recent first
///   instead.
pub fn build_snapshot(
    generated_at: DateTime<Utc>,
    reports: Vec<SourceReport>,
) -> DashboardSnapshot {
    let mut boards: Vec<Board> = Vec::new();
    let mut sources = Vec::with_capacity(reports.len());

    for report in reports {
        let mut group = Group {
            source: report.name.clone(),
            icon: report.icon.clone(),
            columns: report
                .columns
                .iter()
                .map(|name| Column {
                    name: name.clone(),
                    cards: Vec::new(),
                })
                .collect(),
        };

        let mut refreshing = false;
        let outcome = match report.outcome {
            SourceOutcome::Pending => {
                sources.push(SourceHealth {
                    name: report.name.clone(),
                    status: SourceStatus::Refreshing,
                    last_refresh_at: None,
                    message: None,
                });
                None
            }
            SourceOutcome::Failed(error) => {
                sources.push(SourceHealth {
                    name: report.name.clone(),
                    status: SourceStatus::Failed,
                    last_refresh_at: None,
                    message: Some(error.to_string()),
                });
                None
            }
            SourceOutcome::Fresh {
                batch,
                refreshed_at,
            } => Some((batch, refreshed_at, Vec::new())),
            SourceOutcome::Stale {
                batch,
                refreshed_at,
                error,
            } => Some((
                batch,
                refreshed_at,
                vec![format!("refresh failed: {error}; showing stale data")],
            )),
            SourceOutcome::Cached {
                batch,
                refreshed_at,
            } => {
                refreshing = true;
                Some((
                    batch,
                    refreshed_at,
                    vec!["showing data from the previous run".to_owned()],
                ))
            }
        };

        if let Some((batch, refreshed_at, mut problems)) = outcome {
            problems.extend(batch.warnings);
            let mut seen: HashSet<(String, String)> = HashSet::new();
            for item in batch.items {
                if item
                    .card
                    .url
                    .as_deref()
                    .is_some_and(|url| !is_http_url(url))
                {
                    problems.push(format!(
                        "dropped card `{}`: url is not http(s)",
                        item.card.id
                    ));
                    continue;
                }
                if !seen.insert((item.column.clone(), item.card.id.clone())) {
                    problems.push(format!(
                        "dropped duplicate card id `{}` in column `{}`",
                        item.card.id, item.column
                    ));
                    continue;
                }
                let column = match group.columns.iter().position(|c| c.name == item.column) {
                    Some(index) => &mut group.columns[index],
                    None => {
                        group.columns.push(Column {
                            name: item.column,
                            cards: Vec::new(),
                        });
                        group.columns.last_mut().expect("just pushed")
                    }
                };
                column.cards.push(item.card);
            }

            sources.push(SourceHealth {
                name: report.name,
                status: if refreshing {
                    SourceStatus::Refreshing
                } else if problems.is_empty() {
                    SourceStatus::Ready
                } else {
                    SourceStatus::Degraded
                },
                last_refresh_at: Some(refreshed_at),
                message: (!problems.is_empty()).then(|| problems.join("; ")),
            });
        }

        for column in &mut group.columns {
            let oldest_first = report.sorts.get(&column.name) == Some(&StackSort::Oldest);
            column.cards.sort_by(|a, b| {
                let recency = if oldest_first {
                    a.updated_at.cmp(&b.updated_at)
                } else {
                    b.updated_at.cmp(&a.updated_at)
                };
                Reverse(a.severity)
                    .cmp(&Reverse(b.severity))
                    .then_with(|| a.due_at.is_none().cmp(&b.due_at.is_none()))
                    .then_with(|| a.due_at.cmp(&b.due_at))
                    .then(recency)
            });
        }

        match boards.iter_mut().find(|b| b.name == report.board) {
            Some(board) => board.groups.push(group),
            None => boards.push(Board {
                name: report.board,
                groups: vec![group],
            }),
        }
    }

    DashboardSnapshot {
        generated_at,
        boards,
        sources,
        config_error: None,
    }
}

pub(crate) fn is_http_url(url: &str) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}
