//! GitLab as a pending-work source: open merge requests and issues of the
//! authenticated user, and pending to-do items, via the REST API (v4).

use std::time::Duration;

use chrono::{DateTime, Local, NaiveDate, Utc};
use futures_util::future::join_all;
use pending_core::{
    BoxFuture, CardSeverity, PendingCard, PendingSource, SourceBatch, SourceError, SourceItem,
};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::redirect::Policy;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

pub const DEFAULT_BASE_URL: &str = "https://gitlab.com";
/// Environment variable holding the personal access token (`read_api`).
pub const TOKEN_ENV: &str = "GITLAB_TOKEN";
/// Environment variable naming a self-managed instance.
pub const BASE_URL_ENV: &str = "GITLAB_BASE_URL";

/// Per-request limit, below the aggregator's default refresh timeout (60s)
/// so a hanging request becomes a warning instead of failing the refresh.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum page size of the GitLab API.
const PAGE_SIZE: usize = 100;
/// Pages read per stack before giving up with a warning.
const MAX_PAGES: u32 = 5;
/// Stacks asked for at once.
const MAX_CONCURRENT_STACKS: usize = 3;

/// One stack: merge requests, issues or to-do items. Set exactly one of
/// `merge_requests`, `issues` and `todos`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitlabColumn {
    #[serde(skip)]
    pub name: String,
    /// Open merge requests by your role in them.
    #[serde(default)]
    pub merge_requests: Option<MergeRequests>,
    /// Open issues by your role in them.
    #[serde(default)]
    pub issues: Option<Issues>,
    /// `true` for pending to-do items.
    #[serde(default)]
    pub todos: Option<bool>,
    /// Only this group and its subgroups, by full path (or numeric id).
    #[serde(default)]
    pub group: Option<String>,
    /// Only this project, by full path (or numeric id).
    #[serde(default)]
    pub project: Option<String>,
    /// Only items carrying every one of these labels.
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    /// Only drafts (`true`) or only non-drafts (`false`); merge requests.
    #[serde(default)]
    pub draft: Option<bool>,
    /// Severity of every card in this stack. Without it, review requests
    /// are warnings, overdue issues critical and the rest info.
    #[serde(default)]
    pub severity: Option<CardSeverity>,
}

/// Which merge requests a stack shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeRequests {
    /// You are a reviewer.
    ReviewRequested,
    /// You are an assignee.
    Assigned,
    /// You created it.
    Authored,
}

impl MergeRequests {
    /// The API's `scope` value.
    fn scope(self) -> &'static str {
        match self {
            Self::ReviewRequested => "reviews_for_me",
            Self::Assigned => "assigned_to_me",
            Self::Authored => "created_by_me",
        }
    }
}

/// Which issues a stack shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Issues {
    /// You are an assignee.
    Assigned,
    /// You created it.
    Authored,
}

impl Issues {
    /// The API's `scope` value.
    fn scope(self) -> &'static str {
        match self {
            Self::Assigned => "assigned_to_me",
            Self::Authored => "created_by_me",
        }
    }
}

impl GitlabColumn {
    /// A stack needs exactly one of `merge_requests`, `issues` and `todos`;
    /// the narrowing keys must fit that kind.
    pub fn validate(&self) -> Result<(), String> {
        let kinds = [
            self.merge_requests.is_some(),
            self.issues.is_some(),
            self.todos.is_some(),
        ];
        if kinds.iter().filter(|set| **set).count() != 1 {
            return Err("set exactly one of `merge_requests`, `issues` or `todos`".to_owned());
        }
        if self.todos == Some(false) {
            return Err(
                "`todos` must be `true`; use `enabled = false` to switch a stack off".to_owned(),
            );
        }
        if self.todos.is_some() {
            for (key, set) in [
                ("group", self.group.is_some()),
                ("project", self.project.is_some()),
                ("labels", self.labels.is_some()),
            ] {
                if set {
                    return Err(format!("`{key}` does not apply to `todos`"));
                }
            }
        }
        if self.draft.is_some() && self.merge_requests.is_none() {
            return Err("`draft` applies to `merge_requests` only".to_owned());
        }
        if self.group.is_some() && self.project.is_some() {
            return Err("set at most one of `group` or `project`".to_owned());
        }
        for (key, path) in [("group", &self.group), ("project", &self.project)] {
            if let Some(path) = path
                && !is_path(path)
            {
                return Err(format!(
                    "`{key}` is a full path such as `acme/platform`, found `{path}`"
                ));
            }
        }
        if let Some(labels) = &self.labels {
            if labels.is_empty() {
                return Err("`labels` needs at least one label".to_owned());
            }
            if let Some(bad) = labels
                .iter()
                .find(|label| label.trim().is_empty() || label.contains(','))
            {
                return Err(format!(
                    "`labels` entries are single label names, found `{bad}`"
                ));
            }
        }
        Ok(())
    }

