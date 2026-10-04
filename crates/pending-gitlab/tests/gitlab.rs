//! The GitLab source turns merge requests, issues and to-do items into cards
//! without leaking the token or provider details. Every test talks to a
//! local stub, never to a real GitLab instance.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Local, NaiveDate, Utc};
use pending_core::{CardSeverity, PendingCard, PendingSource, SourceBatch};
use pending_gitlab::{GitlabColumn, GitlabSource};
use serde_json::{Value, json};

const TOKEN: &str = "glpat-secret-test-token";

const MERGE_REQUESTS: &str = "/api/v4/merge_requests";
const ISSUES: &str = "/api/v4/issues";
const TODOS: &str = "/api/v4/todos";

/// Canned reply: HTTP status, headers and body.
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
            body: Value::Array(items),
        }
    }

    fn status(status: StatusCode, message: &str) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: json!({ "message": message }),
        }
    }

    fn header(mut self, name: &'static str, value: &str) -> Self {
        self.headers.push((name, value.to_owned()));
        self
    }
}

/// What a request is answered by: its raw path and `scope` parameter (empty
/// for to-dos).
type Key = (String, String);

#[derive(Clone, Default)]
struct Stub {
    /// Replies per key, one per page; an empty list otherwise.
    replies: Arc<Mutex<HashMap<Key, Vec<Reply>>>>,
    /// Every request seen, as `path?query`, exactly as sent.
    requests: Arc<Mutex<Vec<String>>>,
    /// The `PRIVATE-TOKEN` header of every request.
    tokens: Arc<Mutex<Vec<String>>>,
    /// How long each reply takes.
    delay: Arc<Mutex<Duration>>,
    /// Extra wait for one key.
    slow: Arc<Mutex<Option<(Key, Duration)>>>,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
}

impl Stub {
    fn set(&self, path: &str, scope: &str, pages: Vec<Reply>) {
        self.replies
            .lock()
            .unwrap()
            .insert((path.to_owned(), scope.to_owned()), pages);
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

async fn handle(
    State(stub): State<Stub>,
    uri: Uri,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let now = stub.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
    stub.max_in_flight.fetch_max(now, Ordering::SeqCst);

    stub.requests.lock().unwrap().push(uri.to_string());
    stub.tokens.lock().unwrap().push(
        headers
            .get("private-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned(),
    );

    let key = (
        uri.path().to_owned(),
        params.get("scope").cloned().unwrap_or_default(),
    );
    let mut wait = *stub.delay.lock().unwrap();
    if let Some((slow, extra)) = stub.slow.lock().unwrap().clone()
        && slow == key
    {
        wait += extra;
    }
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }

    let page: usize = params
        .get("page")
        .and_then(|page| page.parse().ok())
        .unwrap_or(1);
    let reply = stub
        .replies
        .lock()
        .unwrap()
        .get(&key)
        .and_then(|pages| pages.get(page - 1).cloned())
        .unwrap_or_else(|| Reply::items(Vec::new()));

    stub.in_flight.fetch_sub(1, Ordering::SeqCst);
    let mut response = (reply.status, axum::Json(reply.body)).into_response();
    for (name, value) in reply.headers {
        response.headers_mut().insert(name, value.parse().unwrap());
    }
    response
}

async fn serve(stub: Stub) -> String {
    let app = Router::new().fallback(handle).with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn merge_request(project: &str, project_id: u64, iid: u64, title: &str) -> Value {
    json!({
        "id": 9000 + iid,
        "iid": iid,
        "project_id": project_id,
        "title": title,
        "state": "opened",
        "draft": false,
        "work_in_progress": false,
        "user_notes_count": 0,
        "updated_at": "2026-09-28T10:00:00.000Z",
        "author": { "id": 1, "username": "alice" },
        "references": { "short": format!("!{iid}"), "full": format!("{project}!{iid}") },
        "web_url": format!("https://gitlab.com/{project}/-/merge_requests/{iid}"),
    })
}

fn issue(project: &str, project_id: u64, iid: u64, title: &str) -> Value {
    json!({
        "id": 7000 + iid,
        "iid": iid,
        "project_id": project_id,
        "title": title,
        "state": "opened",
        "user_notes_count": 0,
        "due_date": null,
        "updated_at": "2026-09-27T08:30:00.000+02:00",
        "author": { "id": 2, "username": "bob" },
        "references": { "short": format!("#{iid}"), "full": format!("{project}#{iid}") },
        "web_url": format!("https://gitlab.com/{project}/-/issues/{iid}"),
    })
}

fn todo(id: u64, action: &str, title: &str) -> Value {
    json!({
        "id": id,
        "project": { "id": 5, "path_with_namespace": "acme/api" },
        "author": { "id": 3, "username": "carol" },
        "action_name": action,
        "target_type": "MergeRequest",
        "target": { "id": 1, "iid": 4, "title": title },
        "target_url": "https://gitlab.com/acme/api/-/merge_requests/4",
        "body": "the note text",
        "state": "pending",
        "created_at": "2026-09-20T09:00:00.000Z",
        "updated_at": "2026-09-21T09:00:00.000Z",
    })
}

/// A stack from its config text, named `name`, as the runtime builds it.
fn stack(name: &str, filter: &str) -> GitlabColumn {
    let mut column: GitlabColumn = toml::from_str(filter).expect("valid stack");
    column.validate().expect("valid stack");
    column.name = name.to_owned();
    column
}

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 3).unwrap()
}

fn source(base: &str) -> GitlabSource {
    GitlabSource::new(base, Ok(TOKEN.to_owned())).with_today(today)
}

fn cards<'a>(batch: &'a SourceBatch, column: &str) -> Vec<&'a PendingCard> {
    batch
        .items
        .iter()
        .filter(|item| item.column == column)
        .map(|item| &item.card)
        .collect()
}

