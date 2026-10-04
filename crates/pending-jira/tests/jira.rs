//! The Jira source runs one JQL query per stack, against a local stub server.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use pending_core::{CardSeverity, PendingCard, PendingSource, SourceBatch, SourceError};
use pending_jira::{JiraAuth, JiraColumn, JiraSettings, JiraSource, parse_timestamp};
use serde_json::{Value, json};

const EMAIL: &str = "me@example.com";
const API_TOKEN: &str = "cloud_secret_token";
const PAT: &str = "datacenter_secret_pat";
/// `me@example.com:cloud_secret_token` in base64.
const BASIC: &str = "Basic bWVAZXhhbXBsZS5jb206Y2xvdWRfc2VjcmV0X3Rva2Vu";

const CLOUD_PATH: &str = "/rest/api/3/search/jql";
const DATA_CENTER_PATH: &str = "/rest/api/2/search";

const IN_PROGRESS: &str =
    r#"assignee = currentUser() AND statusCategory = "In Progress" ORDER BY updated DESC"#;
const TO_DO: &str =
    r#"assignee = currentUser() AND statusCategory = "To Do" ORDER BY priority DESC, updated DESC"#;

#[derive(Clone, Default)]
struct Stub {
    /// Replies per JQL, served in order: the first request gets the first.
    pages: Arc<Mutex<HashMap<String, Vec<Reply>>>>,
    /// JQL queries whose requests hang.
    slow: Arc<Mutex<Vec<String>>>,
    /// How long each reply to a JQL query takes.
    delays: Arc<Mutex<HashMap<String, Duration>>>,
    seen: Arc<Mutex<Vec<Seen>>>,
    /// Requests that reached the redirect target.
    elsewhere: Arc<Mutex<Vec<String>>>,
    /// Requests being served right now, and the most seen at once.
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct Seen {
    path: &'static str,
    authorization: String,
    params: HashMap<String, String>,
}

#[derive(Clone)]
struct Reply {
    status: StatusCode,
    body: Value,
    headers: Vec<(&'static str, String)>,
}

fn ok(body: Value) -> Reply {
    Reply {
        status: StatusCode::OK,
        body,
        headers: Vec::new(),
    }
}

fn status(status: StatusCode, body: Value) -> Reply {
    Reply {
        status,
        body,
        headers: Vec::new(),
    }
}

impl Reply {
    fn header(mut self, name: &'static str, value: &str) -> Self {
        self.headers.push((name, value.to_owned()));
        self
    }
}

impl Stub {
    fn reply(&self, jql: &str, replies: Vec<Reply>) {
        self.pages.lock().unwrap().insert(jql.to_owned(), replies);
    }

