//! Todoist as a pending-work source: one column per Todoist filter query
//! (the same syntax as the Todoist app, e.g. `today | overdue`).

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, Utc};
use futures_util::future::join_all;
use pending_core::{
    BoxFuture, CardSeverity, PendingCard, PendingSource, SourceBatch, SourceError, SourceItem,
};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;

pub const DEFAULT_API_URL: &str = "https://api.todoist.com";
/// Environment variable holding the personal API token.
pub const TOKEN_ENV: &str = "TODOIST_API_TOKEN";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum page size of the Todoist API.
const PAGE_SIZE: &str = "200";
/// Pages read per column before giving up with a warning.
const MAX_PAGES: usize = 10;

/// One column: a Todoist filter query.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodoistColumn {
    #[serde(skip)]
    pub name: String,
    pub filter: String,
}

/// Columns used when the configuration declares none.
pub fn default_columns() -> Vec<TodoistColumn> {
    vec![
        TodoistColumn {
            name: "Today".to_owned(),
            filter: "today | overdue".to_owned(),
        },
        TodoistColumn {
            name: "Next 7 days".to_owned(),
            filter: "7 days & !today & !overdue".to_owned(),
        },
    ]
}

pub struct TodoistSource {
    client: Client,
    api_url: String,
    token: Result<String, String>,
    columns: Vec<TodoistColumn>,
    today: fn() -> NaiveDate,
}

fn local_today() -> NaiveDate {
    Local::now().date_naive()
}

impl TodoistSource {
    /// With `Err`, every refresh fails with that message (missing token).
    pub fn new(api_url: impl Into<String>, token: Result<String, String>) -> Self {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            client,
            api_url: api_url.into().trim_end_matches('/').to_owned(),
            token,
            columns: default_columns(),
            today: local_today,
        }
    }

    /// Reads the token from [`TOKEN_ENV`].
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Self {
        let token = env(TOKEN_ENV)
            .map(|token| token.trim().to_owned())
            .filter(|token| !token.is_empty())
            .ok_or_else(|| {
                format!(
                    "Todoist is not configured: set {TOKEN_ENV} to the API token from Todoist \
                     Settings → Integrations → Developer, then restart"
                )
            });
        Self::new(DEFAULT_API_URL, token)
    }

    /// Replaces the default columns; an empty list keeps the defaults.
    pub fn with_columns(mut self, columns: Vec<TodoistColumn>) -> Self {
        if !columns.is_empty() {
            self.columns = columns;
        }
        self
    }

    /// The date "overdue" is measured against (the local date by default).
    pub fn with_today(mut self, today: fn() -> NaiveDate) -> Self {
        self.today = today;
        self
    }

    async fn refresh_with(&self, token: &str) -> Result<SourceBatch, String> {
        let mut batch = SourceBatch::default();

        let projects: HashMap<String, String> = match self.pages(token, "/projects", &[]).await {
            Ok((projects, _)) => projects
                .iter()
                .filter_map(|p| {
                    Some((
                        p.get("id")?.as_str()?.to_owned(),
                        p.get("name")?.as_str()?.to_owned(),
                    ))
                })
                .collect(),
            Err(message) => {
                batch
                    .warnings
                    .push(format!("project names unavailable: {message}"));
                HashMap::new()
            }
        };

        let results = join_all(
            self.columns
                .iter()
                .map(|column| self.filter_tasks(token, &column.filter)),
        )
        .await;

        let today = (self.today)();
        let mut failures = Vec::new();
        for (column, result) in self.columns.iter().zip(results) {
            match result {
                Ok((tasks, truncated)) => {
                    if truncated {
                        batch.warnings.push(format!(
                            "{}: more than {MAX_PAGES} pages of tasks; showing the first ones",
                            column.name
                        ));
                    }
                    batch.items.extend(tasks.iter().filter_map(|task| {
                        Some(SourceItem {
                            column: column.name.clone(),
                            card: to_card(task, &projects, today)?,
                        })
                    }));
                }
                Err(message) => failures.push(format!("{}: {message}", column.name)),
            }
        }

        if failures.len() == self.columns.len() {
            return Err(format!(
                "all Todoist filters failed; {}",
                failures.join("; ")
            ));
        }
        batch.warnings.extend(failures);
        Ok(batch)
    }

    /// Every task matching a Todoist filter query.
    async fn filter_tasks(&self, token: &str, query: &str) -> Result<(Vec<Value>, bool), String> {
        self.pages(token, "/tasks/filter", &[("query", query)])
            .await
    }

    /// Every page of a list endpoint; `true` when pages were left unread.
    async fn pages(
        &self,
        token: &str,
        path: &str,
        extra: &[(&str, &str)],
    ) -> Result<(Vec<Value>, bool), String> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut query = vec![("limit", PAGE_SIZE)];
            query.extend_from_slice(extra);
            if let Some(cursor) = &cursor {
                query.push(("cursor", cursor.as_str()));
            }
            let page = self.get(token, path, &query).await?;
            let next = page
                .get("next_cursor")
                .and_then(Value::as_str)
                .filter(|next| !next.is_empty())
                .map(str::to_owned);
            if let Some(Value::Array(items)) = page.get("results") {
                all.extend(items.iter().cloned());
            }
            match next {
                // A repeated cursor would loop; report the list as cut short.
                Some(next) if cursor.as_ref() == Some(&next) => return Ok((all, true)),
                Some(next) => cursor = Some(next),
                None => return Ok((all, false)),
            }
        }
        Ok((all, true))
    }

    /// GET `/api/v1{path}`. Errors never contain the token or the URL.
    async fn get(&self, token: &str, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
        let response = self
            .client
            .get(format!("{}/api/v1{path}", self.api_url))
            .query(query)
            .bearer_auth(token)
            .send()
            .await
            .map_err(describe)?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("Todoist API returned {status}"));
        }
        response
            .json::<Value>()
            .await
            .map_err(|error| format!("unexpected Todoist response: {}", describe(error)))
    }
}

