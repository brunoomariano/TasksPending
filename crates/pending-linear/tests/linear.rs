//! The Linear source runs one GraphQL `issues` query per stack against a
//! local stub; the real API is never called.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use chrono::{Local, TimeZone, Utc};
use pending_core::{CardSeverity, PendingSource, SourceBatch, SourceError};
use pending_linear::{
    Assignee, LinearColumn, LinearSettings, LinearSource, Priority, StateType, default_columns,
};
use serde_json::{Value, json};

const KEY: &str = "lin_api_secret_key";

#[derive(Clone)]
struct Reply {
    status: StatusCode,
    headers: Vec<(&'static str, String)>,
    body: Value,
    delay: Option<Duration>,
}

fn reply(status: StatusCode, body: Value) -> Reply {
    Reply {
        status,
        headers: Vec::new(),
        body,
        delay: None,
    }
}

/// A successful page of issues.
fn page(nodes: Vec<Value>, next: Option<&str>) -> Reply {
    reply(
        StatusCode::OK,
        json!({ "data": { "issues": {
            "nodes": nodes,
            "pageInfo": { "hasNextPage": next.is_some(), "endCursor": next },
        } } }),
    )
}

fn graphql_error(status: StatusCode, message: &str, code: &str) -> Reply {
    reply(
        status,
        json!({ "errors": [
            { "message": message, "extensions": { "code": code } },
            { "message": "second error" },
        ] }),
    )
}

type Responder = Arc<dyn Fn(&Value) -> Reply + Send + Sync>;

#[derive(Clone)]
struct Stub {
    /// Decides the reply from the request body (`query` and `variables`).
    responder: Responder,
    /// `Authorization` header of every request.
    auth: Arc<Mutex<Vec<String>>>,
    /// Body of every request.
    bodies: Arc<Mutex<Vec<Value>>>,
    /// Requests that reached the redirect target.
    elsewhere: Arc<AtomicUsize>,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
}

impl Stub {
    fn new(responder: impl Fn(&Value) -> Reply + Send + Sync + 'static) -> Self {
        Self {
            responder: Arc::new(responder),
            auth: Arc::default(),
            bodies: Arc::default(),
            elsewhere: Arc::default(),
            in_flight: Arc::default(),
            max_in_flight: Arc::default(),
        }
    }

