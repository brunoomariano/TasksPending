//! Linear as a pending-work source: one GraphQL `issues` query per stack,
//! filtered by Linear itself.
//!
//! Every stack key becomes part of an `IssueFilter`, so only matching issues
//! travel. Stacks are queried a few at a time, each request with its own
//! timeout; a failing stack becomes a warning and the others still show.

use std::time::Duration;

use chrono::{DateTime, Local, NaiveDate, Utc};
use futures_util::future::join_all;
use pending_core::{
    BoxFuture, CardSeverity, PendingCard, PendingSource, SourceBatch, SourceError, SourceItem,
};
use reqwest::redirect::Policy;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// Linear's GraphQL endpoint; `LINEAR_API_URL` replaces it.
pub const DEFAULT_API_URL: &str = "https://api.linear.app/graphql";

/// Per request; a stack whose request times out becomes a warning.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PAGE_SIZE: usize = 100;
/// Pages read per stack before giving up with a warning.
const MAX_PAGES: usize = 5;
/// Stacks queried at once.
const MAX_CONCURRENT_QUERIES: usize = 3;
/// Longest provider error message shown.
const MAX_MESSAGE_CHARS: usize = 300;

/// The one query the source sends; the stack's filter and the page cursor go
/// in the variables.
const ISSUES_QUERY: &str = "\
query PendingIssues($filter: IssueFilter, $first: Int!, $after: String) {
  issues(filter: $filter, first: $first, after: $after, orderBy: updatedAt) {
    nodes {
      identifier
      title
      url
      priority
      dueDate
      updatedAt
      team { name }
      state { name }
      assignee { displayName }
    }
    pageInfo { hasNextPage endCursor }
  }
}";

/// State types that count as open when a stack lists neither types nor
/// state names.
const OPEN_TYPES: [StateType; 4] = [
    StateType::Triage,
    StateType::Backlog,
    StateType::Unstarted,
    StateType::Started,
];

/// Linear's workflow state types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StateType {
    Triage,
    Backlog,
    Unstarted,
    Started,
    Completed,
    Canceled,
}

impl StateType {
    fn as_str(self) -> &'static str {
        match self {
            StateType::Triage => "triage",
            StateType::Backlog => "backlog",
            StateType::Unstarted => "unstarted",
            StateType::Started => "started",
            StateType::Completed => "completed",
            StateType::Canceled => "canceled",
        }
    }
}

/// Linear's priorities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Urgent,
    High,
    Medium,
    Low,
    None,
}

impl Priority {
    /// The number Linear stores: 0 is no priority, 1 urgent … 4 low.
    fn number(self) -> u8 {
        match self {
            Priority::None => 0,
            Priority::Urgent => 1,
            Priority::High => 2,
            Priority::Medium => 3,
            Priority::Low => 4,
        }
    }
}

/// Whose issues a stack shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Assignee {
    /// Assigned to the owner of the API key.
    #[default]
    Me,
    /// Assigned to nobody.
    None,
    /// Assigned to someone, but not to the owner of the API key.
    Others,
    /// Anyone or nobody.
    Any,
}

/// One stack: a filter over the workspace's issues, applied by Linear. Empty
/// lists match everything; without `state_type` or `state`, only open state
/// types match.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinearColumn {
    #[serde(skip)]
    pub name: String,
    #[serde(default)]
    pub assignee: Assignee,
    #[serde(default)]
    pub state_type: Vec<StateType>,
    /// State names, e.g. `In Review` (case-insensitive).
    #[serde(default)]
    pub state: Vec<String>,
    /// Team keys, e.g. `ENG` (the prefix of the team's issue references).
    #[serde(default)]
    pub team: Vec<String>,
    #[serde(default)]
    pub priority: Vec<Priority>,
}

impl LinearColumn {
    /// Rejects blank state names and team keys, which would match nothing.
    pub fn validate(&self) -> Result<(), String> {
        for (key, values) in [("state", &self.state), ("team", &self.team)] {
            if values.iter().any(|value| value.trim().is_empty()) {
                return Err(format!("`{key}` has an empty entry"));
            }
        }
        Ok(())
    }

