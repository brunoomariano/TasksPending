//! Scheduled refresh of pending sources.
//!
//! [`Aggregator`] refreshes every source on its own interval in the background
//! and keeps the latest [`SourceOutcome`] of each one in memory. Reading a
//! snapshot never waits for a source.

use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use pending_core::{
    DashboardSnapshot, PendingSource, SourceBatch, SourceError, SourceOutcome, SourceReport,
    build_snapshot,
};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{info, warn};

pub mod cache;
pub mod config;

use crate::cache::Cache;

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
    shared: Arc<Shared>,
    _tasks: Arc<Tasks>,
}

/// State the refresh loops and the handles share.
struct Shared {
    reports: RwLock<Vec<SourceReport>>,
    wake: Notify,
    cache: Option<Cache>,
}

impl Shared {
    fn reports(&self) -> Vec<SourceReport> {
        // Writers never panic while holding the lock, but a poisoned lock must
        // not take every snapshot request down with it.
        self.reports
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Writes every source's last good batch. Failures only log: the cache is
    /// a convenience, never a reason to stop refreshing.
    fn save_cache(&self) {
        let Some(cache) = &self.cache else {
            return;
        };
        let entries: Vec<_> = self
            .reports()
            .into_iter()
            .filter_map(|report| match report.outcome {
                SourceOutcome::Fresh {
                    batch,
                    refreshed_at,
                }
                | SourceOutcome::Stale {
                    batch,
                    refreshed_at,
                    ..
                }
                | SourceOutcome::Cached {
                    batch,
                    refreshed_at,
                } => Some((report.name, refreshed_at, batch)),
                SourceOutcome::Pending | SourceOutcome::Failed(_) => None,
            })
            .collect();
        if let Err(error) = cache.save(&entries) {
            warn!(%error, "could not write the source cache");
        }
    }
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
        Self::start_with_cache(specs, timeout, None)
    }

    /// Like [`Aggregator::start`], but sources begin with their last good
    /// batch from `cache`, and every successful refresh is written back.
    pub fn start_with_cache(
        specs: Vec<SourceSpec>,
        timeout: Duration,
        cache: Option<Cache>,
    ) -> Self {
        let mut cached = cache.as_ref().map(Cache::load).unwrap_or_default();
        let reports = specs
            .iter()
            .map(|spec| SourceReport {
                name: spec.name.clone(),
                lane: spec.lane.clone(),
                outcome: match cached.remove(&spec.name) {
                    Some((refreshed_at, batch)) => SourceOutcome::Cached {
                        batch,
                        refreshed_at,
                    },
                    None => SourceOutcome::Pending,
                },
            })
            .collect();
        let shared = Arc::new(Shared {
            reports: RwLock::new(reports),
            wake: Notify::new(),
            cache,
        });

        let tasks = specs
            .into_iter()
            .enumerate()
            .map(|(index, spec)| tokio::spawn(refresh_loop(index, spec, timeout, shared.clone())))
            .collect();

        Self {
            shared,
            _tasks: Arc::new(Tasks(tasks)),
        }
    }

    /// Wakes every source that is waiting for its next interval. A source in
    /// the middle of a refresh finishes that one and is not refreshed twice.
    pub fn refresh_now(&self) {
        self.shared.wake.notify_waiters();
    }

    pub fn snapshot(&self) -> DashboardSnapshot {
        build_snapshot(Utc::now(), self.shared.reports())
    }
}

async fn refresh_loop(index: usize, spec: SourceSpec, timeout: Duration, shared: Arc<Shared>) {
    let name = spec.name.clone();
    let mut failures: u32 = 0;
    loop {
        let started = Instant::now();
        let result = refresh_once(&spec.source, timeout).await;
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

        let succeeded = result.is_ok();
        let wait = match &result {
            Ok(_) => {
                failures = 0;
                spec.interval
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                let wait = backoff(spec.interval, failures, error.retry_at());
                warn!(source = %name, failures, next_in = ?wait, "backing off");
                wait
            }
        };
        {
            let mut reports = shared
                .reports
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            let report = &mut reports[index];
            let previous = std::mem::replace(&mut report.outcome, SourceOutcome::Pending);
            report.outcome = next_outcome(previous, result);
        }
        if succeeded {
            shared.save_cache();
        }

        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = shared.wake.notified() => {}
        }
    }
}

/// Longest backoff, as a multiple of the source's interval.
const MAX_BACKOFF_FACTOR: u32 = 16;

/// Longest wait honoured from a source's `retry_at`, so a wrong local clock
/// cannot park a source for days.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(60 * 60);

/// Wait after the `failures`-th failure in a row: the interval doubled per
/// failure, up to [`MAX_BACKOFF_FACTOR`] times, and never before `retry_at`
/// (capped at [`MAX_RETRY_WAIT`]).
fn backoff(interval: Duration, failures: u32, retry_at: Option<DateTime<Utc>>) -> Duration {
    let factor = 2u32.saturating_pow(failures).min(MAX_BACKOFF_FACTOR);
    let wait = interval.saturating_mul(factor);
    let until_retry = retry_at
        .and_then(|at| (at - Utc::now()).to_std().ok())
        .unwrap_or_default()
        .min(MAX_RETRY_WAIT);
    wait.max(until_retry)
}

/// Runs one refresh in its own task so a panicking source becomes a failed
/// refresh instead of killing the loop. The task is aborted on timeout and when
/// the loop itself is aborted.
async fn refresh_once(
    source: &Arc<dyn PendingSource>,
    timeout: Duration,
) -> Result<SourceBatch, SourceError> {
    let source = source.clone();
    let mut task = AbortOnDrop(tokio::spawn(async move { source.refresh().await }));

    match tokio::time::timeout(timeout, &mut task.0).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) if error.is_panic() => Err(SourceError::new(format!(
            "refresh panicked: {}",
            panic_message(error.into_panic())
        ))),
        Ok(Err(_)) => Err(SourceError::new("refresh was cancelled")),
        Err(_) => Err(SourceError::new(format!("timed out after {timeout:?}"))),
    }
}

struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

fn next_outcome(
    previous: SourceOutcome,
    result: Result<SourceBatch, SourceError>,
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
            }
            | SourceOutcome::Cached {
                batch,
                refreshed_at,
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
