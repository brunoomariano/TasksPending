//! A fonte Google lê as agendas visíveis pela API do Calendar, com token do
//! GNOME Online Accounts (aqui, um provedor falso).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use chrono::{DateTime, TimeZone, Utc};
use pending_core::{BoxFuture, CardSeverity, PendingSource, SourceBatch};
use pending_google::{GoogleColumn, GoogleSource, TokenProvider};
use pending_ical::When;
use serde_json::{Value, json};

/// Tokens "t1", "t2"… a cada pedido; `refresh = true` conta à parte.
#[derive(Default)]
struct FakeTokens {
    issued: AtomicUsize,
    refreshes: AtomicUsize,
    fail: bool,
}

impl TokenProvider for FakeTokens {
    fn token(&self, refresh: bool) -> BoxFuture<'_, Result<String, String>> {
        if refresh {
            self.refreshes.fetch_add(1, Ordering::SeqCst);
        }
        let n = self.issued.fetch_add(1, Ordering::SeqCst) + 1;
        let fail = self.fail;
        Box::pin(async move {
            if fail {
                Err("no Google account in GNOME Online Accounts".to_owned())
            } else {
                Ok(format!("t{n}"))
            }
        })
    }
}

#[derive(Clone, Default)]
struct Stub {
    calendars: Arc<Mutex<Value>>,
    events: Arc<Mutex<HashMap<String, Value>>>,
    /// Tokens the API accepts; others get 401.
    valid: Arc<Mutex<Vec<String>>>,
    params: Arc<Mutex<Vec<HashMap<String, String>>>>,
}

fn authorized(stub: &Stub, headers: &HeaderMap) -> bool {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    stub.valid.lock().unwrap().iter().any(|t| t == token)
}

async fn calendar_list(State(stub): State<Stub>, headers: HeaderMap) -> Response {
    if !authorized(&stub, &headers) {
        return (StatusCode::UNAUTHORIZED, "expired").into_response();
    }
    axum::Json(stub.calendars.lock().unwrap().clone()).into_response()
}

async fn events(
    State(stub): State<Stub>,
    headers: HeaderMap,
    Path(calendar): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    if !authorized(&stub, &headers) {
        return (StatusCode::UNAUTHORIZED, "expired").into_response();
    }
    stub.params.lock().unwrap().push(params);
    let events = stub.events.lock().unwrap().get(&calendar).cloned();
    match events {
        Some(items) => axum::Json(json!({ "items": items })).into_response(),
        None => (StatusCode::NOT_FOUND, "no such calendar").into_response(),
    }
}

