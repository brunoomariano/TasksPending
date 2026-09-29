//! The aggregator polls each source at its own pace and keeps the last state.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use pending_core::{
    BoxFuture, CardSeverity, DashboardSnapshot, PendingCard, PendingSource, SourceBatch,
    SourceError, SourceHealth, SourceItem, SourceStatus,
};
use pending_runtime::{Aggregator, SourceSpec};

const TIMEOUT: Duration = Duration::from_secs(30);

/// Test source: replays the script in order and repeats the last step.
struct Scripted {
    name: &'static str,
    delay: Duration,
    calls: Arc<AtomicUsize>,
    script: Mutex<VecDeque<Result<SourceBatch, SourceError>>>,
}

impl Scripted {
    fn new(
        name: &'static str,
        script: Vec<Result<SourceBatch, SourceError>>,
    ) -> (Arc<Self>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let source = Arc::new(Self {
            name,
            delay: Duration::ZERO,
            calls: calls.clone(),
            script: Mutex::new(script.into()),
        });
        (source, calls)
    }

    fn slow(name: &'static str, delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            name,
            delay,
            calls: Arc::new(AtomicUsize::new(0)),
            script: Mutex::new(vec![Ok(batch("slow-card"))].into()),
        })
    }
}

impl PendingSource for Scripted {
    fn columns(&self) -> Vec<String> {
        vec!["Review".to_owned()]
    }

    fn refresh(&self) -> BoxFuture<'_, Result<SourceBatch, SourceError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut script = self.script.lock().unwrap();
        let next = if script.len() > 1 {
            script.pop_front().unwrap()
        } else {
            script.front().cloned().unwrap()
        };
        let delay = self.delay;
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            next
        })
    }
}

fn batch(id: &str) -> SourceBatch {
    SourceBatch {
        items: vec![SourceItem {
            column: "Review".to_owned(),
            card: PendingCard {
                id: id.to_owned(),
                title: id.to_owned(),
                body: String::new(),
                source: "test".to_owned(),
                url: None,
                severity: CardSeverity::Info,
                due_at: None,
                updated_at: Utc::now(),
            },
        }],
        warnings: Vec::new(),
    }
}

fn spec(source: Arc<Scripted>, interval_secs: u64) -> SourceSpec {
    named_spec(source.name, source, interval_secs)
}

fn named_spec(name: &str, source: Arc<dyn PendingSource>, interval_secs: u64) -> SourceSpec {
    SourceSpec {
        name: name.to_owned(),
        source,
        board: "Work".to_owned(),
        interval: Duration::from_secs(interval_secs),
        timeout: None,
        icon: None,
        sorts: Default::default(),
    }
}

fn health<'a>(snapshot: &'a DashboardSnapshot, name: &str) -> &'a SourceHealth {
    snapshot
        .sources
        .iter()
        .find(|s| s.name == name)
        .expect("source health present")
}

fn card_ids(snapshot: &DashboardSnapshot) -> Vec<String> {
    snapshot
        .boards
        .iter()
        .flat_map(|b| &b.groups)
        .flat_map(|g| &g.columns)
        .flat_map(|c| &c.cards)
        .map(|c| c.id.clone())
        .collect()
}

async fn advance(secs: u64) {
    tokio::time::sleep(Duration::from_secs(secs)).await;
}

/// While the first poll hasn't returned, the source shows as refreshing;
/// when it returns, the cards appear and the source is ready.
#[tokio::test(start_paused = true)]
async fn source_is_refreshing_until_its_first_result_arrives() {
    let aggregator = Aggregator::start(
        vec![spec(Scripted::slow("github", Duration::from_secs(10)), 60)],
        TIMEOUT,
    );

    advance(1).await;
    let snapshot = aggregator.snapshot();
    assert_eq!(health(&snapshot, "github").status, SourceStatus::Refreshing);
    assert!(card_ids(&snapshot).is_empty());

    advance(10).await;
    let snapshot = aggregator.snapshot();
    assert_eq!(health(&snapshot, "github").status, SourceStatus::Ready);
    assert_eq!(card_ids(&snapshot), vec!["slow-card"]);
}