fn utc(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
}

/// Without configured stacks the source asks for review requests, assigned
/// merge requests, assigned issues and pending to-do items, each with one
/// request carrying the token in the `PRIVATE-TOKEN` header, and turns the
/// replies into cards in those four stacks.
#[tokio::test]
async fn default_stacks_show_reviews_assignments_and_todos() {
    let stub = Stub::default();
    stub.set(
        MERGE_REQUESTS,
        "reviews_for_me",
        vec![Reply::items(vec![merge_request(
            "acme/api",
            5,
            12,
            "Add login",
        )])],
    );
    stub.set(
        MERGE_REQUESTS,
        "assigned_to_me",
        vec![Reply::items(vec![merge_request(
            "acme/web",
            6,
            3,
            "Fix footer",
        )])],
    );
    stub.set(
        ISSUES,
        "assigned_to_me",
        vec![Reply::items(vec![issue("acme/api", 5, 7, "Crash on save")])],
    );
    stub.set(
        TODOS,
        "",
        vec![Reply::items(vec![todo(
            41,
            "review_requested",
            "Refactor auth",
        )])],
    );
    let base = serve(stub.clone()).await;

    let source = source(&base);
    assert_eq!(
        source.columns(),
        [
            "Review requested",
            "Assigned merge requests",
            "Assigned issues",
            "To-dos"
        ]
    );
    let batch = source.refresh().await.expect("refresh");
    assert_eq!(batch.warnings, Vec::<String>::new());

    let review = cards(&batch, "Review requested");
    assert_eq!(review.len(), 1);
    assert_eq!(review[0].id, "gitlab:mr:5:12");
    assert_eq!(review[0].title, "Add login");
    assert_eq!(review[0].body, "acme/api!12 · @alice");
    assert_eq!(review[0].source, "gitlab");
    assert_eq!(
        review[0].url.as_deref(),
        Some("https://gitlab.com/acme/api/-/merge_requests/12")
    );
    assert_eq!(review[0].severity, CardSeverity::Warning);
    assert_eq!(review[0].updated_at, utc("2026-09-28T10:00:00Z"));
    assert_eq!(review[0].due_at, None);

    let assigned = cards(&batch, "Assigned merge requests");
    assert_eq!(assigned[0].id, "gitlab:mr:6:3");
    assert_eq!(assigned[0].severity, CardSeverity::Info);

    let issues = cards(&batch, "Assigned issues");
    assert_eq!(issues[0].id, "gitlab:issue:5:7");
    assert_eq!(issues[0].title, "Crash on save");
    assert_eq!(issues[0].body, "acme/api#7 · @bob");
    assert_eq!(
        issues[0].url.as_deref(),
        Some("https://gitlab.com/acme/api/-/issues/7")
    );
    assert_eq!(issues[0].severity, CardSeverity::Info);
    assert_eq!(issues[0].updated_at, utc("2026-09-27T06:30:00Z"));

    let todos = cards(&batch, "To-dos");
    assert_eq!(todos[0].id, "gitlab:todo:41");
    assert_eq!(todos[0].title, "Refactor auth");
    assert_eq!(todos[0].body, "review requested · acme/api · @carol");
    assert_eq!(
        todos[0].url.as_deref(),
        Some("https://gitlab.com/acme/api/-/merge_requests/4")
    );
    assert_eq!(todos[0].severity, CardSeverity::Info);
    assert_eq!(todos[0].updated_at, utc("2026-09-21T09:00:00Z"));

    let mut requests = stub.requests();
    requests.sort();
    assert_eq!(
        requests,
        [
            "/api/v4/issues?scope=assigned_to_me&state=opened&order_by=updated_at&sort=desc&per_page=100&page=1",
            "/api/v4/merge_requests?scope=assigned_to_me&state=opened&order_by=updated_at&sort=desc&non_archived=true&per_page=100&page=1",
            "/api/v4/merge_requests?scope=reviews_for_me&state=opened&order_by=updated_at&sort=desc&non_archived=true&per_page=100&page=1",
            "/api/v4/todos?state=pending&per_page=100&page=1",
        ]
    );
    let tokens = stub.tokens.lock().unwrap().clone();
    assert_eq!(tokens, vec![TOKEN; 4]);
}

