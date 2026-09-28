//! O agregador consulta cada fonte no próprio ritmo e mantém o último estado.

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

/// Fonte de teste: responde o roteiro em ordem e repete o último passo.
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

fn spec(source: Arc<Scripted>, interval_secs: u64) -> SourceSpec {
    named_spec(source.name, source, interval_secs)
}

fn named_spec(name: &str, source: Arc<dyn PendingSource>, interval_secs: u64) -> SourceSpec {
    SourceSpec {
        name: name.to_owned(),
        source,
        lane: "Work".to_owned(),
        interval: Duration::from_secs(interval_secs),
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
        .lanes
        .iter()
        .flat_map(|l| &l.sections)
        .flat_map(|s| &s.cards)
        .map(|c| c.id.clone())
        .collect()
}

async fn advance(secs: u64) {
    tokio::time::sleep(Duration::from_secs(secs)).await;
}

/// Enquanto a primeira consulta não volta, a fonte aparece como atualizando;
/// quando volta, os cards aparecem e a fonte fica pronta.
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

/// Cada fonte tem o próprio intervalo: uma fonte rápida não espera a lenta e a
/// lenta não é consultada no ritmo da rápida.
#[tokio::test(start_paused = true)]
async fn each_source_refreshes_on_its_own_interval() {
    let (fast, fast_calls) = Scripted::new("fast", vec![Ok(batch("f"))]);
    let (slow, slow_calls) = Scripted::new("slow", vec![Ok(batch("s"))]);
    let _aggregator = Aggregator::start(vec![spec(fast, 10), spec(slow, 60)], TIMEOUT);

    advance(65).await;

    assert_eq!(fast_calls.load(Ordering::SeqCst), 7, "t=0,10,..,60");
    assert_eq!(slow_calls.load(Ordering::SeqCst), 2, "t=0,60");
}

/// Se o refresh falha depois de um sucesso, os últimos cards continuam na tela
/// e a fonte fica degradada explicando a falha.
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

/// Sem nenhum sucesso anterior, a falha aparece como falha, com o motivo.
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

/// Uma fonte que trava não segura o dashboard para sempre: depois do timeout o
/// refresh conta como falha.
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

/// Quando o último handle do agregador é descartado, as fontes param de ser
/// consultadas.
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

/// Fonte que entra em pânico nas primeiras consultas e depois responde.
struct Panicky {
    calls: Arc<AtomicUsize>,
    panics: usize,
}

impl PendingSource for Panicky {
    fn refresh(&self) -> BoxFuture<'_, Result<SourceBatch, SourceError>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call < self.panics {
            panic!("bug in source");
        }
        Box::pin(async { Ok(batch("recovered")) })
    }
}

/// Um bug que faz a fonte entrar em pânico não pode congelar o status dela:
/// o refresh conta como falha com o motivo, e a fonte continua sendo
/// consultada no intervalo seguinte.
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

    // A falha dobra a espera: a nova consulta acontece em t=20.
    advance(20).await;
    let snapshot = aggregator.snapshot();
    assert_eq!(health(&snapshot, "panicky").status, SourceStatus::Ready);
    assert_eq!(card_ids(&snapshot), vec!["recovered"]);
}

/// Falhas seguidas continuam mostrando o horário do último sucesso, e o
/// primeiro sucesso depois delas deixa a fonte pronta de novo.
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

    // Falhas em t=10 e t=30 (backoff de 20s e 40s); sucesso em t=70.
    advance(50).await;
    let snapshot = aggregator.snapshot();
    assert_eq!(health(&snapshot, "github").status, SourceStatus::Ready);
    assert_eq!(card_ids(&snapshot), vec!["new"]);
}

/// A mensagem de timeout mostra a duração real, mesmo abaixo de um segundo.
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

/// Pedir atualização agora consulta todas as fontes sem esperar o intervalo, e
/// o próximo refresh volta a contar a partir daí.
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

/// Uma fonte que falha seguidas vezes é consultada cada vez menos (o dobro do
/// intervalo a cada falha), para não martelar uma API fora do ar; o primeiro
/// sucesso volta ao intervalo normal.
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

    // Falhas em t=0 e t=20 (espera 2x); sucesso em t=60 (espera 4x).
    advance(1).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    advance(20).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    advance(30).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2, "second failure waits 40s");
    advance(10).await;
    assert_eq!(calls.load(Ordering::SeqCst), 3);

    // Depois do sucesso, volta ao intervalo de 10s.
    advance(10).await;
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

/// Quando a API diz até quando está limitando (rate limit), a fonte só volta a
/// ser consultada depois desse horário.
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

/// Um refresh manual não fura o horário de liberação de uma API com limite de
/// taxa: a fonte só é consultada depois dele; fontes sem limite atualizam na
/// hora.
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