/// Each source has its own interval: a fast source doesn't wait for the slow
/// one, and the slow one isn't polled at the fast one's pace.
#[tokio::test(start_paused = true)]
async fn each_source_refreshes_on_its_own_interval() {
    let (fast, fast_calls) = Scripted::new("fast", vec![Ok(batch("f"))]);
    let (slow, slow_calls) = Scripted::new("slow", vec![Ok(batch("s"))]);
    let _aggregator = Aggregator::start(vec![spec(fast, 10), spec(slow, 60)], TIMEOUT);

    advance(65).await;

    assert_eq!(fast_calls.load(Ordering::SeqCst), 7, "t=0,10,..,60");
    assert_eq!(slow_calls.load(Ordering::SeqCst), 2, "t=0,60");
}

/// If a refresh fails after a success, the last cards stay on screen and the
/// source is degraded, explaining the failure.
#[tokio::test(start_paused = true)]
async fn failure_after_success_keeps_the_last_cards() {
    let (github, _) = Scripted::new(
        "github",
        vec![
            Ok(batch("a")),
            Err(SourceError::new("503 from api.github.com")),
        ],
    );
    let aggregator = Aggregator::start(vec![spec(github, 10)], TIMEOUT);

    advance(15).await;

    let snapshot = aggregator.snapshot();
    assert_eq!(card_ids(&snapshot), vec!["a"]);
    let github = health(&snapshot, "github");
    assert_eq!(github.status, SourceStatus::Degraded);
    assert!(
        github.message.as_deref().unwrap_or("").contains("503"),
        "{github:?}"
    );
}

/// With no previous success, the failure shows as failed, with the reason.
#[tokio::test(start_paused = true)]
async fn failure_without_previous_success_is_failed() {
    let (github, _) = Scripted::new("github", vec![Err(SourceError::new("401 bad credentials"))]);
    let aggregator = Aggregator::start(vec![spec(github, 10)], TIMEOUT);

    advance(1).await;

    let snapshot = aggregator.snapshot();
    let github = health(&snapshot, "github");
    assert_eq!(github.status, SourceStatus::Failed);
    assert_eq!(github.message.as_deref(), Some("401 bad credentials"));
}

/// A hung source doesn't hold the dashboard forever: after the timeout the
/// refresh counts as a failure.
#[tokio::test(start_paused = true)]
async fn refresh_that_exceeds_the_timeout_fails() {
    let aggregator = Aggregator::start(
        vec![spec(Scripted::slow("github", Duration::from_secs(600)), 60)],
        Duration::from_secs(5),
    );

    advance(6).await;

    let snapshot = aggregator.snapshot();
    let github = health(&snapshot, "github");
    assert_eq!(github.status, SourceStatus::Failed);
    assert!(
        github
            .message
            .as_deref()
            .unwrap_or("")
            .contains("timed out"),
        "{github:?}"
    );
}

/// A slow source can get more time than the global timeout without raising
/// it for every other source.
#[tokio::test(start_paused = true)]
async fn a_source_timeout_overrides_the_global_one() {
    let mut patient = spec(Scripted::slow("plane", Duration::from_secs(30)), 600);
    patient.timeout = Some(Duration::from_secs(60));
    let aggregator = Aggregator::start(
        vec![
            patient,
            spec(Scripted::slow("github", Duration::from_secs(30)), 600),
        ],
        Duration::from_secs(5),
    );

    advance(31).await;

    let snapshot = aggregator.snapshot();
    assert_eq!(health(&snapshot, "plane").status, SourceStatus::Ready);
    assert_eq!(health(&snapshot, "github").status, SourceStatus::Failed);
}