    /// The `IssueFilter` Linear applies for this stack.
    fn filter(&self) -> Value {
        let mut filter = Map::new();
        match self.assignee {
            Assignee::Me => {
                filter.insert("assignee".to_owned(), json!({ "isMe": { "eq": true } }));
            }
            Assignee::None => {
                filter.insert("assignee".to_owned(), json!({ "null": true }));
            }
            Assignee::Others => {
                filter.insert(
                    "assignee".to_owned(),
                    json!({ "null": false, "isMe": { "eq": false } }),
                );
            }
            Assignee::Any => {}
        }

        // Naming states picks them in any type ("Done" is completed).
        let types: &[StateType] = match (self.state_type.is_empty(), self.state.is_empty()) {
            (false, _) => &self.state_type,
            (true, true) => &OPEN_TYPES,
            (true, false) => &[],
        };
        if !types.is_empty() {
            let types: Vec<&str> = types.iter().map(|t| t.as_str()).collect();
            filter.insert("state".to_owned(), json!({ "type": { "in": types } }));
        }
        if !self.state.is_empty() {
            // `in` compares with case; one case-insensitive alternative per name.
            let names: Vec<Value> = self
                .state
                .iter()
                .map(|name| json!({ "state": { "name": { "eqIgnoreCase": name.trim() } } }))
                .collect();
            filter.insert("or".to_owned(), Value::Array(names));
        }
        if !self.team.is_empty() {
            let keys: Vec<&str> = self.team.iter().map(|key| key.trim()).collect();
            filter.insert("team".to_owned(), json!({ "key": { "in": keys } }));
        }
        if !self.priority.is_empty() {
            let numbers: Vec<u8> = self.priority.iter().map(|p| p.number()).collect();
            filter.insert("priority".to_owned(), json!({ "in": numbers }));
        }
        Value::Object(filter)
    }
}

/// Stacks used when the configuration declares none: your open issues by
/// state type.
pub fn default_columns() -> Vec<LinearColumn> {
    [
        ("In progress", StateType::Started),
        ("To do", StateType::Unstarted),
        ("Backlog", StateType::Backlog),
    ]
    .into_iter()
    .map(|(name, state_type)| LinearColumn {
        name: name.to_owned(),
        state_type: vec![state_type],
        ..LinearColumn::default()
    })
    .collect()
}

#[derive(Clone)]
pub struct LinearSettings {
    /// GraphQL endpoint, e.g. `https://api.linear.app/graphql`.
    pub api_url: String,
    /// A personal API key.
    pub api_key: String,
}

impl std::fmt::Debug for LinearSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinearSettings")
            .field("api_url", &self.api_url)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl LinearSettings {
    /// Reads `LINEAR_API_KEY` and the optional `LINEAR_API_URL` (defaults to
    /// Linear's endpoint). The error names the missing variable.
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let read = |key: &str| {
            env(key)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        let api_key = read("LINEAR_API_KEY")
            .ok_or("Linear is not configured: set LINEAR_API_KEY, then restart")?;
        Ok(Self {
            api_url: read("LINEAR_API_URL").unwrap_or_else(|| DEFAULT_API_URL.to_owned()),
            api_key,
        })
    }
}

pub struct LinearSource {
    client: Client,
    settings: Result<LinearSettings, String>,
    request_timeout: Duration,
    columns: Vec<LinearColumn>,
}

impl LinearSource {
    /// With `Err`, every refresh fails with that message (missing settings).
    pub fn new(settings: Result<LinearSettings, String>) -> Self {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            // The key goes in `Authorization`; it must not follow a redirect
            // to another address.
            .redirect(Policy::none())
            .build()
            .unwrap_or_default();
        Self {
            client,
            settings,
            request_timeout: REQUEST_TIMEOUT,
            columns: default_columns(),
        }
    }

