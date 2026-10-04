//! GitHub as a pending-work source: review requests, the authenticated
//! user's open pull requests split by next action and assigned issues, via
//! the search API, plus notifications and repositories' open security alerts.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::future::join_all;
use pending_core::{
    BoxFuture, CardSeverity, PendingCard, PendingSource, SourceBatch, SourceError, SourceItem,
};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};

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
/// Alerts read per repository and kind (first page only). A full page is
/// reported as a warning.
const ALERT_PAGE_SIZE: usize = 100;
/// Pull requests per GraphQL review-state request; `nodes` takes at most 100.
const GRAPHQL_BATCH: usize = 100;

/// Draft, review decision and the check rollup of the last commit, for a list
/// of pull requests: one node list, so checks cost no extra request.
const REVIEW_STATE_QUERY: &str = "query($ids: [ID!]!) { nodes(ids: $ids) { ... on PullRequest { \
     id isDraft reviewDecision \
     commits(last: 1) { nodes { commit { statusCheckRollup { state } } } } } } }";

/// One column: a GitHub search query (the search syntax of github.com).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubColumn {
    #[serde(skip)]
    pub name: String,
    /// A search query (github.com search syntax). Set exactly one of
    /// `query`, `notifications` and `alerts`.
    #[serde(default)]
    pub query: Option<String>,
    /// Notifications instead of a search.
    #[serde(default)]
    pub notifications: Option<Notifications>,
    /// Open security alerts (Dependabot, secret scanning, code scanning) of
    /// these `owner/repo` repositories instead of a search.
    #[serde(default)]
    pub alerts: Option<Vec<String>>,
    /// Severity of every card in this column. Without it, searches are
    /// `info`, unread notifications are warnings and alerts follow their own
    /// severity.
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
    /// A column needs exactly one of `query`, `notifications` and `alerts`;
    /// `alerts` lists at least one repository, each as `owner/repo`.
    pub fn validate(&self) -> Result<(), String> {
        let origins = [
            self.query.is_some(),
            self.notifications.is_some(),
            self.alerts.is_some(),
        ];
        if origins.iter().filter(|set| **set).count() != 1 {
            return Err("set exactly one of `query`, `notifications` or `alerts`".to_owned());
        }
        if let Some(repos) = &self.alerts {
            if repos.is_empty() {
                return Err("`alerts` needs at least one repository".to_owned());
            }
            if let Some(bad) = repos.iter().find(|repo| !is_repo_name(repo)) {
                return Err(format!("`alerts` entries are `owner/repo`, found `{bad}`"));
            }
        }
        Ok(())
    }
}

/// `owner/repo`: two non-empty parts of letters, digits, `-`, `_` and `.`.
fn is_repo_name(repo: &str) -> bool {
    let valid = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    repo.split_once('/')
        .is_some_and(|(owner, name)| valid(owner) && valid(name))
}

