//! Plane as a pending-work source: open work items assigned to the owner of
//! the API key, across every project of one workspace.
//!
//! Filters are applied locally: some Plane deployments ignore the `assignees`
//! and `state_group` query parameters (observed by PlaneCockpit).

use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use pending_core::{
    BoxFuture, CardSeverity, PendingCard, PendingSource, SourceBatch, SourceError, SourceItem,
};
use reqwest::Client;
use serde_json::Value;
use tokio::task::JoinSet;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PAGE_SIZE: &str = "100";
/// Pages read per project before giving up with a warning.
const MAX_PAGES: usize = 20;

/// Sections in display order, by Plane state group.
const SECTIONS: [(&str, &str); 3] = [
    ("started", "In progress"),
    ("unstarted", "To do"),
    ("backlog", "Backlog"),
];

#[derive(Debug, Clone)]
pub struct PlaneSettings {
    /// Instance URL, e.g. `https://plane.example.com` (cloud: `https://api.plane.so`).
    pub base_url: String,
    pub workspace_slug: String,
    pub api_key: String,
}

impl PlaneSettings {
    /// Reads `PLANE_BASE_URL`, `PLANE_WORKSPACE_SLUG` and `PLANE_API_KEY`.
    /// The error names the missing variables.
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let read = |key: &str| {
            env(key)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let base_url = read("PLANE_BASE_URL");
        let workspace_slug = read("PLANE_WORKSPACE_SLUG");
        let api_key = read("PLANE_API_KEY");

        match (base_url, workspace_slug, api_key) {
            (Some(base_url), Some(workspace_slug), Some(api_key)) => Ok(Self {
                base_url,
                workspace_slug,
                api_key,
            }),
            (base_url, workspace_slug, api_key) => {
                let missing: Vec<&str> = [
                    ("PLANE_BASE_URL", base_url.is_none()),
                    ("PLANE_WORKSPACE_SLUG", workspace_slug.is_none()),
                    ("PLANE_API_KEY", api_key.is_none()),
                ]
                .into_iter()
                .filter_map(|(name, missing)| missing.then_some(name))
                .collect();
                Err(format!(
                    "Plane is not configured: set {}, then restart",
                    missing.join(", ")
                ))
            }
        }
    }
}

pub struct PlaneSource {
    client: Client,
    settings: Result<PlaneSettings, String>,
}

impl PlaneSource {
    /// With `Err`, every refresh fails with that message (missing settings).
    pub fn new(settings: Result<PlaneSettings, String>) -> Self {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            client,
            settings: settings.map(|mut s| {
                s.base_url = s.base_url.trim_end_matches('/').to_owned();
                s
            }),
        }
    }
}

impl PendingSource for PlaneSource {
    fn refresh(&self) -> BoxFuture<'_, Result<SourceBatch, SourceError>> {
        Box::pin(async move {
            match &self.settings {
                Ok(settings) => {
                    let api = Api {
                        client: self.client.clone(),
                        settings: settings.clone(),
                    };
                    api.refresh().await.map_err(SourceError::new)
                }
                Err(message) => Err(SourceError::new(message.clone())),
            }
        })
    }
}

#[derive(Clone)]
struct Api {
    client: Client,
    settings: PlaneSettings,
}

struct Project {
    id: String,
    identifier: String,
    name: String,
}

impl Api {
    async fn refresh(self) -> Result<SourceBatch, String> {
        let me = self.get("/users/me/", &[]).await?;
        let me = me
            .get("id")
            .or_else(|| me.pointer("/member/id"))
            .and_then(Value::as_str)
            .ok_or("unexpected Plane response for the current user")?
            .to_owned();

        let workspace = format!("/workspaces/{}", self.settings.workspace_slug);
        let projects: Vec<Project> =
            results(self.get(&format!("{workspace}/projects/"), &[]).await?)
                .iter()
                .filter_map(|project| {
                    Some(Project {
                        id: project.get("id")?.as_str()?.to_owned(),
                        identifier: project.get("identifier")?.as_str()?.to_owned(),
                        name: str_field(project, "name").to_owned(),
                    })
                })
                .collect();

        let mut tasks = JoinSet::new();
        for (index, project) in projects.iter().enumerate() {
            let api = self.clone();
            let path = format!("{workspace}/projects/{}/issues/", project.id);
            tasks.spawn(async move { (index, api.issues(&path).await) });
        }
        let mut per_project = vec![None; projects.len()];
        while let Some(joined) = tasks.join_next().await {
            let (index, result) = joined.map_err(|error| format!("Plane task failed: {error}"))?;
            per_project[index] = Some(result);
        }

        let today = Utc::now().date_naive();
        let mut batch = SourceBatch::default();
        let mut failures = Vec::new();
        let mut items = Vec::new();
        for (project, result) in projects.iter().zip(per_project) {
            match result.expect("every project task reports") {
                Ok((issues, truncated)) => {
                    if truncated {
                        batch.warnings.push(format!(
                            "{}: more than {MAX_PAGES} pages of work items; showing the first ones",
                            project.identifier
                        ));
                    }
                    items.extend(
                        issues
                            .iter()
                            .filter_map(|issue| self.to_item(issue, project, &me, today)),
                    );
                }
                Err(message) => failures.push(format!("{}: {message}", project.identifier)),
            }
        }

        if !projects.is_empty() && failures.len() == projects.len() {
            return Err(format!(
                "all Plane projects failed; {}",
                failures.join("; ")
            ));
        }
        // Sections appear in state order: in progress, to do, backlog.
        items.sort_by_key(|(rank, _)| *rank);
        batch.items = items.into_iter().map(|(_, item)| item).collect();
        batch.warnings.extend(failures);
        Ok(batch)
    }