/// Once the aggregator's last handle is dropped, the sources stop being
/// polled.
#[tokio::test(start_paused = true)]
async fn dropping_the_aggregator_stops_refreshing() {
    let (github, calls) = Scripted::new("github", vec![Ok(batch("a"))]);
    let aggregator = Aggregator::start(vec![spec(github, 10)], TIMEOUT);
    let handle = aggregator.clone();

    advance(1).await;
    drop(aggregator);
    advance(20).await;
    assert_eq!(calls.load(Ordering::SeqCst), 3, "clone keeps it alive");

    drop(handle);
    advance(1).await;
    let after_drop = calls.load(Ordering::SeqCst);
    advance(60).await;
    assert_eq!(calls.load(Ordering::SeqCst), after_drop);
}

/// Source that panics on the first polls and then responds.
struct Panicky {
    calls: Arc<AtomicUsize>,
    panics: usize,
}

impl PendingSource for Panicky {
    fn columns(&self) -> Vec<String> {
        vec!["Review".to_owned()]
    }

    fn refresh(&self) -> BoxFuture<'_, Result<SourceBatch, SourceError>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call < self.panics {
            panic!("bug in source");
        }
        Box::pin(async { Ok(batch("recovered")) })
    }
}

/// A bug that makes the source panic can't freeze its status: the refresh
/// counts as a failure with the reason, and the source is still polled at
/// the next interval.
#[tokio::test(start_paused = true)]
async fn a_panicking_source_fails_and_keeps_being_refreshed() {
    let calls = Arc::new(AtomicUsize::new(0));
    let source = Arc::new(Panicky {
        calls: calls.clone(),
        panics: 1,
    });
    let aggregator = Aggregator::start(vec![named_spec("panicky", source, 10)], TIMEOUT);

    advance(1).await;
    let snapshot = aggregator.snapshot();
    let panicky = health(&snapshot, "panicky");
    assert_eq!(panicky.status, SourceStatus::Failed);
    assert!(
        panicky
            .message
            .as_deref()
            .unwrap_or("")
            .contains("bug in source"),
        "{panicky:?}"
    );

    // A failure doubles the wait: the next poll happens at t=20.
    advance(20).await;
    let snapshot = aggregator.snapshot();
    assert_eq!(health(&snapshot, "panicky").status, SourceStatus::Ready);
    assert_eq!(card_ids(&snapshot), vec!["recovered"]);
}

/// Consecutive failures keep showing the time of the last success, and the
/// first success after them makes the source ready again.
#[tokio::test(start_paused = true)]
async fn repeated_failures_keep_the_last_success_time_until_recovery() {
    let (github, _) = Scripted::new(
        "github",
        vec![
            Ok(batch("old")),
            Err(SourceError::new("503")),
            Err(SourceError::new("503")),
            Ok(batch("new")),
        ],
    );
    let aggregator = Aggregator::start(vec![spec(github, 10)], TIMEOUT);

    advance(1).await;
    let success_at = health(&aggregator.snapshot(), "github").last_refresh_at;
    assert!(success_at.is_some());

    advance(20).await;
    let snapshot = aggregator.snapshot();
    let github = health(&snapshot, "github");
    assert_eq!(github.status, SourceStatus::Degraded);
    assert_eq!(github.last_refresh_at, success_at);
    assert_eq!(card_ids(&snapshot), vec!["old"]);

    // Failures at t=10 and t=30 (backoff of 20s and 40s); success at t=70.
    advance(50).await;
    let snapshot = aggregator.snapshot();
    assert_eq!(health(&snapshot, "github").status, SourceStatus::Ready);
    assert_eq!(card_ids(&snapshot), vec!["new"]);
}