/// Merge request cards say when the merge request is a draft and how many
/// comments it has; authored ones are plain info, and a stack `severity`
/// replaces the default of its kind. Older instances mark drafts only with
/// `work_in_progress`, and one without `references` still gets a reference.
#[tokio::test]
async fn merge_request_cards_show_draft_and_comments() {
    let stub = Stub::default();
    let mut draft = merge_request("acme/api", 5, 20, "Draft: big change");
    draft["draft"] = json!(true);
    draft["user_notes_count"] = json!(3);
    let mut old = merge_request("acme/api", 5, 21, "WIP: old instance");
    old.as_object_mut().unwrap().remove("draft");
    old.as_object_mut().unwrap().remove("references");
    old["work_in_progress"] = json!(true);
    old["user_notes_count"] = json!(1);
    stub.set(
        MERGE_REQUESTS,
        "created_by_me",
        vec![Reply::items(vec![draft, old])],
    );
    stub.set(
        MERGE_REQUESTS,
        "reviews_for_me",
        vec![Reply::items(vec![merge_request("acme/api", 5, 22, "Calm")])],
    );
    let base = serve(stub.clone()).await;

    let batch = source(&base)
        .with_columns(vec![
            stack("Mine", "merge_requests = \"authored\""),
            stack(
                "Reviews",
                "merge_requests = \"review_requested\"\nseverity = \"critical\"",
            ),
        ])
        .refresh()
        .await
        .expect("refresh");

    let mine = cards(&batch, "Mine");
    assert_eq!(mine[0].body, "acme/api!20 · @alice · draft · 3 comments");
    assert_eq!(mine[0].severity, CardSeverity::Info);
    assert_eq!(mine[1].id, "gitlab:mr:5:21");
    assert_eq!(mine[1].body, "!21 · @alice · draft · 1 comment");
    assert_eq!(cards(&batch, "Reviews")[0].severity, CardSeverity::Critical);
}

/// An issue with a due date shows it and sorts by it (`due_at` is local
/// midnight of that day); a date before today is overdue, which makes the
/// card critical unless the stack sets its own severity.
#[tokio::test]
async fn issue_cards_show_due_dates_and_overdue_ones_are_critical() {
    let stub = Stub::default();
    let mut later = issue("acme/api", 5, 1, "Later");
    later["due_date"] = json!("2026-10-10");
    later["user_notes_count"] = json!(2);
    let mut due_today = issue("acme/api", 5, 2, "Today");
    due_today["due_date"] = json!("2026-10-03");
    let mut late = issue("acme/api", 5, 3, "Late");
    late["due_date"] = json!("2026-10-01");
    let items = vec![later, due_today, late];
    stub.set(ISSUES, "assigned_to_me", vec![Reply::items(items.clone())]);
    stub.set(ISSUES, "created_by_me", vec![Reply::items(items)]);
    let base = serve(stub.clone()).await;

    let batch = source(&base)
        .with_columns(vec![
            stack("Assigned", "issues = \"assigned\""),
            stack("Created", "issues = \"authored\"\nseverity = \"info\""),
        ])
        .refresh()
        .await
        .expect("refresh");

    let local_midnight = |year, month, day| {
        NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_local_timezone(Local)
            .earliest()
            .map(|at| at.with_timezone(&Utc))
    };
    let assigned = cards(&batch, "Assigned");
    assert_eq!(
        assigned[0].body,
        "acme/api#1 · @bob · 2 comments · due 2026-10-10"
    );
    assert_eq!(assigned[0].due_at, local_midnight(2026, 10, 10));
    assert_eq!(assigned[0].severity, CardSeverity::Info);
    assert_eq!(assigned[1].body, "acme/api#2 · @bob · due 2026-10-03");
    assert_eq!(assigned[1].severity, CardSeverity::Info);
    assert_eq!(
        assigned[2].body,
        "acme/api#3 · @bob · overdue since 2026-10-01"
    );
    assert_eq!(assigned[2].due_at, local_midnight(2026, 10, 1));
    assert_eq!(assigned[2].severity, CardSeverity::Critical);

    let created = cards(&batch, "Created");
    assert_eq!(created[2].severity, CardSeverity::Info);
    assert!(
        stub.requests()
            .iter()
            .any(|request| request.starts_with("/api/v4/issues?scope=created_by_me&state=opened")),
        "{:?}",
        stub.requests()
    );
}

