//! Jira as a pending-work source: one JQL query per stack.
//!
//! Jira Cloud (e-mail and API token, HTTP Basic) is asked through the
//! enhanced search, `GET /rest/api/3/search/jql`, paged with `nextPageToken`.
//! Jira Data Center (personal access token, Bearer) is asked through
//! `GET /rest/api/2/search`, paged with `startAt`. Both get the same JQL and
//! the same list of fields, and their issues have the same shape.

use std::time::Duration;

use chrono::{DateTime, Local, NaiveDate, Utc};
use futures_util::future::join_all;
use pending_core::{
    BoxFuture, CardSeverity, PendingCard, PendingSource, SourceBatch, SourceError, SourceItem,
};
use reqwest::header::HeaderMap;
use reqwest::redirect::Policy;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::Value;

/// Per request; a stack whose request times out becomes a warning.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Queries running at once. Each stack is one query.
const MAX_CONCURRENT_QUERIES: usize = 3;
/// Issues asked per page; the server may send fewer.
const PAGE_SIZE: &str = "100";
/// Pages read per stack before giving up with a warning.
const MAX_PAGES: usize = 5;
/// The search returns only what is asked for (ids alone by default on Cloud).
const FIELDS: &str = "summary,status,priority,assignee,duedate,updated,project";
/// Longest piece of a server error message shown.
const MAX_DETAIL_CHARS: usize = 300;

const CLOUD_SEARCH_PATH: &str = "/rest/api/3/search/jql";
const DATA_CENTER_SEARCH_PATH: &str = "/rest/api/2/search";

/// One column: a JQL query.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JiraColumn {
    #[serde(skip)]
    pub name: String,
    /// A JQL query, as typed in Jira's issue search.
    pub jql: String,
    /// Severity of every card in this column. Without it, overdue issues
    /// and the highest priorities are critical and high ones are warnings.
    #[serde(default)]
    pub severity: Option<CardSeverity>,
}

impl JiraColumn {
    /// The query must not be blank.
    pub fn validate(&self) -> Result<(), String> {
        if self.jql.trim().is_empty() {
            return Err("`jql` must not be empty".to_owned());
        }
        Ok(())
    }
}

/// Columns used when the configuration declares none: your issues by status
/// category.
pub fn default_columns() -> Vec<JiraColumn> {
    let column = |name: &str, jql: &str| JiraColumn {
        name: name.to_owned(),
        jql: jql.to_owned(),
        severity: None,
    };
    vec![
        column(
            "In progress",
            r#"assignee = currentUser() AND statusCategory = "In Progress" ORDER BY updated DESC"#,
        ),
        column(
            "To do",
            r#"assignee = currentUser() AND statusCategory = "To Do" ORDER BY priority DESC, updated DESC"#,
        ),
    ]
}

/// How the source logs in, which also says which Jira it talks to.
#[derive(Clone, PartialEq, Eq)]
pub enum JiraAuth {
    /// Jira Cloud: HTTP Basic with the account's e-mail and an API token.
    Cloud { email: String, api_token: String },
    /// Jira Data Center: a personal access token sent as a Bearer token.
    DataCenter { token: String },
}

#[derive(Clone)]
pub struct JiraSettings {
    /// Site address, e.g. `https://yourcompany.atlassian.net`; a Data Center
    /// context path (`https://jira.example.com/jira`) is kept.
    pub base_url: String,
    pub auth: JiraAuth,
}

impl std::fmt::Debug for JiraSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let flavour = match self.auth {
            JiraAuth::Cloud { .. } => "cloud",
            JiraAuth::DataCenter { .. } => "data center",
        };
        f.debug_struct("JiraSettings")
            .field("base_url", &self.base_url)
            .field("flavour", &flavour)
            .field("credentials", &"<redacted>")
            .finish()
    }
}

