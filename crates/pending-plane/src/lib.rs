//! Plane as a pending-work source: open work items assigned to the owner of
//! the API key, across every project of one workspace.
//!
//! Filters are applied locally: some Plane deployments ignore the `assignees`
//! and `state_group` query parameters (observed by PlaneCockpit).

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Local, NaiveDate, Utc};
use pending_core::{
    BoxFuture, CardSeverity, PendingCard, PendingSource, SourceBatch, SourceError, SourceItem,
};
use reqwest::Client;
use reqwest::redirect::Policy;
use serde_json::Value;
use tokio::task::JoinSet;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Time one project may take (all its pages); past it the project becomes a
/// warning instead of failing the whole refresh.
const DEFAULT_PROJECT_BUDGET: Duration = Duration::from_secs(20);
const PAGE_SIZE: &str = "100";
/// Pages read per list before giving up with a warning.
const MAX_PAGES: usize = 20;
const CLOUD_API_HOST: &str = "https://api.plane.so";
const CLOUD_WEB_URL: &str = "https://app.plane.so";

/// Sections in display order, by Plane state group.
const SECTIONS: [(&str, &str); 3] = [
    ("started", "In progress"),
    ("unstarted", "To do"),
    ("backlog", "Backlog"),
];
/// State groups that are never pending.
const CLOSED_GROUPS: [&str; 2] = ["completed", "cancelled"];

#[derive(Clone)]
pub struct PlaneSettings {
    /// API origin, e.g. `https://plane.example.com` or `https://api.plane.so`.
    pub base_url: String,
    /// Web app origin used in card links (differs from the API on Plane Cloud).
    pub web_url: String,
    pub workspace_slug: String,
    pub api_key: String,
}

