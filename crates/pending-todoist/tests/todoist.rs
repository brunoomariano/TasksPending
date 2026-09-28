//! A fonte Todoist traz tarefas pelos filtros do próprio Todoist.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use chrono::NaiveDate;
use pending_core::{CardSeverity, PendingSource, SourceBatch};
use pending_todoist::{TodoistColumn, TodoistSource};
use serde_json::{Value, json};

const TOKEN: &str = "todoist_secret_token";

#[derive(Clone, Default)]
struct Stub {
    /// Pages per filter query, served in order following `cursor`.
    tasks: Arc<Mutex<HashMap<String, Vec<Value>>>>,
    projects: Arc<Mutex<Value>>,
    auth: Arc<Mutex<Vec<String>>>,
    status: Arc<Mutex<Option<StatusCode>>>,
}

fn record(stub: &Stub, headers: &HeaderMap) {
    stub.auth.lock().unwrap().push(
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned(),
    );
}

async fn filter(
    State(stub): State<Stub>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    record(&stub, &headers);
    if let Some(status) = *stub.status.lock().unwrap() {
        return (status, "nope").into_response();
    }
    let query = params.get("query").cloned().unwrap_or_default();
    let page: usize = params
        .get("cursor")
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let pages = stub
        .tasks
        .lock()
        .unwrap()
        .get(&query)
        .cloned()
        .unwrap_or_default();
    let results = pages.get(page).cloned().unwrap_or(json!([]));
    let next = (page + 1 < pages.len()).then(|| (page + 1).to_string());
    axum::Json(json!({ "results": results, "next_cursor": next })).into_response()
}

async fn projects(State(stub): State<Stub>, headers: HeaderMap) -> Response {
    record(&stub, &headers);
    axum::Json(json!({ "results": stub.projects.lock().unwrap().clone(), "next_cursor": null }))
        .into_response()
}

async fn serve(stub: Stub) -> String {
    let app = Router::new()
        .route("/api/v1/tasks/filter", get(filter))
        .route("/api/v1/projects", get(projects))
        .with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn task(id: &str, content: &str, priority: u8, due: Option<&str>) -> Value {
    json!({
        "id": id,
        "content": content,
        "description": "",
        "project_id": "p1",
        "priority": priority,
        "labels": ["deep"],
        "due": due.map(|date| json!({ "date": date, "string": "every day", "is_recurring": false })),
        "updated_at": "2026-09-28T10:00:00Z",
    })
}

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 9, 28).unwrap()
}

fn source(base: String) -> TodoistSource {
    TodoistSource::new(base, Ok(TOKEN.to_owned())).with_today(today)
}

fn ids(batch: &SourceBatch) -> Vec<(String, String)> {
    batch
        .items
        .iter()
        .map(|item| (item.column.clone(), item.card.id.clone()))
        .collect()
}

/// Sem colunas configuradas, aparecem "Today" (hoje e atrasadas) e "Next 7
/// days"; cada card traz o projeto, o prazo e o link para a tarefa, e o token
/// vai no cabeçalho de todas as chamadas.
#[tokio::test]
async fn default_columns_show_today_and_the_next_days() {
    let stub = Stub::default();
    *stub.projects.lock().unwrap() = json!([{ "id": "p1", "name": "Home" }]);
    stub.tasks.lock().unwrap().insert(
        "today | overdue".to_owned(),
        vec![json!([task("t1", "Pay rent", 1, Some("2026-09-28"))])],
    );
    stub.tasks.lock().unwrap().insert(
        "7 days & !today & !overdue".to_owned(),
        vec![json!([task(
            "t2",
            "Dentist",
            1,
            Some("2026-10-01T14:30:00")
        )])],
    );
    let auth = stub.auth.clone();
    let base = serve(stub).await;

    let source = source(base);
    assert_eq!(source.columns(), vec!["Today", "Next 7 days"]);
    let batch = source.refresh().await.expect("refresh succeeds");

    assert_eq!(
        ids(&batch),
        vec![
            ("Today".to_owned(), "todoist:t1".to_owned()),
            ("Next 7 days".to_owned(), "todoist:t2".to_owned()),
        ]
    );
    let card = &batch.items[0].card;
    assert_eq!(card.title, "Pay rent");
    assert_eq!(card.source, "todoist");
    assert!(card.body.contains("Home"), "{}", card.body);
    assert_eq!(
        card.url.as_deref(),
        Some("https://app.todoist.com/app/task/t1")
    );
    assert!(card.due_at.is_some());
    let dentist = &batch.items[1].card;
    assert!(dentist.due_at.is_some(), "floating due time is read");
    assert!(
        dentist.body.contains("2026-10-01 14:30"),
        "{}",
        dentist.body
    );
    assert!(
        auth.lock()
            .unwrap()
            .iter()
            .all(|a| a == &format!("Bearer {TOKEN}"))
    );
}

