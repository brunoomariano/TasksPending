//! A fonte GitHub traduz buscas da API em cards, sem vazar detalhes do provedor.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use pending_core::{CardSeverity, PendingSource, SourceBatch};
use pending_github::{GithubSource, resolve_token};
use serde_json::{Value, json};

const TOKEN: &str = "ghp_secret_test_token";

/// Resposta programada por seção: status HTTP, cabeçalhos e corpo.
#[derive(Clone)]
struct Reply {
    status: StatusCode,
    headers: Vec<(&'static str, String)>,
    body: Value,
}

impl Reply {
    fn items(items: Vec<Value>) -> Self {
        Self {
            status: StatusCode::OK,
            headers: Vec::new(),
            body: json!({ "total_count": items.len(), "incomplete_results": false, "items": items }),
        }
    }

    fn status(status: StatusCode, message: &str) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: json!({ "message": message }),
        }
    }
}

#[derive(Clone, Default)]
struct Stub {
    /// Resposta por trecho da query (`review-requested`, `author`, `assignee`).
    replies: Arc<Mutex<HashMap<&'static str, Reply>>>,
    auth_headers: Arc<Mutex<Vec<String>>>,
}

async fn search(
    State(stub): State<Stub>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    stub.auth_headers.lock().unwrap().push(
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned(),
    );
    assert!(
        headers.contains_key("user-agent"),
        "GitHub requires a User-Agent"
    );

    let q = params.get("q").cloned().unwrap_or_default();
    let replies = stub.replies.lock().unwrap();
    let reply = replies
        .iter()
        .find(|(key, _)| q.contains(*key))
        .map(|(_, reply)| reply.clone())
        .unwrap_or_else(|| Reply::items(Vec::new()));

    let mut response = (reply.status, axum::Json(reply.body)).into_response();
    for (name, value) in reply.headers {
        response.headers_mut().insert(name, value.parse().unwrap());
    }
    response
}

