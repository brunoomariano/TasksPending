//! The GitHub source turns API searches into cards without leaking provider details.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use pending_core::{CardSeverity, PendingSource, SourceBatch};
use pending_github::{GithubSource, resolve_token};
use serde_json::{Value, json};

const TOKEN: &str = "ghp_secret_test_token";

/// Canned reply per section: HTTP status, headers and body.
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
    /// Reply per query fragment (`review-requested`, `author`, `assignee`).
    replies: Arc<Mutex<HashMap<&'static str, Reply>>>,
    auth_headers: Arc<Mutex<Vec<String>>>,
    /// Notifications served by `/notifications`, and the `all` param seen.
    notifications: Arc<Mutex<Value>>,
    notifications_all: Arc<Mutex<Vec<String>>>,
    /// Pull request state per node id served by `/graphql`, the id lists
    /// asked for, and a reply that replaces the served one when set.
    pr_states: Arc<Mutex<HashMap<String, Value>>>,
    graphql_requests: Arc<Mutex<Vec<Vec<String>>>>,
    graphql_reply: Arc<Mutex<Option<Reply>>>,
    /// Reply per `owner/repo/kind` (`dependabot`, `secret-scanning`,
    /// `code-scanning`), an empty list otherwise, and the alert requests seen
    /// as `owner/repo/kind?state=…&per_page=…`.
    alerts: Arc<Mutex<HashMap<String, Reply>>>,
    alert_requests: Arc<Mutex<Vec<String>>>,
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

async fn notifications(
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
    stub.notifications_all
        .lock()
        .unwrap()
        .push(params.get("all").cloned().unwrap_or_default());
    axum::Json(stub.notifications.lock().unwrap().clone()).into_response()
}

