//! Scheduled refresh of pending sources.
//!
//! [`Aggregator`] refreshes every source on its own interval in the background
//! and keeps the latest [`SourceOutcome`] of each one in memory. Reading a
//! snapshot never waits for a source.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use chrono::Utc;
use pending_core::{
    DashboardSnapshot, PendingSource, SourceError, SourceOutcome, SourceReport, build_snapshot,
};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{info, warn};

pub mod config;

/// A source plus how the dashboard names, schedules and places it.
pub struct SourceSpec {
    /// Name from the source's configuration, shown in source health.
    pub name: String,
    pub source: Arc<dyn PendingSource>,
    /// Lane from the source's configuration.
    pub lane: String,
    /// Wait between the end of one refresh and the start of the next.
    pub interval: Duration,
}

impl std::fmt::Debug for SourceSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceSpec")
            .field("name", &self.name)
            .field("lane", &self.lane)
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

/// Cheap-to-clone handle to the running refresh loops. The loops stop when the
/// last handle is dropped.
#[derive(Clone)]
pub struct Aggregator {
    reports: Arc<RwLock<Vec<SourceReport>>>,
    _tasks: Arc<Tasks>,
}

struct Tasks(Vec<JoinHandle<()>>);

impl Drop for Tasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

impl Aggregator {
    /// Starts one refresh loop per source. Must be called inside a Tokio
    /// runtime. A refresh slower than `timeout` counts as a failure.
    pub fn start(specs: Vec<SourceSpec>, timeout: Duration) -> Self {
        let reports = Arc::new(RwLock::new(
            specs
                .iter()
                .map(|spec| SourceReport {
                    name: spec.name.clone(),
                    lane: spec.lane.clone(),
                    outcome: SourceOutcome::Pending,
                })
                .collect(),
        ));

        let tasks = specs
            .into_iter()
            .enumerate()
            .map(|(index, spec)| tokio::spawn(refresh_loop(index, spec, timeout, reports.clone())))
            .collect();

        Self {
            reports,
            _tasks: Arc::new(Tasks(tasks)),
        }
    }

    pub fn snapshot(&self) -> DashboardSnapshot {
        let reports = self.reports.read().expect("reports lock poisoned").clone();
        build_snapshot(Utc::now(), reports)
    }
}

async fn refresh_loop(
    index: usize,
    spec: SourceSpec,
    timeout: Duration,
    reports: Arc<RwLock<Vec<SourceReport>>>,
) {
    let name = spec.name.clone();
    loop {
        let started = Instant::now();
        let result = match tokio::time::timeout(timeout, spec.source.refresh()).await {
            Ok(result) => result,
            Err(_) => Err(SourceError::new(format!(
                "timed out after {}s",
                timeout.as_secs()
            ))),
        };
        let elapsed_ms = started.elapsed().as_millis();

        match &result {
            Ok(batch) => info!(
                source = %name,
                elapsed_ms,
                items = batch.items.len(),
                warnings = batch.warnings.len(),
                "source refreshed"
            ),
            Err(error) => warn!(source = %name, elapsed_ms, %error, "source refresh failed"),
        }

        {
            let mut reports = reports.write().expect("reports lock poisoned");
            let report = &mut reports[index];
            let previous = std::mem::replace(&mut report.outcome, SourceOutcome::Pending);
            report.outcome = next_outcome(previous, result);
        }

        tokio::time::sleep(spec.interval).await;
    }
}

fn next_outcome(
    previous: SourceOutcome,
    result: Result<pending_core::SourceBatch, SourceError>,
) -> SourceOutcome {
    match (result, previous) {
        (Ok(batch), _) => SourceOutcome::Fresh {
            batch,
            refreshed_at: Utc::now(),
        },
        (
            Err(error),
            SourceOutcome::Fresh {
                batch,
                refreshed_at,
            }
            | SourceOutcome::Stale {
                batch,
                refreshed_at,
                ..
            },
        ) => SourceOutcome::Stale {
            batch,
            refreshed_at,
            error,
        },
        (Err(error), SourceOutcome::Pending | SourceOutcome::Failed(_)) => {
            SourceOutcome::Failed(error)
        }
    }
}