/// A to-do card is titled after its target and tells the action, where it
/// happened and who caused it. Group-level to-dos have no project, and
/// to-dos without a target title fall back to the note or the target type.
#[tokio::test]
async fn todo_cards_describe_the_action_and_its_target() {
    let stub = Stub::default();
    let mut epic = todo(50, "mentioned", "ignored");
    epic["project"] = Value::Null;
    epic["group"] = json!({ "id": 9, "full_path": "acme/platform" });
    epic["target_type"] = json!("Epic");
    epic["target"] = json!({ "id": 2, "title": "Roadmap" });
    let mut bare = todo(51, "member_access_requested", "ignored");
    bare.as_object_mut().unwrap().remove("project");
    bare.as_object_mut().unwrap().remove("author");
    bare.as_object_mut().unwrap().remove("updated_at");
    bare["target"] = json!({ "id": 3 });
    bare["body"] = json!("");
    bare["target_type"] = json!("Namespace");
    stub.set(
        TODOS,
        "",
        vec![Reply::items(vec![
            todo(49, "assigned", "Ship it"),
            epic,
            bare,
        ])],
    );
    let base = serve(stub.clone()).await;

    let batch = source(&base)
        .with_columns(vec![stack("Inbox", "todos = true\nseverity = \"warning\"")])
        .refresh()
        .await
        .expect("refresh");

    let inbox = cards(&batch, "Inbox");
    assert_eq!(inbox[0].title, "Ship it");
    assert_eq!(inbox[0].body, "assigned · acme/api · @carol");
    assert_eq!(inbox[0].severity, CardSeverity::Warning);
    assert_eq!(inbox[1].title, "Roadmap");
    assert_eq!(inbox[1].body, "mentioned · acme/platform · @carol");
    assert_eq!(inbox[2].id, "gitlab:todo:51");
    assert_eq!(inbox[2].title, "Namespace");
    assert_eq!(inbox[2].body, "member access requested");
    assert_eq!(inbox[2].updated_at, utc("2026-09-20T09:00:00Z"));
}

/// `group` and `project` narrow a stack to one group (with its subgroups) or
/// one project, by path, percent-encoded in the request path; `labels` and
/// `draft` become query parameters.
#[tokio::test]
async fn stacks_can_be_narrowed_by_group_project_labels_and_draft() {
    let stub = Stub::default();
    stub.set(
        "/api/v4/groups/acme%2Fplatform/merge_requests",
        "reviews_for_me",
        vec![Reply::items(vec![merge_request(
            "acme/platform/core",
            8,
            2,
            "Scoped",
        )])],
    );
    let base = serve(stub.clone()).await;

    let batch = source(&base)
        .with_columns(vec![
            stack(
                "Platform reviews",
                "merge_requests = \"review_requested\"\ngroup = \"acme/platform\"\ndraft = false",
            ),
            stack(
                "API bugs",
                "issues = \"assigned\"\nproject = \"acme/api\"\nlabels = [\"bug\", \"needs review\"]",
            ),
            stack(
                "API drafts",
                "merge_requests = \"authored\"\nproject = \"acme/api\"\ndraft = true",
            ),
            stack("Group issues", "issues = \"authored\"\ngroup = \"acme\""),
        ])
        .refresh()
        .await
        .expect("refresh");

    assert_eq!(cards(&batch, "Platform reviews")[0].title, "Scoped");
    let mut requests = stub.requests();
    requests.sort();
    assert_eq!(
        requests,
        [
            "/api/v4/groups/acme%2Fplatform/merge_requests?scope=reviews_for_me&state=opened&order_by=updated_at&sort=desc&non_archived=true&draft=false&per_page=100&page=1",
            "/api/v4/groups/acme/issues?scope=created_by_me&state=opened&order_by=updated_at&sort=desc&per_page=100&page=1",
            "/api/v4/projects/acme%2Fapi/issues?scope=assigned_to_me&state=opened&order_by=updated_at&sort=desc&labels=bug%2Cneeds+review&per_page=100&page=1",
            "/api/v4/projects/acme%2Fapi/merge_requests?scope=created_by_me&state=opened&order_by=updated_at&sort=desc&draft=true&per_page=100&page=1",
        ]
    );
}

