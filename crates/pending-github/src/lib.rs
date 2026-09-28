//! GitHub as a pending-work source: review requests, open pull requests and
//! assigned issues of the authenticated user, via the search API.

use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::future::join_all;
use pending_core::{
    BoxFuture, CardSeverity, PendingCard, PendingSource, SourceBatch, SourceError, SourceItem,
};
use reqwest::{Client, StatusCode};
use serde::Deserialize;

pub const DEFAULT_API_URL: &str = "https://api.github.com";

/// Results per search. More than this is reported as a warning.
const PAGE_SIZE: usize = 50;

/// Per-search limit, below the aggregator's default refresh timeout (60s) so a
/// hanging search becomes a warning instead of failing the whole refresh.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Searches running at once. Each column is one search, and the search API
/// allows 30 per minute per user.
const MAX_CONCURRENT_SEARCHES: usize = 3;

/// One column: a GitHub search query (the search syntax of github.com).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubColumn {
    #[serde(skip)]
    pub name: String,
    /// A search query (github.com search syntax). Set this or `notifications`.
    #[serde(default)]
    pub query: Option<String>,
    /// Notifications instead of a search.
    #[serde(default)]
    pub notifications: Option<Notifications>,
    /// Severity of the cards in this column (default `info`; unread
    /// notifications are warnings).
    #[serde(default)]
    pub severity: Option<CardSeverity>,
}

/// Which notifications a column shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Notifications {
    /// Read and unread. The REST API cannot tell which ones were marked as
    /// done on github.com, so those show too. `inbox` is the old name.
    #[serde(alias = "inbox")]
    All,
    /// Unread only; matches the "Unread" tab of github.com.
    Unread,
}

impl GithubColumn {
    /// A column needs exactly one of `query` and `notifications`.
    pub fn validate(&self) -> Result<(), String> {
        match (&self.query, &self.notifications) {
            (Some(_), None) | (None, Some(_)) => Ok(()),
            _ => Err("set exactly one of `query` or `notifications`".to_owned()),
        }
    }
}

/// Columns used when the configuration declares none.
pub fn default_columns() -> Vec<GithubColumn> {
    let column = |name: &str, query: &str, severity| GithubColumn {
        name: name.to_owned(),
        query: Some(query.to_owned()),
        notifications: None,
        severity: Some(severity),
    };
    vec![
        column(
            "Review requested",
            "is:open is:pr archived:false review-requested:@me",
            CardSeverity::Warning,
        ),
        column(
            "My pull requests",
            "is:open is:pr archived:false author:@me",
            CardSeverity::Info,
        ),
        column(
            "Assigned issues",
            "is:open is:issue archived:false assignee:@me",
            CardSeverity::Info,
        ),
    ]
}

pub struct GithubSource {
    client: Client,
    api_url: String,
    token: Option<String>,
    request_timeout: Duration,
    columns: Vec<GithubColumn>,
}

impl GithubSource {
    /// `token` is `None` when none could be resolved; every refresh then fails
    /// with a setup hint.
    pub fn new(api_url: impl Into<String>, token: Option<String>) -> Self {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            client,
            api_url: api_url.into().trim_end_matches('/').to_owned(),
            token,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            columns: default_columns(),
        }
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Replaces the default columns; an empty list keeps the defaults.
    pub fn with_columns(mut self, columns: Vec<GithubColumn>) -> Self {
        if !columns.is_empty() {
            self.columns = columns;
        }
        self
    }

