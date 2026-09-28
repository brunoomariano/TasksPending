//! GitHub as a pending-work source: review requests, open pull requests and
//! assigned issues of the authenticated user, via the search API.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use pending_core::{
    BoxFuture, CardSeverity, PendingCard, PendingSource, SourceBatch, SourceError, SourceItem,
};
use reqwest::{Client, StatusCode};
use serde::Deserialize;

pub const DEFAULT_API_URL: &str = "https://api.github.com";

/// Results per search. More than this is reported as a warning.
const PAGE_SIZE: usize = 50;

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
}

impl GithubSource {
    /// `token` is `None` when none could be resolved; every refresh then fails
    /// with a setup hint.
    pub fn new(api_url: impl Into<String>, token: Option<String>) -> Self {
        Self {
            client: Client::new(),
            api_url: api_url.into().trim_end_matches('/').to_owned(),
            token,
        }
    }

    async fn refresh_with(&self, token: &str) -> Result<SourceBatch, SourceError> {
        let mut seen = HashSet::new();
        let mut batch = SourceBatch::default();
        let mut failures = Vec::new();

        for search in &SEARCHES {
            match self.search(token, search.query).await {
                Ok(page) => {
                    if page.total_count > page.items.len() {
                        batch.warnings.push(format!(
                            "{}: showing {} of {} results",
                            search.section,
                            page.items.len(),
                            page.total_count
                        ));
                    }
                    for issue in page.items {
                        let card = issue.into_card(search.severity);
                        if seen.insert(card.id.clone()) {
                            batch.items.push(SourceItem {
                                section: search.section.to_owned(),
                                card,
                            });
                        }
                    }
                }
                Err(message) => failures.push(format!("{}: {message}", search.section)),
            }
        }

        if failures.len() == SEARCHES.len() {
            return Err(SourceError::new(format!(
                "all GitHub searches failed; {}",
                failures.join("; ")
            )));
        }
        batch.warnings.extend(failures);
        Ok(batch)
    }

    /// Returns a message safe to show and log: it never contains the token or
    /// the request URL.
    async fn search(&self, token: &str, query: &str) -> Result<SearchPage, String> {
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
            .bearer_auth(token)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .header(
                "user-agent",
                concat!("tasks-pending/", env!("CARGO_PKG_VERSION")),
            )
            .send()
            .await
            .map_err(|error| error.without_url().to_string())?;

        let status = response.status();
        if status.is_success() {
            return response
                .json::<SearchPage>()
                .await
                .map_err(|error| format!("unexpected GitHub response: {}", error.without_url()));
        }

        if let Some(reset) = rate_limit_reset(status, response.headers()) {
            return Err(format!("GitHub rate limit exceeded; resets at {reset}"));
        }
        let message = response
            .json::<ApiError>()
            .await
            .map(|body| body.message)
            .unwrap_or_default();
        Err(format!("GitHub API returned {status}: {message}")
            .trim_end_matches([':', ' '])
            .to_owned())
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

fn rate_limit_reset(status: StatusCode, headers: &reqwest::header::HeaderMap) -> Option<String> {
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
    Some(DateTime::from_timestamp(reset, 0)?.to_rfc3339())
}

/// Token precedence: `GITHUB_TOKEN`, `GH_TOKEN`, then `gh`. Empty values are
/// ignored.
pub fn resolve_token(
    env: &dyn Fn(&str) -> Option<String>,
    gh: &dyn Fn() -> Option<String>,
) -> Option<String> {
    let non_empty = |value: Option<String>| value.filter(|v| !v.trim().is_empty());
    non_empty(env("GITHUB_TOKEN"))
        .or_else(|| non_empty(env("GH_TOKEN")))
        .or_else(|| non_empty(gh()))
}

/// Token from `gh auth token`, if the GitHub CLI is installed and logged in.
pub fn gh_cli_token() -> Option<String> {
    let output = std::process::Command::new("gh")
        .args(["auth", "token"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!token.is_empty()).then_some(token)
}

#[derive(Deserialize)]
struct SearchPage {
    total_count: usize,
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
    fn into_card(self, severity: CardSeverity) -> PendingCard {
        let repo = self
            .repository_url
            .rsplit_once("/repos/")
            .map_or(self.repository_url.as_str(), |(_, repo)| repo);
        let reference = format!("{repo}#{}", self.number);
        let body = match &self.user {
            Some(user) => format!("{reference} · @{}", user.login),
            None => reference.clone(),
        };

        PendingCard {
            id: format!("github:{reference}"),
            title: self.title,
            body,
            source: "github".to_owned(),
            url: Some(self.html_url),
            severity,
            updated_at: self.updated_at,
        }
    }
}