async fn serve(stub: Stub) -> String {
    let app = Router::new()
        .route("/users/me/calendarList", get(calendar_list))
        .route("/calendars/{calendar}/events", get(events))
        .with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn now() -> DateTime<Utc> {
    // Segunda-feira, 28/09/2026, 12:00 UTC.
    Utc.with_ymd_and_hms(2026, 9, 28, 12, 0, 0).unwrap()
}

fn timed(id: &str, summary: &str, start: &str, end: &str) -> Value {
    json!({
        "id": id,
        "status": "confirmed",
        "summary": summary,
        "htmlLink": format!("https://www.google.com/calendar/event?eid={id}"),
        "start": { "dateTime": start },
        "end": { "dateTime": end },
        "updated": "2026-09-20T10:00:00Z",
    })
}

fn calendars() -> Value {
    json!({ "items": [
        { "id": "me@acme.com", "summary": "Bruno", "primary": true, "selected": true, "accessRole": "owner" },
        { "id": "team@acme.com", "summary": "Team", "selected": true, "accessRole": "reader" },
        { "id": "hidden@acme.com", "summary": "Hidden", "selected": false, "accessRole": "reader" },
    ]})
}

fn stub() -> Stub {
    let stub = Stub::default();
    *stub.calendars.lock().unwrap() = calendars();
    stub.valid.lock().unwrap().push("t1".to_owned());
    stub
}

fn source(base: String, tokens: Arc<FakeTokens>) -> GoogleSource {
    GoogleSource::new(tokens)
        .with_api_url(base)
        .with_clock(now, chrono_tz::UTC)
}

fn columns(batch: &SourceBatch) -> Vec<(String, String)> {
    batch
        .items
        .iter()
        .map(|i| (i.column.clone(), i.card.title.clone()))
        .collect()
}

/// Os eventos das agendas visíveis (primária e compartilhadas marcadas) viram
/// cards por faixa de tempo, com link para o evento; agendas ocultas no Google
/// ficam de fora. A API expande as recorrências (singleEvents).
#[tokio::test]
async fn visible_calendars_become_time_bucket_columns() {
    let stub = stub();
    stub.events.lock().unwrap().insert(
        "me@acme.com".to_owned(),
        json!([timed(
            "e1",
            "Planning",
            "2026-09-28T15:00:00Z",
            "2026-09-28T16:00:00Z"
        )]),
    );
    stub.events.lock().unwrap().insert(
        "team@acme.com".to_owned(),
        json!([{
            "id": "e2",
            "status": "confirmed",
            "summary": "Offsite",
            "htmlLink": "https://www.google.com/calendar/event?eid=e2",
            "start": { "date": "2026-09-29" },
            "end": { "date": "2026-09-30" },
        }]),
    );
    stub.events.lock().unwrap().insert(
        "hidden@acme.com".to_owned(),
        json!([timed(
            "e3",
            "Hidden",
            "2026-09-28T15:00:00Z",
            "2026-09-28T16:00:00Z"
        )]),
    );
    let params = stub.params.clone();
    let base = serve(stub).await;

    let source = source(base, Arc::new(FakeTokens::default()));
    assert_eq!(
        source.columns(),
        vec!["Now", "Today", "Tomorrow", "Next 30 days"]
    );
    let batch = source.refresh().await.expect("refresh succeeds");

    assert_eq!(
        columns(&batch),
        vec![
            ("Today".to_owned(), "Planning".to_owned()),
            ("Tomorrow".to_owned(), "Offsite".to_owned()),
        ]
    );
    let planning = &batch.items[0].card;
    assert_eq!(
        planning.url.as_deref(),
        Some("https://www.google.com/calendar/event?eid=e1")
    );
    assert!(planning.body.contains("Bruno"), "{}", planning.body);
    assert!(
        batch.items[1].card.body.contains("all day"),
        "{}",
        batch.items[1].card.body
    );
    let params = params.lock().unwrap();
    assert!(
        params
            .iter()
            .all(|p| p.get("singleEvents").map(String::as_str) == Some("true"))
    );
}

/// Eventos cancelados ou que eu recusei não aparecem.
#[tokio::test]
async fn cancelled_and_declined_events_are_hidden() {
    let stub = stub();
    let mut declined = timed(
        "d",
        "Declined",
        "2026-09-28T15:00:00Z",
        "2026-09-28T16:00:00Z",
    );
    declined["attendees"] = json!([
        { "email": "me@acme.com", "self": true, "responseStatus": "declined" },
        { "email": "x@acme.com", "responseStatus": "accepted" }
    ]);
    let mut cancelled = timed(
        "c",
        "Cancelled",
        "2026-09-28T15:00:00Z",
        "2026-09-28T16:00:00Z",
    );
    cancelled["status"] = json!("cancelled");
    stub.events.lock().unwrap().insert(
        "me@acme.com".to_owned(),
        json!([
            declined,
            cancelled,
            timed("k", "Kept", "2026-09-28T17:00:00Z", "2026-09-28T18:00:00Z")
        ]),
    );
    stub.events
        .lock()
        .unwrap()
        .insert("team@acme.com".to_owned(), json!([]));
    let base = serve(stub).await;

    let batch = source(base, Arc::new(FakeTokens::default()))
        .refresh()
        .await
        .expect("refresh succeeds");

    assert_eq!(
        columns(&batch),
        vec![("Today".to_owned(), "Kept".to_owned())]
    );
}

/// Colunas configuradas agrupam faixas de tempo e podem filtrar agendas pelo
/// nome.
#[tokio::test]
async fn configured_columns_filter_time_buckets_and_calendars() {
    let stub = stub();
    stub.events.lock().unwrap().insert(
        "me@acme.com".to_owned(),
        json!([timed(
            "e1",
            "Mine",
            "2026-09-28T15:00:00Z",
            "2026-09-28T16:00:00Z"
        )]),
    );
    stub.events.lock().unwrap().insert(
        "team@acme.com".to_owned(),
        json!([timed(
            "e2",
            "Team sync",
            "2026-09-28T13:00:00Z",
            "2026-09-28T14:00:00Z"
        )]),
    );
    let base = serve(stub).await;

    let batch = source(base, Arc::new(FakeTokens::default()))
        .with_columns(vec![
            GoogleColumn {
                name: "Hoje".to_owned(),
                when: vec![When::Now, When::Today],
                calendar: Vec::new(),
            },
            GoogleColumn {
                name: "Time".to_owned(),
                when: Vec::new(),
                calendar: vec!["team".to_owned()],
            },
        ])
        .refresh()
        .await
        .expect("refresh succeeds");

    assert_eq!(
        columns(&batch),
        vec![
            ("Hoje".to_owned(), "Team sync".to_owned()),
            ("Time".to_owned(), "Team sync".to_owned()),
            ("Hoje".to_owned(), "Mine".to_owned()),
        ]
    );
}

/// Com o token expirado, a fonte pede credenciais novas ao GOA e tenta de
/// novo, uma vez.
#[tokio::test]
async fn an_expired_token_is_renewed_once() {
    let stub = stub();
    stub.valid.lock().unwrap().clear();
    stub.valid.lock().unwrap().push("t2".to_owned());
    stub.events
        .lock()
        .unwrap()
        .insert("me@acme.com".to_owned(), json!([]));
    stub.events
        .lock()
        .unwrap()
        .insert("team@acme.com".to_owned(), json!([]));
    let base = serve(stub).await;
    let tokens = Arc::new(FakeTokens::default());

    source(base, tokens.clone())
        .refresh()
        .await
        .expect("second token works");

    assert_eq!(tokens.refreshes.load(Ordering::SeqCst), 1);
}

/// Sem conta no GOA, a fonte explica o que fazer; uma agenda com erro vira
/// aviso e as outras continuam.
#[tokio::test]
async fn missing_account_and_failing_calendars_are_explained() {
    let base = serve(stub()).await;
    let error = source(
        base,
        Arc::new(FakeTokens {
            fail: true,
            ..FakeTokens::default()
        }),
    )
    .refresh()
    .await
    .expect_err("no account");
    assert!(
        error.to_string().contains("GNOME Online Accounts"),
        "{error}"
    );

    let stub = stub();
    stub.events.lock().unwrap().insert(
        "me@acme.com".to_owned(),
        json!([timed(
            "e1",
            "Mine",
            "2026-09-28T15:00:00Z",
            "2026-09-28T16:00:00Z"
        )]),
    );
    // team@acme.com has no events route → 404.
    let base = serve(stub).await;
    let batch = source(base, Arc::new(FakeTokens::default()))
        .refresh()
        .await
        .expect("partial");
    assert_eq!(columns(&batch).len(), 1);
    assert!(
        batch
            .warnings
            .iter()
            .any(|w| w.contains("Team") && w.contains("404")),
        "{:?}",
        batch.warnings
    );
    let _ = CardSeverity::Info;
}

/// O limite de eventos vale por coluna: uma coluna filtrada por agenda não
/// fica vazia só porque outras agendas têm muitos eventos antes.
#[tokio::test]
async fn the_event_limit_applies_per_column_after_the_calendar_filter() {
    let stub = stub();
    let busy: Vec<Value> = (0..30)
        .map(|i| {
            let start = format!("2026-09-{:02}T13:00:00Z", 28 + i % 3);
            let end = format!("2026-09-{:02}T13:30:00Z", 28 + i % 3);
            timed(&format!("b{i}"), &format!("Busy {i}"), &start, &end)
        })
        .collect();
    stub.events
        .lock()
        .unwrap()
        .insert("me@acme.com".to_owned(), json!(busy));
    stub.events.lock().unwrap().insert(
        "team@acme.com".to_owned(),
        json!([timed(
            "t",
            "Team late",
            "2026-10-20T13:00:00Z",
            "2026-10-20T14:00:00Z"
        )]),
    );
    let base = serve(stub).await;

    let batch = source(base, Arc::new(FakeTokens::default()))
        .with_columns(vec![GoogleColumn {
            name: "Team".to_owned(),
            when: Vec::new(),
            calendar: vec!["Team".to_owned()],
        }])
        .refresh()
        .await
        .expect("refresh succeeds");

    assert_eq!(
        columns(&batch),
        vec![("Team".to_owned(), "Team late".to_owned())]
    );
}

/// Uma reunião presente na primária e numa agenda compartilhada aparece uma
/// vez por coluna, e também na coluna filtrada pela agenda compartilhada.
#[tokio::test]
async fn a_shared_meeting_belongs_to_every_calendar_it_is_in() {
    let stub = stub();
    let mut shared = timed(
        "m",
        "Shared",
        "2026-09-28T15:00:00Z",
        "2026-09-28T16:00:00Z",
    );
    shared["iCalUID"] = json!("meeting-1@google.com");
    stub.events
        .lock()
        .unwrap()
        .insert("me@acme.com".to_owned(), json!([shared.clone()]));
    stub.events
        .lock()
        .unwrap()
        .insert("team@acme.com".to_owned(), json!([shared]));
    let base = serve(stub).await;

    let batch = source(base, Arc::new(FakeTokens::default()))
        .with_columns(vec![
            GoogleColumn {
                name: "Tudo".to_owned(),
                when: Vec::new(),
                calendar: Vec::new(),
            },
            GoogleColumn {
                name: "Team".to_owned(),
                when: Vec::new(),
                calendar: vec!["team".to_owned()],
            },
        ])
        .refresh()
        .await
        .expect("refresh succeeds");

    assert_eq!(
        columns(&batch),
        vec![
            ("Tudo".to_owned(), "Shared".to_owned()),
            ("Team".to_owned(), "Shared".to_owned()),
        ]
    );
}

/// Uma reunião que recusei na minha agenda não volta pela cópia de outra
/// agenda em que ela aparece.
#[tokio::test]
async fn a_meeting_declined_in_my_calendar_stays_hidden_everywhere() {
    let stub = stub();
    let mut mine = timed(
        "m",
        "Declined",
        "2026-09-28T15:00:00Z",
        "2026-09-28T16:00:00Z",
    );
    mine["iCalUID"] = json!("meeting-2@google.com");
    mine["attendees"] =
        json!([{ "email": "me@acme.com", "self": true, "responseStatus": "declined" }]);
    let mut theirs = timed(
        "m",
        "Declined",
        "2026-09-28T15:00:00Z",
        "2026-09-28T16:00:00Z",
    );
    theirs["iCalUID"] = json!("meeting-2@google.com");
    theirs["attendees"] =
        json!([{ "email": "team@acme.com", "self": true, "responseStatus": "accepted" }]);
    stub.events
        .lock()
        .unwrap()
        .insert("me@acme.com".to_owned(), json!([mine]));
    stub.events
        .lock()
        .unwrap()
        .insert("team@acme.com".to_owned(), json!([theirs]));
    let base = serve(stub).await;

    let batch = source(base, Arc::new(FakeTokens::default()))
        .refresh()
        .await
        .expect("refresh succeeds");

    assert!(batch.items.is_empty(), "{:?}", columns(&batch));
}

/// Um 403 de cota não é tratado como token inválido: a mensagem diz o motivo
/// que o Google deu, sem mandar reconfigurar a conta.
#[tokio::test]
async fn quota_errors_are_not_reported_as_bad_tokens() {
    let app = Router::new().route(
        "/users/me/calendarList",
        get(|| async {
            (
                StatusCode::FORBIDDEN,
                axum::Json(json!({ "error": { "code": 403, "errors": [{ "reason": "rateLimitExceeded" }] } })),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let tokens = Arc::new(FakeTokens::default());

    let error = source(base, tokens.clone())
        .refresh()
        .await
        .expect_err("quota");

    let message = error.to_string();
    assert!(message.contains("rateLimitExceeded"), "{message}");
    assert!(!message.contains("rejected the access token"), "{message}");
    assert_eq!(
        tokens.refreshes.load(Ordering::SeqCst),
        0,
        "no credential renewal"
    );
}