/// Columns used when the configuration declares none: one per next action.
///
/// The four columns of my pull requests do not overlap. Drafts go only to
/// "Drafts". "Not approved yet" excludes the two decided states instead of
/// asking for `review:none` or `review:required`: `review:none` leaves out a
/// pull request that only got comment reviews, and `review:required` matches
/// only repositories that require reviews.
pub fn default_columns() -> Vec<GithubColumn> {
    let column = |name: &str, query: &str, severity| GithubColumn {
        name: name.to_owned(),
        query: Some(query.to_owned()),
        notifications: None,
        alerts: None,
        severity: Some(severity),
    };
    vec![
        column(
            "Review requested",
            "is:open is:pr archived:false review-requested:@me",
            CardSeverity::Warning,
        ),
        column(
            "Returned to you",
            "is:open is:pr archived:false author:@me draft:false review:changes_requested",
            CardSeverity::Warning,
        ),
        column(
            "Ready to merge",
            "is:open is:pr archived:false author:@me draft:false review:approved",
            CardSeverity::Info,
        ),
        column(
            "Not approved yet",
            "is:open is:pr archived:false author:@me draft:false -review:approved -review:changes_requested",
            CardSeverity::Info,
        ),
        column(
            "Drafts",
            "is:open is:pr archived:false author:@me draft:true",
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

        let mut entries = Vec::new();
        for (column, result) in self.columns.iter().zip(results) {
            match result {
                Ok((found, warnings)) => {
                    batch.warnings.extend(
                        warnings
                            .into_iter()
                            .map(|warning| format!("{}: {warning}", column.name)),
                    );
                    entries.extend(found.into_iter().map(|entry| (column.name.clone(), entry)));
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

        let (states, warning) = self.review_states(token, &entries).await;
        batch.warnings.extend(warning);
        batch.items = entries
            .into_iter()
            .map(|(column, entry)| SourceItem {
                column,
                card: entry.finish(&states),
            })
            .collect();
        Ok(batch)
    }

    /// Draft, review and check state of every pull request among `entries`,
    /// by node id, asked [`GRAPHQL_BATCH`] at a time. A failure is a warning:
    /// the cards show without review state and checks.
    async fn review_states(
        &self,
        token: &str,
        entries: &[(String, Entry)],
    ) -> (HashMap<String, PullRequestState>, Option<String>) {
        let mut ids: Vec<&str> = Vec::new();
        let mut seen = HashSet::new();
        for (_, entry) in entries {
            if let Some(id) = entry.node_id.as_deref()
                && seen.insert(id)
            {
                ids.push(id);
            }
        }

        let mut states = HashMap::new();
        let mut problems = Vec::new();
        for chunk in ids.chunks(GRAPHQL_BATCH) {
            let request = GraphqlRequest {
                query: REVIEW_STATE_QUERY,
                variables: IdsVariables { ids: chunk },
            };
            let reply: GraphqlReply = match self.post_graphql(token, &request).await {
                Ok(reply) => reply,
                Err(failure) => {
                    // The next chunk would most likely fail the same way.
                    problems.push(failure.message);
                    break;
                }
            };
            for error in reply.errors {
                // A token that may not read check status gets one FORBIDDEN
                // error per pull request for that field alone: the checks
                // are left out, which is not worth a warning on every
                // refresh. Any other error there (a timeout, an outage) is
                // a problem to report.
                let checks_denied = error.kind.as_deref() == Some(FORBIDDEN)
                    && matches!(
                        error.path.last(),
                        Some(PathSegment::Field(field)) if field == "statusCheckRollup"
                    );
                if !checks_denied && !problems.contains(&error.message) {
                    problems.push(error.message);
                }
            }
            let nodes = reply.data.map(|data| data.nodes).unwrap_or_default();
            for node in nodes.into_iter().flatten() {
                if let Some(id) = node.id.clone() {
                    states.insert(id, node);
                }
            }
        }

        let warning = (!problems.is_empty()).then(|| {
            format!(
                "pull request review state and checks unavailable: {}",
                problems.join("; ")
            )
        });
        (states, warning)
    }

    /// The cards of one column, plus warnings about them.
    async fn column_items(
        &self,
        token: &str,
        column: &GithubColumn,
    ) -> Result<(Vec<Entry>, Vec<String>), SearchFailure> {
        if let Some(repos) = &column.alerts {
            return self.alert_items(token, repos, column.severity).await;
        }
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
                .map(|thread| Entry::ready(thread.into_card(column.severity)))
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
            .map(|issue| issue.into_entry(severity, &self.api_url))
            .collect();
        Ok((cards, warnings))
    }

    /// Open alerts of every kind for each repository, a few requests at a
    /// time. A kind that cannot be read (disabled, no permission, error) is a
    /// warning naming repository and kind; the column fails only when
    /// nothing could be read.
    async fn alert_items(
        &self,
        token: &str,
        repos: &[String],
        severity: Option<CardSeverity>,
    ) -> Result<(Vec<Entry>, Vec<String>), SearchFailure> {
        let requests: Vec<(&str, AlertKind)> = repos
            .iter()
            .flat_map(|repo| AlertKind::ALL.map(|kind| (repo.as_str(), kind)))
            .collect();
        let mut results = Vec::with_capacity(requests.len());
        for chunk in requests.chunks(MAX_CONCURRENT_SEARCHES) {
            results.extend(
                join_all(
                    chunk
                        .iter()
                        .map(|(repo, kind)| self.repo_alerts(token, repo, *kind)),
                )
                .await,
            );
        }

        let mut entries = Vec::new();
        let mut warnings = Vec::new();
        let mut failures = Vec::new();
        let mut retry_at = None;
        // 404 means the alert type is not set up for the repository (e.g. no
        // code scanning analysis yet): no alerts, no warning. A repository
        // where every type answers 404 is more likely a typo, though.
        let mut missing: HashMap<&str, usize> = HashMap::new();
        for ((repo, kind), result) in requests.iter().zip(results) {
            let result = match result {
                Err(failure)
                    if failure.status == Some(StatusCode::NOT_FOUND)
                        && failure.retry_at.is_none() =>
                {
                    *missing.entry(*repo).or_default() += 1;
                    Ok(Vec::new())
                }
                other => other,
            };
            match result {
                Ok(cards) => {
                    if cards.len() >= ALERT_PAGE_SIZE {
                        warnings.push(format!(
                            "{repo}: showing the first {ALERT_PAGE_SIZE} {} alerts",
                            kind.label()
                        ));
                    }
                    entries.extend(cards.into_iter().map(|mut card| {
                        if let Some(severity) = severity {
                            card.severity = severity;
                        }
                        Entry::ready(card)
                    }));
                }
                Err(failure) => {
                    let hint = match failure.status {
                        Some(StatusCode::FORBIDDEN) if failure.retry_at.is_none() => {
                            " (disabled for the repository, or the token cannot read it)"
                        }
                        _ => "",
                    };
                    retry_at = retry_at.max(failure.retry_at);
                    failures.push(format!(
                        "{repo}: {} alerts unavailable: {}{hint}",
                        kind.label(),
                        failure.message
                    ));
                }
            }
        }

        for repo in repos {
            if missing.get(repo.as_str()) == Some(&AlertKind::ALL.len()) {
                warnings.push(format!(
                    "{repo}: no alerts could be read (404 for every alert type); \
                     check the repository name and that the token can see it"
                ));
            }
        }

        if !failures.is_empty() && failures.len() == requests.len() {
            return Err(SearchFailure {
                message: failures.join("; "),
                retry_at,
                status: None,
            });
        }
        warnings.extend(failures);
        Ok((entries, warnings))
    }

    /// The open alerts of one kind in one repository, as cards.
    async fn repo_alerts(
        &self,
        token: &str,
        repo: &str,
        kind: AlertKind,
    ) -> Result<Vec<PendingCard>, SearchFailure> {
        let path = format!("/repos/{repo}/{}/alerts", kind.path());
        let per_page = ALERT_PAGE_SIZE.to_string();
        let mut query = vec![("state", "open"), ("per_page", per_page.as_str())];
        if kind == AlertKind::SecretScanning {
            // Otherwise the reply carries the secret itself.
            query.push(("hide_secret", "true"));
        }
        Ok(match kind {
            AlertKind::Dependabot => self
                .get_json::<Vec<DependabotAlert>>(token, &path, &query)
                .await?
                .into_iter()
                .map(|alert| alert.into_card(repo))
                .collect(),
            AlertKind::SecretScanning => self
                .get_json::<Vec<SecretScanningAlert>>(token, &path, &query)
                .await?
                .into_iter()
                .map(|alert| alert.into_card(repo))
                .collect(),
            AlertKind::CodeScanning => self
                .get_json::<Vec<CodeScanningAlert>>(token, &path, &query)
                .await?
                .into_iter()
                .map(|alert| alert.into_card(repo))
                .collect(),
        })
    }

    /// GET `{api}{path}`. Returns a message safe to show and log: it never
    /// contains the token or the request URL.
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        token: &str,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, SearchFailure> {
        let request = self
            .client
            .get(format!("{}{path}", self.api_url))
            .query(query);
        self.send(token, request).await
    }

    /// POST a GraphQL request to the API's GraphQL endpoint.
    async fn post_graphql<T: serde::de::DeserializeOwned>(
        &self,
        token: &str,
        body: &impl serde::Serialize,
    ) -> Result<T, SearchFailure> {
        let request = self.client.post(graphql_url(&self.api_url)).json(body);
        self.send(token, request).await
    }

    /// Sends an authenticated request and reads a JSON reply. Errors are
    /// safe to show and log: never the token or the request URL.
    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        token: &str,
        request: reqwest::RequestBuilder,
    ) -> Result<T, SearchFailure> {
        let response = request
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
                status: Some(status),
            });
        }
        let message = response
            .json::<ApiError>()
            .await
            .map(|body| body.message)
            .unwrap_or_default();
        Err(SearchFailure {
            message: format!("GitHub API returned {status}: {message}")
                .trim_end_matches([':', ' '])
                .to_owned(),
            retry_at: None,
            status: Some(status),
        })
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
    /// HTTP status of an error reply; `None` for transport errors.
    status: Option<StatusCode>,
}

impl From<String> for SearchFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            retry_at: None,
            status: None,
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
    #[serde(default)]
    comments: u64,
    /// GraphQL id; the review-state lookup uses it for pull requests.
    #[serde(default)]
    node_id: Option<String>,
    /// Present only on pull requests.
    #[serde(default)]
    pull_request: Option<serde::de::IgnoredAny>,
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
    fn into_entry(self, severity: CardSeverity, api_url: &str) -> Entry {
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
        let body = match &self.user {
            Some(user) => format!("{reference} · @{}", user.login),
            None => reference.clone(),
        };

        Entry {
            card: PendingCard {
                id: format!("github:{reference}"),
                title: self.title,
                body,
                source: "github".to_owned(),
                url: Some(self.html_url),
                severity,
                due_at: None,
                updated_at: self.updated_at,
            },
            node_id: self.pull_request.and(self.node_id),
            draft: self.draft,
            comments: self.comments,
        }
    }
}