/// A stack sets exactly one of `merge_requests`, `issues` and `todos`;
/// `group` and `project` exclude each other and, like `labels`, make no
/// sense for to-dos; `draft` is for merge requests only. Typos are rejected.
#[test]
fn stacks_are_validated() {
    let check = |filter: &str| -> Result<(), String> {
        toml::from_str::<GitlabColumn>(filter)
            .map_err(|error| error.message().to_owned())?
            .validate()
    };

    for valid in [
        "merge_requests = \"review_requested\"",
        "merge_requests = \"assigned\"\ngroup = \"acme\"\nlabels = [\"bug\"]\ndraft = true",
        "issues = \"authored\"\nproject = \"acme/api\"\nseverity = \"warning\"",
        "issues = \"assigned\"\nproject = \"42\"",
        "issues = \"assigned\"\nproject = \"acme/.github\"",
        "issues = \"assigned\"\ngroup = \"acme/v1.2\"",
        "todos = true",
    ] {
        assert_eq!(check(valid), Ok(()), "{valid}");
    }

    for (invalid, expected) in [
        ("", "exactly one of"),
        (
            "merge_requests = \"assigned\"\nissues = \"assigned\"",
            "exactly one of",
        ),
        ("issues = \"assigned\"\ntodos = true", "exactly one of"),
        ("todos = false", "`todos`"),
        ("merge_requests = \"mine\"", "mine"),
        ("issues = \"review_requested\"", "review_requested"),
        ("merge_request = \"assigned\"", "merge_request"),
        (
            "issues = \"assigned\"\ngroup = \"a\"\nproject = \"a/b\"",
            "`group` or `project`",
        ),
        ("todos = true\ngroup = \"acme\"", "`group`"),
        ("todos = true\nlabels = [\"bug\"]", "`labels`"),
        ("issues = \"assigned\"\ndraft = true", "`draft`"),
        ("issues = \"assigned\"\ngroup = \"\"", "`group`"),
        (
            "issues = \"assigned\"\nproject = \"/acme/api\"",
            "`project`",
        ),
        ("issues = \"assigned\"\nproject = \"..\"", "`project`"),
        (
            "issues = \"assigned\"\nproject = \"acme/../../admin\"",
            "`project`",
        ),
        ("issues = \"assigned\"\ngroup = \"acme/./api\"", "`group`"),
        ("issues = \"assigned\"\ngroup = \".\"", "`group`"),
        ("issues = \"assigned\"\nlabels = []", "`labels`"),
        ("issues = \"assigned\"\nlabels = [\"a,b\"]", "`labels`"),
    ] {
        let error = check(invalid).expect_err(invalid);
        assert!(error.contains(expected), "{invalid}: {error}");
    }
}

/// Lists are read 100 at a time, following `x-next-page`; after five pages
/// the stack stops and warns that there is more.
#[tokio::test]
async fn pages_are_followed_up_to_a_cap_with_a_warning() {
    let stub = Stub::default();
    let page = |first: u64, next: &str| {
        Reply::items(
            (first..first + 2)
                .map(|iid| merge_request("acme/api", 5, iid, "MR"))
                .collect(),
        )
        .header("x-next-page", next)
    };
    stub.set(
        MERGE_REQUESTS,
        "assigned_to_me",
        vec![page(1, "2"), page(3, "3"), page(5, "")],
    );
    stub.set(
        MERGE_REQUESTS,
        "created_by_me",
        (1..=7)
            .map(|n| page(n * 10, &(n + 1).to_string()))
            .collect(),
    );
    let base = serve(stub.clone()).await;

    let batch = source(&base)
        .with_columns(vec![
            stack("Assigned", "merge_requests = \"assigned\""),
            stack("Mine", "merge_requests = \"authored\""),
        ])
        .refresh()
        .await
        .expect("refresh");

    let ids: Vec<&str> = cards(&batch, "Assigned")
        .iter()
        .map(|card| card.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "gitlab:mr:5:1",
            "gitlab:mr:5:2",
            "gitlab:mr:5:3",
            "gitlab:mr:5:4",
            "gitlab:mr:5:5",
            "gitlab:mr:5:6"
        ]
    );
    assert_eq!(cards(&batch, "Mine").len(), 10);
    assert_eq!(
        batch.warnings,
        ["Mine: more than 5 pages of results; showing the first ones"]
    );
    let authored_pages = stub
        .requests()
        .iter()
        .filter(|request| request.contains("scope=created_by_me"))
        .count();
    assert_eq!(authored_pages, 5);
}

