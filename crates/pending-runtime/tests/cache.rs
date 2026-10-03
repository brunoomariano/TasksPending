//! Each source's last good result survives restarts.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use pending_core::{
    BoxFuture, CardSeverity, PendingCard, PendingSource, SourceBatch, SourceError, SourceItem,
    SourceStatus,
};
use pending_runtime::cache::Cache;
use pending_runtime::{Aggregator, SourceSpec};

struct Fixed {
    delay: Duration,
    result: Result<SourceBatch, SourceError>,
}

impl PendingSource for Fixed {
    fn columns(&self) -> Vec<String> {
        vec!["Review".to_owned()]
    }

    fn refresh(&self) -> BoxFuture<'_, Result<SourceBatch, SourceError>> {
        let delay = self.delay;
        let result = self.result.clone();
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            result
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

fn spec(result: Result<SourceBatch, SourceError>, delay_secs: u64) -> SourceSpec {
    SourceSpec {
        name: "github".to_owned(),
        source: Arc::new(Fixed {
            delay: Duration::from_secs(delay_secs),
            result,
        }),
        board: "Work".to_owned(),
        interval: Duration::from_secs(300),
        timeout: None,
        icon: None,
        sorts: Default::default(),
        excludes: Default::default(),
    }
}

fn cache_file(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cache-tests")
        .join(test);
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("state/cache.json")
}

fn card_ids(aggregator: &Aggregator) -> Vec<String> {
    aggregator
        .snapshot()
        .boards
        .iter()
        .flat_map(|b| &b.groups)
        .flat_map(|g| &g.columns)
        .flat_map(|c| &c.cards)
        .map(|c| c.id.clone())
        .collect()
}

fn status(aggregator: &Aggregator) -> SourceStatus {
    aggregator.snapshot().sources[0].status
}

async fn advance(secs: u64) {
    tokio::time::sleep(Duration::from_secs(secs)).await;
}

/// A successful refresh is saved; on the next startup, the cards show up
/// immediately, with the source still refreshing, instead of an empty screen.
#[tokio::test(start_paused = true)]
async fn a_restart_shows_the_last_good_cards_while_refreshing() {
    let path = cache_file("restart");
    {
        let first = Aggregator::start_with_cache(
            vec![spec(Ok(batch("from-first-run")), 0)],
            Duration::from_secs(30),
            Some(Cache::new(path.clone())),
        );
        advance(1).await;
        assert_eq!(status(&first), SourceStatus::Ready);
    }

    let second = Aggregator::start_with_cache(
        vec![spec(Ok(batch("from-second-run")), 10)],
        Duration::from_secs(30),
        Some(Cache::new(path)),
    );
    advance(1).await;
    assert_eq!(card_ids(&second), vec!["from-first-run"]);
    assert_eq!(status(&second), SourceStatus::Refreshing);

    advance(10).await;
    assert_eq!(card_ids(&second), vec!["from-second-run"]);
    assert_eq!(status(&second), SourceStatus::Ready);
}

/// If the first poll after a restart fails, the cached cards stay on screen
/// as stale data, with the reason.
#[tokio::test(start_paused = true)]
async fn a_failure_after_restart_keeps_the_cached_cards_as_stale() {
    let path = cache_file("stale");
    Cache::new(path.clone())
        .save(&[("github".to_owned(), Utc::now(), batch("cached"))])
        .unwrap();

    let aggregator = Aggregator::start_with_cache(
        vec![spec(Err(SourceError::new("503")), 0)],
        Duration::from_secs(30),
        Some(Cache::new(path)),
    );
    advance(1).await;

    assert_eq!(card_ids(&aggregator), vec!["cached"]);
    assert_eq!(status(&aggregator), SourceStatus::Degraded);
}

/// A corrupted cache doesn't block startup: it is ignored and rewritten on
/// the next successful refresh.
#[tokio::test(start_paused = true)]
async fn a_corrupt_cache_is_ignored_and_rewritten() {
    let path = cache_file("corrupt");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{ not json").unwrap();

    let aggregator = Aggregator::start_with_cache(
        vec![spec(Ok(batch("fresh")), 0)],
        Duration::from_secs(30),
        Some(Cache::new(path.clone())),
    );
    advance(1).await;

    assert_eq!(card_ids(&aggregator), vec!["fresh"]);
    let entries = Cache::new(path).load();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries["github"].1.items[0].card.id, "fresh");
}

/// Cards cached before a pattern was added to the config are filtered on
/// startup too: an excluded card does not show until the first refresh.
#[tokio::test(start_paused = true)]
async fn cached_cards_are_filtered_by_the_current_exclude_patterns() {
    let path = cache_file("exclude");
    let mut cached = batch("bump-serde");
    cached.items.extend(batch("fix-login").items);
    Cache::new(path.clone())
        .save(&[("github".to_owned(), Utc::now(), cached)])
        .unwrap();
    let source: pending_core::SourceConfig = toml::from_str(
        "name = \"github\"\nkind = \"plane\"\n[[stacks]]\nname = \"Review\"\nexclude = [\"^bump\"]",
    )
    .expect("valid toml");

    let aggregator = Aggregator::start_with_cache(
        vec![SourceSpec {
            excludes: pending_runtime::exclude::Excludes::from_stacks(&source.stacks)
                .expect("valid patterns"),
            ..spec(Ok(batch("fresh")), 10)
        }],
        Duration::from_secs(30),
        Some(Cache::new(path)),
    );
    advance(1).await;

    assert_eq!(card_ids(&aggregator), vec!["fix-login"]);
    assert_eq!(status(&aggregator), SourceStatus::Refreshing);
}

/// Several sources finishing at once write the cache concurrently without
/// corrupting the file or losing writes.
#[test]
fn concurrent_saves_never_corrupt_the_file() {
    let path = cache_file("concurrent");
    let cache = Arc::new(Cache::new(path.clone()));

    let threads: Vec<_> = (0..8)
        .map(|n| {
            let cache = cache.clone();
            std::thread::spawn(move || {
                for _ in 0..20 {
                    cache
                        .save(&[(format!("source-{n}"), Utc::now(), batch("x"))])
                        .expect("save succeeds");
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(serde_json_is_valid(&text), "{text}");
    assert_eq!(cache.load().len(), 8, "every source's entry is kept");
}

/// API and TUI with different configs share the file: each updates its own
/// sources without erasing the other's.
#[test]
fn processes_with_different_sources_keep_each_others_entries() {
    let path = cache_file("merge");
    let api = Cache::new(path.clone());
    let tui = Cache::new(path.clone());

    api.save(&[("github".to_owned(), Utc::now(), batch("gh"))])
        .unwrap();
    tui.save(&[("plane".to_owned(), Utc::now(), batch("pl"))])
        .unwrap();
    api.save(&[("github".to_owned(), Utc::now(), batch("gh2"))])
        .unwrap();

    let entries = Cache::new(path).load();
    assert_eq!(entries["github"].1.items[0].card.id, "gh2");
    assert_eq!(entries["plane"].1.items[0].card.id, "pl");
}

/// The cache holds PR titles and private items: only the owner can read it.
#[cfg(unix)]
#[test]
fn the_cache_file_is_private() {
    use std::os::unix::fs::PermissionsExt;

    let path = cache_file("private");
    Cache::new(path.clone())
        .save(&[("github".to_owned(), Utc::now(), batch("gh"))])
        .unwrap();

    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

fn serde_json_is_valid(text: &str) -> bool {
    text.starts_with('{')
        && text.ends_with('}')
        && text.matches('{').count() == text.matches('}').count()
}

/// A late write with older data doesn't erase the newer entry another write
/// already left in the file.
#[test]
fn a_late_save_with_older_data_does_not_win() {
    let path = cache_file("newest-wins");
    let cache = Cache::new(path.clone());
    let newer = Utc::now();
    let older = newer - chrono::Duration::minutes(5);

    cache
        .save(&[("github".to_owned(), newer, batch("new"))])
        .unwrap();
    cache
        .save(&[("github".to_owned(), older, batch("old"))])
        .unwrap();

    assert_eq!(cache.load()["github"].1.items[0].card.id, "new");
}

/// A cache written by an earlier version (cards with `section`) is still
/// read after the upgrade.
#[test]
fn caches_from_the_previous_format_still_load() {
    let path = cache_file("old-format");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"version":1,"sources":{"github":{"refreshed_at":"2026-09-28T10:00:00Z","batch":{"items":[{"section":"Review requested","card":{"id":"github:o/r#1","title":"t","body":"b","source":"github","url":null,"severity":"info","updated_at":"2026-09-28T10:00:00Z"}}],"warnings":[]}}}}"#,
    )
    .unwrap();

    let entries = Cache::new(path).load();

    assert_eq!(entries["github"].1.items[0].column, "Review requested");
}