/// Tarefa atrasada é crítica; prioridade p1 (4 na API) é aviso.
#[tokio::test]
async fn overdue_tasks_are_critical_and_p1_is_a_warning() {
    let stub = Stub::default();
    stub.tasks.lock().unwrap().insert(
        "today | overdue".to_owned(),
        vec![json!([
            task("late", "Late", 1, Some("2026-09-20")),
            task("p1", "Urgent", 4, Some("2026-09-28")),
            task("plain", "Plain", 1, None),
        ])],
    );
    let base = serve(stub).await;

    let batch = source(base).refresh().await.expect("refresh succeeds");

    let severity = |id: &str| {
        batch
            .items
            .iter()
            .find(|i| i.card.id == format!("todoist:{id}"))
            .map(|i| i.card.severity)
    };
    assert_eq!(severity("late"), Some(CardSeverity::Critical));
    assert_eq!(severity("p1"), Some(CardSeverity::Warning));
    assert_eq!(severity("plain"), Some(CardSeverity::Info));
    let late = batch
        .items
        .iter()
        .find(|i| i.card.id == "todoist:late")
        .unwrap();
    assert!(late.card.body.contains("overdue"), "{}", late.card.body);
}

/// Colunas configuradas usam os filtros do Todoist; as páginas são seguidas.
#[tokio::test]
async fn configured_columns_use_todoist_filters_across_pages() {
    let stub = Stub::default();
    stub.tasks.lock().unwrap().insert(
        "#Work & @waiting".to_owned(),
        vec![
            json!([task("a", "First", 1, None)]),
            json!([task("b", "Second", 1, None)]),
        ],
    );
    let base = serve(stub).await;
    let source = source(base).with_columns(vec![TodoistColumn {
        name: "Waiting".to_owned(),
        filter: "#Work & @waiting".to_owned(),
    }]);

    let batch = source.refresh().await.expect("refresh succeeds");

    assert_eq!(
        ids(&batch),
        vec![
            ("Waiting".to_owned(), "todoist:a".to_owned()),
            ("Waiting".to_owned(), "todoist:b".to_owned()),
        ]
    );
}

/// Token inválido falha com o status, sem expor o token; sem token, a fonte
/// explica onde obtê-lo.
#[tokio::test]
async fn auth_problems_are_explained_without_the_token() {
    let stub = Stub::default();
    *stub.status.lock().unwrap() = Some(StatusCode::UNAUTHORIZED);
    let base = serve(stub).await;

    let error = source(base.clone()).refresh().await.expect_err("401");
    assert!(error.to_string().contains("401"), "{error}");
    assert!(!error.to_string().contains(TOKEN), "{error}");

    let error = TodoistSource::from_env(&|_| None)
        .refresh()
        .await
        .expect_err("no token");
    assert!(error.to_string().contains("TODOIST_API_TOKEN"), "{error}");
    assert!(error.to_string().contains("Integrations"), "{error}");
}

/// Prazos com fração de segundo ou em UTC exato são lidos; uma tarefa com
/// horário que já passou hoje conta como atrasada.
#[tokio::test]
async fn due_times_with_fractions_or_utc_are_read_and_past_times_are_overdue() {
    let stub = Stub::default();
    stub.tasks.lock().unwrap().insert(
        "today | overdue".to_owned(),
        vec![json!([
            task("frac", "Fractional", 1, Some("2026-10-02T09:00:00.000000")),
            task("utc", "Exact", 1, Some("2026-10-02T12:00:00Z")),
            task("past", "Earlier", 1, Some("2020-01-01T09:00:00")),
        ])],
    );
    let base = serve(stub).await;

    let batch = source(base).refresh().await.expect("refresh succeeds");

    let get = |id: &str| {
        batch
            .items
            .iter()
            .find(|i| i.card.id == format!("todoist:{id}"))
            .unwrap()
    };
    assert!(get("frac").card.due_at.is_some());
    assert_eq!(
        get("utc").card.due_at.map(|at| at.to_rfc3339()),
        Some("2026-10-02T12:00:00+00:00".to_owned())
    );
    assert_eq!(get("past").card.severity, CardSeverity::Critical);
}
