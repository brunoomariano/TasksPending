use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::model::{DashboardSnapshot, Lane, PendingCard, Section, SourceHealth};

/// A card plus where it belongs on the dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedCard {
    pub lane: String,
    pub section: String,
    pub card: PendingCard,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AssembleError {
    #[error("duplicate card id `{0}`")]
    DuplicateCardId(String),
    #[error("card `{0}` has a url that is not http(s)")]
    UnsafeCardUrl(String),
}

/// Groups placed cards into lanes and sections.
///
/// Lanes and sections keep first-seen order. Cards inside a section are sorted
/// by severity (critical first), then by most recent `updated_at`. Card ids must
/// be unique across the whole snapshot, and card urls must be http(s).
pub fn assemble(
    generated_at: DateTime<Utc>,
    cards: impl IntoIterator<Item = PlacedCard>,
    sources: Vec<SourceHealth>,
) -> Result<DashboardSnapshot, AssembleError> {
    let mut seen_ids = std::collections::HashSet::new();
    let mut lanes: Vec<Lane> = Vec::new();

    for PlacedCard {
        lane,
        section,
        card,
    } in cards
    {
        if !seen_ids.insert(card.id.clone()) {
            return Err(AssembleError::DuplicateCardId(card.id));
        }
        if card.url.as_deref().is_some_and(|url| !is_http_url(url)) {
            return Err(AssembleError::UnsafeCardUrl(card.id));
        }

        let lane = match lanes.iter().position(|l| l.name == lane) {
            Some(index) => &mut lanes[index],
            None => {
                lanes.push(Lane {
                    name: lane,
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

    for section in lanes.iter_mut().flat_map(|l| l.sections.iter_mut()) {
        section
            .cards
            .sort_by_key(|card| std::cmp::Reverse((card.severity, card.updated_at)));
    }

    Ok(DashboardSnapshot {
        generated_at,
        lanes,
        sources,
    })
}

fn is_http_url(url: &str) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}