/// A card whose body is finished once the review state of pull requests is
/// known.
struct Entry {
    card: PendingCard,
    /// GraphQL id, for pull requests only.
    node_id: Option<String>,
    draft: bool,
    comments: u64,
}

impl Entry {
    /// A card with nothing more to add.
    fn ready(card: PendingCard) -> Self {
        Self {
            card,
            node_id: None,
            draft: false,
            comments: 0,
        }
    }

    /// Appends draft, review state, checks and comment count to the body;
    /// changes requested or failing checks make the card at least a warning.
    fn finish(self, states: &HashMap<String, PullRequestState>) -> PendingCard {
        let mut card = self.card;
        let state = self.node_id.as_ref().and_then(|id| states.get(id));
        if self.draft || state.is_some_and(|state| state.is_draft == Some(true)) {
            card.body.push_str(" · draft");
        }
        let review = match state.and_then(|state| state.review_decision.as_deref()) {
            Some("CHANGES_REQUESTED") => {
                card.severity = card.severity.max(CardSeverity::Warning);
                Some("changes requested")
            }
            Some("APPROVED") => Some("approved"),
            Some("REVIEW_REQUIRED") => Some("review required"),
            _ => None,
        };
        if let Some(review) = review {
            card.body.push_str(" · ");
            card.body.push_str(review);
        }
        // No rollup (a repository without checks) or an unknown state says
        // nothing.
        let checks = match state.and_then(PullRequestState::checks) {
            Some("FAILURE" | "ERROR") => {
                card.severity = card.severity.max(CardSeverity::Warning);
                Some("checks failing")
            }
            Some("PENDING" | "EXPECTED") => Some("checks pending"),
            Some("SUCCESS") => Some("checks passing"),
            _ => None,
        };
        if let Some(checks) = checks {
            card.body.push_str(" · ");
            card.body.push_str(checks);
        }
        match self.comments {
            0 => {}
            1 => card.body.push_str(" · 1 comment"),
            n => card.body.push_str(&format!(" · {n} comments")),
        }
        card
    }
}