impl PendingSource for TodoistSource {
    fn columns(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.name.clone()).collect()
    }

    fn refresh(&self) -> BoxFuture<'_, Result<SourceBatch, SourceError>> {
        Box::pin(async move {
            match &self.token {
                Ok(token) => self.refresh_with(token).await.map_err(SourceError::new),
                Err(message) => Err(SourceError::new(message.clone())),
            }
        })
    }
}

/// When a task is due: all-day at local midnight, a floating time in the
/// local zone, or an exact instant. `true` for all-day.
fn due(task: &Value) -> Option<(DateTime<Utc>, NaiveDate, bool)> {
    let date = task.pointer("/due/date")?.as_str()?;
    if let Ok(day) = NaiveDate::parse_from_str(date, "%Y-%m-%d") {
        let at = day
            .and_hms_opt(0, 0, 0)?
            .and_local_timezone(Local)
            .earliest()?
            .with_timezone(&Utc);
        return Some((at, day, true));
    }
    if let Ok(at) = DateTime::parse_from_rfc3339(date) {
        let at = at.with_timezone(&Utc);
        return Some((at, at.with_timezone(&Local).date_naive(), false));
    }
    let naive = NaiveDateTime::parse_from_str(date, "%Y-%m-%dT%H:%M:%S%.f").ok()?;
    let at = naive
        .and_local_timezone(Local)
        .earliest()?
        .with_timezone(&Utc);
    Some((at, naive.date(), false))
}

fn to_card(
    task: &Value,
    projects: &HashMap<String, String>,
    today: NaiveDate,
) -> Option<PendingCard> {
    let id = task.get("id")?.as_str()?;
    let due = due(task);
    // Timed tasks are late once their time passes; all-day ones the next day.
    let overdue = due.is_some_and(|(at, day, all_day)| {
        if all_day {
            day < today
        } else {
            at < Utc::now()
        }
    });
    // The API's 4 is the app's p1.
    let priority = task.get("priority").and_then(Value::as_u64).unwrap_or(1);

    let mut body = task
        .get("project_id")
        .and_then(Value::as_str)
        .and_then(|project| projects.get(project))
        .cloned()
        .unwrap_or_else(|| "Todoist".to_owned());
    if let Some((at, day, all_day)) = due {
        let when = if all_day {
            day.to_string()
        } else {
            at.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        };
        body.push_str(&format!(
            " · {} {when}",
            if overdue { "overdue since" } else { "due" }
        ));
    }
    if let Some(labels) = task.get("labels").and_then(Value::as_array) {
        for label in labels.iter().filter_map(Value::as_str) {
            body.push_str(&format!(" @{label}"));
        }
    }

    let timestamp = |key: &str| {
        task.get(key)
            .and_then(Value::as_str)
            .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
            .map(|at| at.with_timezone(&Utc))
    };
    Some(PendingCard {
        id: format!("todoist:{id}"),
        title: task
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("(no title)")
            .to_owned(),
        body,
        source: "todoist".to_owned(),
        url: Some(format!("https://app.todoist.com/app/task/{id}")),
        due_at: due.map(|(at, _, _)| at),
        severity: if overdue {
            CardSeverity::Critical
        } else if priority >= 4 {
            CardSeverity::Warning
        } else {
            CardSeverity::Info
        },
        updated_at: timestamp("updated_at")
            .or_else(|| timestamp("added_at"))
            .unwrap_or(DateTime::UNIX_EPOCH),
    })
}

fn describe(error: reqwest::Error) -> String {
    pending_http::describe_error(error, CONNECT_TIMEOUT, REQUEST_TIMEOUT)
}