async fn serve(stub: Stub) -> String {
    let app = Router::new()
        .route("/search/issues", get(search))
        .with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn item(repo: &str, number: u64, title: &str, is_pr: bool) -> Value {
    let mut value = json!({
        "number": number,
        "title": title,
        "html_url": format!("https://github.com/{repo}/{}/{number}", if is_pr { "pull" } else { "issues" }),
        "repository_url": format!("https://api.github.com/repos/{repo}"),
        "updated_at": "2026-09-28T10:00:00Z",
        "user": { "login": "octocat" },
    });
    if is_pr {
        value["pull_request"] = json!({ "url": "…" });
    }
    value
}

fn stub_with(replies: Vec<(&'static str, Reply)>) -> Stub {
    let stub = Stub::default();
    stub.replies.lock().unwrap().extend(replies);
    stub
}

async fn refresh(stub: Stub) -> Result<SourceBatch, pending_core::SourceError> {
    let base = serve(stub).await;
    GithubSource::new(base, Some(TOKEN.to_owned()))
        .refresh()
        .await
}

fn sections(batch: &SourceBatch) -> Vec<(String, String)> {
    batch
        .items
        .iter()
        .map(|item| (item.column.clone(), item.card.id.clone()))
        .collect()
}

/// As pendências viram três seções: revisões pedidas a mim, meus PRs abertos e
/// issues atribuídas a mim. Cada card traz link, repositório e número, e o
/// pedido de revisão é destacado como aviso, porque bloqueia outra pessoa.
#[tokio::test]
async fn pending_github_work_becomes_cards_in_three_sections() {
    let batch = refresh(stub_with(vec![
        (
            "review-requested",
            Reply::items(vec![item("o/api", 7, "Add cache", true)]),
        ),
        (
            "author",
            Reply::items(vec![item("o/web", 3, "Fix layout", true)]),
        ),
        (
            "assignee",
            Reply::items(vec![item("o/api", 9, "Crash on start", false)]),
        ),
    ]))
    .await
    .expect("refresh succeeds");

    assert_eq!(
        sections(&batch),
        vec![
            ("Review requested".to_owned(), "github:o/api#7".to_owned()),
            ("My pull requests".to_owned(), "github:o/web#3".to_owned()),
            ("Assigned issues".to_owned(), "github:o/api#9".to_owned()),
        ]
    );
    let review = &batch.items[0].card;
    assert_eq!(review.title, "Add cache");
    assert_eq!(
        review.url.as_deref(),
        Some("https://github.com/o/api/pull/7")
    );
    assert!(review.body.contains("o/api#7"), "{}", review.body);
    assert_eq!(review.source, "github");
    assert_eq!(review.severity, CardSeverity::Warning);
    assert_eq!(batch.items[1].card.severity, CardSeverity::Info);
    assert_eq!(review.updated_at.to_rfc3339(), "2026-09-28T10:00:00+00:00");
    assert!(batch.warnings.is_empty());
}

/// O token vai no cabeçalho de autorização de todas as buscas.
#[tokio::test]
async fn every_search_is_authenticated_with_the_token() {
    let stub = Stub::default();
    let auth = stub.auth_headers.clone();

    refresh(stub).await.expect("refresh succeeds");

    let auth = auth.lock().unwrap();
    assert_eq!(auth.len(), 3);
    assert!(
        auth.iter().all(|h| h == &format!("Bearer {TOKEN}")),
        "{auth:?}"
    );
}

/// Colunas são filtros independentes: o mesmo item achado por duas buscas
/// aparece nas duas colunas.
#[tokio::test]
async fn an_item_found_by_two_searches_appears_in_both_columns() {
    let shared = item("o/api", 7, "Add cache", true);
    let batch = refresh(stub_with(vec![
        ("review-requested", Reply::items(vec![shared.clone()])),
        ("assignee", Reply::items(vec![shared])),
    ]))
    .await
    .expect("refresh succeeds");

    assert_eq!(
        sections(&batch),
        vec![
            ("Review requested".to_owned(), "github:o/api#7".to_owned()),
            ("Assigned issues".to_owned(), "github:o/api#7".to_owned()),
        ]
    );
}

/// Colunas da configuração substituem as padrão: cada uma é uma busca, com a
/// gravidade escolhida.
#[tokio::test]
async fn configured_columns_run_their_own_queries() {
    let stub = stub_with(vec![(
        "org:acme",
        Reply::items(vec![item("acme/app", 1, "Acme PR", true)]),
    )]);
    let base = serve(stub).await;
    let source = GithubSource::new(base, Some(TOKEN.to_owned())).with_columns(vec![
        pending_github::GithubColumn {
            name: "Acme reviews".to_owned(),
            query: "is:open is:pr org:acme review-requested:@me".to_owned(),
            severity: Some(CardSeverity::Critical),
        },
    ]);

    assert_eq!(source.columns(), vec!["Acme reviews".to_owned()]);
    let batch = source.refresh().await.expect("refresh succeeds");
    assert_eq!(
        sections(&batch),
        vec![("Acme reviews".to_owned(), "github:acme/app#1".to_owned())]
    );
    assert_eq!(batch.items[0].card.severity, CardSeverity::Critical);
}

/// Se uma das buscas falha, as outras seções continuam na tela e o aviso diz
/// qual seção ficou de fora e por quê.
#[tokio::test]
async fn one_failed_search_keeps_the_other_sections_with_a_warning() {
    let batch = refresh(stub_with(vec![
        (
            "review-requested",
            Reply::status(StatusCode::BAD_GATEWAY, "upstream"),
        ),
        (
            "author",
            Reply::items(vec![item("o/web", 3, "Fix layout", true)]),
        ),
    ]))
    .await
    .expect("partial refresh still succeeds");

    assert_eq!(
        sections(&batch),
        vec![("My pull requests".to_owned(), "github:o/web#3".to_owned())]
    );
    assert_eq!(batch.warnings.len(), 1);
    assert!(
        batch.warnings[0].contains("Review requested"),
        "{:?}",
        batch.warnings
    );
    assert!(batch.warnings[0].contains("502"), "{:?}", batch.warnings);
}

/// Quando todas as buscas falham, o refresh falha com o motivo da API, e a
/// mensagem nunca contém o token.
#[tokio::test]
async fn all_searches_failing_is_an_error_without_the_token() {
    let bad = Reply::status(StatusCode::UNAUTHORIZED, "Bad credentials");
    let error = refresh(stub_with(vec![
        ("review-requested", bad.clone()),
        ("author", bad.clone()),
        ("assignee", bad),
    ]))
    .await
    .expect_err("every search failed");

    let message = error.to_string();
    assert!(message.contains("401"), "{message}");
    assert!(message.contains("Bad credentials"), "{message}");
    assert!(!message.contains(TOKEN), "{message}");
}

/// Limite de taxa estourado é explicado como tal, com o horário de liberação.
#[tokio::test]
async fn rate_limit_is_reported_with_the_reset_time() {
    let mut limited = Reply::status(StatusCode::FORBIDDEN, "API rate limit exceeded");
    limited.headers = vec![
        ("x-ratelimit-remaining", "0".to_owned()),
        ("x-ratelimit-reset", "1790600000".to_owned()),
    ];
    let error = refresh(stub_with(vec![
        ("review-requested", limited.clone()),
        ("author", limited.clone()),
        ("assignee", limited),
    ]))
    .await
    .expect_err("rate limited");

    let message = error.to_string();
    assert!(message.contains("rate limit"), "{message}");
    assert!(message.contains("2026-09-28T"), "{message}");
    assert_eq!(
        error.retry_at(),
        chrono::DateTime::from_timestamp(1_790_600_000, 0),
        "the aggregator waits until the reset"
    );
}

/// Se a busca achou mais itens do que os carregados, a fonte avisa quantos
/// ficaram de fora em vez de esconder isso.
#[tokio::test]
async fn truncated_results_are_reported() {
    let mut reply = Reply::items(vec![item("o/api", 1, "One", true)]);
    reply.body["total_count"] = json!(120);
    let batch = refresh(stub_with(vec![("author", reply)]))
        .await
        .expect("refresh succeeds");

    assert_eq!(batch.items.len(), 1);
    assert!(
        batch
            .warnings
            .iter()
            .any(|w| w.contains("My pull requests") && w.contains("120")),
        "{:?}",
        batch.warnings
    );
}

/// Sem token, a fonte não consulta nada e explica como configurar.
#[tokio::test]
async fn missing_token_fails_with_a_setup_hint() {
    let base = serve(Stub::default()).await;

    let error = GithubSource::new(base, None)
        .refresh()
        .await
        .expect_err("no token");

    let message = error.to_string();
    assert!(message.contains("GITHUB_TOKEN"), "{message}");
    assert!(message.contains("gh auth login"), "{message}");
}

/// O token vem de `GITHUB_TOKEN`, depois `GH_TOKEN`, depois do `gh auth token`;
/// valores vazios são ignorados.
#[test]
fn token_follows_env_then_gh_cli_precedence() {
    let gh = || Some("from-gh".to_owned());
    let no_gh = || None;

    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |key: &str| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    };

    assert_eq!(
        resolve_token(&env(&[("GITHUB_TOKEN", "a"), ("GH_TOKEN", "b")]), &gh),
        Some("a".to_owned())
    );
    assert_eq!(
        resolve_token(&env(&[("GITHUB_TOKEN", ""), ("GH_TOKEN", "b")]), &gh),
        Some("b".to_owned())
    );
    assert_eq!(resolve_token(&env(&[]), &gh), Some("from-gh".to_owned()));
    assert_eq!(resolve_token(&env(&[]), &no_gh), None);
}

/// Com a API fora do ar, o erro diz o que aconteceu na conexão, sem URL e
/// sem token.
#[tokio::test]
async fn transport_errors_explain_the_cause_without_url_or_token() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);

    let error = GithubSource::new(base.clone(), Some(TOKEN.to_owned()))
        .refresh()
        .await
        .expect_err("connection refused");

    let message = error.to_string().to_lowercase();
    assert!(message.contains("connect"), "{message}");
    assert!(!message.contains(&base), "{message}");
    assert!(!message.contains(TOKEN), "{message}");
}