async fn graphql(
    State(stub): State<Stub>,
    headers: HeaderMap,
    axum::Json(request): axum::Json<Value>,
) -> Response {
    assert_eq!(
        headers.get("authorization").and_then(|v| v.to_str().ok()),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    let query = request["query"].as_str().unwrap_or_default();
    assert!(query.contains("reviewDecision"), "{query}");
    let ids: Vec<String> = request["variables"]["ids"]
        .as_array()
        .expect("ids variable")
        .iter()
        .map(|id| id.as_str().unwrap().to_owned())
        .collect();
    stub.graphql_requests.lock().unwrap().push(ids.clone());
    if let Some(reply) = stub.graphql_reply.lock().unwrap().clone() {
        return (reply.status, axum::Json(reply.body)).into_response();
    }
    let states = stub.pr_states.lock().unwrap();
    let nodes: Vec<Value> = ids
        .iter()
        .map(|id| states.get(id).cloned().unwrap_or(Value::Null))
        .collect();
    axum::Json(json!({ "data": { "nodes": nodes } })).into_response()
}

async fn alerts(
    State(stub): State<Stub>,
    axum::extract::Path((owner, repo, kind)): axum::extract::Path<(String, String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let key = format!("{owner}/{repo}/{kind}");
    let mut request = format!(
        "{key}?state={}&per_page={}",
        params.get("state").cloned().unwrap_or_default(),
        params.get("per_page").cloned().unwrap_or_default()
    );
    if let Some(hide) = params.get("hide_secret") {
        request.push_str(&format!("&hide_secret={hide}"));
    }
    stub.alert_requests.lock().unwrap().push(request);
    let reply = stub.alerts.lock().unwrap().get(&key).cloned();
    match reply {
        Some(reply) => (reply.status, axum::Json(reply.body)).into_response(),
        None => axum::Json(json!([])).into_response(),
    }
}

async fn serve(stub: Stub) -> String {
    let app = Router::new()
        .route("/search/issues", get(search))
        .route("/notifications", get(notifications))
        .route("/graphql", post(graphql))
        .route("/repos/{owner}/{repo}/{kind}/alerts", get(alerts))
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
        value["node_id"] = json!(node_id(repo, number));
    }
    value
}

fn node_id(repo: &str, number: u64) -> String {
    format!("PR_{repo}_{number}")
}

fn pr_state(repo: &str, number: u64, draft: bool, decision: Option<&str>) -> (String, Value) {
    let id = node_id(repo, number);
    let state = json!({ "id": id, "isDraft": draft, "reviewDecision": decision });
    (id, state)
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

/// Pending work becomes three sections: reviews requested from me, my open PRs
/// and issues assigned to me. Each card carries link, repository and number,
/// and a review request is highlighted as a warning, since it blocks someone else.
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

/// The token goes in the authorization header of every search.
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

/// Columns are independent filters: the same item found by two searches shows
/// up in both columns.
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

/// Configured columns replace the defaults: each one is a search with the
/// chosen severity.
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
            query: Some("is:open is:pr org:acme review-requested:@me".to_owned()),
            notifications: None,
            alerts: None,
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

/// If one search fails, the other sections stay on screen and the warning says
/// which section was left out and why.
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

/// When every search fails, the refresh fails with the API's reason, and the
/// message never contains the token.
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

/// An exceeded rate limit is explained as such, with the reset time.
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

/// If the search found more items than were loaded, the source says how many
/// were left out instead of hiding it.
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

/// Without a token, the source queries nothing and explains how to set one up.
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

/// The token comes from `GITHUB_TOKEN`, then `GH_TOKEN`, then `gh auth token`;
/// empty values are ignored.
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

/// With the API down, the error says what happened on the connection, without
/// URL or token.
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

/// A hanging search does not take down the others: it hits its own timeout,
/// becomes a warning with the section name, and the other sections show.
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

/// When GitHub itself reports the search as incomplete, the source passes the
/// warning on instead of presenting the result as complete.
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

/// A draft PR is marked on the card, so it doesn't look ready for review.
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

/// Issues and pull requests with comments say how many on the card; none
/// says nothing.
#[tokio::test]
async fn comment_counts_are_shown_when_there_are_comments() {
    let mut discussed = item("o/api", 7, "Add cache", true);
    discussed["comments"] = json!(3);
    let mut issue = item("o/api", 9, "Crash on start", false);
    issue["comments"] = json!(1);
    let mut quiet = item("o/web", 3, "Fix layout", true);
    quiet["comments"] = json!(0);
    let batch = refresh(stub_with(vec![
        ("author", Reply::items(vec![discussed, quiet])),
        ("assignee", Reply::items(vec![issue])),
    ]))
    .await
    .expect("refresh succeeds");

    let bodies: Vec<&str> = batch.items.iter().map(|i| i.card.body.as_str()).collect();
    assert!(bodies[0].ends_with(" · 3 comments"), "{bodies:?}");
    assert!(!bodies[1].contains("comment"), "{bodies:?}");
    assert!(bodies[2].ends_with(" · 1 comment"), "{bodies:?}");
}

/// Pull request cards say whether they are drafts and where their review
/// stands, from one GraphQL request for every pull request of the refresh
/// (issues are not asked about, and a PR in two stacks is asked once). A PR
/// with changes requested is at least a warning.
#[tokio::test]
async fn pull_requests_show_draft_and_review_state_from_one_lookup() {
    let stub = stub_with(vec![
        (
            "review-requested",
            Reply::items(vec![item("o/api", 1, "Needs review", true)]),
        ),
        (
            "author",
            Reply::items(vec![
                item("o/api", 1, "Needs review", true),
                item("o/api", 2, "Approved", true),
                item("o/api", 3, "Rework", true),
                item("o/api", 4, "Draft", true),
            ]),
        ),
        (
            "assignee",
            Reply::items(vec![item("o/api", 9, "Crash on start", false)]),
        ),
    ]);
    stub.pr_states.lock().unwrap().extend([
        pr_state("o/api", 1, false, Some("REVIEW_REQUIRED")),
        pr_state("o/api", 2, false, Some("APPROVED")),
        pr_state("o/api", 3, false, Some("CHANGES_REQUESTED")),
        pr_state("o/api", 4, true, None),
    ]);
    let requests = stub.graphql_requests.clone();

    let batch = refresh(stub).await.expect("refresh succeeds");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1, "one lookup: {requests:?}");
    let mut ids = requests[0].clone();
    ids.sort();
    assert_eq!(
        ids,
        ["PR_o/api_1", "PR_o/api_2", "PR_o/api_3", "PR_o/api_4"]
    );

    let card = |column: &str, id: &str| {
        batch
            .items
            .iter()
            .find(|i| i.column == column && i.card.id == id)
            .map(|i| i.card.clone())
            .unwrap_or_else(|| panic!("{column} {id}"))
    };
    let review = card("Review requested", "github:o/api#1");
    assert!(review.body.contains("review required"), "{}", review.body);
    assert_eq!(review.severity, CardSeverity::Warning);
    let mine = card("My pull requests", "github:o/api#1");
    assert!(mine.body.contains("review required"), "{}", mine.body);
    assert_eq!(mine.severity, CardSeverity::Info);
    let approved = card("My pull requests", "github:o/api#2");
    assert!(approved.body.contains("approved"), "{}", approved.body);
    assert_eq!(approved.severity, CardSeverity::Info);
    let rework = card("My pull requests", "github:o/api#3");
    assert!(rework.body.contains("changes requested"), "{}", rework.body);
    assert_eq!(rework.severity, CardSeverity::Warning);
    let draft = card("My pull requests", "github:o/api#4");
    assert!(draft.body.contains("draft"), "{}", draft.body);
    assert_eq!(draft.body.matches("draft").count(), 1, "{}", draft.body);
    let issue = card("Assigned issues", "github:o/api#9");
    assert_eq!(issue.body, "o/api#9 · @octocat");
    assert!(batch.warnings.is_empty(), "{:?}", batch.warnings);
}

/// A refresh without pull requests makes no GraphQL request.
#[tokio::test]
async fn no_pull_requests_means_no_review_lookup() {
    let stub = stub_with(vec![(
        "assignee",
        Reply::items(vec![item("o/api", 9, "Crash on start", false)]),
    )]);
    let requests = stub.graphql_requests.clone();

    refresh(stub).await.expect("refresh succeeds");

    assert!(requests.lock().unwrap().is_empty());
}

/// When the review lookup fails, pull requests still show, without review
/// state, and a warning says the review state is missing.
#[tokio::test]
async fn a_failed_review_lookup_keeps_the_cards_with_a_warning() {
    let stub = stub_with(vec![(
        "author",
        Reply::items(vec![item("o/api", 3, "Rework", true)]),
    )]);
    *stub.graphql_reply.lock().unwrap() = Some(Reply::status(StatusCode::BAD_GATEWAY, "upstream"));

    let batch = refresh(stub).await.expect("cards still load");

    assert_eq!(
        sections(&batch),
        vec![("My pull requests".to_owned(), "github:o/api#3".to_owned())]
    );
    assert_eq!(batch.items[0].card.body, "o/api#3 · @octocat");
    assert_eq!(batch.warnings.len(), 1, "{:?}", batch.warnings);
    assert!(
        batch.warnings[0].contains("review state"),
        "{:?}",
        batch.warnings
    );
    assert!(batch.warnings[0].contains("502"), "{:?}", batch.warnings);
    assert!(!batch.warnings[0].contains(TOKEN));
}

/// GraphQL reports problems in an `errors` list, even with status 200: the
/// data that came back is used and the errors become a warning.
#[tokio::test]
async fn graphql_errors_become_a_warning() {
    for (body, shows_state) in [
        (
            json!({
                "data": { "nodes": [
                    { "id": "PR_o/api_3", "isDraft": false, "reviewDecision": "APPROVED" }
                ] },
                "errors": [{ "message": "Could not resolve to a node with the global id" }],
            }),
            true,
        ),
        (
            json!({ "errors": [{ "message": "Something went wrong" }] }),
            false,
        ),
    ] {
        let stub = stub_with(vec![(
            "author",
            Reply::items(vec![item("o/api", 3, "Rework", true)]),
        )]);
        *stub.graphql_reply.lock().unwrap() = Some(Reply {
            status: StatusCode::OK,
            headers: Vec::new(),
            body,
        });

        let batch = refresh(stub).await.expect("cards still load");

        assert_eq!(batch.items.len(), 1);
        assert_eq!(
            batch.items[0].card.body.contains("approved"),
            shows_state,
            "{}",
            batch.items[0].card.body
        );
        assert_eq!(batch.warnings.len(), 1, "{:?}", batch.warnings);
        assert!(
            batch.warnings[0].contains("review state")
                && (batch.warnings[0].contains("Could not resolve")
                    || batch.warnings[0].contains("Something went wrong")),
            "{:?}",
            batch.warnings
        );
    }
}

/// Review state is asked for at most 100 pull requests per request, the
/// most GraphQL `nodes` accepts.
#[tokio::test]
async fn review_lookups_ask_for_at_most_a_hundred_pull_requests() {
    use pending_github::GithubColumn;

    let page = |stack: &str| {
        Reply::items(
            (1..=50)
                .map(|n| item(&format!("o/{stack}"), n, "PR", true))
                .collect(),
        )
    };
    let stub = stub_with(vec![
        ("q1", page("a")),
        ("q2", page("b")),
        ("q3", page("c")),
    ]);
    let requests = stub.graphql_requests.clone();
    let base = serve(stub).await;
    let column = |name: &str| GithubColumn {
        name: name.to_owned(),
        query: Some(name.to_owned()),
        notifications: None,
        alerts: None,
        severity: None,
    };
    GithubSource::new(base, Some(TOKEN.to_owned()))
        .with_columns(vec![column("q1"), column("q2"), column("q3")])
        .refresh()
        .await
        .expect("refresh succeeds");

    let sizes: Vec<usize> = requests.lock().unwrap().iter().map(Vec::len).collect();
    assert_eq!(sizes, [100, 50]);
}

/// The id uses owner and repository even when one of them is named `repos`.
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

/// A token with a trailing space or newline (common when copied from a file)
/// is used without those characters.
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

/// The token comes from `gh auth token` output; failing or empty output, or a
/// hanging `gh`, yields no token, and the hang gives up at the limit.
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

fn notification(id: &str, kind: &str, api_url: Option<&str>, unread: bool) -> Value {
    json!({
        "id": id,
        "unread": unread,
        "reason": "review_requested",
        "updated_at": "2026-09-28T10:00:00Z",
        "subject": { "title": format!("{kind} {id}"), "type": kind, "url": api_url },
        "repository": { "full_name": "o/api", "html_url": "https://github.com/o/api" },
    })
}

/// An `all` column shows read and unread notifications; each card points to
/// the PR, the issue or, for other types, the repository, and unread ones
/// show as warnings.
#[tokio::test]
async fn notification_columns_show_the_inbox() {
    use pending_github::{GithubColumn, Notifications};

    let stub = Stub::default();
    *stub.notifications.lock().unwrap() = json!([
        notification(
            "1",
            "PullRequest",
            Some("https://api.github.com/repos/o/api/pulls/7"),
            true
        ),
        notification(
            "2",
            "Issue",
            Some("https://api.github.com/repos/o/api/issues/9"),
            false
        ),
        notification("3", "Release", None, false),
    ]);
    let seen_all = stub.notifications_all.clone();
    let base = serve(stub).await;
    let source = GithubSource::new(base, Some(TOKEN.to_owned())).with_columns(vec![GithubColumn {
        name: "Notifications".to_owned(),
        query: None,
        notifications: Some(Notifications::All),
        alerts: None,
        severity: None,
    }]);

    let batch = source.refresh().await.expect("refresh succeeds");

    assert_eq!(
        seen_all.lock().unwrap().as_slice(),
        ["true"],
        "read ones too"
    );
    let cards: Vec<(&str, Option<&str>, CardSeverity)> = batch
        .items
        .iter()
        .map(|i| {
            (
                i.card.title.as_str(),
                i.card.url.as_deref(),
                i.card.severity,
            )
        })
        .collect();
    assert_eq!(
        cards,
        vec![
            (
                "PullRequest 1",
                Some("https://github.com/o/api/pull/7"),
                CardSeverity::Warning
            ),
            (
                "Issue 2",
                Some("https://github.com/o/api/issues/9"),
                CardSeverity::Info
            ),
            (
                "Release 3",
                Some("https://github.com/o/api"),
                CardSeverity::Info
            ),
        ]
    );
    assert!(
        batch.items[0].card.body.contains("o/api"),
        "{}",
        batch.items[0].card.body
    );
    assert!(
        batch.items[0].card.body.contains("review requested"),
        "{}",
        batch.items[0].card.body
    );
    assert!(batch.items.iter().all(|i| i.column == "Notifications"));
}

/// `unread` asks for unread ones only.
#[tokio::test]
async fn unread_notification_columns_ask_only_for_unread() {
    use pending_github::{GithubColumn, Notifications};

    let stub = Stub::default();
    *stub.notifications.lock().unwrap() = json!([]);
    let seen_all = stub.notifications_all.clone();
    let base = serve(stub).await;
    let source = GithubSource::new(base, Some(TOKEN.to_owned())).with_columns(vec![GithubColumn {
        name: "Unread".to_owned(),
        query: None,
        notifications: Some(Notifications::Unread),
        alerts: None,
        severity: None,
    }]);

    source.refresh().await.expect("refresh succeeds");

    assert_eq!(seen_all.lock().unwrap().as_slice(), ["false"]);
}

fn alerts_source(base: String, repos: &[&str]) -> GithubSource {
    GithubSource::new(base, Some(TOKEN.to_owned())).with_columns(vec![
        pending_github::GithubColumn {
            name: "Security".to_owned(),
            query: None,
            notifications: None,
            alerts: Some(repos.iter().map(|repo| (*repo).to_owned()).collect()),
            severity: None,
        },
    ])
}

fn dependabot_alert(number: u64, package: &str, severity: &str, summary: &str) -> Value {
    json!({
        "number": number,
        "state": "open",
        "html_url": format!("https://github.com/o/api/security/dependabot/{number}"),
        "created_at": "2026-09-01T10:00:00Z",
        "updated_at": "2026-09-20T10:00:00Z",
        "dependency": { "package": { "ecosystem": "npm", "name": package }, "manifest_path": "package-lock.json" },
        "security_advisory": { "ghsa_id": "GHSA-xxxx", "summary": summary, "severity": severity },
        "security_vulnerability": { "severity": severity, "package": { "ecosystem": "npm", "name": package } },
    })
}

fn ok(body: Value) -> Reply {
    Reply {
        status: StatusCode::OK,
        headers: Vec::new(),
        body,
    }
}

/// An `alerts` stack shows the open Dependabot, secret scanning and code
/// scanning alerts of each listed repository as cards linking to the alert.
/// Leaked secrets are critical; critical and high advisories or rules are
/// critical and warning; the rest is info.
#[tokio::test]
async fn open_security_alerts_become_cards() {
    let stub = Stub::default();
    stub.alerts.lock().unwrap().extend([
        (
            "o/api/dependabot".to_owned(),
            ok(json!([
                dependabot_alert(1, "lodash", "critical", "Prototype pollution in lodash"),
                dependabot_alert(2, "minimist", "medium", "Minimist bug"),
            ])),
        ),
        (
            "o/api/secret-scanning".to_owned(),
            ok(json!([{
                "number": 3,
                "state": "open",
                "html_url": "https://github.com/o/api/security/secret-scanning/3",
                "created_at": "2026-09-02T10:00:00Z",
                "updated_at": null,
                "secret_type": "aws_access_key_id",
                "secret_type_display_name": "AWS Access Key ID",
            }])),
        ),
        (
            "o/web/code-scanning".to_owned(),
            ok(json!([{
                "number": 4,
                "state": "open",
                "html_url": "https://github.com/o/web/security/code-scanning/4",
                "created_at": "2026-09-03T10:00:00Z",
                "updated_at": "2026-09-04T10:00:00Z",
                "rule": {
                    "id": "js/sql-injection",
                    "severity": "error",
                    "security_severity_level": "high",
                    "description": "Database query built from user-controlled sources",
                },
                "tool": { "name": "CodeQL" },
            }])),
        ),
    ]);
    let requests = stub.alert_requests.clone();
    let base = serve(stub).await;

    let batch = alerts_source(base, &["o/api", "o/web"])
        .refresh()
        .await
        .expect("refresh succeeds");

    let mut requests = requests.lock().unwrap().clone();
    requests.sort();
    assert_eq!(
        requests,
        [
            "o/api/code-scanning?state=open&per_page=100",
            "o/api/dependabot?state=open&per_page=100",
            "o/api/secret-scanning?state=open&per_page=100&hide_secret=true",
            "o/web/code-scanning?state=open&per_page=100",
            "o/web/dependabot?state=open&per_page=100",
            "o/web/secret-scanning?state=open&per_page=100&hide_secret=true",
        ]
    );
    assert!(batch.warnings.is_empty(), "{:?}", batch.warnings);
    assert!(batch.items.iter().all(|i| i.column == "Security"));

    let card = |url: &str| {
        batch
            .items
            .iter()
            .find(|i| i.card.url.as_deref() == Some(url))
            .map(|i| i.card.clone())
            .unwrap_or_else(|| panic!("no card for {url}"))
    };
    let lodash = card("https://github.com/o/api/security/dependabot/1");
    assert_eq!(lodash.title, "o/api: Prototype pollution in lodash");
    assert!(lodash.body.contains("Dependabot"), "{}", lodash.body);
    assert!(lodash.body.contains("lodash"), "{}", lodash.body);
    assert!(lodash.body.contains("critical"), "{}", lodash.body);
    assert_eq!(lodash.severity, CardSeverity::Critical);
    assert_eq!(lodash.updated_at.to_rfc3339(), "2026-09-20T10:00:00+00:00");
    let minimist = card("https://github.com/o/api/security/dependabot/2");
    assert_eq!(minimist.severity, CardSeverity::Info);

    let secret = card("https://github.com/o/api/security/secret-scanning/3");
    assert!(secret.title.starts_with("o/api: "), "{}", secret.title);
    assert!(
        secret.title.contains("AWS Access Key ID"),
        "{}",
        secret.title
    );
    assert!(secret.body.contains("Secret scanning"), "{}", secret.body);
    assert_eq!(secret.severity, CardSeverity::Critical);
    assert_eq!(secret.updated_at.to_rfc3339(), "2026-09-02T10:00:00+00:00");

    let code = card("https://github.com/o/web/security/code-scanning/4");
    assert_eq!(
        code.title,
        "o/web: Database query built from user-controlled sources"
    );
    assert!(code.body.contains("Code scanning"), "{}", code.body);
    assert!(code.body.contains("js/sql-injection"), "{}", code.body);
    assert!(code.body.contains("high"), "{}", code.body);
    assert_eq!(code.severity, CardSeverity::Warning);

    let mut ids: Vec<&str> = batch.items.iter().map(|i| i.card.id.as_str()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 4, "ids are unique: {ids:?}");
}

/// An alert type that is not set up for a repository (404, e.g. no code
/// scanning analysis yet) just has no alerts: no warning, so a normal repo
/// does not keep the source degraded. One the token may not read, or that is
/// disabled (403), is a warning naming the repository and the alert type;
/// the other types and repositories still show.
#[tokio::test]
async fn unavailable_alert_types_are_warnings_naming_repo_and_type() {
    let stub = Stub::default();
    stub.alerts.lock().unwrap().extend([
        (
            "o/api/dependabot".to_owned(),
            ok(json!([dependabot_alert(1, "lodash", "high", "Bug")])),
        ),
        (
            "o/api/secret-scanning".to_owned(),
            Reply::status(
                StatusCode::NOT_FOUND,
                "Secret scanning is disabled on this repository.",
            ),
        ),
        (
            "o/web/code-scanning".to_owned(),
            Reply::status(
                StatusCode::FORBIDDEN,
                "Resource not accessible by integration",
            ),
        ),
    ]);
    let base = serve(stub).await;

    let batch = alerts_source(base, &["o/api", "o/web"])
        .refresh()
        .await
        .expect("the rest still loads");

    assert_eq!(batch.items.len(), 1);
    assert_eq!(batch.items[0].card.severity, CardSeverity::Warning);
    assert_eq!(batch.warnings.len(), 1, "{:?}", batch.warnings);
    let code = &batch.warnings[0];
    assert!(
        code.contains("code scanning") && code.contains("o/web") && code.contains("403"),
        "{code}"
    );
    assert!(batch.warnings.iter().all(|w| !w.contains(TOKEN)));
}

/// A repository where every alert type answers 404 is most likely a typo or
/// a repository the token cannot see: that is a warning naming it.
#[tokio::test]
async fn a_repo_with_no_alert_endpoints_is_a_warning() {
    let stub = Stub::default();
    for kind in ["dependabot", "secret-scanning", "code-scanning"] {
        stub.alerts.lock().unwrap().insert(
            format!("o/typo/{kind}"),
            Reply::status(StatusCode::NOT_FOUND, "Not Found"),
        );
    }
    stub.alerts.lock().unwrap().insert(
        "o/api/dependabot".to_owned(),
        ok(json!([dependabot_alert(1, "lodash", "high", "Bug")])),
    );
    let base = serve(stub).await;

    let batch = alerts_source(base, &["o/api", "o/typo"])
        .refresh()
        .await
        .expect("o/api still loads");

    assert_eq!(batch.items.len(), 1);
    assert_eq!(batch.warnings.len(), 1, "{:?}", batch.warnings);
    assert!(batch.warnings[0].contains("o/typo"), "{:?}", batch.warnings);
}

/// When no alert of a stack can be read, the stack fails like a failed
/// search: here the only stack, so the refresh fails with the reasons.
#[tokio::test]
async fn a_stack_whose_alerts_all_fail_fails() {
    let stub = Stub::default();
    for kind in ["dependabot", "secret-scanning", "code-scanning"] {
        stub.alerts.lock().unwrap().insert(
            format!("o/api/{kind}"),
            Reply::status(StatusCode::FORBIDDEN, "Resource not accessible"),
        );
    }
    let base = serve(stub).await;

    let error = alerts_source(base, &["o/api"])
        .refresh()
        .await
        .expect_err("nothing could be read");

    let message = error.to_string();
    assert!(message.contains("Security"), "{message}");
    assert!(message.contains("Dependabot"), "{message}");
    assert!(message.contains("403"), "{message}");
}

/// Only the first 100 alerts of a type are read; a full page says so.
#[tokio::test]
async fn a_full_page_of_alerts_is_reported() {
    let stub = Stub::default();
    let page: Vec<Value> = (1..=100)
        .map(|n| dependabot_alert(n, "pkg", "low", "Bug"))
        .collect();
    stub.alerts
        .lock()
        .unwrap()
        .insert("o/api/dependabot".to_owned(), ok(json!(page)));
    let base = serve(stub).await;

    let batch = alerts_source(base, &["o/api"])
        .refresh()
        .await
        .expect("refresh succeeds");

    assert_eq!(batch.items.len(), 100);
    assert!(
        batch
            .warnings
            .iter()
            .any(|w| w.contains("o/api") && w.contains("Dependabot") && w.contains("100")),
        "{:?}",
        batch.warnings
    );
}