/// `{api}/graphql`, or `/api/graphql` for a GitHub Enterprise Server REST
/// base ending in `/api/v3`.
fn graphql_url(api_url: &str) -> String {
    match api_url.strip_suffix("/api/v3") {
        Some(host) => format!("{host}/api/graphql"),
        None => format!("{api_url}/graphql"),
    }
}

#[derive(Serialize)]
struct GraphqlRequest<'a> {
    query: &'a str,
    variables: IdsVariables<'a>,
}

#[derive(Serialize)]
struct IdsVariables<'a> {
    ids: &'a [&'a str],
}

#[derive(Deserialize)]
struct GraphqlReply {
    #[serde(default)]
    data: Option<NodesData>,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Deserialize)]
struct NodesData {
    /// `null` for ids that no longer resolve.
    #[serde(default, deserialize_with = "null_as_default")]
    nodes: Vec<Option<PullRequestState>>,
}

/// GraphQL error type for a field the token may not read.
const FORBIDDEN: &str = "FORBIDDEN";

#[derive(Deserialize)]
struct GraphqlError {
    message: String,
    /// GitHub's error class, such as `FORBIDDEN` or `NOT_FOUND`.
    #[serde(default, rename = "type")]
    kind: Option<String>,
    /// Where in the reply the error applies, e.g.
    /// `["nodes", 0, "commits", ..., "statusCheckRollup"]`.
    #[serde(default, deserialize_with = "null_as_default")]
    path: Vec<PathSegment>,
}