    /// The list endpoint and filter parameters of this stack.
    fn request(&self) -> (String, Vec<(&'static str, String)>) {
        let (resource, scope, merge_requests) = match (self.merge_requests, self.issues) {
            (Some(which), _) => ("merge_requests", which.scope(), true),
            (None, Some(which)) => ("issues", which.scope(), false),
            (None, None) => {
                return ("/todos".to_owned(), vec![("state", "pending".to_owned())]);
            }
        };
        let path = match (&self.group, &self.project) {
            (Some(group), _) => format!("/groups/{}/{resource}", encode_segment(group)),
            (None, Some(project)) => format!("/projects/{}/{resource}", encode_segment(project)),
            (None, None) => format!("/{resource}"),
        };
        let mut query = vec![
            ("scope", scope.to_owned()),
            ("state", "opened".to_owned()),
            ("order_by", "updated_at".to_owned()),
            ("sort", "desc".to_owned()),
        ];
        // Archived projects are read-only: nothing there is pending. The
        // project endpoint has no such parameter.
        if merge_requests && self.project.is_none() {
            query.push(("non_archived", "true".to_owned()));
        }
        if let Some(labels) = &self.labels {
            query.push(("labels", labels.join(",")));
        }
        if let Some(draft) = self.draft {
            query.push(("draft", draft.to_string()));
        }
        (path, query)
    }
}

/// A group or project path (or numeric id): non-empty parts separated by
/// `/`, without spaces.
fn is_path(path: &str) -> bool {
    !path.is_empty()
        && path
            .split('/')
            .all(|part| !part.is_empty() && !part.chars().any(char::is_whitespace))
}

/// One URL path segment: everything but unreserved characters is
/// percent-encoded, so `acme/api` becomes `acme%2Fapi` as the API expects.
fn encode_segment(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Stacks used when the configuration declares none.
pub fn default_columns() -> Vec<GitlabColumn> {
    let column = |name: &str| GitlabColumn {
        name: name.to_owned(),
        ..GitlabColumn::default()
    };
    vec![
        GitlabColumn {
            merge_requests: Some(MergeRequests::ReviewRequested),
            ..column("Review requested")
        },
        GitlabColumn {
            merge_requests: Some(MergeRequests::Assigned),
            ..column("Assigned merge requests")
        },
        GitlabColumn {
            issues: Some(Issues::Assigned),
            ..column("Assigned issues")
        },
        GitlabColumn {
            todos: Some(true),
            ..column("To-dos")
        },
    ]
}

pub struct GitlabSource {
    client: Client,
    /// `{base}/api/v4`.
    api_url: String,
    token: Result<String, String>,
    request_timeout: Duration,
    columns: Vec<GitlabColumn>,
    today: fn() -> NaiveDate,
}

fn local_today() -> NaiveDate {
    Local::now().date_naive()
}

impl GitlabSource {
    /// `base_url` is the instance's address (`https://gitlab.com`). With
    /// `Err`, every refresh fails with that message (missing token).
    pub fn new(base_url: impl Into<String>, token: Result<String, String>) -> Self {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            // The token is a custom header, which reqwest would forward to
            // whatever host a redirect points at.
            .redirect(Policy::none())
            .build()
            .unwrap_or_default();
        let base_url = base_url.into();
        let base_url = base_url.trim().trim_end_matches('/');
        let base_url = base_url.strip_suffix("/api/v4").unwrap_or(base_url);
        Self {
            client,
            api_url: format!("{base_url}/api/v4"),
            token,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            columns: default_columns(),
            today: local_today,
        }
    }

    /// Reads the token from [`TOKEN_ENV`] and the instance from
    /// [`BASE_URL_ENV`] (default [`DEFAULT_BASE_URL`]).
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Self {
        let non_empty = |key: &str| {
            env(key)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        let token = non_empty(TOKEN_ENV).ok_or_else(|| {
            format!(
                "GitLab is not configured: set {TOKEN_ENV} to a personal access token with the \
                 read_api scope (and {BASE_URL_ENV} for a self-managed instance), then restart"
            )
        });
        let base_url = non_empty(BASE_URL_ENV).unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        // The token is a header on every request: never send it unencrypted.
        let token = token.and_then(|token| {
            pending_http::require_encrypted(&base_url, BASE_URL_ENV)
                .map(|()| token)
                .map_err(|problem| format!("GitLab is not configured: {problem}"))
        });
        Self::new(base_url, token)
    }

    /// Replaces the default stacks; an empty list keeps the defaults.
    pub fn with_columns(mut self, columns: Vec<GitlabColumn>) -> Self {
        if !columns.is_empty() {
            self.columns = columns;
        }
        self
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// The date "overdue" is measured against (the local date by default).
    pub fn with_today(mut self, today: fn() -> NaiveDate) -> Self {
        self.today = today;
        self
    }

    async fn refresh_with(&self, token: &str) -> Result<SourceBatch, SourceError> {
        let mut header = HeaderValue::from_str(token).map_err(|_| {
            SourceError::new(format!(
                "{TOKEN_ENV} has characters that cannot be sent in a header"
            ))
        })?;
        header.set_sensitive(true);
        let auth = Auth { token, header };

        // A few at a time, so an instance never gets a burst. Results keep
        // stack order.
        let mut results = Vec::with_capacity(self.columns.len());
        for chunk in self.columns.chunks(MAX_CONCURRENT_STACKS) {
            results.extend(
                join_all(chunk.iter().map(|column| self.column_cards(&auth, column))).await,
            );
        }

        let mut batch = SourceBatch::default();
        let mut failures = Vec::new();
        let mut retry_at = None;
        for (column, result) in self.columns.iter().zip(results) {
            match result {
                Ok((cards, truncated)) => {
                    if truncated {
                        batch.warnings.push(format!(
                            "{}: more than {MAX_PAGES} pages of results; showing the first ones",
                            column.name
                        ));
                    }
                    batch.items.extend(cards.into_iter().map(|card| SourceItem {
                        column: column.name.clone(),
                        card,
                    }));
                }
                Err(failure) => {
                    retry_at = retry_at.max(failure.retry_at);
                    failures.push(format!("{}: {}", column.name, failure.message));
                }
            }
        }

        if failures.len() == self.columns.len() {
            let error = SourceError::new(format!(
                "all GitLab requests failed; {}",
                failures.join("; ")
            ));
            return Err(match retry_at {
                Some(at) => error.with_retry_at(at),
                None => error,
            });
        }
        batch.warnings.extend(failures);
        Ok(batch)
    }

    /// The cards of one stack; `true` when pages were left unread.
    async fn column_cards(
        &self,
        auth: &Auth<'_>,
        column: &GitlabColumn,
    ) -> Result<(Vec<PendingCard>, bool), Failure> {
        let (path, query) = column.request();
        let hint = NotFoundHint::of(column);
        if column.merge_requests.is_some() {
            let default = match column.merge_requests {
                Some(MergeRequests::ReviewRequested) => CardSeverity::Warning,
                _ => CardSeverity::Info,
            };
            let severity = column.severity.unwrap_or(default);
            let (items, truncated) = self
                .pages::<MergeRequest>(auth, &path, &query, hint)
                .await?;
            let cards = items
                .into_iter()
                .map(|item| item.into_card(severity))
                .collect();
            Ok((cards, truncated))
        } else if column.issues.is_some() {
            let today = (self.today)();
            let (items, truncated) = self.pages::<Issue>(auth, &path, &query, hint).await?;
            let cards = items
                .into_iter()
                .map(|item| item.into_card(column.severity, today))
                .collect();
            Ok((cards, truncated))
        } else {
            let severity = column.severity.unwrap_or(CardSeverity::Info);
            let (items, truncated) = self.pages::<Todo>(auth, &path, &query, hint).await?;
            let cards = items
                .into_iter()
                .map(|item| item.into_card(severity))
                .collect();
            Ok((cards, truncated))
        }
    }

    /// Every page of a list endpoint, up to [`MAX_PAGES`]; `true` when pages
    /// were left unread.
    async fn pages<T: DeserializeOwned>(
        &self,
        auth: &Auth<'_>,
        path: &str,
        filter: &[(&'static str, String)],
        hint: NotFoundHint,
    ) -> Result<(Vec<T>, bool), Failure> {
        let mut all = Vec::new();
        let mut page = 1u32;
        for _ in 0..MAX_PAGES {
            let mut query = filter.to_vec();
            query.push(("per_page", PAGE_SIZE.to_string()));
            query.push(("page", page.to_string()));
            let (items, headers): (Vec<T>, HeaderMap) = self.get(auth, path, &query, hint).await?;

            // `x-next-page` is empty on the last page. Should a proxy drop
            // the header, a full page means there may be more.
            let next = match headers.get("x-next-page") {
                Some(value) => value
                    .to_str()
                    .ok()
                    .and_then(|next| next.trim().parse::<u32>().ok()),
                None => (items.len() >= PAGE_SIZE).then_some(page + 1),
            };
            all.extend(items);
            match next {
                // A page that does not advance would loop.
                Some(next) if next > page => page = next,
                _ => return Ok((all, false)),
            }
        }
        Ok((all, true))
    }

    /// GET `{base}/api/v4{path}`. Errors are safe to show and log: never the
    /// token or the request URL.
    async fn get<T: DeserializeOwned>(
        &self,
        auth: &Auth<'_>,
        path: &str,
        query: &[(&'static str, String)],
        hint: NotFoundHint,
    ) -> Result<(T, HeaderMap), Failure> {
        let response = self
            .client
            .get(format!("{}{path}", self.api_url))
            .query(query)
            .timeout(self.request_timeout)
            .header("PRIVATE-TOKEN", auth.header.clone())
            .header("accept", "application/json")
            .header(
                "user-agent",
                concat!("tasks-pending/", env!("CARGO_PKG_VERSION")),
            )
            .send()
            .await
            .map_err(|error| self.describe(error))?;

        let status = response.status();
        if status.is_success() {
            let headers = response.headers().clone();
            let body = response
                .json::<T>()
                .await
                .map_err(|error| format!("unexpected GitLab response: {}", self.describe(error)))?;
            return Ok((body, headers));
        }
        if status.is_redirection() {
            return Err(format!(
                "GitLab API returned {status}; redirects are not followed, check {BASE_URL_ENV}"
            )
            .into());
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_at = retry_time(response.headers());
            let message = match retry_at {
                Some(at) => format!("GitLab rate limit exceeded; retry at {}", at.to_rfc3339()),
                None => "GitLab rate limit exceeded".to_owned(),
            };
            return Err(Failure { message, retry_at });
        }

        // `{"message": …}` for most errors, `{"error": …}` for OAuth ones;
        // rate limiting and proxies answer in plain text or HTML.
        let body = response.json::<Value>().await.unwrap_or_default();
        let detail = ["message", "error"]
            .iter()
            .find_map(|key| body.get(key).and_then(Value::as_str))
            .map(|detail| detail.trim().replace(auth.token, "***"))
            // GitLab's default message repeats the status ("401 Unauthorized").
            .filter(|detail| {
                !detail.is_empty() && !detail.eq_ignore_ascii_case(&status.to_string())
            });
        let mut message = format!("GitLab API returned {status}");
        if let Some(detail) = detail {
            message.push_str(": ");
            message.push_str(&detail);
        }
        match status {
            StatusCode::UNAUTHORIZED => {
                message.push_str(&format!(
                    "; check that {TOKEN_ENV} is valid and not expired"
                ));
            }
            StatusCode::FORBIDDEN => message.push_str("; the token needs the read_api scope"),
            StatusCode::NOT_FOUND => message.push_str(hint.text()),
            _ => {}
        }
        Err(message.into())
    }

    /// The error and its causes, without the request URL.
    fn describe(&self, error: reqwest::Error) -> String {
        pending_http::describe_error(error, CONNECT_TIMEOUT, self.request_timeout)
    }
}

impl PendingSource for GitlabSource {
    fn columns(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.name.clone()).collect()
    }

    fn refresh(&self) -> BoxFuture<'_, Result<SourceBatch, SourceError>> {
        Box::pin(async move {
            match &self.token {
                Ok(token) => self.refresh_with(token).await,
                Err(message) => Err(SourceError::new(message.clone())),
            }
        })
    }
}

/// The token, as text (to keep it out of messages) and as a header value.
struct Auth<'a> {
    token: &'a str,
    header: HeaderValue,
}

/// What a 404 most likely means for a stack.
#[derive(Clone, Copy)]
enum NotFoundHint {
    Group,
    Project,
    /// Not narrowed: the instance address is the suspect.
    Instance,
}

impl NotFoundHint {
    fn of(column: &GitlabColumn) -> Self {
        if column.group.is_some() {
            Self::Group
        } else if column.project.is_some() {
            Self::Project
        } else {
            Self::Instance
        }
    }

    fn text(self) -> &'static str {
        match self {
            Self::Group => "; check the `group` path and that the token can see it",
            Self::Project => "; check the `project` path and that the token can see it",
            Self::Instance => "; check GITLAB_BASE_URL",
        }
    }
}

/// Why one stack failed; safe to show and log.
struct Failure {
    message: String,
    /// When GitLab says the rate limit resets.
    retry_at: Option<DateTime<Utc>>,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self {
            message,
            retry_at: None,
        }
    }
}

/// When a rate-limited request may be tried again: `Retry-After` (seconds
/// from now), else `RateLimit-Reset` (Unix time).
fn retry_time(headers: &HeaderMap) -> Option<DateTime<Utc>> {
    let number =
        |name: &str| -> Option<i64> { headers.get(name)?.to_str().ok()?.trim().parse().ok() };
    if let Some(seconds) = number("retry-after").filter(|seconds| *seconds >= 0) {
        return Utc::now().checked_add_signed(chrono::Duration::try_seconds(seconds)?);
    }
    DateTime::from_timestamp(number("ratelimit-reset")?, 0)
}

#[derive(Deserialize)]
struct User {
    username: String,
}

#[derive(Deserialize)]
struct References {
    /// `group/project!12` or `group/project#12`.
    full: String,
}

/// `reference · @author`, the start of a merge request or issue body.
fn reference_and_author(
    references: Option<References>,
    fallback: String,
    author: Option<User>,
) -> String {
    let mut body = references.map_or(fallback, |references| references.full);
    if let Some(author) = author {
        body.push_str(&format!(" · @{}", author.username));
    }
    body
}

fn push_comments(body: &mut String, count: u64) {
    match count {
        0 => {}
        1 => body.push_str(" · 1 comment"),
        n => body.push_str(&format!(" · {n} comments")),
    }
}

#[derive(Deserialize)]
struct MergeRequest {
    iid: u64,
    project_id: u64,
    title: String,
    web_url: String,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    author: Option<User>,
    #[serde(default)]
    draft: bool,
    /// The old name of `draft`.
    #[serde(default)]
    work_in_progress: bool,
    #[serde(default)]
    user_notes_count: u64,
    #[serde(default)]
    references: Option<References>,
}

impl MergeRequest {
    fn into_card(self, severity: CardSeverity) -> PendingCard {
        let mut body = reference_and_author(self.references, format!("!{}", self.iid), self.author);
        if self.draft || self.work_in_progress {
            body.push_str(" · draft");
        }
        push_comments(&mut body, self.user_notes_count);
        PendingCard {
            id: format!("gitlab:mr:{}:{}", self.project_id, self.iid),
            title: self.title,
            body,
            source: "gitlab".to_owned(),
            url: Some(self.web_url),
            due_at: None,
            severity,
            updated_at: self.updated_at,
        }
    }
}

#[derive(Deserialize)]
struct Issue {
    iid: u64,
    project_id: u64,
    title: String,
    web_url: String,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    author: Option<User>,
    #[serde(default)]
    user_notes_count: u64,
    #[serde(default)]
    due_date: Option<NaiveDate>,
    #[serde(default)]
    references: Option<References>,
}

impl Issue {
    /// A due date before `today` is overdue: critical unless the stack sets
    /// its own severity.
    fn into_card(self, severity: Option<CardSeverity>, today: NaiveDate) -> PendingCard {
        let mut body = reference_and_author(self.references, format!("#{}", self.iid), self.author);
        push_comments(&mut body, self.user_notes_count);
        let overdue = self.due_date.is_some_and(|due| due < today);
        if let Some(due) = self.due_date {
            body.push_str(&format!(
                " · {} {due}",
                if overdue { "overdue since" } else { "due" }
            ));
        }
        PendingCard {
            id: format!("gitlab:issue:{}:{}", self.project_id, self.iid),
            title: self.title,
            body,
            source: "gitlab".to_owned(),
            url: Some(self.web_url),
            // Local midnight, matching how "overdue" is decided.
            due_at: self
                .due_date
                .and_then(|due| due.and_hms_opt(0, 0, 0))
                .and_then(|due| due.and_local_timezone(Local).earliest())
                .map(|due| due.with_timezone(&Utc)),
            severity: severity.unwrap_or(if overdue {
                CardSeverity::Critical
            } else {
                CardSeverity::Info
            }),
            updated_at: self.updated_at,
        }
    }
}

/// One to-do item.
#[derive(Deserialize)]
struct Todo {
    id: u64,
    #[serde(default)]
    action_name: Option<String>,
    #[serde(default)]
    target_type: Option<String>,
    #[serde(default)]
    target: Option<TodoTarget>,
    #[serde(default)]
    target_url: Option<String>,
    /// The note or description that caused the to-do.
    #[serde(default)]
    body: Option<String>,
    /// Absent for group-level to-dos (epics, access requests).
    #[serde(default)]
    project: Option<TodoProject>,
    #[serde(default)]
    group: Option<TodoGroup>,
    #[serde(default)]
    author: Option<User>,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    updated_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct TodoTarget {
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize)]
struct TodoProject {
    #[serde(default)]
    path_with_namespace: Option<String>,
}

#[derive(Deserialize)]
struct TodoGroup {
    #[serde(default)]
    full_path: Option<String>,
}

impl Todo {
    fn into_card(self, severity: CardSeverity) -> PendingCard {
        let filled = |text: Option<String>| text.filter(|text| !text.trim().is_empty());
        let title = filled(self.target.and_then(|target| target.title))
            .or_else(|| filled(self.body))
            .or_else(|| filled(self.target_type))
            .unwrap_or_else(|| "To-do".to_owned());
        let place = self
            .project
            .and_then(|project| project.path_with_namespace)
            .or_else(|| self.group.and_then(|group| group.full_path));
        let parts = [
            filled(self.action_name).map(|action| action.replace('_', " ")),
            filled(place),
            self.author.map(|author| format!("@{}", author.username)),
        ];
        PendingCard {
            id: format!("gitlab:todo:{}", self.id),
            title,
            body: parts.into_iter().flatten().collect::<Vec<_>>().join(" · "),
            source: "gitlab".to_owned(),
            url: self.target_url,
            due_at: None,
            severity,
            updated_at: self
                .updated_at
                .or(self.created_at)
                .unwrap_or(DateTime::UNIX_EPOCH),
        }
    }
}
