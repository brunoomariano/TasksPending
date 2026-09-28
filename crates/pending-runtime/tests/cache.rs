//! O último resultado bom de cada fonte sobrevive a reinícios.

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
            section: "Review".to_owned(),
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
        lane: "Work".to_owned(),
        interval: Duration::from_secs(300),
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
        .lanes
        .iter()
        .flat_map(|l| &l.sections)
        .flat_map(|s| &s.cards)
        .map(|c| c.id.clone())
        .collect()
}

fn status(aggregator: &Aggregator) -> SourceStatus {
    aggregator.snapshot().sources[0].status
}

async fn advance(secs: u64) {
    tokio::time::sleep(Duration::from_secs(secs)).await;
}

/// Um refresh bem-sucedido fica gravado; na próxima subida, os cards aparecem
/// na hora, com a fonte ainda atualizando, em vez de uma tela vazia.
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

/// Se a primeira consulta depois do reinício falha, os cards do cache
/// continuam na tela como dado antigo, com o motivo.
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

/// Um cache corrompido não impede a subida: é ignorado e reescrito no
/// próximo refresh bem-sucedido.
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