/// Uma busca que trava não derruba as outras: ela vence o tempo limite
/// próprio, vira aviso com o nome da seção, e as demais seções aparecem.
#[tokio::test]
async fn a_hanging_search_times_out_without_losing_the_other_sections() {
    let app = Router::new().route(
        "/search/issues",
        get(|Query(params): Query<HashMap<String, String>>| async move {
            let q = params.get("q").cloned().unwrap_or_default();
            if q.contains("review-requested") {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
            let items = if q.contains("author") {
                vec![item("o/web", 3, "Fix layout", true)]
            } else {
                Vec::new()
            };
            axum::Json(json!({ "total_count": items.len(), "items": items }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let batch = GithubSource::new(base, Some(TOKEN.to_owned()))
        .with_request_timeout(std::time::Duration::from_millis(300))
        .refresh()
        .await
        .expect("other sections still load");

    assert_eq!(
        sections(&batch),
        vec![("My pull requests".to_owned(), "github:o/web#3".to_owned())]
    );
    assert!(
        batch
            .warnings
            .iter()
            .any(|w| w.contains("Review requested") && w.contains("timed out")),
        "{:?}",
        batch.warnings
    );
}

/// Quando o próprio GitHub avisa que a busca ficou incompleta, a fonte
/// repassa o aviso em vez de apresentar o resultado como completo.
#[tokio::test]
async fn incomplete_search_results_are_reported() {
    let mut reply = Reply::items(vec![item("o/api", 1, "One", true)]);
    reply.body["incomplete_results"] = json!(true);
    let batch = refresh(stub_with(vec![("author", reply)]))
        .await
        .expect("refresh succeeds");

    assert!(
        batch
            .warnings
            .iter()
            .any(|w| w.contains("My pull requests") && w.contains("incomplete")),
        "{:?}",
        batch.warnings
    );
}

/// PR em rascunho é marcado no card, para não parecer pronto para revisão.
#[tokio::test]
async fn draft_pull_requests_are_marked() {
    let mut draft = item("o/api", 7, "Add cache", true);
    draft["draft"] = json!(true);
    let batch = refresh(stub_with(vec![("author", Reply::items(vec![draft]))]))
        .await
        .expect("refresh succeeds");

    assert!(
        batch.items[0].card.body.contains("draft"),
        "{}",
        batch.items[0].card.body
    );
}

/// O id usa dono e repositório mesmo quando algum deles se chama `repos`.
#[tokio::test]
async fn card_id_keeps_owner_and_repo_named_repos() {
    let batch = refresh(stub_with(vec![(
        "author",
        Reply::items(vec![item("repos/repos", 5, "Odd names", true)]),
    )]))
    .await
    .expect("refresh succeeds");

    assert_eq!(batch.items[0].card.id, "github:repos/repos#5");
}

/// Um token com espaço ou quebra de linha no fim (comum ao copiar de arquivo)
/// é usado sem esses caracteres.
#[test]
fn tokens_are_trimmed() {
    let env = |key: &str| (key == "GITHUB_TOKEN").then(|| "abc\n".to_owned());
    assert_eq!(resolve_token(&env, &|| None), Some("abc".to_owned()));
}

#[cfg(unix)]
fn fake_gh(name: &str, script: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("fake-gh");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// O token vem da saída do `gh auth token`; saída com erro, vazia ou um `gh`
/// que trava não viram token, e o travamento desiste no limite.
#[cfg(unix)]
#[test]
fn gh_cli_token_handles_success_failure_and_hangs() {
    use pending_github::gh_cli_token_with;
    use std::time::{Duration, Instant};

    let ok = fake_gh("ok", "echo ' gho_from_cli '");
    let fails = fake_gh("fails", "echo 'not logged in' >&2; exit 1");
    let empty = fake_gh("empty", "true");
    let hangs = fake_gh("hangs", "sleep 30");
    let limit = Duration::from_millis(300);

    assert_eq!(
        gh_cli_token_with(ok.to_str().unwrap(), limit),
        Some("gho_from_cli".to_owned())
    );
    assert_eq!(gh_cli_token_with(fails.to_str().unwrap(), limit), None);
    assert_eq!(gh_cli_token_with(empty.to_str().unwrap(), limit), None);
    assert_eq!(gh_cli_token_with("/nonexistent/gh", limit), None);

    let started = Instant::now();
    assert_eq!(gh_cli_token_with(hangs.to_str().unwrap(), limit), None);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "gave up at the limit"
    );
}