/// GraphQL writes `null` where a list could not be produced; read it like
/// an absent field instead of rejecting the whole reply.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PathSegment {
    Field(String),
    #[allow(dead_code)]
    Index(u64),
}

/// What GraphQL says about one pull request.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestState {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    is_draft: Option<bool>,
    #[serde(default)]
    review_decision: Option<String>,
    /// The last commit only (`commits(last: 1)`).
    #[serde(default)]
    commits: Option<CommitList>,
}

impl PullRequestState {
    /// Check rollup state of the last commit (`SUCCESS`, `FAILURE`, `ERROR`,
    /// `PENDING`, `EXPECTED`); `None` when the repository has no checks.
    fn checks(&self) -> Option<&str> {
        self.commits
            .as_ref()?
            .nodes
            .last()?
            .as_ref()?
            .commit
            .status_check_rollup
            .as_ref()?
            .state
            .as_deref()
    }
}

#[derive(Deserialize)]
struct CommitList {
    #[serde(default, deserialize_with = "null_as_default")]
    nodes: Vec<Option<CommitNode>>,
}

#[derive(Deserialize)]
struct CommitNode {
    commit: Commit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Commit {
    #[serde(default)]
    status_check_rollup: Option<CheckRollup>,
}

#[derive(Deserialize)]
struct CheckRollup {
    #[serde(default)]
    state: Option<String>,
}

/// The security alert APIs an `alerts` column reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlertKind {
    Dependabot,
    SecretScanning,
    CodeScanning,
}

impl AlertKind {
    const ALL: [Self; 3] = [Self::Dependabot, Self::SecretScanning, Self::CodeScanning];

    /// Path segment under `/repos/{owner}/{repo}/`.
    fn path(self) -> &'static str {
        match self {
            Self::Dependabot => "dependabot",
            Self::SecretScanning => "secret-scanning",
            Self::CodeScanning => "code-scanning",
        }
    }

    /// Name in warnings.
    fn label(self) -> &'static str {
        match self {
            Self::Dependabot => "Dependabot",
            Self::SecretScanning => "secret scanning",
            Self::CodeScanning => "code scanning",
        }
    }
}

/// Card severity of an advisory or rule level: `critical` is critical,
/// `high` (and the code scanning rule level `error`) a warning, anything
/// else info.
fn alert_severity(level: &str) -> CardSeverity {
    match level.to_ascii_lowercase().as_str() {
        "critical" => CardSeverity::Critical,
        "high" | "error" => CardSeverity::Warning,
        _ => CardSeverity::Info,
    }
}

/// What every alert card is made of.
struct AlertCard<'a> {
    repo: &'a str,
    kind: AlertKind,
    number: u64,
    title: String,
    body: String,
    html_url: String,
    severity: CardSeverity,
    updated_at: DateTime<Utc>,
}

impl AlertCard<'_> {
    fn into_card(self) -> PendingCard {
        let repo = self.repo;
        PendingCard {
            id: format!("github:alert:{repo}:{}:{}", self.kind.path(), self.number),
            title: format!("{repo}: {}", self.title),
            body: self.body,
            source: "github".to_owned(),
            url: Some(self.html_url),
            due_at: None,
            severity: self.severity,
            updated_at: self.updated_at,
        }
    }
}

