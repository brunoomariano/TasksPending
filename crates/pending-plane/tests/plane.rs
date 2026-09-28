//! A fonte Plane traz os itens abertos atribuídos ao usuário da API key.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use pending_core::{CardSeverity, PendingSource, SourceBatch, SourceError};
use pending_plane::{PlaneSettings, PlaneSource};
use serde_json::{Value, json};

const KEY: &str = "plane_secret_key";
const ME: &str = "user-me";

#[derive(Clone, Default)]
struct Stub {
    me: Arc<Mutex<Option<Reply>>>,
    projects: Arc<Mutex<Value>>,
    /// Second page of projects, served for `cursor=1`.
    projects_page2: Arc<Mutex<Option<Value>>>,
    states: Arc<Mutex<Value>>,
    /// Project ids whose issue requests hang.
    slow: Arc<Mutex<Vec<String>>>,
    /// Pages per project id, served in order following `cursor`.
    issues: Arc<Mutex<HashMap<String, Vec<Reply>>>>,
    keys: Arc<Mutex<Vec<String>>>,
    cursors: Arc<Mutex<Vec<String>>>,
}

#[derive(Clone)]
struct Reply {
    status: StatusCode,
    body: Value,
}

fn ok(body: Value) -> Reply {
    Reply {
        status: StatusCode::OK,
        body,
    }
}

fn record_key(stub: &Stub, headers: &HeaderMap) {
    stub.keys.lock().unwrap().push(
        headers
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned(),
    );
}

async fn me(State(stub): State<Stub>, headers: HeaderMap) -> Response {
    record_key(&stub, &headers);
    let reply = stub
        .me
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| ok(json!({ "id": ME, "display_name": "me" })));
    (reply.status, axum::Json(reply.body)).into_response()
}

async fn projects(
    State(stub): State<Stub>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    record_key(&stub, &headers);
    if params.get("cursor").map(String::as_str) == Some("1") {
        let page = stub
            .projects_page2
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(json!([]));
        return axum::Json(page).into_response();
    }
    axum::Json(stub.projects.lock().unwrap().clone()).into_response()
}

async fn states(State(stub): State<Stub>, headers: HeaderMap) -> Response {
    record_key(&stub, &headers);
    axum::Json(stub.states.lock().unwrap().clone()).into_response()
}