impl std::fmt::Debug for PlaneSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaneSettings")
            .field("base_url", &self.base_url)
            .field("web_url", &self.web_url)
            .field("workspace_slug", &self.workspace_slug)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl PlaneSettings {
    /// Reads `PLANE_BASE_URL`, `PLANE_WORKSPACE_SLUG`, `PLANE_API_KEY` and the
    /// optional `PLANE_WEB_URL` (defaults to the API origin, or
    /// `https://app.plane.so` for Plane Cloud). The error names the missing
    /// variables.
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let read = |key: &str| {
            env(key)
                .map(|v| v.trim().trim_end_matches('/').to_owned())
                .filter(|v| !v.is_empty())
        };
        let base_url = read("PLANE_BASE_URL");
        let workspace_slug = read("PLANE_WORKSPACE_SLUG");
        let api_key = read("PLANE_API_KEY");

        match (base_url, workspace_slug, api_key) {
            (Some(base_url), Some(workspace_slug), Some(api_key)) => {
                let web_url = read("PLANE_WEB_URL").unwrap_or_else(|| {
                    if base_url == CLOUD_API_HOST {
                        CLOUD_WEB_URL.to_owned()
                    } else {
                        base_url.clone()
                    }
                });
                Ok(Self {
                    base_url,
                    web_url,
                    workspace_slug,
                    api_key,
                })
            }
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
    project_budget: Duration,
}

impl PlaneSource {
    /// With `Err`, every refresh fails with that message (missing settings).
    pub fn new(settings: Result<PlaneSettings, String>) -> Self {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            // The API key is a custom header, which reqwest would forward to
            // whatever host a redirect points at.
            .redirect(Policy::none())
            .build()
            .unwrap_or_default();
        Self {
            client,
            settings: settings.map(|mut s| {
                s.base_url = s.base_url.trim_end_matches('/').to_owned();
                s.web_url = s.web_url.trim_end_matches('/').to_owned();
                s
            }),
            project_budget: DEFAULT_PROJECT_BUDGET,
        }
    }

    pub fn with_project_budget(mut self, budget: Duration) -> Self {
        self.project_budget = budget;
        self
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
                    api.refresh(self.project_budget)
                        .await
                        .map_err(SourceError::new)
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

/// State id → (group, name), for work items returned without `expand`.
type States = HashMap<String, (String, String)>;

struct ProjectItems {
    issues: Vec<Value>,
    truncated: bool,
    states: States,
}

impl Api {
    async fn refresh(self, project_budget: Duration) -> Result<SourceBatch, String> {
        let me = self.get("/users/me/", &[]).await?;
        let me = me
            .get("id")
            .or_else(|| me.pointer("/member/id"))
            .and_then(Value::as_str)
            .ok_or("unexpected Plane response for the current user")?
            .to_owned();

        let workspace = format!("/workspaces/{}", self.settings.workspace_slug);
        let (projects, projects_truncated) =
            self.pages(&format!("{workspace}/projects/"), &[]).await?;
        let projects: Vec<Project> = projects
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
        let mut task_index = HashMap::new();
        for (index, project) in projects.iter().enumerate() {
            let api = self.clone();
            let path = format!("{workspace}/projects/{}", project.id);
            let handle = tasks.spawn(async move {
                match tokio::time::timeout(project_budget, api.project_items(&path)).await {
                    Ok(result) => result,
                    Err(_) => Err(format!("timed out after {project_budget:?}")),
                }
            });
            task_index.insert(handle.id(), index);
        }
        let mut per_project: Vec<Option<Result<ProjectItems, String>>> =
            (0..projects.len()).map(|_| None).collect();
        while let Some(joined) = tasks.join_next_with_id().await {
            // A panicking project becomes a failure of that project only.
            let (id, result) = match joined {
                Ok((id, result)) => (id, result),
                Err(error) => (error.id(), Err(format!("project task failed: {error}"))),
            };
            per_project[task_index[&id]] = Some(result);
        }

        let today = Local::now().date_naive();
        let mut batch = SourceBatch::default();
        if projects_truncated {
            batch.warnings.push(format!(
                "more than {MAX_PAGES} pages of projects; showing the first ones"
            ));
        }
        let mut failures = Vec::new();
        let mut items = Vec::new();
        for (project, result) in projects.iter().zip(per_project) {
            match result.expect("every project task reports") {
                Ok(found) => {
                    if found.truncated {
                        batch.warnings.push(format!(
                            "{}: more than {MAX_PAGES} pages of work items; showing the first ones",
                            project.identifier
                        ));
                    }
                    let mut unknown_state = 0usize;
                    for issue in &found.issues {
                        match self.to_item(issue, project, &me, &found.states, today) {
                            Placement::Shown(rank, item) => items.push((rank, item)),
                            Placement::UnknownState => unknown_state += 1,
                            Placement::Hidden => {}
                        }
                    }
                    if unknown_state > 0 {
                        batch.warnings.push(format!(
                            "{}: {unknown_state} work items with an unknown state were skipped",
                            project.identifier
                        ));
                    }
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

    /// Work items of one project, plus its states when items came back
    /// without `expand`.
    async fn project_items(&self, project_path: &str) -> Result<ProjectItems, String> {
        let (issues, truncated) = self
            .pages(
                &format!("{project_path}/issues/"),
                &[("expand", "state,assignees")],
            )
            .await?;

        let mut states = States::new();
        if issues
            .iter()
            .any(|issue| issue.get("state").is_some_and(Value::is_string))
        {
            let (list, _) = self.pages(&format!("{project_path}/states/"), &[]).await?;
            for state in list {
                if let (Some(id), Some(group)) = (
                    state.get("id").and_then(Value::as_str),
                    state.get("group").and_then(Value::as_str),
                ) {
                    states.insert(
                        id.to_owned(),
                        (group.to_owned(), str_field(&state, "name").to_owned()),
                    );
                }
            }
        }
        Ok(ProjectItems {
            issues,
            truncated,
            states,
        })
    }

    /// Every page of a list endpoint; `true` when pages were left unread.
    async fn pages(
        &self,
        path: &str,
        extra: &[(&str, &str)],
    ) -> Result<(Vec<Value>, bool), String> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut query = vec![("per_page", PAGE_SIZE)];
            query.extend_from_slice(extra);
            if let Some(cursor) = &cursor {
                query.push(("cursor", cursor.as_str()));
            }
            let page = self.get(path, &query).await?;

            let more = page.get("next_page_results").and_then(Value::as_bool) != Some(false)
                && page.get("total_pages").and_then(Value::as_u64) != Some(1);
            let next = page
                .get("next_cursor")
                .and_then(Value::as_str)
                .filter(|next| !next.is_empty())
                .map(str::to_owned);
            let items = results(page);
            let empty = items.is_empty();
            all.extend(items);

            match next {
                // A repeated cursor would loop over the same page.
                Some(next) if more && !empty && cursor.as_ref() != Some(&next) => {
                    cursor = Some(next);
                }
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
        states: &States,
        today: NaiveDate,
    ) -> Placement {
        let assigned = issue
            .get("assignees")
            .and_then(Value::as_array)
            .is_some_and(|assignees| {
                assignees
                    .iter()
                    .any(|a| a.as_str().or_else(|| a.get("id").and_then(Value::as_str)) == Some(me))
            });
        if !assigned {
            return Placement::Hidden;
        }

        // Expanded state object, or a state id looked up in the project's states.
        let (group, state_name) = match issue.get("state") {
            Some(state @ Value::Object(_)) => (
                state.get("group").and_then(Value::as_str),
                state.get("name").and_then(Value::as_str),
            ),
            Some(Value::String(id)) => match states.get(id) {
                Some((group, name)) => (Some(group.as_str()), Some(name.as_str())),
                None => (None, None),
            },
            _ => (None, None),
        };
        let Some(group) = group else {
            return Placement::UnknownState;
        };
        if CLOSED_GROUPS.contains(&group) {
            return Placement::Hidden;
        }
        let Some((rank, section)) = SECTIONS
            .iter()
            .enumerate()
            .find(|(_, (g, _))| *g == group)
            .map(|(rank, (_, section))| (rank, *section))
        else {
            return Placement::UnknownState;
        };

        let (Some(id), Some(sequence)) = (
            issue.get("id").and_then(Value::as_str),
            issue.get("sequence_id").and_then(Value::as_u64),
        ) else {
            return Placement::Hidden;
        };
        let reference = format!("{}-{sequence}", project.identifier);
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
        if let Some(state_name) = state_name.filter(|name| !name.is_empty()) {
            body.push_str(&format!(" · {state_name}"));
        }
        if let Some(due) = due {
            body.push_str(&format!(
                " · {} {due}",
                if overdue { "overdue since" } else { "due" }
            ));
        }

        let timestamp = |key: &str| {
            issue
                .get(key)
                .and_then(Value::as_str)
                .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
                .map(|at| at.with_timezone(&Utc))
        };
        let settings = &self.settings;
        Placement::Shown(
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
                        settings.web_url, settings.workspace_slug, project.id
                    )),
                    // Local midnight, matching how "overdue" is decided.
                    due_at: due
                        .and_then(|due| due.and_hms_opt(0, 0, 0))
                        .and_then(|due| due.and_local_timezone(Local).earliest())
                        .map(|due| due.with_timezone(&Utc)),
                    severity,
                    updated_at: timestamp("updated_at")
                        .or_else(|| timestamp("created_at"))
                        .unwrap_or(DateTime::UNIX_EPOCH),
                },
            },
        )
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
        if status.is_redirection() {
            return Err(format!(
                "Plane API returned {status}; redirects are not followed, check PLANE_BASE_URL"
            ));
        }
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

enum Placement {
    Shown(usize, SourceItem),
    /// Not assigned to me, closed, or malformed.
    Hidden,
    /// Its state group could not be determined.
    UnknownState,
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

fn describe(error: reqwest::Error) -> String {
    pending_http::describe_error(error, CONNECT_TIMEOUT, REQUEST_TIMEOUT)
}