    /// Replaces the default stacks; an empty list keeps the defaults.
    pub fn with_columns(mut self, columns: Vec<LinearColumn>) -> Self {
        if !columns.is_empty() {
            self.columns = columns;
        }
        self
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    async fn refresh_with(&self, settings: &LinearSettings) -> Result<SourceBatch, SourceError> {
        // A few at a time, to stay far from Linear's rate limits. Results
        // keep stack order.
        let mut results = Vec::with_capacity(self.columns.len());
        for chunk in self.columns.chunks(MAX_CONCURRENT_QUERIES) {
            results.extend(
                join_all(
                    chunk
                        .iter()
                        .map(|column| self.column_issues(settings, column)),
                )
                .await,
            );
        }

        let today = Local::now().date_naive();
        let mut batch = SourceBatch::default();
        let mut failures = Vec::new();
        let mut retry_at = None;
        for (column, result) in self.columns.iter().zip(results) {
            match result {
                Ok((issues, truncated)) => {
                    if truncated {
                        batch.warnings.push(format!(
                            "{}: more than {} issues; showing the first ones",
                            column.name,
                            MAX_PAGES * PAGE_SIZE
                        ));
                    }
                    batch.items.extend(
                        issues
                            .into_iter()
                            .filter_map(|issue| issue.into_card(column.assignee, today))
                            .map(|card| SourceItem {
                                column: column.name.clone(),
                                card,
                            }),
                    );
                }
                Err(failure) => {
                    retry_at = retry_at.max(failure.retry_at);
                    failures.push(format!("{}: {}", column.name, failure.message));
                }
            }
        }

        if failures.len() == self.columns.len() {
            let error = SourceError::new(format!(
                "all Linear queries failed; {}",
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

    /// Every page of one stack's issues; `true` when pages were left unread.
    async fn column_issues(
        &self,
        settings: &LinearSettings,
        column: &LinearColumn,
    ) -> Result<(Vec<Issue>, bool), Failure> {
        let filter = column.filter();
        let mut issues = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let page = self
                .post(
                    settings,
                    &json!({
                        "query": ISSUES_QUERY,
                        "variables": { "filter": filter, "first": PAGE_SIZE, "after": cursor },
                    }),
                )
                .await?;
            issues.extend(page.nodes);
            match page.page_info.end_cursor {
                // A repeated cursor would loop over the same page.
                Some(next) if page.page_info.has_next_page && cursor.as_ref() != Some(&next) => {
                    cursor = Some(next);
                }
                _ => return Ok((issues, false)),
            }
        }
        Ok((issues, true))
    }

    /// POSTs one GraphQL request. Errors are safe to show and log: never the
    /// API key or the request URL.
    async fn post(&self, settings: &LinearSettings, body: &Value) -> Result<Connection, Failure> {
        let response = self
            .client
            .post(&settings.api_url)
            .timeout(self.request_timeout)
            // A personal API key goes bare; `Bearer` is for OAuth tokens.
            .header("authorization", &settings.api_key)
            .header(
                "user-agent",
                concat!("tasks-pending/", env!("CARGO_PKG_VERSION")),
            )
            .json(body)
            .send()
            .await
            .map_err(|error| self.describe(error))?;

        let status = response.status();
        if status.is_redirection() {
            return Err(format!(
                "Linear API returned {status}; redirects are not followed, check LINEAR_API_URL"
            )
            .into());
        }
        let reset = rate_limit_reset(response.headers());
        let reply = response.json::<Reply>().await;

        let error = match &reply {
            Ok(reply) => reply.errors.first(),
            Err(_) => None,
        };
        let code = error
            .and_then(|error| error.extensions.as_ref())
            .and_then(|extensions| extensions.code.as_deref())
            .unwrap_or_default();
        // Shown to the user: bounded, and without the key if the server echoes it.
        let detail: String = error
            .map(|error| error.message.replace(&settings.api_key, "<redacted>"))
            .unwrap_or_default()
            .chars()
            .take(MAX_MESSAGE_CHARS)
            .collect();

        // Linear answers 400 with this code when a limit is exhausted.
        if code == "RATELIMITED" || status == StatusCode::TOO_MANY_REQUESTS {
            return Err(Failure {
                message: match reset {
                    Some(at) => {
                        format!("Linear rate limit exceeded; resets at {}", at.to_rfc3339())
                    }
                    None => "Linear rate limit exceeded".to_owned(),
                },
                retry_at: reset,
            });
        }
        if code == "AUTHENTICATION_ERROR"
            || matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
        {
            return Err(with_detail(
                format!("Linear rejected the API key ({status}); check LINEAR_API_KEY"),
                &detail,
            )
            .into());
        }
        if error.is_some() || !status.is_success() {
            return Err(with_detail(format!("Linear API returned {status}"), &detail).into());
        }

        match reply {
            Ok(Reply {
                data: Some(Data {
                    issues: Some(issues),
                }),
                ..
            }) => Ok(issues),
            Ok(_) => Err("unexpected Linear response: no issues in the reply"
                .to_owned()
                .into()),
            Err(error) => {
                Err(format!("unexpected Linear response: {}", self.describe(error)).into())
            }
        }
    }

    /// The error and its causes, without the request URL.
    fn describe(&self, error: reqwest::Error) -> String {
        pending_http::describe_error(error, CONNECT_TIMEOUT, self.request_timeout)
    }
}

impl PendingSource for LinearSource {
    fn columns(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.name.clone()).collect()
    }

    fn refresh(&self) -> BoxFuture<'_, Result<SourceBatch, SourceError>> {
        Box::pin(async move {
            match &self.settings {
                Ok(settings) => self.refresh_with(settings).await,
                Err(message) => Err(SourceError::new(message.clone())),
            }
        })
    }
}

/// Why one stack's query failed; safe to show and log.
struct Failure {
    message: String,
    /// When Linear says the rate limit resets.
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

fn with_detail(message: String, detail: &str) -> String {
    if detail.is_empty() {
        message
    } else {
        format!("{message}: {detail}")
    }
}

/// When the exhausted limit resets, from Linear's `X-RateLimit-*-Reset`
/// headers (UTC epoch milliseconds). Requests and complexity are limited
/// separately: the exhausted one decides; when neither says it is exhausted,
/// the soonest reset.
fn rate_limit_reset(headers: &reqwest::header::HeaderMap) -> Option<DateTime<Utc>> {
    let number = |name: &str| -> Option<i64> {
        let value = headers.get(name)?.to_str().ok()?.trim();
        // Tolerates a decimal point.
        value.split('.').next()?.parse().ok()
    };
    let limits: Vec<(bool, DateTime<Utc>)> = ["requests", "complexity"]
        .iter()
        .filter_map(|kind| {
            let reset = number(&format!("x-ratelimit-{kind}-reset"))?;
            let exhausted =
                number(&format!("x-ratelimit-{kind}-remaining")).is_some_and(|left| left <= 0);
            Some((exhausted, DateTime::from_timestamp_millis(reset)?))
        })
        .collect();
    let exhausted = limits
        .iter()
        .filter(|(exhausted, _)| *exhausted)
        .map(|(_, at)| *at)
        .max();
    exhausted.or_else(|| limits.iter().map(|(_, at)| *at).min())
}

#[derive(Deserialize)]
struct Reply {
    #[serde(default)]
    data: Option<Data>,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Deserialize)]
struct Data {
    #[serde(default)]
    issues: Option<Connection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connection {
    #[serde(default)]
    nodes: Vec<Issue>,
    #[serde(default)]
    page_info: PageInfo,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    #[serde(default)]
    has_next_page: bool,
    #[serde(default)]
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
struct GraphqlError {
    #[serde(default)]
    message: String,
    #[serde(default)]
    extensions: Option<Extensions>,
}

#[derive(Deserialize)]
struct Extensions {
    #[serde(default)]
    code: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Issue {
    #[serde(default)]
    identifier: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    /// 0 is no priority, 1 urgent, 2 high, 3 medium, 4 low (a float in the API).
    #[serde(default)]
    priority: Option<f64>,
    /// `YYYY-MM-DD`.
    #[serde(default)]
    due_date: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    team: Option<Named>,
    #[serde(default)]
    state: Option<Named>,
    #[serde(default)]
    assignee: Option<User>,
}

#[derive(Deserialize)]
struct Named {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct User {
    #[serde(default)]
    display_name: Option<String>,
}

impl Issue {
    /// The card for an issue; `None` without a reference. `stack_assignee`
    /// is the stack's filter: only stacks that are not just mine name the
    /// assignee.
    fn into_card(self, stack_assignee: Assignee, today: NaiveDate) -> Option<PendingCard> {
        let identifier = self.identifier.filter(|id| !id.trim().is_empty())?;
        let due = self
            .due_date
            .as_deref()
            .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok());
        let overdue = due.is_some_and(|due| due < today);
        let priority = self.priority.map(|p| p.round() as i64).unwrap_or(0);
        let severity = match priority {
            _ if overdue => CardSeverity::Critical,
            1 => CardSeverity::Critical,
            2 => CardSeverity::Warning,
            _ => CardSeverity::Info,
        };

        // The reference is the card title; the issue title leads the details.
        let non_empty = |text: Option<String>| text.filter(|text| !text.trim().is_empty());
        let mut details: Vec<String> = [
            non_empty(self.title),
            non_empty(self.team.and_then(|team| team.name)),
            non_empty(self.state.and_then(|state| state.name)),
        ]
        .into_iter()
        .flatten()
        .collect();
        if stack_assignee != Assignee::Me
            && let Some(name) = non_empty(self.assignee.and_then(|user| user.display_name))
        {
            details.push(format!("@{name}"));
        }
        if let Some(due) = due {
            details.push(format!(
                "{} {due}",
                if overdue { "overdue since" } else { "due" }
            ));
        }

        Some(PendingCard {
            id: format!("linear:{identifier}"),
            title: identifier,
            body: details.join(" · "),
            source: "linear".to_owned(),
            url: non_empty(self.url),
            // Local midnight, matching how "overdue" is decided.
            due_at: due
                .and_then(|due| due.and_hms_opt(0, 0, 0))
                .and_then(|due| due.and_local_timezone(Local).earliest())
                .map(|due| due.with_timezone(&Utc)),
            severity,
            updated_at: self
                .updated_at
                .as_deref()
                .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
                .map(|at| at.with_timezone(&Utc))
                .unwrap_or(DateTime::UNIX_EPOCH),
        })
    }
}
