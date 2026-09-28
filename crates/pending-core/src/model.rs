use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DashboardSnapshot {
    pub generated_at: DateTime<Utc>,
    /// Areas of work (Work, Personal, …), shown as tabs.
    pub boards: Vec<Board>,
    pub sources: Vec<SourceHealth>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Board {
    pub name: String,
    /// One group of columns per source, in configuration order.
    pub groups: Vec<Group>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Group {
    /// Configured source name; matches a `SourceHealth::name`.
    pub source: String,
    pub columns: Vec<Column>,
}

/// A kanban column: the cards matching one of the source's filters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Column {
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
    /// When the card is due or starts (deadline, event). Cards with one sort
    /// soonest first.
    #[serde(default)]
    pub due_at: Option<DateTime<Utc>>,
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

#[cfg(test)]
mod tests {
    use super::*;

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
            due_at: None,
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
