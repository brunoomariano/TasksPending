use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::snapshot::{PlacedCard, assemble};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DashboardSnapshot {
    pub generated_at: DateTime<Utc>,
    pub lanes: Vec<Lane>,
    pub sources: Vec<SourceHealth>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Lane {
    pub name: String,
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Section {
    pub name: String,
    pub cards: Vec<PendingCard>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PendingCard {
    pub id: String,
    pub title: String,
    pub body: String,
    pub source: String,
    pub url: Option<String>,
    pub severity: CardSeverity,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum CardSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SourceHealth {
    pub name: String,
    pub status: SourceStatus,
    pub last_refresh_at: Option<DateTime<Utc>>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum SourceStatus {
    Ready,
    Refreshing,
    Degraded,
    Failed,
}

pub fn sample_snapshot() -> DashboardSnapshot {
    let at = DateTime::parse_from_rfc3339("2026-09-25T00:00:00Z")
        .expect("valid sample timestamp")
        .with_timezone(&Utc);
    let card = |id: &str, title: &str, body: &str, severity| PendingCard {
        id: id.to_owned(),
        title: title.to_owned(),
        body: body.to_owned(),
        source: "sample".to_owned(),
        url: None,
        severity,
        updated_at: at,
    };

    assemble(
        at,
        [
            PlacedCard {
                lane: "Work".to_owned(),
                section: "Review".to_owned(),
                card: card(
                    "sample:review:1",
                    "Design the first real source contract",
                    "Define refresh, cache, and error semantics before binding to GitHub.",
                    CardSeverity::Info,
                ),
            },
            PlacedCard {
                lane: "Personal".to_owned(),
                section: "Next".to_owned(),
                card: card(
                    "sample:personal:1",
                    "Choose the first frontend interaction",
                    "Start with cards by lane and section, then add filters.",
                    CardSeverity::Warning,
                ),
            },
        ],
        vec![SourceHealth {
            name: "sample".to_owned(),
            status: SourceStatus::Ready,
            last_refresh_at: Some(at),
            message: None,
        }],
    )
    .expect("sample card ids are unique")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_snapshot_has_lanes_and_sources() {
        let snapshot = sample_snapshot();
        assert!(!snapshot.lanes.is_empty());
        assert!(!snapshot.sources.is_empty());
    }

    /// O frontend e integrações externas leem timestamps como RFC3339 e o link
    /// do card como string ou null; o JSON do snapshot não pode mudar de forma
    /// quando os tipos internos ficam mais fortes.
    #[test]
    fn card_serializes_rfc3339_timestamp_and_optional_url() {
        let card = PendingCard {
            id: "github:pr:1".to_owned(),
            title: "Review".to_owned(),
            body: String::new(),
            source: "github".to_owned(),
            url: Some("https://github.com/o/r/pull/1".to_owned()),
            severity: CardSeverity::Info,
            updated_at: chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 9, 28, 10, 30, 0)
                .unwrap(),
        };

        let value = serde_json::to_value(&card).expect("serialize card");
        assert_eq!(value["updated_at"], "2026-09-28T10:30:00Z");
        assert_eq!(value["url"], "https://github.com/o/r/pull/1");

        let without_url = PendingCard { url: None, ..card };
        let value = serde_json::to_value(&without_url).expect("serialize card");
        assert!(value["url"].is_null());
    }

    #[test]
    fn severity_serializes_as_lowercase() {
        let value = serde_json::to_value(CardSeverity::Critical).expect("serialize severity");
        assert_eq!(value, serde_json::json!("critical"));
    }
}