/// A stack whose request fails becomes a warning naming it while the other
/// stacks still show; when every stack fails, the source fails.
#[tokio::test]
async fn one_failing_stack_is_a_warning_and_all_failing_is_an_error() {
    let stub = Stub::default();
    stub.set(
        MERGE_REQUESTS,
        "reviews_for_me",
        vec![Reply::status(
            StatusCode::INTERNAL_SERVER_ERROR,
            "500 Internal Server Error",
        )],
    );
    stub.set(
        ISSUES,
        "assigned_to_me",
        vec![Reply::items(vec![issue("acme/api", 5, 7, "Still here")])],
    );
    let base = serve(stub.clone()).await;

    let batch = source(&base).refresh().await.expect("partial refresh");
    assert_eq!(cards(&batch, "Assigned issues").len(), 1);
    assert_eq!(
        batch.warnings,
        ["Review requested: GitLab API returned 500 Internal Server Error"]
    );

    for (path, scope) in [
        (MERGE_REQUESTS, "assigned_to_me"),
        (ISSUES, "assigned_to_me"),
        (TODOS, ""),
    ] {
        stub.set(
            path,
            scope,
            vec![Reply::status(StatusCode::BAD_GATEWAY, "upstream is down")],
        );
    }
    let error = source(&base).refresh().await.expect_err("all failed");
    let message = error.to_string();
    assert!(
        message.starts_with("all GitLab requests failed; "),
        "{message}"
    );
    assert!(
        message.contains("To-dos: GitLab API returned 502 Bad Gateway: upstream is down"),
        "{message}"
    );
    assert_eq!(error.retry_at(), None);
}

/// A rejected token (401) fails with a hint about `GITLAB_TOKEN`, a
/// forbidden request (403) hints at the `read_api` scope, and a group or
/// project that is not found (404) hints at its path. No message ever
/// contains the token, even when the server echoes it.
#[tokio::test]
async fn auth_errors_carry_a_hint_and_never_the_token() {
    let stub = Stub::default();
    for (path, scope) in [
        (MERGE_REQUESTS, "reviews_for_me"),
        (MERGE_REQUESTS, "assigned_to_me"),
        (ISSUES, "assigned_to_me"),
        (TODOS, ""),
    ] {
        stub.set(
            path,
            scope,
            vec![Reply::status(StatusCode::UNAUTHORIZED, "401 Unauthorized")],
        );
    }
    let base = serve(stub.clone()).await;

    let message = source(&base)
        .refresh()
        .await
        .expect_err("bad token")
        .to_string();
    assert!(
        message.contains(
            "To-dos: GitLab API returned 401 Unauthorized; check that GITLAB_TOKEN is valid and not expired"
        ),
        "{message}"
    );
    assert!(!message.contains(TOKEN), "{message}");
    assert!(!message.contains(&base), "{message}");

    stub.set(
        MERGE_REQUESTS,
        "reviews_for_me",
        vec![Reply {
            status: StatusCode::FORBIDDEN,
            headers: Vec::new(),
            body: json!({ "error": format!("insufficient_scope for {TOKEN}") }),
        }],
    );
    stub.set(
        "/api/v4/projects/acme%2Ftypo/issues",
        "assigned_to_me",
        vec![Reply::status(
            StatusCode::NOT_FOUND,
            "404 Project Not Found",
        )],
    );
    stub.set(TODOS, "", vec![Reply::items(Vec::new())]);
    let batch = source(&base)
        .with_columns(vec![
            stack("Reviews", "merge_requests = \"review_requested\""),
            stack("Typo", "issues = \"assigned\"\nproject = \"acme/typo\""),
            stack("To-dos", "todos = true"),
        ])
        .refresh()
        .await
        .expect("partial refresh");
    assert_eq!(
        batch.warnings,
        [
            "Reviews: GitLab API returned 403 Forbidden: insufficient_scope for ***; the token needs the read_api scope",
            "Typo: GitLab API returned 404 Not Found: 404 Project Not Found; check the `project` path and that the token can see it",
        ]
    );
}