#[derive(Deserialize)]
struct DependabotAlert {
    number: u64,
    html_url: String,
    created_at: DateTime<Utc>,
    #[serde(default)]
    updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    dependency: Option<DependabotDependency>,
    security_advisory: DependabotAdvisory,
}

#[derive(Deserialize)]
struct DependabotDependency {
    #[serde(default)]
    package: Option<DependabotPackage>,
}

#[derive(Deserialize)]
struct DependabotPackage {
    ecosystem: String,
    name: String,
}

#[derive(Deserialize)]
struct DependabotAdvisory {
    summary: String,
    severity: String,
}

impl DependabotAlert {
    fn into_card(self, repo: &str) -> PendingCard {
        let mut body = "Dependabot".to_owned();
        if let Some(package) = self.dependency.and_then(|d| d.package) {
            body.push_str(&format!(" · {} {}", package.ecosystem, package.name));
        }
        body.push_str(&format!(" · {}", self.security_advisory.severity));
        AlertCard {
            repo,
            kind: AlertKind::Dependabot,
            number: self.number,
            severity: alert_severity(&self.security_advisory.severity),
            title: self.security_advisory.summary,
            body,
            html_url: self.html_url,
            updated_at: self.updated_at.unwrap_or(self.created_at),
        }
        .into_card()
    }
}

#[derive(Deserialize)]
struct SecretScanningAlert {
    number: u64,
    html_url: String,
    created_at: DateTime<Utc>,
    #[serde(default)]
    updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    secret_type: Option<String>,
    #[serde(default)]
    secret_type_display_name: Option<String>,
}

impl SecretScanningAlert {
    /// A leaked secret is always critical: it is usable until revoked.
    fn into_card(self, repo: &str) -> PendingCard {
        let secret = self
            .secret_type_display_name
            .or(self.secret_type)
            .unwrap_or_else(|| "secret".to_owned());
        AlertCard {
            repo,
            kind: AlertKind::SecretScanning,
            number: self.number,
            title: format!("{secret} exposed"),
            body: format!("Secret scanning · {secret} · critical"),
            html_url: self.html_url,
            severity: CardSeverity::Critical,
            updated_at: self.updated_at.unwrap_or(self.created_at),
        }
        .into_card()
    }
}

#[derive(Deserialize)]
struct CodeScanningAlert {
    number: u64,
    html_url: String,
    created_at: DateTime<Utc>,
    #[serde(default)]
    updated_at: Option<DateTime<Utc>>,
    rule: CodeScanningRule,
    #[serde(default)]
    tool: Option<CodeScanningTool>,
}

#[derive(Deserialize)]
struct CodeScanningRule {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    /// `none`, `note`, `warning` or `error`.
    #[serde(default)]
    severity: Option<String>,
    /// `low`, `medium`, `high` or `critical`; security rules only.
    #[serde(default)]
    security_severity_level: Option<String>,
}

#[derive(Deserialize)]
struct CodeScanningTool {
    name: String,
}

impl CodeScanningAlert {
    /// The security severity decides when the rule has one, else the rule's
    /// own level.
    fn into_card(self, repo: &str) -> PendingCard {
        let rule = self.rule;
        let level = rule
            .security_severity_level
            .or(rule.severity)
            .unwrap_or_default();
        let title = rule
            .description
            .clone()
            .or_else(|| rule.name.clone())
            .or_else(|| rule.id.clone())
            .unwrap_or_else(|| "code scanning alert".to_owned());
        let mut body = "Code scanning".to_owned();
        for part in [self.tool.map(|tool| tool.name), rule.id]
            .into_iter()
            .flatten()
        {
            body.push_str(&format!(" · {part}"));
        }
        if !level.is_empty() {
            body.push_str(&format!(" · {level}"));
        }
        AlertCard {
            repo,
            kind: AlertKind::CodeScanning,
            number: self.number,
            title,
            body,
            html_url: self.html_url,
            severity: alert_severity(&level),
            updated_at: self.updated_at.unwrap_or(self.created_at),
        }
        .into_card()
    }
}