    async fn refresh_with(&self, token: &str) -> Result<SourceBatch, SourceError> {
        let mut batch = SourceBatch::default();
        let mut failures = Vec::new();
        let mut retry_at = None;

        // A few at a time: GitHub discourages concurrent search requests
        // (secondary rate limit). Results keep column order.
        let mut results = Vec::with_capacity(self.columns.len());
        for chunk in self.columns.chunks(MAX_CONCURRENT_SEARCHES) {
            results.extend(
                join_all(chunk.iter().map(|column| self.column_items(token, column))).await,
            );
        }

        for (column, result) in self.columns.iter().zip(results) {
            match result {
                Ok((cards, warnings)) => {
                    batch.warnings.extend(
                        warnings
                            .into_iter()
                            .map(|warning| format!("{}: {warning}", column.name)),
                    );
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
                "all GitHub searches failed; {}",
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

    /// The cards of one column, plus warnings about them.
    async fn column_items(
        &self,
        token: &str,
        column: &GithubColumn,
    ) -> Result<(Vec<PendingCard>, Vec<String>), SearchFailure> {
        if let Some(mode) = column.notifications {
            let all = match mode {
                Notifications::All => "true",
                Notifications::Unread => "false",
            };
            let per_page = PAGE_SIZE.to_string();
            let threads: Vec<Notification> = self
                .get_json(
                    token,
                    "/notifications",
                    &[("all", all), ("per_page", per_page.as_str())],
                )
                .await?;
            let warnings = if threads.len() >= PAGE_SIZE {
                vec![format!("showing the latest {PAGE_SIZE} notifications")]
            } else {
                Vec::new()
            };
            let cards = threads
                .into_iter()
                .map(|thread| thread.into_card(column.severity))
                .collect();
            return Ok((cards, warnings));
        }

        let query = column.query.as_deref().unwrap_or_default();
        let per_page = PAGE_SIZE.to_string();
        let page: SearchPage = self
            .get_json(
                token,
                "/search/issues",
                &[
                    ("q", query),
                    ("per_page", per_page.as_str()),
                    ("sort", "updated"),
                    ("order", "desc"),
                ],
            )
            .await?;
        let mut warnings = Vec::new();
        if page.incomplete_results {
            warnings.push("GitHub reported incomplete results".to_owned());
        }
        if page.total_count > page.items.len() {
            warnings.push(format!(
                "showing {} of {} results",
                page.items.len(),
                page.total_count
            ));
        }
        let severity = column.severity.unwrap_or(CardSeverity::Info);
        let cards = page
            .items
            .into_iter()
            .map(|issue| issue.into_card(severity, &self.api_url))
            .collect();
        Ok((cards, warnings))
    }

    /// GET `{api}{path}`. Returns a message safe to show and log: it never
    /// contains the token or the request URL.
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        token: &str,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, SearchFailure> {
        let response = self
            .client
            .get(format!("{}{path}", self.api_url))
            .query(query)
            .timeout(self.request_timeout)
            .bearer_auth(token)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .header(
                "user-agent",
                concat!("tasks-pending/", env!("CARGO_PKG_VERSION")),
            )
            .send()
            .await
            .map_err(|error| self.describe(error))?;

        let status = response.status();
        if status.is_success() {
            return response.json::<T>().await.map_err(|error| {
                format!("unexpected GitHub response: {}", self.describe(error)).into()
            });
        }

        if let Some(reset) = rate_limit_reset(status, response.headers()) {
            return Err(SearchFailure {
                message: format!(
                    "GitHub rate limit exceeded; resets at {}",
                    reset.to_rfc3339()
                ),
                retry_at: Some(reset),
            });
        }
        let message = response
            .json::<ApiError>()
            .await
            .map(|body| body.message)
            .unwrap_or_default();
        Err(format!("GitHub API returned {status}: {message}")
            .trim_end_matches([':', ' '])
            .to_owned()
            .into())
    }
}

impl GithubSource {
    /// The error and its causes, without the request URL.
    fn describe(&self, error: reqwest::Error) -> String {
        pending_http::describe_error(error, CONNECT_TIMEOUT, self.request_timeout)
    }
}

impl PendingSource for GithubSource {
    fn columns(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.name.clone()).collect()
    }

    fn refresh(&self) -> BoxFuture<'_, Result<SourceBatch, SourceError>> {
        Box::pin(async move {
            match &self.token {
                Some(token) => self.refresh_with(token).await,
                None => Err(SourceError::new(
                    "no GitHub token: set GITHUB_TOKEN (or GH_TOKEN), or run `gh auth login`, then restart",
                )),
            }
        })
    }
}

/// Why one search failed; safe to show and log.
struct SearchFailure {
    message: String,
    /// When GitHub says the rate limit resets.
    retry_at: Option<DateTime<Utc>>,
}

impl From<String> for SearchFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            retry_at: None,
        }
    }
}

fn rate_limit_reset(
    status: StatusCode,
    headers: &reqwest::header::HeaderMap,
) -> Option<DateTime<Utc>> {
    let limited = matches!(
        status,
        StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
    );
    let remaining = headers.get("x-ratelimit-remaining")?.to_str().ok()?;
    if !limited || remaining != "0" {
        return None;
    }
    let reset: i64 = headers
        .get("x-ratelimit-reset")?
        .to_str()
        .ok()?
        .parse()
        .ok()?;
    DateTime::from_timestamp(reset, 0)
}

