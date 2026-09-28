//! Built-in source with fixed cards, used until real sources are configured.

use chrono::{DateTime, Utc};

use crate::config::DEFAULT_BOARD;
use crate::model::{CardSeverity, DashboardSnapshot, PendingCard};
use crate::snapshot::{SourceOutcome, SourceReport, build_snapshot};
use crate::source::{BoxFuture, PendingSource, SourceBatch, SourceError, SourceItem};

pub struct SampleSource;

impl SampleSource {
    const NAME: &str = "sample";

    fn batch(at: DateTime<Utc>) -> SourceBatch {
        let item = |column: &str, id: &str, title: &str, body: &str, severity| SourceItem {
            column: column.to_owned(),
            card: PendingCard {
                id: id.to_owned(),
                title: title.to_owned(),
                body: body.to_owned(),
                source: Self::NAME.to_owned(),
                url: None,
                severity,
                due_at: None,
                updated_at: at,
            },
        };

        SourceBatch {
            items: vec![
                item(
                    "Review",
                    "sample:review:1",
                    "Design the first real source contract",
                    "Define refresh, cache, and error semantics before binding to GitHub.",
                    CardSeverity::Info,
                ),
                item(
                    "Next",
                    "sample:next:1",
                    "Choose the first frontend interaction",
                    "Boards as tabs, one column per filter.",
                    CardSeverity::Warning,
                ),
            ],
            warnings: Vec::new(),
        }
    }
}

impl PendingSource for SampleSource {
    fn columns(&self) -> Vec<String> {
        vec!["Review".to_owned(), "Next".to_owned()]
    }

    fn refresh(&self) -> BoxFuture<'_, Result<SourceBatch, SourceError>> {
        Box::pin(std::future::ready(Ok(Self::batch(sample_time()))))
    }
}

fn sample_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-09-25T00:00:00Z")
        .expect("valid sample timestamp")
        .with_timezone(&Utc)
}

pub fn sample_snapshot() -> DashboardSnapshot {
    let at = sample_time();
    build_snapshot(
        at,
        vec![SourceReport {
            name: SampleSource::NAME.to_owned(),
            board: DEFAULT_BOARD.to_owned(),
            columns: SampleSource.columns(),
            outcome: SourceOutcome::Fresh {
                batch: SampleSource::batch(at),
                refreshed_at: at,
            },
        }],
    )
}