/// The timeout message shows the real duration, even below one second.
#[tokio::test(start_paused = true)]
async fn timeout_message_shows_sub_second_durations() {
    let aggregator = Aggregator::start(
        vec![spec(Scripted::slow("github", Duration::from_secs(600)), 60)],
        Duration::from_millis(500),
    );

    advance(1).await;

    let snapshot = aggregator.snapshot();
    let message = health(&snapshot, "github")
        .message
        .clone()
        .unwrap_or_default();
    assert!(message.contains("500ms"), "{message}");
}

/// Asking to refresh now polls every source without waiting for the
/// interval, and the next refresh counts from there.
#[tokio::test(start_paused = true)]
async fn refresh_now_refreshes_every_source_immediately() {
    let (a, a_calls) = Scripted::new("a", vec![Ok(batch("a"))]);
    let (b, b_calls) = Scripted::new("b", vec![Ok(batch("b"))]);
    let aggregator = Aggregator::start(vec![spec(a, 60), spec(b, 300)], TIMEOUT);

    advance(5).await;
    assert_eq!(a_calls.load(Ordering::SeqCst), 1);

    aggregator.refresh_now();
    tokio::time::sleep(Duration::from_millis(1)).await;

    assert_eq!(a_calls.load(Ordering::SeqCst), 2);
    assert_eq!(b_calls.load(Ordering::SeqCst), 2);

    advance(59).await;
    assert_eq!(
        a_calls.load(Ordering::SeqCst),
        2,
        "interval restarts after refresh_now"
    );
    advance(2).await;
    assert_eq!(a_calls.load(Ordering::SeqCst), 3);
}

/// A source that keeps failing is polled less and less often (double the
/// interval per failure), so as not to hammer an API that is down; the first
/// success returns to the normal interval.
#[tokio::test(start_paused = true)]
async fn failing_sources_back_off_and_recover() {
    let (github, calls) = Scripted::new(
        "github",
        vec![
            Err(SourceError::new("503")),
            Err(SourceError::new("503")),
            Ok(batch("a")),
        ],
    );
    let _aggregator = Aggregator::start(vec![spec(github, 10)], TIMEOUT);

    // Failures at t=0 and t=20 (2x wait); success at t=60 (4x wait).
    advance(1).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    advance(20).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    advance(30).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2, "second failure waits 40s");
    advance(10).await;
    assert_eq!(calls.load(Ordering::SeqCst), 3);

    // After the success, back to the 10s interval.
    advance(10).await;
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

/// When the API says until when it is rate limiting, the source is only
/// polled again after that time.
#[tokio::test(start_paused = true)]
async fn rate_limited_sources_wait_until_the_reset() {
    let retry_at = Utc::now() + chrono::Duration::seconds(300);
    let (github, calls) = Scripted::new(
        "github",
        vec![
            Err(SourceError::new("rate limited").with_retry_at(retry_at)),
            Ok(batch("a")),
        ],
    );
    let _aggregator = Aggregator::start(vec![spec(github, 10)], TIMEOUT);

    advance(290).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    advance(15).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

/// A manual refresh doesn't bypass a rate-limited API's release time: that
/// source is only polled after it; sources without a limit refresh right
/// away.
#[tokio::test(start_paused = true)]
async fn refresh_now_respects_rate_limits() {
    let retry_at = Utc::now() + chrono::Duration::seconds(300);
    let (limited, limited_calls) = Scripted::new(
        "limited",
        vec![
            Err(SourceError::new("rate limited").with_retry_at(retry_at)),
            Ok(batch("a")),
        ],
    );
    let (free, free_calls) = Scripted::new("free", vec![Ok(batch("b"))]);
    let aggregator = Aggregator::start(vec![spec(limited, 10), spec(free, 60)], TIMEOUT);

    advance(5).await;
    aggregator.refresh_now();
    tokio::time::sleep(Duration::from_millis(1)).await;

    assert_eq!(free_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        limited_calls.load(Ordering::SeqCst),
        1,
        "still rate limited"
    );

    advance(300).await;
    assert_eq!(limited_calls.load(Ordering::SeqCst), 2);
}