/// A rate-limited reply (429) reports when to try again, from `Retry-After`
/// (seconds), so the runtime waits instead of hammering the instance.
#[tokio::test]
async fn rate_limits_report_when_to_retry() {
    let stub = Stub::default();
    let limited = || Reply {
        status: StatusCode::TOO_MANY_REQUESTS,
        headers: vec![("retry-after", "120".to_owned())],
        body: json!("Retry later"),
    };
    stub.set(TODOS, "", vec![limited()]);
    let base = serve(stub.clone()).await;
    let todos_only = || source(&base).with_columns(vec![stack("To-dos", "todos = true")]);

    let before = Utc::now();
    let error = todos_only().refresh().await.expect_err("rate limited");
    let retry_at = error.retry_at().expect("retry time");
    let wait = (retry_at - before).num_seconds();
    assert!((119..=125).contains(&wait), "{wait}");
    assert!(
        error
            .to_string()
            .contains("To-dos: GitLab rate limit exceeded; retry at "),
        "{error}"
    );

    // Without `Retry-After`, `RateLimit-Reset` (Unix time) says when.
    stub.set(
        TODOS,
        "",
        vec![Reply {
            status: StatusCode::TOO_MANY_REQUESTS,
            headers: vec![("ratelimit-reset", "1900000000".to_owned())],
            body: json!("Retry later"),
        }],
    );
    let error = todos_only().refresh().await.expect_err("rate limited");
    assert_eq!(error.retry_at(), DateTime::from_timestamp(1_900_000_000, 0));

    // One limited stack among working ones is only a warning.
    stub.set(TODOS, "", vec![limited()]);
    let batch = source(&base).refresh().await.expect("partial refresh");
    assert_eq!(batch.warnings.len(), 1, "{:?}", batch.warnings);
    assert!(batch.warnings[0].starts_with("To-dos: GitLab rate limit exceeded"));
}

/// Redirects are not followed: the token is a custom header that would
/// otherwise travel to whatever host the redirect names.
#[tokio::test]
async fn redirects_are_not_followed() {
    let elsewhere = Stub::default();
    let elsewhere_base = serve(elsewhere.clone()).await;
    let stub = Stub::default();
    stub.set(
        TODOS,
        "",
        vec![Reply {
            status: StatusCode::FOUND,
            headers: vec![("location", format!("{elsewhere_base}/api/v4/todos"))],
            body: json!([]),
        }],
    );
    let base = serve(stub.clone()).await;

    let error = source(&base)
        .with_columns(vec![stack("To-dos", "todos = true")])
        .refresh()
        .await
        .expect_err("redirected");

    assert!(
        error.to_string().contains(
            "GitLab API returned 302 Found; redirects are not followed, check GITLAB_BASE_URL"
        ),
        "{error}"
    );
    assert_eq!(elsewhere.requests(), Vec::<String>::new());
}

/// Without a token every refresh fails naming the variable to set, and no
/// request is made. `GITLAB_BASE_URL` points the source at a self-managed
/// instance (a trailing slash or `/api/v4` is tolerated).
#[tokio::test]
async fn settings_come_from_the_environment() {
    let stub = Stub::default();
    let base = serve(stub.clone()).await;

    for token in [None, Some("  ")] {
        let env = |key: &str| match key {
            "GITLAB_TOKEN" => token.map(str::to_owned),
            "GITLAB_BASE_URL" => Some(base.clone()),
            _ => None,
        };
        let error = GitlabSource::from_env(&env)
            .refresh()
            .await
            .expect_err("no token");
        assert!(error.to_string().contains("set GITLAB_TOKEN"), "{error}");
    }
    assert_eq!(stub.requests(), Vec::<String>::new());

    for url in [format!("{base}/"), format!("{base}/api/v4/")] {
        let env = |key: &str| match key {
            "GITLAB_TOKEN" => Some(format!(" {TOKEN}\n")),
            "GITLAB_BASE_URL" => Some(url.clone()),
            _ => None,
        };
        GitlabSource::from_env(&env)
            .with_columns(vec![stack("To-dos", "todos = true")])
            .refresh()
            .await
            .expect("refresh");
    }
    assert_eq!(
        stub.requests(),
        vec!["/api/v4/todos?state=pending&per_page=100&page=1"; 2]
    );
    assert_eq!(stub.tokens.lock().unwrap().clone(), vec![TOKEN; 2]);
}

/// Each request has its own time limit, so a hanging stack becomes a warning
/// instead of holding the whole refresh until the aggregator gives up.
#[tokio::test]
async fn a_hanging_stack_times_out_as_a_warning() {
    let stub = Stub::default();
    *stub.slow.lock().unwrap() = Some((
        (ISSUES.to_owned(), "assigned_to_me".to_owned()),
        Duration::from_secs(5),
    ));
    stub.set(
        TODOS,
        "",
        vec![Reply::items(vec![todo(1, "assigned", "T")])],
    );
    let base = serve(stub.clone()).await;

    let batch = source(&base)
        .with_request_timeout(Duration::from_millis(200))
        .refresh()
        .await
        .expect("partial refresh");

    assert_eq!(cards(&batch, "To-dos").len(), 1);
    assert_eq!(batch.warnings, ["Assigned issues: timed out after 200ms"]);
}