async fn issues(
    State(stub): State<Stub>,
    headers: HeaderMap,
    Path((_slug, project)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    record_key(&stub, &headers);
    if stub.slow.lock().unwrap().contains(&project) {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    }
    let cursor = params.get("cursor").cloned().unwrap_or_default();
    stub.cursors.lock().unwrap().push(cursor.clone());
    let pages = stub.issues.lock().unwrap();
    let pages = pages.get(&project).cloned().unwrap_or_default();
    let index: usize = cursor.parse().unwrap_or(0);
    let reply = pages
        .get(index)
        .cloned()
        .unwrap_or_else(|| ok(json!({ "results": [] })));
    (reply.status, axum::Json(reply.body)).into_response()
}

async fn serve(stub: Stub) -> String {
    let app = Router::new()
        .route("/api/v1/users/me/", get(me))
        .route("/api/v1/workspaces/{slug}/projects/", get(projects))
        .route(
            "/api/v1/workspaces/{slug}/projects/{project}/issues/",
            get(issues),
        )
        .route(
            "/api/v1/workspaces/{slug}/projects/{project}/states/",
            get(states),
        )
        .with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn issue(id: &str, seq: u64, name: &str, group: &str, assignee: &str) -> Value {
    json!({
        "id": id,
        "sequence_id": seq,
        "name": name,
        "priority": "none",
        "state": { "id": format!("state-{group}"), "name": group.to_uppercase(), "group": group },
        "assignees": [{ "id": assignee, "display_name": "x" }],
        "updated_at": "2026-09-28T10:00:00Z",
        "target_date": null,
    })
}

fn source(base: String) -> PlaneSource {
    PlaneSource::new(Ok(PlaneSettings {
        web_url: base.clone(),
        base_url: base,
        workspace_slug: "acme".to_owned(),
        api_key: KEY.to_owned(),
    }))
}

async fn refresh(stub: Stub) -> Result<SourceBatch, SourceError> {
    let base = serve(stub).await;
    source(base).refresh().await
}

fn one_project(stub: &Stub, issues: Vec<Value>) {
    *stub.projects.lock().unwrap() =
        json!([{ "id": "p1", "identifier": "API", "name": "Backend" }]);
    stub.issues
        .lock()
        .unwrap()
        .insert("p1".to_owned(), vec![ok(json!({ "results": issues }))]);
}

fn ids(batch: &SourceBatch) -> Vec<(String, String)> {
    batch
        .items
        .iter()
        .map(|item| (item.section.clone(), item.card.id.clone()))
        .collect()
}

/// Itens abertos atribuídos a mim viram cards por estado: em andamento, a
/// fazer e backlog. Concluídos, cancelados e itens de outras pessoas ficam de
/// fora, mesmo que o servidor ignore os filtros e mande tudo.
#[tokio::test]
async fn open_items_assigned_to_me_become_cards_by_state() {
    let stub = Stub::default();
    one_project(
        &stub,
        vec![
            issue("i1", 1, "Fix login", "started", ME),
            issue("i2", 2, "Write docs", "unstarted", ME),
            issue("i3", 3, "Someday", "backlog", ME),
            issue("i4", 4, "Done already", "completed", ME),
            issue("i5", 5, "Dropped", "cancelled", ME),
            issue("i6", 6, "Not mine", "started", "someone-else"),
        ],
    );

    let batch = refresh(stub).await.expect("refresh succeeds");

    assert_eq!(
        ids(&batch),
        vec![
            ("In progress".to_owned(), "plane:API-1".to_owned()),
            ("To do".to_owned(), "plane:API-2".to_owned()),
            ("Backlog".to_owned(), "plane:API-3".to_owned()),
        ]
    );
    let card = &batch.items[0].card;
    assert_eq!(card.title, "Fix login");
    assert_eq!(card.source, "plane");
    assert!(card.body.contains("API-1"), "{}", card.body);
    assert!(card.body.contains("Backend"), "{}", card.body);
    assert!(
        card.url
            .as_deref()
            .unwrap()
            .ends_with("/acme/projects/p1/issues/i1")
    );
    assert_eq!(card.updated_at.to_rfc3339(), "2026-09-28T10:00:00+00:00");
}

/// Prioridade urgente vira crítico e alta vira aviso; prazo vencido também é
/// crítico e aparece no card.
#[tokio::test]
async fn priority_and_overdue_dates_set_severity() {
    let stub = Stub::default();
    let mut urgent = issue("i1", 1, "Urgent", "started", ME);
    urgent["priority"] = json!("urgent");
    let mut high = issue("i2", 2, "High", "started", ME);
    high["priority"] = json!("high");
    let mut overdue = issue("i3", 3, "Late", "unstarted", ME);
    overdue["target_date"] = json!("2020-01-01");
    one_project(&stub, vec![urgent, high, overdue]);

    let batch = refresh(stub).await.expect("refresh succeeds");

    let severity = |id: &str| {
        batch
            .items
            .iter()
            .find(|item| item.card.id == id)
            .map(|item| item.card.severity)
    };
    assert_eq!(severity("plane:API-1"), Some(CardSeverity::Critical));
    assert_eq!(severity("plane:API-2"), Some(CardSeverity::Warning));
    assert_eq!(severity("plane:API-3"), Some(CardSeverity::Critical));
    let late = batch
        .items
        .iter()
        .find(|i| i.card.id == "plane:API-3")
        .unwrap();
    assert!(late.card.body.contains("overdue"), "{}", late.card.body);
}

/// Estado e responsáveis podem vir como ids (sem expand) e a lista de
/// projetos pode vir num envelope `results`; os dois formatos funcionam.
#[tokio::test]
async fn unexpanded_fields_and_wrapped_projects_are_understood() {
    let stub = Stub::default();
    *stub.projects.lock().unwrap() = json!({
        "results": [{ "id": "p1", "identifier": "API", "name": "Backend" }]
    });
    *stub.states.lock().unwrap() =
        json!([{ "id": "state-uuid", "name": "Doing", "group": "started" }]);
    let mut raw = issue("i1", 1, "Raw", "started", ME);
    raw["state"] = json!("state-uuid");
    raw["assignees"] = json!([ME]);
    stub.issues
        .lock()
        .unwrap()
        .insert("p1".to_owned(), vec![ok(json!({ "results": [raw] }))]);

    let batch = refresh(stub).await.expect("refresh succeeds");

    assert_eq!(
        ids(&batch),
        vec![("In progress".to_owned(), "plane:API-1".to_owned())]
    );
}

/// Os itens de um projeto são lidos página a página enquanto o Plane indica
/// que há mais.
#[tokio::test]
async fn issue_pages_are_followed() {
    let stub = Stub::default();
    *stub.projects.lock().unwrap() =
        json!([{ "id": "p1", "identifier": "API", "name": "Backend" }]);
    stub.issues.lock().unwrap().insert(
        "p1".to_owned(),
        vec![
            ok(json!({
                "results": [issue("i1", 1, "First", "started", ME)],
                "next_cursor": "1",
                "next_page_results": true,
            })),
            ok(json!({
                "results": [issue("i2", 2, "Second", "started", ME)],
                "next_cursor": "2",
                "next_page_results": false,
            })),
        ],
    );
    let cursors = stub.cursors.clone();

    let batch = refresh(stub).await.expect("refresh succeeds");

    assert_eq!(batch.items.len(), 2);
    assert_eq!(cursors.lock().unwrap().len(), 2, "stops when no more pages");
}

/// Um projeto com falha vira aviso com o identificador dele; os outros
/// projetos continuam na tela.
#[tokio::test]
async fn a_failing_project_becomes_a_warning() {
    let stub = Stub::default();
    *stub.projects.lock().unwrap() = json!([
        { "id": "p1", "identifier": "API", "name": "Backend" },
        { "id": "p2", "identifier": "WEB", "name": "Frontend" }
    ]);
    stub.issues.lock().unwrap().insert(
        "p1".to_owned(),
        vec![ok(
            json!({ "results": [issue("i1", 1, "Ok", "started", ME)] }),
        )],
    );
    stub.issues.lock().unwrap().insert(
        "p2".to_owned(),
        vec![Reply {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: json!({ "error": "boom" }),
        }],
    );

    let batch = refresh(stub).await.expect("partial refresh succeeds");

    assert_eq!(ids(&batch).len(), 1);
    assert!(
        batch
            .warnings
            .iter()
            .any(|w| w.contains("WEB") && w.contains("500")),
        "{:?}",
        batch.warnings
    );
}

/// Chave inválida falha com o status da API, sem a chave na mensagem; todas
/// as chamadas mandam a chave no cabeçalho `x-api-key`.
#[tokio::test]
async fn bad_key_fails_without_leaking_it() {
    let stub = Stub::default();
    *stub.me.lock().unwrap() = Some(Reply {
        status: StatusCode::UNAUTHORIZED,
        body: json!({ "detail": "Invalid token." }),
    });
    let keys = stub.keys.clone();

    let error = refresh(stub).await.expect_err("bad key");

    let message = error.to_string();
    assert!(message.contains("401"), "{message}");
    assert!(message.contains("Invalid token."), "{message}");
    assert!(!message.contains(KEY), "{message}");
    assert!(keys.lock().unwrap().iter().all(|k| k == KEY));
}

/// Sem as variáveis de ambiente, a fonte explica quais faltam.
#[tokio::test]
async fn missing_settings_name_the_variables() {
    let env = |key: &str| (key == "PLANE_BASE_URL").then(|| "https://plane.example".to_owned());

    let error = PlaneSource::new(PlaneSettings::from_env(&env))
        .refresh()
        .await
        .expect_err("incomplete settings");

    let message = error.to_string();
    assert!(message.contains("PLANE_WORKSPACE_SLUG"), "{message}");
    assert!(message.contains("PLANE_API_KEY"), "{message}");
    assert!(!message.contains("PLANE_BASE_URL"), "{message}");
}

/// Sem expand, o grupo do estado vem da lista de estados do projeto; itens
/// concluídos continuam de fora, e estado desconhecido não vira backlog.
#[tokio::test]
async fn unexpanded_completed_items_stay_hidden() {
    let stub = Stub::default();
    *stub.states.lock().unwrap() = json!({
        "results": [{ "id": "s-done", "name": "Done", "group": "completed" }]
    });
    let mut done = issue("i1", 1, "Done", "completed", ME);
    done["state"] = json!("s-done");
    let mut unknown = issue("i2", 2, "Unknown", "started", ME);
    unknown["state"] = json!("s-missing");
    one_project(&stub, vec![done, unknown]);

    let batch = refresh(stub).await.expect("refresh succeeds");

    assert!(batch.items.is_empty(), "{:?}", ids(&batch));
    assert!(
        batch
            .warnings
            .iter()
            .any(|w| w.contains("API") && w.contains("state")),
        "{:?}",
        batch.warnings
    );
}

/// A lista de projetos é lida página a página.
#[tokio::test]
async fn project_pages_are_followed() {
    let stub = Stub::default();
    *stub.projects.lock().unwrap() = json!({
        "results": [{ "id": "p1", "identifier": "API", "name": "Backend" }],
        "next_cursor": "1",
        "next_page_results": true,
    });
    *stub.projects_page2.lock().unwrap() = Some(json!({
        "results": [{ "id": "p2", "identifier": "WEB", "name": "Frontend" }],
        "next_page_results": false,
    }));
    stub.issues.lock().unwrap().insert(
        "p2".to_owned(),
        vec![ok(
            json!({ "results": [issue("i1", 1, "Second page", "started", ME)] }),
        )],
    );

    let batch = refresh(stub).await.expect("refresh succeeds");

    assert_eq!(batch.items[0].card.id, "plane:WEB-1");
}

/// Um projeto lento vira aviso depois do limite dele; os outros aparecem.
#[tokio::test]
async fn a_slow_project_becomes_a_warning() {
    let stub = Stub::default();
    *stub.projects.lock().unwrap() = json!([
        { "id": "p1", "identifier": "API", "name": "Backend" },
        { "id": "p2", "identifier": "WEB", "name": "Frontend" }
    ]);
    stub.issues.lock().unwrap().insert(
        "p1".to_owned(),
        vec![ok(
            json!({ "results": [issue("i1", 1, "Fast", "started", ME)] }),
        )],
    );
    stub.slow.lock().unwrap().push("p2".to_owned());
    let base = serve(stub).await;

    let batch = source(base)
        .with_project_budget(std::time::Duration::from_millis(300))
        .refresh()
        .await
        .expect("fast project still shows");

    assert_eq!(ids(&batch).len(), 1);
    assert!(
        batch
            .warnings
            .iter()
            .any(|w| w.contains("WEB") && w.contains("timed out")),
        "{:?}",
        batch.warnings
    );
}

/// A chave nunca é enviada a outro servidor por redirecionamento.
#[tokio::test]
async fn redirects_are_not_followed_with_the_key() {
    let elsewhere = Stub::default();
    let leaked = elsewhere.keys.clone();
    let elsewhere_base = serve(elsewhere).await;
    let app = Router::new().route(
        "/api/v1/users/me/",
        get(move || {
            let target = format!("{elsewhere_base}/api/v1/users/me/");
            async move { (StatusCode::FOUND, [("location", target)]).into_response() }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let error = source(base).refresh().await.expect_err("redirect refused");

    assert!(error.to_string().contains("302"), "{error}");
    assert!(
        leaked.lock().unwrap().is_empty(),
        "key sent to another host"
    );
}

/// No Plane Cloud a API fica em api.plane.so e o app em app.plane.so; os
/// links dos cards apontam para o app. PLANE_WEB_URL permite outro endereço.
#[test]
fn cloud_cards_link_to_the_web_app() {
    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |key: &str| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    };

    let cloud = PlaneSettings::from_env(&env(&[
        ("PLANE_BASE_URL", "https://api.plane.so/"),
        ("PLANE_WORKSPACE_SLUG", "acme"),
        ("PLANE_API_KEY", "k"),
    ]))
    .unwrap();
    assert_eq!(cloud.web_url, "https://app.plane.so");

    let custom = PlaneSettings::from_env(&env(&[
        ("PLANE_BASE_URL", "https://plane-api.example"),
        ("PLANE_WEB_URL", "https://plane.example"),
        ("PLANE_WORKSPACE_SLUG", "acme"),
        ("PLANE_API_KEY", "k"),
    ]))
    .unwrap();
    assert_eq!(custom.web_url, "https://plane.example");

    assert!(
        !format!("{custom:?}").contains("\"k\""),
        "Debug must hide the key"
    );
}
