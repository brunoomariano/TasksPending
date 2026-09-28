//! GitHub as a pending-work source: review requests, open pull requests and
//! assigned issues of the authenticated user, via the search API.

use std::collections::HashSet;
use std::time::Duration;

use chrono::{DateTime, Utc};
use pending_core::{
    BoxFuture, CardSeverity, PendingCard, PendingSource, SourceBatch, SourceError, SourceItem,
};
use reqwest::{Client, StatusCode};
use serde::Deserialize;

pub const DEFAULT_API_URL: &str = "https://api.github.com";

/// Results per search. More than this is reported as a warning.
const PAGE_SIZE: usize = 50;

/// Per-search limit, below the aggregator's default refresh timeout (30s) so a
/// hanging search becomes a warning instead of failing the whole refresh.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

struct Search {
    section: &'static str,
    query: &'static str,
    severity: CardSeverity,
}

/// Searches run in this order; an item found by two searches stays in the first.
const SEARCHES: [Search; 3] = [
    Search {
        section: "Review requested",
        query: "is:open is:pr archived:false review-requested:@me",
        severity: CardSeverity::Warning,
    },
    Search {
        section: "My pull requests",
        query: "is:open is:pr archived:false author:@me",
        severity: CardSeverity::Info,
    },
    Search {
        section: "Assigned issues",
        query: "is:open is:issue archived:false assignee:@me",
        severity: CardSeverity::Info,
    },
];

pub struct GithubSource {
    client: Client,
    api_url: String,
    token: Option<String>,
    request_timeout: Duration,
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
        }
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    async fn refresh_with(&self, token: &str) -> Result<SourceBatch, SourceError> {
        let mut seen = HashSet::new();
        let mut batch = SourceBatch::default();
        let mut failures = Vec::new();
        let mut retry_at = None;

        let [review, authored, assigned] = &SEARCHES;
        let (review, authored, assigned) = tokio::join!(
            self.search(token, review.query),
            self.search(token, authored.query),
            self.search(token, assigned.query),
        );

        for (search, result) in SEARCHES.iter().zip([review, authored, assigned]) {
            match result {
                Ok(page) => {
                    if page.incomplete_results {
                        batch.warnings.push(format!(
                            "{}: GitHub reported incomplete results",
                            search.section
                        ));
                    }
                    if page.total_count > page.items.len() {
                        batch.warnings.push(format!(
                            "{}: showing {} of {} results",
                            search.section,
                            page.items.len(),
                            page.total_count
                        ));
                    }
                    for issue in page.items {
                        let card = issue.into_card(search.severity, &self.api_url);
                        if seen.insert(card.id.clone()) {
                            batch.items.push(SourceItem {
                                section: search.section.to_owned(),
                                card,
                            });
                        }
                    }
                }
                Err(failure) => {
                    retry_at = retry_at.max(failure.retry_at);
                    failures.push(format!("{}: {}", search.section, failure.message));
                }
            }
        }

        if failures.len() == SEARCHES.len() {
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

    /// Returns a message safe to show and log: it never contains the token or
    /// the request URL.
    async fn search(&self, token: &str, query: &str) -> Result<SearchPage, SearchFailure> {
        let per_page = PAGE_SIZE.to_string();
        let response = self
            .client
            .get(format!("{}/search/issues", self.api_url))
            .query(&[
                ("q", query),
                ("per_page", per_page.as_str()),
                ("sort", "updated"),
                ("order", "desc"),
            ])
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
            return response.json::<SearchPage>().await.map_err(|error| {
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
        if error.is_timeout() {
            return if error.is_connect() {
                format!("connection timed out after {CONNECT_TIMEOUT:?}")
            } else {
                format!("timed out after {:?}", self.request_timeout)
            };
        }
        let error = error.without_url();
        let mut message = error.to_string();
        let mut source = std::error::Error::source(&error);
        while let Some(cause) = source {
            message.push_str(": ");
            message.push_str(&cause.to_string());
            source = cause.source();
        }
        message
    }
}

impl PendingSource for GithubSource {
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
    let mut child = std::process::Command::new("gh")
        .args(["auth", "token"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    let deadline = std::time::Instant::now() + GH_TIMEOUT;
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
