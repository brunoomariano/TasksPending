use std::collections::HashSet;

use chrono::{DateTime, Utc};

use crate::model::{DashboardSnapshot, Lane, PendingCard, Section, SourceHealth, SourceStatus};
use crate::source::{SourceBatch, SourceError};

/// The result of refreshing one configured source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceReport {
    pub name: String,
    /// Lane from the source's configuration.
    pub lane: String,
    pub refreshed_at: DateTime<Utc>,
    pub outcome: Result<SourceBatch, SourceError>,
}

/// Builds the dashboard from every source's report. A bad source never takes
/// the dashboard down.
///
/// - A failed source shows as `Failed` with its error, and contributes no cards.
/// - Cards with a non-http(s) url, or an id already used by an earlier card,
///   are dropped; their source shows as `Degraded` and names them.
/// - Source warnings also make the source `Degraded`.
/// - Lanes and sections keep first-seen order. Cards inside a section are
///   sorted by severity (critical first), then by most recent `updated_at`.
pub fn build_snapshot(
    generated_at: DateTime<Utc>,
    reports: Vec<SourceReport>,
) -> DashboardSnapshot {
    let mut seen_ids = HashSet::new();
    let mut lanes: Vec<Lane> = Vec::new();
    let mut sources = Vec::with_capacity(reports.len());

    for report in reports {
        let batch = match report.outcome {
            Ok(batch) => batch,
            Err(error) => {
                sources.push(SourceHealth {
                    name: report.name,
                    status: SourceStatus::Failed,
                    last_refresh_at: None,
                    message: Some(error.to_string()),
                });
                continue;
            }
        };

        let mut problems = batch.warnings;
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
            if !seen_ids.insert(item.card.id.clone()) {
                problems.push(format!("dropped duplicate card id `{}`", item.card.id));
                continue;
            }
            place(&mut lanes, &report.lane, item.section, item.card);
        }

        sources.push(SourceHealth {
            name: report.name,
            status: if problems.is_empty() {
                SourceStatus::Ready
            } else {
                SourceStatus::Degraded
            },
            last_refresh_at: Some(report.refreshed_at),
            message: (!problems.is_empty()).then(|| problems.join("; ")),
        });
    }

    for section in lanes.iter_mut().flat_map(|l| l.sections.iter_mut()) {
        section
            .cards
            .sort_by_key(|card| std::cmp::Reverse((card.severity, card.updated_at)));
    }

    DashboardSnapshot {
        generated_at,
        lanes,
        sources,
    }
}

fn place(lanes: &mut Vec<Lane>, lane: &str, section: String, card: PendingCard) {
    let lane = match lanes.iter().position(|l| l.name == lane) {
        Some(index) => &mut lanes[index],
        None => {
            lanes.push(Lane {
                name: lane.to_owned(),
                sections: Vec::new(),
            });
            lanes.last_mut().expect("just pushed")
        }
    };
    let section = match lane.sections.iter().position(|s| s.name == section) {
        Some(index) => &mut lane.sections[index],
        None => {
            lane.sections.push(Section {
                name: section,
                cards: Vec::new(),
            });
            lane.sections.last_mut().expect("just pushed")
        }
    };
    section.cards.push(card);
}

fn is_http_url(url: &str) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}