impl JiraSettings {
    /// Reads `JIRA_BASE_URL` and either `JIRA_EMAIL` with `JIRA_API_TOKEN`
    /// (Jira Cloud) or `JIRA_TOKEN` alone (Jira Data Center). With all three
    /// credentials set, the Cloud pair wins. The error names the missing
    /// variables.
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let read = |key: &str| {
            env(key)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let base_url = read("JIRA_BASE_URL").map(|url| url.trim_end_matches('/').to_owned());
        let auth = match (
            read("JIRA_EMAIL"),
            read("JIRA_API_TOKEN"),
            read("JIRA_TOKEN"),
        ) {
            (Some(email), Some(api_token), _) => Ok(JiraAuth::Cloud { email, api_token }),
            (_, _, Some(token)) => Ok(JiraAuth::DataCenter { token }),
            (Some(_), None, None) => {
                Err("JIRA_API_TOKEN (Jira Cloud; or JIRA_TOKEN alone for Jira Data Center)")
            }
            (None, Some(_), None) => {
                Err("JIRA_EMAIL (Jira Cloud; or JIRA_TOKEN alone for Jira Data Center)")
            }
            (None, None, None) => {
                Err("JIRA_EMAIL with JIRA_API_TOKEN (Jira Cloud) or JIRA_TOKEN (Jira Data Center)")
            }
        };

        match (base_url, auth) {
            (Some(base_url), Ok(auth)) => {
                // The credentials are a header on every request: never
                // send them unencrypted.
                pending_http::require_encrypted(&base_url, "JIRA_BASE_URL")
                    .map_err(|problem| format!("Jira is not configured: {problem}"))?;
                Ok(Self { base_url, auth })
            }
            (base_url, auth) => {
                let mut missing = Vec::new();
                if base_url.is_none() {
                    missing.push("JIRA_BASE_URL");
                }
                if let Err(credentials) = auth {
                    missing.push(credentials);
                }
                Err(format!(
                    "Jira is not configured: set {}, then restart",
                    missing.join(", and ")
                ))
            }
        }
    }
}

pub struct JiraSource {
    client: Client,
    settings: Result<JiraSettings, String>,
    request_timeout: Duration,
    columns: Vec<JiraColumn>,
    today: fn() -> NaiveDate,
}

fn local_today() -> NaiveDate {
    Local::now().date_naive()
}

impl JiraSource {
    /// With `Err`, every refresh fails with that message (missing settings).
    pub fn new(settings: Result<JiraSettings, String>) -> Self {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            // A redirect could point at another host; the credentials must
            // not follow it there.
            .redirect(Policy::none())
            .build()
            .unwrap_or_default();
        Self {
            client,
            settings: settings.map(|mut s| {
                s.base_url = s.base_url.trim().trim_end_matches('/').to_owned();
                s
            }),
            request_timeout: REQUEST_TIMEOUT,
            columns: default_columns(),
            today: local_today,
        }
    }