    fn issues(&self, jql: &str, issues: Vec<Value>) {
        self.reply(jql, vec![ok(json!({ "issues": issues }))]);
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

/// Counts a request as in flight until dropped.
struct InFlight(Arc<AtomicUsize>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn search(
    path: &'static str,
    stub: Stub,
    headers: HeaderMap,
    params: HashMap<String, String>,
) -> Response {
    let now = stub.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
    stub.max_in_flight.fetch_max(now, Ordering::SeqCst);
    let _guard = InFlight(stub.in_flight.clone());
    // Long enough for overlapping requests to be seen.
    tokio::time::sleep(Duration::from_millis(15)).await;

    let jql = params.get("jql").cloned().unwrap_or_default();
    let index = {
        let mut seen = stub.seen.lock().unwrap();
        let index = seen
            .iter()
            .filter(|earlier| earlier.params.get("jql") == Some(&jql))
            .count();
        seen.push(Seen {
            path,
            authorization: headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned(),
            params,
        });
        index
    };
    if stub.slow.lock().unwrap().contains(&jql) {
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
    let delay = stub.delays.lock().unwrap().get(&jql).copied();
    if let Some(delay) = delay {
        tokio::time::sleep(delay).await;
    }

    let reply = stub
        .pages
        .lock()
        .unwrap()
        .get(&jql)
        .and_then(|replies| replies.get(index).cloned())
        .unwrap_or_else(|| ok(json!({ "issues": [], "total": 0 })));
    let mut response = (reply.status, axum::Json(reply.body)).into_response();
    for (name, value) in reply.headers {
        response.headers_mut().insert(
            HeaderName::from_static(name),
            HeaderValue::from_str(&value).unwrap(),
        );
    }
    response
}

async fn cloud(
    State(stub): State<Stub>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    search(CLOUD_PATH, stub, headers, params).await
}

async fn data_center(
    State(stub): State<Stub>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    search(DATA_CENTER_PATH, stub, headers, params).await
}

async fn elsewhere(State(stub): State<Stub>, headers: HeaderMap) -> Response {
    stub.elsewhere.lock().unwrap().push(
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned(),
    );
    axum::Json(json!({ "issues": [] })).into_response()
}

async fn serve(stub: Stub) -> String {
    let app = Router::new()
        .route(CLOUD_PATH, get(cloud))
        .route(DATA_CENTER_PATH, get(data_center))
        .route("/elsewhere", get(elsewhere))
        .with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()
}

fn cloud_source(base: String) -> JiraSource {
    JiraSource::new(Ok(JiraSettings {
        base_url: base,
        auth: JiraAuth::Cloud {
            email: EMAIL.to_owned(),
            api_token: API_TOKEN.to_owned(),
        },
    }))
    .with_today(today)
}

fn data_center_source(base: String) -> JiraSource {
    JiraSource::new(Ok(JiraSettings {
        base_url: base,
        auth: JiraAuth::DataCenter {
            token: PAT.to_owned(),
        },
    }))
    .with_today(today)
}

fn column(name: &str, jql: &str) -> JiraColumn {
    JiraColumn {
        name: name.to_owned(),
        jql: jql.to_owned(),
        severity: None,
    }
}

fn issue(key: &str, summary: &str) -> Value {
    json!({
        "id": "10001",
        "key": key,
        "fields": {
            "summary": summary,
            "status": { "name": "In Progress" },
            "priority": { "name": "Medium" },
            "assignee": { "displayName": "Ana Souza" },
            "duedate": null,
            "updated": "2026-09-28T10:00:00.000+0000",
            "project": { "key": "PROJ", "name": "Platform" },
        },
    })
}

fn with(mut issue: Value, field: &str, value: Value) -> Value {
    issue["fields"][field] = value;
    issue
}

fn ids(batch: &SourceBatch) -> Vec<(String, String)> {
    batch
        .items
        .iter()
        .map(|item| (item.column.clone(), item.card.id.clone()))
        .collect()
}

fn card<'a>(batch: &'a SourceBatch, id: &str) -> &'a PendingCard {
    &batch
        .items
        .iter()
        .find(|item| item.card.id == id)
        .unwrap_or_else(|| panic!("no card {id}"))
        .card
}

fn local_midnight(year: i32, month: u32, day: u32) -> DateTime<Utc> {
    Local
        .with_ymd_and_hms(year, month, day, 0, 0, 0)
        .earliest()
        .unwrap()
        .with_timezone(&Utc)
}

fn assert_no_secret(text: &str) {
    for secret in [API_TOKEN, PAT, EMAIL, "bWVAZXhhbXBsZS5jb20"] {
        assert!(!text.contains(secret), "leaked {secret}: {text}");
    }
}

/// Without configured stacks, the source shows "In progress" and "To do":
/// the issues assigned to the current user, by status category.
#[tokio::test]
async fn default_stacks_show_my_issues_by_status_category() {
    let stub = Stub::default();
    stub.issues(IN_PROGRESS, vec![issue("PROJ-1", "Fix login")]);
    stub.issues(
        TO_DO,
        vec![issue("PROJ-2", "Write docs"), issue("OPS-7", "Rotate keys")],
    );
    let source = cloud_source(serve(stub.clone()).await);

    assert_eq!(source.columns(), ["In progress", "To do"]);
    let batch = source.refresh().await.expect("refresh");

    assert_eq!(batch.warnings, Vec::<String>::new());
    assert_eq!(
        ids(&batch),
        [
            ("In progress".to_owned(), "jira:PROJ-1".to_owned()),
            ("To do".to_owned(), "jira:PROJ-2".to_owned()),
            ("To do".to_owned(), "jira:OPS-7".to_owned()),
        ]
    );
}

/// Jira Cloud (e-mail and API token): the enhanced JQL search endpoint, HTTP
/// Basic auth, 100 issues per page and only the fields the cards use.
#[tokio::test]
async fn cloud_uses_the_enhanced_search_with_basic_auth() {
    let stub = Stub::default();
    let source = cloud_source(serve(stub.clone()).await)
        .with_columns(vec![column("Mine", "assignee = currentUser()")]);

    source.refresh().await.expect("refresh");

    let seen = stub.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, CLOUD_PATH);
    assert_eq!(seen[0].authorization, BASIC);
    assert_eq!(seen[0].params["jql"], "assignee = currentUser()");
    assert_eq!(seen[0].params["maxResults"], "100");
    assert_eq!(
        seen[0].params["fields"],
        "summary,status,priority,assignee,duedate,updated,project"
    );
    assert!(!seen[0].params.contains_key("nextPageToken"));
    assert!(!seen[0].params.contains_key("startAt"));
}

/// Jira Data Center (personal access token): the v2 search endpoint with a
/// Bearer token, paged with `startAt` until `total` is reached.
#[tokio::test]
async fn data_center_uses_the_v2_search_with_a_bearer_token() {
    let stub = Stub::default();
    stub.reply(
        "project = OPS",
        vec![
            ok(json!({
                "startAt": 0, "maxResults": 2, "total": 3,
                "issues": [issue("OPS-1", "One"), issue("OPS-2", "Two")],
            })),
            ok(json!({
                "startAt": 2, "maxResults": 2, "total": 3,
                "issues": [issue("OPS-3", "Three")],
            })),
        ],
    );
    let base = serve(stub.clone()).await;
    let source =
        data_center_source(base.clone()).with_columns(vec![column("Ops", "project = OPS")]);

    let batch = source.refresh().await.expect("refresh");

    assert_eq!(batch.warnings, Vec::<String>::new());
    assert_eq!(batch.items.len(), 3);
    assert_eq!(
        card(&batch, "jira:OPS-3").url.as_deref(),
        Some(format!("{base}/browse/OPS-3").as_str())
    );
    let seen = stub.seen();
    assert_eq!(seen.len(), 2);
    for request in &seen {
        assert_eq!(request.path, DATA_CENTER_PATH);
        assert_eq!(request.authorization, format!("Bearer {PAT}"));
        assert_eq!(request.params["maxResults"], "100");
        assert_eq!(
            request.params["fields"],
            "summary,status,priority,assignee,duedate,updated,project"
        );
        assert!(!request.params.contains_key("nextPageToken"));
    }
    assert_eq!(seen[0].params["startAt"], "0");
    assert_eq!(seen[1].params["startAt"], "2");
}

/// A card shows the issue key as its title, then the summary, project,
/// status, assignee and due date; it links to the issue's page.
#[tokio::test]
async fn cards_show_the_issue_key_and_its_details() {
    let stub = Stub::default();
    stub.issues(
        IN_PROGRESS,
        vec![
            with(
                issue("PROJ-123", "Fix login"),
                "duedate",
                json!("2026-10-05"),
            ),
            with(
                with(issue("PROJ-124", "No owner"), "assignee", Value::Null),
                "priority",
                Value::Null,
            ),
        ],
    );
    let base = serve(stub).await;

    let batch = cloud_source(format!("{base}/"))
        .refresh()
        .await
        .expect("refresh");

    let full = card(&batch, "jira:PROJ-123");
    assert_eq!(full.title, "PROJ-123");
    assert_eq!(
        full.body,
        "Fix login · Platform · In Progress · @Ana Souza · due 2026-10-05"
    );
    assert_eq!(full.source, "jira");
    assert_eq!(
        full.url.as_deref(),
        Some(format!("{base}/browse/PROJ-123").as_str())
    );
    assert_eq!(full.due_at, Some(local_midnight(2026, 10, 5)));
    assert_eq!(
        full.updated_at,
        Utc.with_ymd_and_hms(2026, 9, 28, 10, 0, 0).unwrap()
    );
    assert_eq!(full.severity, CardSeverity::Info);

    let bare = card(&batch, "jira:PROJ-124");
    assert_eq!(bare.body, "No owner · Platform · In Progress");
    assert_eq!(bare.due_at, None);
    assert_eq!(bare.severity, CardSeverity::Info);
}

/// Overdue issues and the highest priorities are critical, high priorities
/// are warnings, the rest is info.
#[tokio::test]
async fn severity_follows_the_due_date_and_the_priority() {
    let priority =
        |key: &str, name: &str| with(issue(key, "x"), "priority", json!({ "name": name }));
    let stub = Stub::default();
    stub.issues(
        IN_PROGRESS,
        vec![
            with(issue("P-1", "Late"), "duedate", json!("2026-09-30")),
            with(issue("P-2", "Today"), "duedate", json!("2026-10-01")),
            priority("P-3", "Highest"),
            priority("P-4", "Blocker"),
            priority("P-5", "critical"),
            priority("P-6", "High"),
            priority("P-7", "Major"),
            priority("P-8", "Low"),
        ],
    );

    let batch = cloud_source(serve(stub).await)
        .refresh()
        .await
        .expect("refresh");

    let severity = |key: &str| card(&batch, &format!("jira:{key}")).severity;
    assert_eq!(severity("P-1"), CardSeverity::Critical);
    assert!(
        card(&batch, "jira:P-1")
            .body
            .ends_with("overdue since 2026-09-30")
    );
    assert_eq!(severity("P-2"), CardSeverity::Info);
    assert_eq!(severity("P-3"), CardSeverity::Critical);
    assert_eq!(severity("P-4"), CardSeverity::Critical);
    assert_eq!(severity("P-5"), CardSeverity::Critical);
    assert_eq!(severity("P-6"), CardSeverity::Warning);
    assert_eq!(severity("P-7"), CardSeverity::Warning);
    assert_eq!(severity("P-8"), CardSeverity::Info);
}

/// A stack's `severity` replaces what the due date and priority would say.
#[tokio::test]
async fn stack_severity_overrides_the_computed_one() {
    let stub = Stub::default();
    stub.issues(
        "watcher = currentUser()",
        vec![
            with(issue("P-1", "Late"), "duedate", json!("2026-09-30")),
            issue("P-2", "Plain"),
        ],
    );
    let source = cloud_source(serve(stub).await).with_columns(vec![JiraColumn {
        severity: Some(CardSeverity::Warning),
        ..column("Watching", "watcher = currentUser()")
    }]);

    let batch = source.refresh().await.expect("refresh");

    assert_eq!(card(&batch, "jira:P-1").severity, CardSeverity::Warning);
    assert_eq!(card(&batch, "jira:P-2").severity, CardSeverity::Warning);
}

/// On Jira Cloud, more results are read by sending back `nextPageToken`
/// until a page comes without one.
#[tokio::test]
async fn cloud_follows_the_next_page_token() {
    let stub = Stub::default();
    stub.reply(
        "project = OPS",
        vec![
            ok(json!({ "issues": [issue("OPS-1", "One")], "nextPageToken": "tok-2", "isLast": false })),
            ok(json!({ "issues": [issue("OPS-2", "Two")], "nextPageToken": "tok-3" })),
            ok(json!({ "issues": [issue("OPS-3", "Three")], "isLast": true })),
        ],
    );
    let source =
        cloud_source(serve(stub.clone()).await).with_columns(vec![column("Ops", "project = OPS")]);

    let batch = source.refresh().await.expect("refresh");

    assert_eq!(batch.warnings, Vec::<String>::new());
    assert_eq!(batch.items.len(), 3);
    let tokens: Vec<Option<String>> = stub
        .seen()
        .iter()
        .map(|request| request.params.get("nextPageToken").cloned())
        .collect();
    assert_eq!(
        tokens,
        [None, Some("tok-2".to_owned()), Some("tok-3".to_owned())]
    );
}

/// A query with more pages than the cap shows the first ones and warns,
/// naming the stack, instead of reading forever.
#[tokio::test]
async fn too_many_pages_are_cut_short_with_a_warning() {
    let stub = Stub::default();
    let pages = (0..20)
        .map(|n| {
            ok(json!({
                "issues": [issue(&format!("OPS-{n}"), "x")],
                "nextPageToken": format!("tok-{n}"),
            }))
        })
        .collect();
    stub.reply("project = OPS", pages);
    let source =
        cloud_source(serve(stub.clone()).await).with_columns(vec![column("Ops", "project = OPS")]);

    let batch = source.refresh().await.expect("refresh");

    assert_eq!(stub.seen().len(), 5);
    assert_eq!(batch.items.len(), 5);
    assert_eq!(batch.warnings.len(), 1, "{:?}", batch.warnings);
    assert!(
        batch.warnings[0].starts_with("Ops: more than 5 pages"),
        "{:?}",
        batch.warnings
    );
}

/// Jira Data Center pages by offset: each request starts after the issues
/// already read, and the same cap of pages applies, with the warning.
#[tokio::test]
async fn data_center_pages_by_offset_up_to_the_cap() {
    let stub = Stub::default();
    let pages = (0..20)
        .map(|n| ok(json!({ "issues": [issue(&format!("OPS-{n}"), "x")], "total": 20 })))
        .collect();
    stub.reply("project = OPS", pages);
    let source = data_center_source(serve(stub.clone()).await)
        .with_columns(vec![column("Ops", "project = OPS")]);

    let batch = source.refresh().await.expect("refresh");

    let offsets: Vec<Option<String>> = stub
        .seen()
        .iter()
        .map(|request| request.params.get("startAt").cloned())
        .collect();
    let expected: Vec<Option<String>> = (0..5).map(|n| Some(n.to_string())).collect();
    assert_eq!(offsets, expected);
    assert_eq!(batch.items.len(), 5);
    assert_eq!(batch.warnings.len(), 1, "{:?}", batch.warnings);
    assert!(
        batch.warnings[0].starts_with("Ops: more than 5 pages"),
        "{:?}",
        batch.warnings
    );

    // A list that ends within the cap has no warning.
    let stub = Stub::default();
    stub.reply(
        "project = OPS",
        vec![
            ok(json!({ "issues": [issue("OPS-1", "One")], "total": 2 })),
            ok(json!({ "issues": [issue("OPS-2", "Two")], "total": 2 })),
        ],
    );
    let source = data_center_source(serve(stub.clone()).await)
        .with_columns(vec![column("Ops", "project = OPS")]);
    let batch = source.refresh().await.expect("refresh");
    assert_eq!(batch.items.len(), 2);
    assert_eq!(batch.warnings, Vec::<String>::new());
    assert_eq!(stub.seen().len(), 2);
}

/// A Cloud page that hands back the token it was asked with would be read
/// forever: the stack stops there and says the list is cut short.
#[tokio::test]
async fn a_repeated_page_token_stops_the_stack() {
    let stub = Stub::default();
    let same = |key: &str| ok(json!({ "issues": [issue(key, "x")], "nextPageToken": "tok" }));
    stub.reply(
        "project = OPS",
        vec![same("OPS-1"), same("OPS-2"), same("OPS-3"), same("OPS-4")],
    );
    let source =
        cloud_source(serve(stub.clone()).await).with_columns(vec![column("Ops", "project = OPS")]);

    let batch = source.refresh().await.expect("refresh");

    assert_eq!(stub.seen().len(), 2);
    assert_eq!(batch.items.len(), 2);
    assert_eq!(batch.warnings.len(), 1, "{:?}", batch.warnings);
    assert!(
        batch.warnings[0].starts_with("Ops: "),
        "{:?}",
        batch.warnings
    );
}

/// Invalid JQL in one stack becomes a warning naming the stack, with the
/// server's explanation; the other stacks still show.
#[tokio::test]
async fn invalid_jql_becomes_a_warning_with_the_server_message() {
    let stub = Stub::default();
    stub.issues("project = OPS", vec![issue("OPS-1", "One")]);
    stub.reply(
        "nonsense =",
        vec![status(
            StatusCode::BAD_REQUEST,
            json!({
                "errorMessages": ["Error in the JQL Query: Expecting a value at line 1."],
                "errors": {},
            }),
        )],
    );
    let source = cloud_source(serve(stub).await).with_columns(vec![
        column("Ops", "project = OPS"),
        column("Broken", "nonsense ="),
    ]);

    let batch = source.refresh().await.expect("partial refresh");

    assert_eq!(ids(&batch), [("Ops".to_owned(), "jira:OPS-1".to_owned())]);
    assert_eq!(
        batch.warnings,
        [
            "Broken: Jira API returned 400 Bad Request: Error in the JQL Query: Expecting a value at line 1."
        ]
    );
}

/// When every stack fails, the source fails; a rejected login explains which
/// credentials each Jira flavour takes, without showing them.
#[tokio::test]
async fn rejected_credentials_fail_the_source_with_a_hint() {
    for data_center in [false, true] {
        let stub = Stub::default();
        for jql in [IN_PROGRESS, TO_DO] {
            stub.reply(jql, vec![status(StatusCode::UNAUTHORIZED, json!({}))]);
        }
        let base = serve(stub).await;
        let source = if data_center {
            data_center_source(base)
        } else {
            cloud_source(base)
        };

        let error = source.refresh().await.expect_err("all stacks failed");

        let message = error.to_string();
        assert!(
            message.starts_with("all Jira queries failed; "),
            "{message}"
        );
        assert!(message.contains("In progress: "), "{message}");
        assert!(message.contains("To do: "), "{message}");
        assert!(message.contains("401"), "{message}");
        assert!(message.contains("JIRA_EMAIL"), "{message}");
        assert!(message.contains("JIRA_API_TOKEN"), "{message}");
        assert!(message.contains("JIRA_TOKEN"), "{message}");
        assert_eq!(error.retry_at(), None);
        assert_no_secret(&message);
    }
}

/// Jira Data Center answers 200 as an anonymous user when the token is
/// rejected, flagging it in a header; that is a login failure, not an empty
/// list.
#[tokio::test]
async fn a_login_failure_flagged_in_a_header_is_not_an_empty_list() {
    let stub = Stub::default();
    stub.reply(
        "project = OPS",
        vec![
            ok(json!({ "startAt": 0, "maxResults": 100, "total": 0, "issues": [] }))
                .header("x-seraph-loginreason", "AUTHENTICATED_FAILED"),
        ],
    );
    let source =
        data_center_source(serve(stub).await).with_columns(vec![column("Ops", "project = OPS")]);

    let error = source.refresh().await.expect_err("login failed");

    let message = error.to_string();
    assert!(message.contains("rejected the credentials"), "{message}");
    assert!(message.contains("JIRA_TOKEN"), "{message}");
    assert_no_secret(&message);
}

/// A 403 is reported with the server's message.
#[tokio::test]
async fn forbidden_is_reported_with_the_server_message() {
    let stub = Stub::default();
    stub.issues("project = OPS", vec![issue("OPS-1", "One")]);
    stub.reply(
        "project = SECRET",
        vec![status(
            StatusCode::FORBIDDEN,
            json!({ "errorMessages": ["You do not have permission."] }),
        )],
    );
    let source = cloud_source(serve(stub).await).with_columns(vec![
        column("Ops", "project = OPS"),
        column("Secret", "project = SECRET"),
    ]);

    let batch = source.refresh().await.expect("partial refresh");

    assert_eq!(batch.items.len(), 1);
    assert_eq!(batch.warnings.len(), 1);
    let warning = &batch.warnings[0];
    assert!(
        warning.starts_with("Secret: Jira API returned 403 Forbidden"),
        "{warning}"
    );
    assert!(warning.contains("You do not have permission."), "{warning}");
}

/// A rate limit is explained as such, and `Retry-After` (seconds) becomes
/// the time before which the source is not asked again.
#[tokio::test]
async fn rate_limit_is_reported_with_the_retry_time() {
    let stub = Stub::default();
    stub.reply(
        "project = OPS",
        vec![status(StatusCode::TOO_MANY_REQUESTS, json!({})).header("retry-after", "120")],
    );
    let source = cloud_source(serve(stub).await).with_columns(vec![column("Ops", "project = OPS")]);

    let before = Utc::now();
    let error = source.refresh().await.expect_err("rate limited");
    let after = Utc::now();

    let message = error.to_string();
    assert!(message.contains("rate limit"), "{message}");
    let retry_at = error.retry_at().expect("retry time");
    assert!(
        retry_at >= before + chrono::Duration::seconds(120),
        "{retry_at}"
    );
    assert!(
        retry_at <= after + chrono::Duration::seconds(120),
        "{retry_at}"
    );
}

/// A rate limit on one stack among working ones is that stack's warning,
/// naming the time the server asked for; the other stacks show, and the
/// source is asked again at its usual interval.
#[tokio::test]
async fn a_rate_limit_on_one_stack_is_a_warning() {
    let stub = Stub::default();
    stub.reply(
        "project = OPS",
        vec![status(StatusCode::TOO_MANY_REQUESTS, json!({})).header("retry-after", "120")],
    );
    stub.issues("project = WEB", vec![issue("WEB-1", "One")]);
    let source = cloud_source(serve(stub).await).with_columns(vec![
        column("Ops", "project = OPS"),
        column("Web", "project = WEB"),
    ]);

    let batch = source.refresh().await.expect("partial refresh");

    assert_eq!(batch.items.len(), 1);
    assert_eq!(batch.warnings.len(), 1, "{:?}", batch.warnings);
    assert!(
        batch.warnings[0].starts_with("Ops: ") && batch.warnings[0].contains("rate limit"),
        "{:?}",
        batch.warnings
    );
}

/// `Retry-After` may also be an HTTP date; without the header there is no
/// retry time, only the explanation.
#[tokio::test]
async fn rate_limit_reads_a_date_and_tolerates_a_missing_header() {
    let stub = Stub::default();
    stub.reply(
        "project = OPS",
        vec![
            status(StatusCode::TOO_MANY_REQUESTS, json!({}))
                .header("retry-after", "Wed, 21 Oct 2026 07:28:00 GMT"),
            status(StatusCode::TOO_MANY_REQUESTS, json!({})),
        ],
    );
    let source = cloud_source(serve(stub).await).with_columns(vec![column("Ops", "project = OPS")]);

    let dated = source.refresh().await.expect_err("rate limited");
    assert_eq!(
        dated.retry_at(),
        Some(Utc.with_ymd_and_hms(2026, 10, 21, 7, 28, 0).unwrap())
    );

    let bare = source.refresh().await.expect_err("rate limited");
    assert!(bare.to_string().contains("rate limit"), "{bare}");
    assert_eq!(bare.retry_at(), None);
}

/// The credentials are never sent to another address through a redirect.
#[tokio::test]
async fn redirects_are_not_followed_with_the_credentials() {
    let stub = Stub::default();
    stub.reply(
        "project = OPS",
        vec![status(StatusCode::FOUND, json!({})).header("location", "/elsewhere")],
    );
    let source =
        cloud_source(serve(stub.clone()).await).with_columns(vec![column("Ops", "project = OPS")]);

    let error = source.refresh().await.expect_err("redirect refused");

    assert!(stub.elsewhere.lock().unwrap().is_empty());
    let message = error.to_string();
    assert!(message.contains("302"), "{message}");
    assert!(message.contains("JIRA_BASE_URL"), "{message}");
}

/// A hanging query hits its own timeout and becomes a warning; the other
/// stacks still show.
#[tokio::test]
async fn a_slow_query_becomes_a_warning_after_its_timeout() {
    let stub = Stub::default();
    stub.issues("project = OPS", vec![issue("OPS-1", "One")]);
    stub.slow.lock().unwrap().push("project = SLOW".to_owned());
    let source = cloud_source(serve(stub).await)
        .with_request_timeout(Duration::from_millis(300))
        .with_columns(vec![
            column("Ops", "project = OPS"),
            column("Slow", "project = SLOW"),
        ]);

    let batch = source.refresh().await.expect("partial refresh");

    assert_eq!(batch.items.len(), 1);
    assert_eq!(batch.warnings, ["Slow: timed out after 300ms"]);
}

/// Each stack also has a time budget for all its pages together: a query
/// whose pages each answer within the request limit, but add up to more than
/// the budget, becomes a warning naming its stack and shows no cards, while
/// the other stacks still show theirs.
#[tokio::test]
async fn a_stack_over_its_time_budget_is_a_warning_naming_it() {
    let stub = Stub::default();
    stub.issues("project = OPS", vec![issue("OPS-1", "One")]);
    let pages = (1..=5)
        .map(|n| {
            let mut page = json!({ "issues": [issue(&format!("SLOW-{n}"), "x")] });
            if n < 5 {
                page["nextPageToken"] = json!(format!("tok-{n}"));
            }
            ok(page)
        })
        .collect();
    stub.reply("project = SLOW", pages);
    // Five pages of 150 ms each: 750 ms in all.
    stub.delays
        .lock()
        .unwrap()
        .insert("project = SLOW".to_owned(), Duration::from_millis(150));
    let source = cloud_source(serve(stub).await)
        .with_stack_budget(Duration::from_millis(400))
        .with_columns(vec![
            column("Ops", "project = OPS"),
            column("Slow", "project = SLOW"),
        ]);

    let batch = source.refresh().await.expect("partial refresh");

    assert_eq!(
        ids(&batch),
        vec![("Ops".to_owned(), "jira:OPS-1".to_owned())]
    );
    assert_eq!(batch.warnings, ["Slow: timed out after 400ms"]);
}

/// The stacks of one refresh run at most three at a time, and the cards
/// keep the stacks' order.
#[tokio::test]
async fn queries_run_at_most_three_at_a_time() {
    let stub = Stub::default();
    let columns: Vec<JiraColumn> = (0..7)
        .map(|n| {
            let jql = format!("project = P{n}");
            stub.issues(&jql, vec![issue(&format!("P{n}-1"), "x")]);
            column(&format!("S{n}"), &jql)
        })
        .collect();
    let source = cloud_source(serve(stub.clone()).await).with_columns(columns);

    let batch = source.refresh().await.expect("refresh");

    let order: Vec<&str> = batch.items.iter().map(|i| i.column.as_str()).collect();
    assert_eq!(order, ["S0", "S1", "S2", "S3", "S4", "S5", "S6"]);
    let most = stub.max_in_flight.load(Ordering::SeqCst);
    assert!((2..=3).contains(&most), "{most} requests at once");
}

/// An unreachable server fails the source without showing the credentials
/// or the address.
#[tokio::test]
async fn an_unreachable_server_fails_without_leaking_anything() {
    // A port nobody listens on.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);

    let error = cloud_source(base.clone())
        .refresh()
        .await
        .expect_err("nothing listens");

    let message = error.to_string();
    assert!(
        message.starts_with("all Jira queries failed; "),
        "{message}"
    );
    assert_no_secret(&message);
    assert!(!message.contains(&base), "{message}");
}

/// A reply that is not a search result is an error, not an empty stack.
#[tokio::test]
async fn an_unexpected_reply_is_an_error() {
    let stub = Stub::default();
    stub.reply("project = OPS", vec![ok(json!({ "unexpected": true }))]);
    let source = cloud_source(serve(stub).await).with_columns(vec![column("Ops", "project = OPS")]);

    let error = source.refresh().await.expect_err("not a search result");

    assert!(
        error.to_string().contains("unexpected Jira response"),
        "{error}"
    );
}

fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    move |key| {
        vars.iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| (*value).to_owned())
    }
}

/// E-mail plus API token select Jira Cloud; a lone token selects Jira Data
/// Center. A trailing slash in the address is dropped.
#[test]
fn settings_pick_the_flavour_from_the_variables() {
    let cloud = JiraSettings::from_env(&env(&[
        ("JIRA_BASE_URL", " https://acme.atlassian.net/ "),
        ("JIRA_EMAIL", EMAIL),
        ("JIRA_API_TOKEN", API_TOKEN),
    ]))
    .expect("cloud settings");
    assert_eq!(cloud.base_url, "https://acme.atlassian.net");
    assert!(
        cloud.auth
            == JiraAuth::Cloud {
                email: EMAIL.to_owned(),
                api_token: API_TOKEN.to_owned(),
            }
    );

    let data_center = JiraSettings::from_env(&env(&[
        ("JIRA_BASE_URL", "https://jira.example.com/jira"),
        ("JIRA_TOKEN", PAT),
    ]))
    .expect("data center settings");
    assert_eq!(data_center.base_url, "https://jira.example.com/jira");
    assert!(
        data_center.auth
            == JiraAuth::DataCenter {
                token: PAT.to_owned(),
            }
    );
}

/// With all three credentials set, the Cloud pair is used. An e-mail with
/// `JIRA_TOKEN` but no `JIRA_API_TOKEN` is a mix-up of the two flavours: it
/// is refused with the way out, instead of being taken for Data Center.
#[test]
fn mixed_credentials_are_settled_or_refused() {
    let all = JiraSettings::from_env(&env(&[
        ("JIRA_BASE_URL", "https://acme.atlassian.net"),
        ("JIRA_EMAIL", EMAIL),
        ("JIRA_API_TOKEN", API_TOKEN),
        ("JIRA_TOKEN", PAT),
    ]))
    .expect("cloud settings");
    assert!(
        all.auth
            == JiraAuth::Cloud {
                email: EMAIL.to_owned(),
                api_token: API_TOKEN.to_owned(),
            }
    );

    let mixed = JiraSettings::from_env(&env(&[
        ("JIRA_BASE_URL", "https://acme.atlassian.net"),
        ("JIRA_EMAIL", EMAIL),
        ("JIRA_TOKEN", PAT),
    ]))
    .expect_err("e-mail with the Data Center token");
    assert_eq!(
        mixed,
        "Jira is not configured: set JIRA_API_TOKEN instead of JIRA_TOKEN (Jira Cloud), or \
         unset JIRA_EMAIL (Jira Data Center), then restart"
    );
    assert_no_secret(&mixed);
}

/// Missing settings are named, and every refresh fails with that message.
#[tokio::test]
async fn missing_settings_are_named() {
    let nothing = JiraSettings::from_env(&env(&[])).expect_err("unset");
    assert_eq!(
        nothing,
        "Jira is not configured: set JIRA_BASE_URL, and JIRA_EMAIL with JIRA_API_TOKEN \
         (Jira Cloud) or JIRA_TOKEN (Jira Data Center), then restart"
    );

    let no_token = JiraSettings::from_env(&env(&[
        ("JIRA_BASE_URL", "https://acme.atlassian.net"),
        ("JIRA_EMAIL", EMAIL),
        ("JIRA_API_TOKEN", "  "),
    ]))
    .expect_err("no token");
    assert_eq!(
        no_token,
        "Jira is not configured: set JIRA_API_TOKEN (Jira Cloud; or JIRA_TOKEN alone for \
         Jira Data Center), then restart"
    );

    let no_email = JiraSettings::from_env(&env(&[
        ("JIRA_BASE_URL", "https://acme.atlassian.net"),
        ("JIRA_API_TOKEN", API_TOKEN),
    ]))
    .expect_err("no e-mail");
    assert!(no_email.contains("set JIRA_EMAIL"), "{no_email}");
    assert_no_secret(&no_email);

    let no_base = JiraSettings::from_env(&env(&[("JIRA_TOKEN", PAT)])).expect_err("no address");
    assert_eq!(
        no_base,
        "Jira is not configured: set JIRA_BASE_URL, then restart"
    );

    let error: SourceError = JiraSource::new(Err(nothing.clone()))
        .refresh()
        .await
        .expect_err("not configured");
    assert_eq!(error.to_string(), nothing);
}

/// The address must be http(s): anything else is a configuration mistake
/// reported by name, not a request to somewhere unexpected.
#[test]
fn a_base_url_without_a_scheme_is_rejected() {
    let error = JiraSettings::from_env(&env(&[
        ("JIRA_BASE_URL", "acme.atlassian.net"),
        ("JIRA_TOKEN", PAT),
    ]))
    .expect_err("no scheme");
    assert!(error.contains("JIRA_BASE_URL"), "{error}");
    assert!(error.contains("https://"), "{error}");
}

/// The settings never print their credentials, even in debug output.
#[test]
fn settings_debug_output_hides_the_credentials() {
    let settings = JiraSettings {
        base_url: "https://acme.atlassian.net".to_owned(),
        auth: JiraAuth::Cloud {
            email: EMAIL.to_owned(),
            api_token: API_TOKEN.to_owned(),
        },
    };
    let text = format!("{settings:?}");
    assert!(text.contains("acme.atlassian.net"), "{text}");
    assert_no_secret(&text);
}

/// A stack needs `jql`, takes an optional `severity`, and rejects unknown
/// keys and an empty query.
#[test]
fn a_stack_is_a_jql_query_with_an_optional_severity() {
    let parsed: JiraColumn =
        serde_json::from_value(json!({ "jql": "sprint in openSprints()", "severity": "warning" }))
            .expect("valid stack");
    assert_eq!(parsed.jql, "sprint in openSprints()");
    assert_eq!(parsed.severity, Some(CardSeverity::Warning));
    assert_eq!(parsed.validate(), Ok(()));

    let missing = serde_json::from_value::<JiraColumn>(json!({})).expect_err("jql is required");
    assert!(missing.to_string().contains("jql"), "{missing}");
    let unknown = serde_json::from_value::<JiraColumn>(json!({ "jql": "x", "filter": "y" }))
        .expect_err("unknown key");
    assert!(unknown.to_string().contains("filter"), "{unknown}");

    let empty: JiraColumn = serde_json::from_value(json!({ "jql": "  " })).unwrap();
    assert_eq!(empty.validate(), Err("`jql` must not be empty".to_owned()));
}

/// Jira writes timestamps with milliseconds and an offset without a colon
/// (`+0000`); RFC 3339 forms are read too, and garbage is refused.
#[test]
fn jira_timestamps_are_parsed() {
    let ten = Utc.with_ymd_and_hms(2026, 9, 28, 10, 0, 0).unwrap();
    assert_eq!(parse_timestamp("2026-09-28T10:00:00.000+0000"), Some(ten));
    assert_eq!(parse_timestamp("2026-09-28T07:00:00.000-0300"), Some(ten));
    assert_eq!(parse_timestamp("2026-09-28T15:30:00.000+0530"), Some(ten));
    assert_eq!(parse_timestamp("2026-09-28T10:00:00+0000"), Some(ten));
    assert_eq!(parse_timestamp("2026-09-28T10:00:00.000Z"), Some(ten));
    assert_eq!(parse_timestamp("2026-09-28T12:00:00+02:00"), Some(ten));
    assert_eq!(
        parse_timestamp("2026-09-28T10:00:00.123+0000"),
        Some(ten + chrono::Duration::milliseconds(123))
    );
    assert_eq!(parse_timestamp("yesterday"), None);
    assert_eq!(parse_timestamp(""), None);
}

/// An issue without a readable update time still shows, and one without a
/// key is skipped.
#[tokio::test]
async fn odd_issues_do_not_break_the_stack() {
    let stub = Stub::default();
    stub.issues(
        IN_PROGRESS,
        vec![
            with(issue("P-1", "No time"), "updated", json!("soon")),
            json!({ "id": "1", "fields": { "summary": "No key" } }),
        ],
    );

    let batch = cloud_source(serve(stub).await)
        .refresh()
        .await
        .expect("refresh");

    assert_eq!(batch.items.len(), 1);
    assert_eq!(card(&batch, "jira:P-1").updated_at, DateTime::UNIX_EPOCH);
}

/// A `JIRA_BASE_URL` without encryption would send the e-mail and token in
/// the clear on every refresh: it is refused, naming the variable. A local
/// address is fine.
#[test]
fn an_unencrypted_site_address_is_refused() {
    let with_url = |url: &'static str| {
        JiraSettings::from_env(&move |key| match key {
            "JIRA_BASE_URL" => Some(url.to_owned()),
            "JIRA_TOKEN" => Some("secret-token".to_owned()),
            _ => None,
        })
    };

    let error = with_url("http://jira.example.com").expect_err("refused");
    assert!(
        error.contains("JIRA_BASE_URL") && error.contains("https://"),
        "{error}"
    );
    assert!(!error.contains("secret-token"));
    assert!(with_url("http://127.0.0.1:8080").is_ok());
    assert!(with_url("https://jira.example.com").is_ok());
}