    fn filters(&self) -> Vec<Value> {
        self.bodies
            .lock()
            .unwrap()
            .iter()
            .map(|body| body["variables"]["filter"].clone())
            .collect()
    }
}

async fn graphql(
    State(stub): State<Stub>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let now = stub.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
    stub.max_in_flight.fetch_max(now, Ordering::SeqCst);
    // Long enough for overlapping requests to be seen.
    tokio::time::sleep(Duration::from_millis(15)).await;

    stub.auth.lock().unwrap().push(
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned(),
    );
    stub.bodies.lock().unwrap().push(body.clone());
    let reply = (stub.responder)(&body);
    if let Some(delay) = reply.delay {
        tokio::time::sleep(delay).await;
    }
    stub.in_flight.fetch_sub(1, Ordering::SeqCst);

    let mut response = (reply.status, axum::Json(reply.body)).into_response();
    for (name, value) in reply.headers {
        response.headers_mut().insert(
            HeaderName::from_static(name),
            HeaderValue::from_str(&value).unwrap(),
        );
    }
    response
}

async fn elsewhere(State(stub): State<Stub>) -> Response {
    stub.elsewhere.fetch_add(1, Ordering::SeqCst);
    axum::Json(json!({})).into_response()
}

async fn serve(stub: Stub) -> String {
    let app = Router::new()
        .route("/graphql", post(graphql))
        .route("/elsewhere", get(elsewhere).post(elsewhere))
        .with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn source(base: &str) -> LinearSource {
    LinearSource::new(Ok(LinearSettings {
        api_url: format!("{base}/graphql"),
        api_key: KEY.to_owned(),
    }))
}

async fn refresh_with(stub: &Stub, columns: Vec<LinearColumn>) -> Result<SourceBatch, SourceError> {
    let base = serve(stub.clone()).await;
    source(&base).with_columns(columns).refresh().await
}

fn column(name: &str) -> LinearColumn {
    LinearColumn {
        name: name.to_owned(),
        ..LinearColumn::default()
    }
}

fn issue(identifier: &str, title: &str) -> Value {
    json!({
        "identifier": identifier,
        "title": title,
        "url": format!("https://linear.app/acme/issue/{identifier}/slug"),
        "priority": 0,
        "dueDate": null,
        "updatedAt": "2026-09-28T10:00:00.000Z",
        "team": { "name": "Engineering" },
        "state": { "name": "In Progress" },
        "assignee": { "displayName": "bruno" },
    })
}

/// The state types a request asks for, e.g. `["started"]`.
fn state_types(body: &Value) -> Vec<String> {
    body["variables"]["filter"]["state"]["type"]["in"]
        .as_array()
        .map(|types| {
            types
                .iter()
                .filter_map(|t| t.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn ids(batch: &SourceBatch) -> Vec<(String, String)> {
    batch
        .items
        .iter()
        .map(|item| (item.column.clone(), item.card.id.clone()))
        .collect()
}

/// Without declared stacks, the source asks for my started, unstarted and
/// backlog issues (one query each) and shows them as In progress, To do and
/// Backlog. The card's title is the issue reference; the issue title, team
/// and state go in the details. The key is sent as the bare `Authorization`
/// value, without `Bearer`.
#[tokio::test]
async fn default_stacks_show_my_open_issues_by_state_type() {
    let stub = Stub::new(|body| match state_types(body).join(",").as_str() {
        "started" => page(vec![issue("ENG-1", "Fix login")], None),
        "unstarted" => page(vec![issue("ENG-2", "Write docs")], None),
        "backlog" => page(vec![issue("OPS-3", "Someday")], None),
        other => panic!("unexpected state types: {other}"),
    });

    let batch = refresh_with(&stub, Vec::new())
        .await
        .expect("refresh succeeds");

    assert_eq!(
        ids(&batch),
        vec![
            ("In progress".to_owned(), "linear:ENG-1".to_owned()),
            ("To do".to_owned(), "linear:ENG-2".to_owned()),
            ("Backlog".to_owned(), "linear:OPS-3".to_owned()),
        ]
    );
    assert!(batch.warnings.is_empty(), "{:?}", batch.warnings);

    let card = &batch.items[0].card;
    assert_eq!(card.title, "ENG-1");
    // My own stack does not repeat my name.
    assert_eq!(card.body, "Fix login · Engineering · In Progress");
    assert_eq!(card.source, "linear");
    assert_eq!(
        card.url.as_deref(),
        Some("https://linear.app/acme/issue/ENG-1/slug")
    );
    assert_eq!(card.severity, CardSeverity::Info);
    assert_eq!(card.due_at, None);
    assert_eq!(card.updated_at.to_rfc3339(), "2026-09-28T10:00:00+00:00");

    let auth = stub.auth.lock().unwrap().clone();
    assert_eq!(auth.len(), 3);
    assert!(auth.iter().all(|value| value == KEY), "{auth:?}");

    for filter in stub.filters() {
        assert_eq!(filter["assignee"], json!({ "isMe": { "eq": true } }));
    }
    let bodies = stub.bodies.lock().unwrap().clone();
    assert_eq!(bodies[0]["variables"]["first"], json!(100));
    assert!(bodies[0]["variables"]["after"].is_null());
    let query = bodies[0]["query"].as_str().unwrap();
    assert!(query.contains("orderBy: updatedAt"), "{query}");
    assert!(default_columns().iter().map(|c| c.name.as_str()).eq([
        "In progress",
        "To do",
        "Backlog"
    ]));
}

/// Every stack key becomes part of the GraphQL filter Linear applies:
/// assignee (me, nobody, somebody else, anyone), state types, state names
/// compared without case, team keys and priorities as Linear's numbers.
#[tokio::test]
async fn stack_filters_become_the_graphql_filter() {
    let stub = Stub::new(|_| page(Vec::new(), None));
    let columns = vec![
        // No state keys: only open state types.
        column("Mine"),
        LinearColumn {
            assignee: Assignee::None,
            state_type: vec![StateType::Triage],
            team: vec!["ENG".to_owned(), "OPS".to_owned()],
            ..column("Triage")
        },
        LinearColumn {
            assignee: Assignee::Others,
            state: vec!["In Review".to_owned(), "QA".to_owned()],
            ..column("Review")
        },
        LinearColumn {
            assignee: Assignee::Any,
            state_type: vec![StateType::Completed, StateType::Canceled],
            state: vec!["Done".to_owned()],
            priority: vec![
                Priority::Urgent,
                Priority::High,
                Priority::Medium,
                Priority::Low,
                Priority::None,
            ],
            ..column("Closed")
        },
    ];

    refresh_with(&stub, columns)
        .await
        .expect("refresh succeeds");

    let mut filters = stub.filters();
    // Requests may arrive in any order.
    filters.sort_by_key(|filter| filter.to_string());
    let mut expected = vec![
        json!({
            "assignee": { "isMe": { "eq": true } },
            "state": { "type": { "in": ["triage", "backlog", "unstarted", "started"] } },
        }),
        json!({
            "assignee": { "null": true },
            "state": { "type": { "in": ["triage"] } },
            "team": { "key": { "in": ["ENG", "OPS"] } },
        }),
        // Naming states picks them in any type.
        json!({
            "assignee": { "null": false, "isMe": { "eq": false } },
            "or": [
                { "state": { "name": { "eqIgnoreCase": "In Review" } } },
                { "state": { "name": { "eqIgnoreCase": "QA" } } },
            ],
        }),
        json!({
            "state": { "type": { "in": ["completed", "canceled"] } },
            "or": [{ "state": { "name": { "eqIgnoreCase": "Done" } } }],
            "priority": { "in": [1, 2, 3, 4, 0] },
        }),
    ];
    expected.sort_by_key(|filter| filter.to_string());
    assert_eq!(filters, expected);
}

/// An overdue issue is critical and says since when; urgent priority is
/// critical and high is a warning. The due date is local midnight, and a
/// stack that is not only mine shows who the issue is assigned to.
#[tokio::test]
async fn priority_and_due_dates_set_severity_and_details() {
    let stub = Stub::new(|_| {
        let mut urgent = issue("ENG-1", "Urgent");
        urgent["priority"] = json!(1);
        let mut high = issue("ENG-2", "High");
        high["priority"] = json!(2.0);
        let mut overdue = issue("ENG-3", "Late");
        overdue["dueDate"] = json!("2020-01-02");
        let mut later = issue("ENG-4", "Later");
        later["dueDate"] = json!("2999-01-02");
        later["priority"] = json!(3);
        later["assignee"] = Value::Null;
        page(vec![urgent, high, overdue, later], None)
    });
    let columns = vec![LinearColumn {
        assignee: Assignee::Any,
        ..column("Everything")
    }];

    let batch = refresh_with(&stub, columns)
        .await
        .expect("refresh succeeds");

    let card = |id: &str| {
        &batch
            .items
            .iter()
            .find(|item| item.card.id == id)
            .unwrap_or_else(|| panic!("no card {id}"))
            .card
    };
    assert_eq!(card("linear:ENG-1").severity, CardSeverity::Critical);
    assert_eq!(card("linear:ENG-2").severity, CardSeverity::Warning);
    assert_eq!(card("linear:ENG-3").severity, CardSeverity::Critical);
    assert_eq!(card("linear:ENG-4").severity, CardSeverity::Info);

    assert_eq!(
        card("linear:ENG-3").body,
        "Late · Engineering · In Progress · @bruno · overdue since 2020-01-02"
    );
    // Unassigned: no name to show.
    assert_eq!(
        card("linear:ENG-4").body,
        "Later · Engineering · In Progress · due 2999-01-02"
    );
    let midnight = Local
        .with_ymd_and_hms(2020, 1, 2, 0, 0, 0)
        .earliest()
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(card("linear:ENG-3").due_at, Some(midnight));
}

/// A stack's issues are read page by page: the next request carries the
/// previous page's `endCursor` as `after`.
#[tokio::test]
async fn pages_are_followed_with_the_end_cursor() {
    let stub = Stub::new(|body| match body["variables"]["after"].as_str() {
        None => page(vec![issue("ENG-1", "First")], Some("cursor-1")),
        Some("cursor-1") => page(vec![issue("ENG-2", "Second")], None),
        Some(other) => panic!("unexpected cursor {other}"),
    });

    let batch = refresh_with(&stub, vec![column("Mine")])
        .await
        .expect("refresh succeeds");

    assert_eq!(
        ids(&batch),
        vec![
            ("Mine".to_owned(), "linear:ENG-1".to_owned()),
            ("Mine".to_owned(), "linear:ENG-2".to_owned()),
        ]
    );
    assert!(batch.warnings.is_empty(), "{:?}", batch.warnings);
    assert_eq!(stub.bodies.lock().unwrap().len(), 2);
}

/// A stack with more pages than the cap stops asking and warns that the
/// list is cut, naming the stack.
#[tokio::test]
async fn too_many_pages_are_cut_with_a_warning() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let stub = Stub::new(move |_| {
        let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
        page(
            vec![issue(&format!("ENG-{n}"), "Many")],
            Some(&format!("cursor-{n}")),
        )
    });

    let batch = refresh_with(&stub, vec![column("Mine")])
        .await
        .expect("refresh succeeds");

    assert_eq!(calls.load(Ordering::SeqCst), 5);
    assert_eq!(batch.items.len(), 5);
    assert_eq!(batch.warnings.len(), 1, "{:?}", batch.warnings);
    assert!(
        batch.warnings[0].starts_with("Mine: more than 500 issues"),
        "{:?}",
        batch.warnings
    );
}

/// When one stack's query fails, the others still show and the failure is a
/// warning naming the stack, with the first GraphQL error message.
#[tokio::test]
async fn one_failing_stack_is_a_warning_naming_it() {
    let stub = Stub::new(|body| match state_types(body).join(",").as_str() {
        "unstarted" => graphql_error(
            StatusCode::BAD_REQUEST,
            "Argument Validation Error",
            "INVALID_INPUT",
        ),
        _ => page(vec![issue("ENG-1", "Works")], None),
    });

    let batch = refresh_with(&stub, Vec::new())
        .await
        .expect("refresh succeeds");

    assert_eq!(
        ids(&batch),
        vec![
            ("In progress".to_owned(), "linear:ENG-1".to_owned()),
            ("Backlog".to_owned(), "linear:ENG-1".to_owned()),
        ]
    );
    assert_eq!(batch.warnings.len(), 1, "{:?}", batch.warnings);
    let warning = &batch.warnings[0];
    assert!(warning.starts_with("To do: "), "{warning}");
    assert!(warning.contains("Argument Validation Error"), "{warning}");
    assert!(!warning.contains("second error"), "{warning}");
}

/// GraphQL can answer 200 with `errors`; that fails the stack too, and when
/// every stack fails the source fails with each stack's reason.
#[tokio::test]
async fn errors_in_a_200_body_fail_the_source_when_every_stack_fails() {
    let stub = Stub::new(|_| graphql_error(StatusCode::OK, "Field does not exist", "GRAPHQL"));

    let error = refresh_with(&stub, Vec::new())
        .await
        .expect_err("every stack failed");

    let message = error.to_string();
    assert!(message.contains("all Linear queries failed"), "{message}");
    for stack in ["In progress", "To do", "Backlog"] {
        assert!(message.contains(&format!("{stack}: ")), "{message}");
    }
    assert!(message.contains("Field does not exist"), "{message}");
    assert_eq!(error.retry_at(), None);
}

/// A rejected key (HTTP 401, or an authentication error code in a 400) says
/// to check `LINEAR_API_KEY`, and never shows the key, even when the server
/// echoes it back.
#[tokio::test]
async fn rejected_keys_point_at_the_variable_without_showing_the_key() {
    for reply in [
        reply(StatusCode::UNAUTHORIZED, json!({})),
        graphql_error(
            StatusCode::BAD_REQUEST,
            &format!("Authentication required, got {KEY}"),
            "AUTHENTICATION_ERROR",
        ),
    ] {
        let stub = Stub::new(move |_| reply.clone());

        let error = refresh_with(&stub, vec![column("Mine")])
            .await
            .expect_err("the key was rejected");

        let message = error.to_string();
        assert!(message.contains("LINEAR_API_KEY"), "{message}");
        assert!(!message.contains(KEY), "{message}");
    }
}

/// Linear reports rate limiting as HTTP 400 with the `RATELIMITED` code; the
/// reset header (epoch milliseconds) becomes the time before which the
/// source is not asked again.
#[tokio::test]
async fn rate_limiting_reports_when_to_retry() {
    let reset = Utc.with_ymd_and_hms(2999, 1, 1, 12, 0, 0).unwrap();
    let stub = Stub::new(move |_| {
        let mut reply = graphql_error(
            StatusCode::BAD_REQUEST,
            "Rate limit exceeded",
            "RATELIMITED",
        );
        reply.headers = vec![
            ("x-ratelimit-requests-remaining", "0".to_owned()),
            (
                "x-ratelimit-requests-reset",
                reset.timestamp_millis().to_string(),
            ),
        ];
        reply
    });

    let error = refresh_with(&stub, Vec::new())
        .await
        .expect_err("rate limited");

    assert_eq!(error.retry_at(), Some(reset));
    let message = error.to_string();
    assert!(message.contains("rate limit"), "{message}");
    assert!(message.contains("2999-01-01T12:00:00"), "{message}");
}

/// The complexity limit has its own reset header; the exhausted limit is the
/// one that says when to retry. Without any header there is no retry time.
#[tokio::test]
async fn rate_limiting_uses_the_exhausted_limit() {
    let requests = Utc.with_ymd_and_hms(2999, 1, 1, 12, 0, 0).unwrap();
    let complexity = Utc.with_ymd_and_hms(2999, 1, 1, 12, 30, 0).unwrap();
    let stub = Stub::new(move |_| {
        let mut reply = graphql_error(StatusCode::BAD_REQUEST, "Too complex", "RATELIMITED");
        reply.headers = vec![
            ("x-ratelimit-requests-remaining", "900".to_owned()),
            (
                "x-ratelimit-requests-reset",
                requests.timestamp_millis().to_string(),
            ),
            ("x-ratelimit-complexity-remaining", "0".to_owned()),
            (
                "x-ratelimit-complexity-reset",
                complexity.timestamp_millis().to_string(),
            ),
        ];
        reply
    });
    let error = refresh_with(&stub, vec![column("Mine")])
        .await
        .expect_err("rate limited");
    assert_eq!(error.retry_at(), Some(complexity));

    let stub = Stub::new(|_| reply(StatusCode::TOO_MANY_REQUESTS, json!({})));
    let error = refresh_with(&stub, vec![column("Mine")])
        .await
        .expect_err("rate limited");
    assert_eq!(error.retry_at(), None);
    assert!(error.to_string().contains("rate limit"), "{error}");
}

/// Without `LINEAR_API_KEY` every refresh fails naming the variable, and no
/// request is made. The API address defaults to Linear's and can be
/// overridden with `LINEAR_API_URL`.
#[tokio::test]
async fn missing_key_fails_naming_the_variable() {
    let settings = LinearSettings::from_env(&|key| match key {
        "LINEAR_API_KEY" => Some("  ".to_owned()),
        _ => None,
    });
    let message = settings.clone().expect_err("no key");
    assert!(message.contains("LINEAR_API_KEY"), "{message}");

    let error = LinearSource::new(settings)
        .refresh()
        .await
        .expect_err("not configured");
    assert!(error.to_string().contains("LINEAR_API_KEY"), "{error}");

    let settings = LinearSettings::from_env(&|key| match key {
        "LINEAR_API_KEY" => Some(format!(" {KEY}\n")),
        _ => None,
    })
    .expect("configured");
    assert_eq!(settings.api_url, "https://api.linear.app/graphql");
    assert_eq!(settings.api_key, KEY);
    assert!(!format!("{settings:?}").contains(KEY));

    let settings = LinearSettings::from_env(&|key| match key {
        "LINEAR_API_KEY" => Some(KEY.to_owned()),
        "LINEAR_API_URL" => Some("http://127.0.0.1:9/graphql".to_owned()),
        _ => None,
    })
    .expect("configured");
    assert_eq!(settings.api_url, "http://127.0.0.1:9/graphql");
}

/// A redirect is not followed, so the key never reaches another address; the
/// error says so.
#[tokio::test]
async fn redirects_are_not_followed() {
    let stub = Stub::new(|_| {
        let mut reply = reply(StatusCode::TEMPORARY_REDIRECT, json!({}));
        reply.headers = vec![("location", "/elsewhere".to_owned())];
        reply
    });

    let error = refresh_with(&stub, vec![column("Mine")])
        .await
        .expect_err("redirected");

    assert_eq!(stub.elsewhere.load(Ordering::SeqCst), 0);
    let message = error.to_string();
    assert!(message.contains("redirects are not followed"), "{message}");
    assert!(message.contains("LINEAR_API_URL"), "{message}");
}

/// A stack whose request hangs is given up after the per-request timeout
/// and becomes a warning; the other stacks still show.
#[tokio::test]
async fn a_hanging_stack_times_out_as_a_warning() {
    let stub = Stub::new(|body| match state_types(body).join(",").as_str() {
        "backlog" => {
            let mut reply = page(Vec::new(), None);
            reply.delay = Some(Duration::from_secs(30));
            reply
        }
        _ => page(vec![issue("ENG-1", "Works")], None),
    });
    let base = serve(stub.clone()).await;

    let batch = source(&base)
        .with_request_timeout(Duration::from_millis(200))
        .refresh()
        .await
        .expect("refresh succeeds");

    assert_eq!(batch.items.len(), 2);
    assert_eq!(batch.warnings.len(), 1, "{:?}", batch.warnings);
    assert!(
        batch.warnings[0].starts_with("Backlog: timed out"),
        "{:?}",
        batch.warnings
    );
}

/// Stacks are queried a few at a time, never all at once, and keep their
/// configured order in the result.
#[tokio::test]
async fn stacks_run_with_a_small_concurrency_limit() {
    let stub = Stub::new(|body| {
        let team = body["variables"]["filter"]["team"]["key"]["in"][0]
            .as_str()
            .unwrap()
            .to_owned();
        page(vec![issue(&format!("{team}-1"), "One")], None)
    });
    let columns: Vec<LinearColumn> = (0..8)
        .map(|n| LinearColumn {
            team: vec![format!("T{n}")],
            ..column(&format!("Stack {n}"))
        })
        .collect();

    let batch = refresh_with(&stub, columns)
        .await
        .expect("refresh succeeds");

    let max = stub.max_in_flight.load(Ordering::SeqCst);
    assert!((2..=3).contains(&max), "{max} requests at once");
    let expected: Vec<(String, String)> = (0..8)
        .map(|n| (format!("Stack {n}"), format!("linear:T{n}-1")))
        .collect();
    assert_eq!(ids(&batch), expected);
}

/// A reply that is not the expected JSON, or an issue without a reference,
/// never panics: the first fails the stack, the second is skipped.
#[tokio::test]
async fn unexpected_replies_are_handled() {
    let stub = Stub::new(|_| reply(StatusCode::OK, json!({ "data": null })));
    let error = refresh_with(&stub, vec![column("Mine")])
        .await
        .expect_err("no data");
    assert!(
        error.to_string().contains("unexpected Linear response"),
        "{error}"
    );

    let stub = Stub::new(|_| reply(StatusCode::BAD_GATEWAY, json!("oops")));
    let error = refresh_with(&stub, vec![column("Mine")])
        .await
        .expect_err("bad gateway");
    assert!(error.to_string().contains("502"), "{error}");

    let stub = Stub::new(|_| {
        let mut nameless = issue("", "No reference");
        nameless["identifier"] = Value::Null;
        let mut bare = json!({ "identifier": "ENG-9", "title": "Bare" });
        bare["updatedAt"] = Value::Null;
        page(vec![nameless, bare], None)
    });
    let batch = refresh_with(&stub, vec![column("Mine")])
        .await
        .expect("refresh succeeds");
    assert_eq!(
        ids(&batch),
        vec![("Mine".to_owned(), "linear:ENG-9".to_owned())]
    );
    assert_eq!(batch.items[0].card.body, "Bare");
    assert_eq!(batch.items[0].card.url, None);
}

/// Stack filters are read from the config's keys; unknown keys and invalid
/// values are rejected, and blank names are refused by `validate`.
#[test]
fn stack_filters_parse_and_validate() {
    let parsed: LinearColumn = serde_json::from_value(json!({
        "assignee": "others",
        "state_type": ["triage", "canceled"],
        "state": ["In Review"],
        "team": ["ENG"],
        "priority": ["urgent", "none"],
    }))
    .expect("valid filter");
    assert_eq!(parsed.assignee, Assignee::Others);
    assert_eq!(
        parsed.state_type,
        vec![StateType::Triage, StateType::Canceled]
    );
    assert_eq!(parsed.priority, vec![Priority::Urgent, Priority::None]);
    assert!(parsed.validate().is_ok());

    for bad in [
        json!({ "assigne": "me" }),
        json!({ "assignee": "somebody" }),
        json!({ "state_type": ["cancelled"] }),
        json!({ "priority": ["p1"] }),
    ] {
        assert!(
            serde_json::from_value::<LinearColumn>(bad.clone()).is_err(),
            "{bad}"
        );
    }

    for blank in [json!({ "state": [" "] }), json!({ "team": [""] })] {
        let column: LinearColumn = serde_json::from_value(blank.clone()).unwrap();
        assert!(column.validate().is_err(), "{blank}");
    }
}