    /// Every page of a project's work items; `true` when pages were left unread.
    async fn issues(&self, path: &str) -> Result<(Vec<Value>, bool), String> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut query = vec![("per_page", PAGE_SIZE), ("expand", "state,assignees")];
            if let Some(cursor) = &cursor {
                query.push(("cursor", cursor.as_str()));
            }
            let page = self.get(path, &query).await?;
            let items = results(page.clone());
            let empty = items.is_empty();
            all.extend(items);

            let more = page.get("next_page_results").and_then(Value::as_bool) != Some(false)
                && page.get("total_pages").and_then(Value::as_u64) != Some(1);
            let next = page
                .get("next_cursor")
                .and_then(Value::as_str)
                .filter(|next| !next.is_empty())
                .map(str::to_owned);
            match next {
                Some(next) if more && !empty => cursor = Some(next),
                _ => return Ok((all, false)),
            }
        }
        Ok((all, true))
    }

    fn to_item(
        &self,
        issue: &Value,
        project: &Project,
        me: &str,
        today: NaiveDate,
    ) -> Option<(usize, SourceItem)> {
        let assigned = issue
            .get("assignees")
            .and_then(Value::as_array)
            .is_some_and(|assignees| {
                assignees
                    .iter()
                    .any(|a| a.as_str().or_else(|| a.get("id").and_then(Value::as_str)) == Some(me))
            });
        if !assigned {
            return None;
        }

        // Without `expand`, `state` is an id; treat an unknown group as backlog.
        let state = issue.get("state");
        let group = state
            .and_then(|s| s.get("group"))
            .and_then(Value::as_str)
            .unwrap_or("backlog");
        let (rank, section) = SECTIONS
            .iter()
            .enumerate()
            .find(|(_, (g, _))| *g == group)
            .map(|(rank, (_, section))| (rank, *section))?;

        let id = issue.get("id")?.as_str()?;
        let reference = format!(
            "{}-{}",
            project.identifier,
            issue.get("sequence_id")?.as_u64()?
        );
        let due = issue
            .get("target_date")
            .and_then(Value::as_str)
            .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok());
        let overdue = due.is_some_and(|due| due < today);

        let severity = match issue.get("priority").and_then(Value::as_str) {
            _ if overdue => CardSeverity::Critical,
            Some("urgent") => CardSeverity::Critical,
            Some("high") => CardSeverity::Warning,
            _ => CardSeverity::Info,
        };

        let mut body = format!("{reference} · {}", project.name);
        if let Some(state_name) = state.and_then(|s| s.get("name")).and_then(Value::as_str) {
            body.push_str(&format!(" · {state_name}"));
        }
        if let Some(due) = due {
            body.push_str(&format!(
                " · {} {due}",
                if overdue { "overdue since" } else { "due" }
            ));
        }

        let settings = &self.settings;
        Some((
            rank,
            SourceItem {
                section: section.to_owned(),
                card: PendingCard {
                    id: format!("plane:{reference}"),
                    title: str_field(issue, "name").to_owned(),
                    body,
                    source: "plane".to_owned(),
                    url: Some(format!(
                        "{}/{}/projects/{}/issues/{id}",
                        settings.base_url, settings.workspace_slug, project.id
                    )),
                    severity,
                    due_at: due
                        .and_then(|due| due.and_hms_opt(0, 0, 0))
                        .map(|due| due.and_utc()),
                    updated_at: issue
                        .get("updated_at")
                        .and_then(Value::as_str)
                        .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
                        .map_or_else(Utc::now, |at| at.with_timezone(&Utc)),
                },
            },
        ))
    }

    /// GET `/api/v1{path}`. Errors never contain the API key or the URL.
    async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
        let response = self
            .client
            .get(format!("{}/api/v1{path}", self.settings.base_url))
            .query(query)
            .header("x-api-key", &self.settings.api_key)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(describe)?;

        let status = response.status();
        if status.is_success() {
            return response
                .json::<Value>()
                .await
                .map_err(|error| format!("unexpected Plane response: {}", describe(error)));
        }
        let body = response.json::<Value>().await.unwrap_or_default();
        let detail = ["detail", "error", "message"]
            .iter()
            .find_map(|key| body.get(*key).and_then(Value::as_str))
            .unwrap_or_default();
        Err(format!("Plane API returned {status}: {detail}")
            .trim_end_matches([':', ' '])
            .to_owned())
    }
}

/// A list response: a bare array or `{ "results": [...] }`.
fn results(value: Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items,
        Value::Object(mut map) => match map.remove("results") {
            Some(Value::Array(items)) => items,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn str_field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// The error and its causes, without the request URL.
fn describe(error: reqwest::Error) -> String {
    if error.is_timeout() {
        return if error.is_connect() {
            format!("connection timed out after {CONNECT_TIMEOUT:?}")
        } else {
            format!("timed out after {REQUEST_TIMEOUT:?}")
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
