use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardSnapshot {
    pub generated_at: String,
    pub lanes: Vec<Lane>,
    pub sources: Vec<SourceHealth>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lane {
    pub name: String,
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub name: String,
    pub cards: Vec<PendingCard>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingCard {
    pub id: String,
    pub title: String,
    pub body: String,
    pub source: String,
    pub severity: CardSeverity,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CardSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceHealth {
    pub name: String,
    pub status: SourceStatus,
    pub last_refresh_at: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceStatus {
    Ready,
    Refreshing,
    Degraded,
    Failed,
}

pub fn sample_snapshot() -> DashboardSnapshot {
    DashboardSnapshot {
        generated_at: "2026-09-25T00:00:00Z".to_owned(),
        sources: vec![SourceHealth {
            name: "sample".to_owned(),
            status: SourceStatus::Ready,
            last_refresh_at: Some("2026-09-25T00:00:00Z".to_owned()),
            message: None,
        }],
        lanes: vec![
            Lane {
                name: "Work".to_owned(),
                sections: vec![Section {
                    name: "Review".to_owned(),
                    cards: vec![PendingCard {
                        id: "sample:review:1".to_owned(),
                        title: "Design the first real source contract".to_owned(),
                        body:
                            "Define refresh, cache, and error semantics before binding to GitHub."
                                .to_owned(),
                        source: "sample".to_owned(),
                        severity: CardSeverity::Info,
                        updated_at: "2026-09-25T00:00:00Z".to_owned(),
                    }],
                }],
            },
            Lane {
                name: "Personal".to_owned(),
                sections: vec![Section {
                    name: "Next".to_owned(),
                    cards: vec![PendingCard {
                        id: "sample:personal:1".to_owned(),
                        title: "Choose the first frontend interaction".to_owned(),
                        body: "Start with cards by lane and section, then add filters.".to_owned(),
                        source: "sample".to_owned(),
                        severity: CardSeverity::Warning,
                        updated_at: "2026-09-25T00:00:00Z".to_owned(),
                    }],
                }],
            },
        ],
    }
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

    #[test]
    fn severity_serializes_as_lowercase() {
        let value = serde_json::to_value(CardSeverity::Critical).expect("serialize severity");
        assert_eq!(value, serde_json::json!("critical"));
    }
}