    /// Replaces the default columns; an empty list keeps the defaults.
    pub fn with_columns(mut self, columns: Vec<JiraColumn>) -> Self {
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

    async fn refresh_with(&self, settings: &JiraSettings) -> Result<SourceBatch, SourceError> {
        // A few at a time, so the server never sees a burst. Results keep
        // column order.
        let mut results = Vec::with_capacity(self.columns.len());
        for chunk in self.columns.chunks(MAX_CONCURRENT_QUERIES) {
            results.extend(
                join_all(
                    chunk
                        .iter()
                        .map(|column| self.search(settings, &column.jql)),
                )
                .await,
            );
        }

        let today = (self.today)();
        let mut batch = SourceBatch::default();
        let mut failures = Vec::new();
        let mut retry_at = None;
        for (column, result) in self.columns.iter().zip(results) {
            match result {
                Ok((issues, truncated)) => {
                    if truncated {
                        batch.warnings.push(format!(
                            "{}: more than {MAX_PAGES} pages of issues; showing the first ones",
                            column.name
                        ));
                    }
                    batch.items.extend(issues.iter().filter_map(|issue| {
                        Some(SourceItem {
                            column: column.name.clone(),
                            card: to_card(issue, &settings.base_url, today, column.severity)?,
                        })
                    }));
                }
                Err(failure) => {
                    retry_at = retry_at.max(failure.retry_at);
                    failures.push(format!("{}: {}", column.name, failure.message));
                }
            }
        }

        if failures.len() == self.columns.len() {
            let error =
                SourceError::new(format!("all Jira queries failed; {}", failures.join("; ")));
            return Err(match retry_at {
                Some(at) => error.with_retry_at(at),
                None => error,
            });
        }
        batch.warnings.extend(failures);
        Ok(batch)
    }

    /// Every issue matching a JQL query, up to [`MAX_PAGES`] pages; `true`
    /// when pages were left unread.
    async fn search(
        &self,
        settings: &JiraSettings,
        jql: &str,
    ) -> Result<(Vec<Value>, bool), Failure> {
        let cloud = matches!(settings.auth, JiraAuth::Cloud { .. });
        let path = if cloud {
            CLOUD_SEARCH_PATH
        } else {
            DATA_CENTER_SEARCH_PATH
        };

        let mut all = Vec::new();
        // Cloud continues from a token, Data Center from an offset.
        let mut token: Option<String> = None;
        let mut start_at = 0usize;
        for _ in 0..MAX_PAGES {
            let start = start_at.to_string();
            let mut query = vec![("jql", jql), ("maxResults", PAGE_SIZE), ("fields", FIELDS)];
            match &token {
                Some(token) if cloud => query.push(("nextPageToken", token.as_str())),
                _ if cloud => {}
                _ => query.push(("startAt", start.as_str())),
            }
            let mut page = self.get(settings, path, &query).await?;

            let Some(Value::Array(issues)) = page.get_mut("issues").map(Value::take) else {
                return Err("unexpected Jira response: no list of issues"
                    .to_owned()
                    .into());
            };
            let count = issues.len();
            all.extend(issues);

            if cloud {
                let next = page
                    .get("nextPageToken")
                    .and_then(Value::as_str)
                    .filter(|next| !next.is_empty())
                    .map(str::to_owned);
                let last = page.get("isLast").and_then(Value::as_bool) == Some(true);
                match next {
                    Some(_) if last => return Ok((all, false)),
                    // A repeated token would loop; report the list as cut short.
                    Some(next) if token.as_ref() == Some(&next) => return Ok((all, true)),
                    Some(next) => token = Some(next),
                    None => return Ok((all, false)),
                }
            } else {
                start_at += count;
                let total = page.get("total").and_then(Value::as_u64).unwrap_or(0);
                if count == 0 || start_at as u64 >= total {
                    return Ok((all, false));
                }
            }
        }
        Ok((all, true))
    }

    /// GET `{base}{path}`. Errors never contain the credentials or the URL.
    async fn get(
        &self,
        settings: &JiraSettings,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<Value, Failure> {
        let request = self
            .client
            .get(format!("{}{path}", settings.base_url))
            .query(query)
            .timeout(self.request_timeout)
            .header("accept", "application/json")
            .header(
                "user-agent",
                concat!("tasks-pending/", env!("CARGO_PKG_VERSION")),
            );
        let request = match &settings.auth {
            JiraAuth::Cloud { email, api_token } => request.basic_auth(email, Some(api_token)),
            JiraAuth::DataCenter { token } => request.bearer_auth(token),
        };
        let response = request.send().await.map_err(|error| self.describe(error))?;

        let status = response.status();
        if status.is_redirection() {
            return Err(format!(
                "Jira API returned {status}; redirects are not followed, check JIRA_BASE_URL"
            )
            .into());
        }
        if status.is_success() {
            // Data Center answers as an anonymous user when the login fails,
            // and says so only in this header.
            if login_failed(response.headers()) {
                return Err(format!(
                    "Jira rejected the credentials: {}",
                    credentials_hint(&settings.auth)
                )
                .into());
            }
            return response.json::<Value>().await.map_err(|error| {
                format!("unexpected Jira response: {}", self.describe(error)).into()
            });
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_at = retry_after(response.headers(), Utc::now());
            return Err(Failure {
                message: match retry_at {
                    Some(at) => {
                        format!("Jira rate limit exceeded; retry after {}", at.to_rfc3339())
                    }
                    None => "Jira rate limit exceeded".to_owned(),
                },
                retry_at,
            });
        }
        if status == StatusCode::UNAUTHORIZED {
            return Err(format!(
                "Jira API returned {status}: {}",
                credentials_hint(&settings.auth)
            )
            .into());
        }
        let body = response.json::<Value>().await.unwrap_or_default();
        Err(
            format!("Jira API returned {status}: {}", error_detail(&body))
                .trim_end_matches([':', ' '])
                .to_owned()
                .into(),
        )
    }

    /// The error and its causes, without the request URL.
    fn describe(&self, error: reqwest::Error) -> String {
        pending_http::describe_error(error, CONNECT_TIMEOUT, self.request_timeout)
    }
}

impl PendingSource for JiraSource {
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

/// Why one query failed; safe to show and log.
struct Failure {
    message: String,
    /// When Jira says to try again (rate limit).
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

/// Which variables to check, by flavour; never their values.
fn credentials_hint(auth: &JiraAuth) -> &'static str {
    match auth {
        JiraAuth::Cloud { .. } => {
            "check JIRA_EMAIL and JIRA_API_TOKEN (Jira Cloud); on Jira Data Center, set only \
             JIRA_TOKEN (a personal access token)"
        }
        JiraAuth::DataCenter { .. } => {
            "check JIRA_TOKEN (a Jira Data Center personal access token); on Jira Cloud, set \
             JIRA_EMAIL and JIRA_API_TOKEN instead"
        }
    }
}

/// `X-Seraph-LoginReason` reporting a failed or refused login.
fn login_failed(headers: &HeaderMap) -> bool {
    headers
        .get("x-seraph-loginreason")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|reason| {
            reason.contains("AUTHENTICATED_FAILED") || reason.contains("AUTHENTICATION_DENIED")
        })
}

/// `Retry-After` as a point in time: a number of seconds or an HTTP date.
fn retry_after(headers: &HeaderMap, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let value = headers.get("retry-after")?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u32>() {
        return Some(now + chrono::Duration::seconds(i64::from(seconds)));
    }
    DateTime::parse_from_rfc2822(value)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// What a Jira error body says: `errorMessages`, then the per-field
/// `errors`, else a plain `message`.
fn error_detail(body: &Value) -> String {
    let mut parts: Vec<&str> = body
        .get("errorMessages")
        .and_then(Value::as_array)
        .map(|messages| messages.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if let Some(errors) = body.get("errors").and_then(Value::as_object) {
        parts.extend(errors.values().filter_map(Value::as_str));
    }
    if parts.is_empty() {
        parts.extend(body.get("message").and_then(Value::as_str));
    }
    let detail = parts.join(" ");
    match detail.char_indices().nth(MAX_DETAIL_CHARS) {
        Some((cut, _)) => format!("{}…", &detail[..cut]),
        None => detail,
    }
}

/// A Jira timestamp, e.g. `2026-09-28T10:00:00.000+0000` (offset without a
/// colon, so not RFC 3339); RFC 3339 is accepted too.
#[doc(hidden)]
pub fn parse_timestamp(text: &str) -> Option<DateTime<Utc>> {
    let text = text.trim();
    DateTime::parse_from_rfc3339(text)
        .or_else(|_| DateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f%z"))
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// The card for an issue; `None` when it has no key.
fn to_card(
    issue: &Value,
    base_url: &str,
    today: NaiveDate,
    severity: Option<CardSeverity>,
) -> Option<PendingCard> {
    let key = issue
        .get("key")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty())?;
    let text = |pointer: &str| {
        issue
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
    };

    let due = text("/fields/duedate").and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
    let overdue = due.is_some_and(|due| due < today);
    let priority = text("/fields/priority/name")
        .unwrap_or_default()
        .to_lowercase();
    let computed = match priority.as_str() {
        _ if overdue => CardSeverity::Critical,
        "highest" | "blocker" | "critical" => CardSeverity::Critical,
        "high" | "major" => CardSeverity::Warning,
        _ => CardSeverity::Info,
    };

    // The key is the card title; the summary leads the details.
    let mut body = text("/fields/summary").unwrap_or("(no summary)").to_owned();
    for part in [text("/fields/project/name"), text("/fields/status/name")]
        .into_iter()
        .flatten()
    {
        body.push_str(&format!(" · {part}"));
    }
    if let Some(assignee) = text("/fields/assignee/displayName") {
        body.push_str(&format!(" · @{assignee}"));
    }
    if let Some(due) = due {
        body.push_str(&format!(
            " · {} {due}",
            if overdue { "overdue since" } else { "due" }
        ));
    }

    Some(PendingCard {
        id: format!("jira:{key}"),
        title: key.to_owned(),
        body,
        source: "jira".to_owned(),
        url: Some(format!("{base_url}/browse/{key}")),
        // Local midnight, matching how "overdue" is decided.
        due_at: due
            .and_then(|due| due.and_hms_opt(0, 0, 0))
            .and_then(|due| due.and_local_timezone(Local).earliest())
            .map(|due| due.with_timezone(&Utc)),
        severity: severity.unwrap_or(computed),
        updated_at: text("/fields/updated")
            .and_then(parse_timestamp)
            .unwrap_or(DateTime::UNIX_EPOCH),
    })
}