/// Each stack also has a time budget for all its pages together: a stack
/// whose pages each answer within the request limit, but add up to more than
/// the budget, becomes a warning naming it and shows no cards, while the
/// other stacks still show theirs.
#[tokio::test]
async fn a_stack_over_its_time_budget_is_a_warning_naming_it() {
    let stub = Stub::default();
    let page = |first: u64, next: &str| {
        Reply::items(vec![merge_request("acme/api", 5, first, "MR")]).header("x-next-page", next)
    };
    stub.set(
        MERGE_REQUESTS,
        "created_by_me",
        vec![
            page(1, "2"),
            page(2, "3"),
            page(3, "4"),
            page(4, "5"),
            page(5, ""),
        ],
    );
    // Five pages of 150 ms each: 750 ms in all.
    *stub.slow.lock().unwrap() = Some((
        (MERGE_REQUESTS.to_owned(), "created_by_me".to_owned()),
        Duration::from_millis(150),
    ));
    stub.set(
        TODOS,
        "",
        vec![Reply::items(vec![todo(1, "assigned", "T")])],
    );
    let base = serve(stub.clone()).await;

    let batch = source(&base)
        .with_stack_budget(Duration::from_millis(400))
        .with_columns(vec![
            stack("Mine", "merge_requests = \"authored\""),
            stack("To-dos", "todos = true"),
        ])
        .refresh()
        .await
        .expect("partial refresh");

    assert_eq!(cards(&batch, "To-dos").len(), 1);
    assert_eq!(cards(&batch, "Mine").len(), 0);
    assert_eq!(batch.warnings, ["Mine: timed out after 400ms"]);
}

/// All the stacks of one refresh share a time limit kept below the
/// aggregator's source timeout: with many slow stacks, the ones the refresh
/// has no time left for become warnings saying so, and the stacks that did
/// answer are still shown, instead of the whole source failing.
#[tokio::test]
async fn many_slow_stacks_do_not_fail_the_whole_source() {
    let stub = Stub::default();
    stub.set(
        TODOS,
        "",
        vec![Reply::items(vec![todo(1, "assigned", "T")])],
    );
    *stub.delay.lock().unwrap() = Duration::from_millis(150);
    let base = serve(stub.clone()).await;

    // Three rounds of three; each round takes 150 ms and the refresh has
    // 400 ms: the third round is cut short.
    let stacks = (0..9)
        .map(|n| stack(&format!("Stack {n}"), "todos = true"))
        .collect();
    let batch = source(&base)
        .with_refresh_budget(Duration::from_millis(400))
        .with_columns(stacks)
        .refresh()
        .await
        .expect("partial refresh");

    for n in 0..6 {
        assert_eq!(cards(&batch, &format!("Stack {n}")).len(), 1, "stack {n}");
    }
    assert_eq!(batch.warnings.len(), 3, "{:?}", batch.warnings);
    for (n, warning) in (6..9).zip(&batch.warnings) {
        assert_eq!(
            warning,
            &format!("Stack {n}: not finished: the refresh used up its 400ms for all stacks")
        );
    }
}

/// The stacks of one refresh are asked for at most three at a time.
#[tokio::test]
async fn stacks_run_three_at_a_time() {
    let stub = Stub::default();
    *stub.delay.lock().unwrap() = Duration::from_millis(60);
    let base = serve(stub.clone()).await;

    let stacks = (0..7)
        .map(|n| stack(&format!("Stack {n}"), "todos = true"))
        .collect();
    source(&base)
        .with_columns(stacks)
        .refresh()
        .await
        .expect("refresh");

    assert_eq!(stub.requests().len(), 7);
    assert_eq!(stub.max_in_flight.load(Ordering::SeqCst), 3);
}

/// A `GITLAB_BASE_URL` without encryption would send the token in the clear
/// on every refresh: the source refuses it, naming the variable, and makes
/// no request. A local address is fine.
#[tokio::test]
async fn an_unencrypted_instance_address_is_refused() {
    let source = GitlabSource::from_env(&|key| match key {
        "GITLAB_TOKEN" => Some("secret-token".to_owned()),
        "GITLAB_BASE_URL" => Some("http://gitlab.example.com".to_owned()),
        _ => None,
    });

    let error = source.refresh().await.expect_err("refused");

    assert!(
        error.to_string().contains("GITLAB_BASE_URL") && error.to_string().contains("https://"),
        "{error}"
    );
    assert!(!error.to_string().contains("secret-token"));
}