/// Token precedence: `GITHUB_TOKEN`, `GH_TOKEN`, then `gh`. Empty values are
/// ignored.
pub fn resolve_token(
    env: &dyn Fn(&str) -> Option<String>,
    gh: &dyn Fn() -> Option<String>,
) -> Option<String> {
    let non_empty =
        |value: Option<String>| value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    non_empty(env("GITHUB_TOKEN"))
        .or_else(|| non_empty(env("GH_TOKEN")))
        .or_else(|| non_empty(gh()))
}

/// Longest wait for `gh auth token`; a locked keyring can block it forever.
const GH_TIMEOUT: Duration = Duration::from_secs(5);

/// Token from `gh auth token`, if the GitHub CLI is installed, logged in and
/// answers within a few seconds.
pub fn gh_cli_token() -> Option<String> {
    gh_cli_token_with("gh", GH_TIMEOUT)
}

/// [`gh_cli_token`] with the program and time limit given, for tests.
#[doc(hidden)]
pub fn gh_cli_token_with(program: &str, limit: Duration) -> Option<String> {
    let mut child = std::process::Command::new(program)
        .args(["auth", "token"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    let deadline = std::time::Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }

    let mut stdout = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut stdout).ok()?;
    let token = stdout.trim().to_owned();
    (!token.is_empty()).then_some(token)
}

/// One notification thread.
#[derive(Deserialize)]
struct Notification {
    id: String,
    unread: bool,
    reason: String,
    updated_at: DateTime<Utc>,
    subject: NotificationSubject,
    repository: NotificationRepository,
}

#[derive(Deserialize)]
struct NotificationSubject {
    title: String,
    #[serde(rename = "type")]
    kind: String,
    /// API URL of the pull request or issue; absent for some kinds.
    url: Option<String>,
}

#[derive(Deserialize)]
struct NotificationRepository {
    full_name: String,
    html_url: String,
}

impl Notification {
    fn into_card(self, severity: Option<CardSeverity>) -> PendingCard {
        // `…/repos/o/r/pulls/7` → `https://github.com/o/r/pull/7`; issues keep
        // `/issues/`; other kinds (releases, discussions…) link to the repo.
        let link = self
            .subject
            .url
            .as_deref()
            .and_then(|url| url.split_once("/repos/"))
            .and_then(|(_, rest)| {
                // rest = "o/r/pulls/7"; the link keeps the repository's host.
                let (repo_and_kind, number) = rest.rsplit_once('/')?;
                let (_, kind) = repo_and_kind.rsplit_once('/')?;
                let kind = match kind {
                    "pulls" => "pull",
                    "issues" => "issues",
                    _ => return None,
                };
                Some(format!("{}/{kind}/{number}", self.repository.html_url))
            })
            .unwrap_or(self.repository.html_url);
        PendingCard {
            id: format!("github:notification:{}", self.id),
            title: self.subject.title,
            body: format!(
                "{} · {} · {}",
                self.repository.full_name,
                self.subject.kind,
                self.reason.replace('_', " ")
            ),
            source: "github".to_owned(),
            url: Some(link),
            due_at: None,
            severity: severity.unwrap_or(if self.unread {
                CardSeverity::Warning
            } else {
                CardSeverity::Info
            }),
            updated_at: self.updated_at,
        }
    }
}

#[derive(Deserialize)]
struct SearchPage {
    total_count: usize,
    #[serde(default)]
    incomplete_results: bool,
    items: Vec<Issue>,
}

#[derive(Deserialize)]
struct Issue {
    number: u64,
    title: String,
    html_url: String,
    repository_url: String,
    updated_at: DateTime<Utc>,
    user: Option<User>,
    #[serde(default)]
    draft: bool,
}

#[derive(Deserialize)]
struct User {
    login: String,
}

#[derive(Deserialize)]
struct ApiError {
    message: String,
}

impl Issue {
    fn into_card(self, severity: CardSeverity, api_url: &str) -> PendingCard {
        let repo = self
            .repository_url
            .strip_prefix(api_url)
            .and_then(|rest| rest.strip_prefix("/repos/"))
            .or_else(|| {
                self.repository_url
                    .split_once("/repos/")
                    .map(|(_, repo)| repo)
            })
            .unwrap_or(&self.repository_url);
        let reference = format!("{repo}#{}", self.number);
        let mut body = match &self.user {
            Some(user) => format!("{reference} · @{}", user.login),
            None => reference.clone(),
        };
        if self.draft {
            body.push_str(" · draft");
        }

        PendingCard {
            id: format!("github:{reference}"),
            title: self.title,
            body,
            source: "github".to_owned(),
            url: Some(self.html_url),
            severity,
            due_at: None,
            updated_at: self.updated_at,
        }
    }
}
