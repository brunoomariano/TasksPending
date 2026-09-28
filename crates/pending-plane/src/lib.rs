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
use serde::Deserialize;
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

/// State groups that count as open when a column lists neither groups nor
/// state names.
const OPEN_GROUPS: [StateGroup; 3] = [
    StateGroup::Backlog,
    StateGroup::Unstarted,
    StateGroup::Started,
];

/// Plane's state groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StateGroup {
    Backlog,
    Unstarted,
    Started,
    Completed,
    Cancelled,
}

impl StateGroup {
    fn as_str(self) -> &'static str {
        match self {
            StateGroup::Backlog => "backlog",
            StateGroup::Unstarted => "unstarted",
            StateGroup::Started => "started",
            StateGroup::Completed => "completed",
            StateGroup::Cancelled => "cancelled",
        }
    }
}

/// Plane's priorities.
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
    fn as_str(self) -> &'static str {
        match self {
            Priority::Urgent => "urgent",
            Priority::High => "high",
            Priority::Medium => "medium",
            Priority::Low => "low",
            Priority::None => "none",
        }
    }
}

/// Whose work items a column shows.
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

/// One column: a filter over the workspace's work items, applied locally.
/// Empty lists match everything; without `state_group` or `state`, only open
/// groups match.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaneColumn {
    #[serde(skip)]
    pub name: String,
    #[serde(default)]
    pub assignee: Assignee,
    #[serde(default)]
    pub state_group: Vec<StateGroup>,
    /// State names, e.g. `In Review` (case-insensitive, accents included).
    #[serde(default)]
    pub state: Vec<String>,
    /// Project identifiers, e.g. `API`.
    #[serde(default)]
    pub project: Vec<String>,
    #[serde(default)]
    pub priority: Vec<Priority>,
}

impl PlaneColumn {
    fn matches(&self, facts: &Facts, me: &str) -> bool {
        let groups_ok = if self.state_group.is_empty() {
            // Naming states picks them in any group ("Done" is completed).
            !self.state.is_empty() || OPEN_GROUPS.iter().any(|g| g.as_str() == facts.group)
        } else {
            self.state_group.iter().any(|g| g.as_str() == facts.group)
        };
        groups_ok
            && self.matches_ignoring_state(facts, me)
            && (self.state.is_empty() || self.state.iter().any(|s| same_name(s, &facts.state_name)))
    }

    /// Whether the item could show here if its state were known.
    fn matches_ignoring_state(&self, facts: &Facts, me: &str) -> bool {
        let mine = facts.assignees.iter().any(|a| a == me);
        let assignee_ok = match self.assignee {
            Assignee::Me => mine,
            Assignee::None => facts.assignees.is_empty(),
            Assignee::Others => !facts.assignees.is_empty() && !mine,
            Assignee::Any => true,
        };
        assignee_ok
            && (self.project.is_empty()
                || self.project.iter().any(|p| same_name(p, &facts.project)))
            && (self.priority.is_empty()
                || self.priority.iter().any(|p| p.as_str() == facts.priority))
    }
}

/// Case-insensitive comparison that also folds accented letters' case.
fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Columns used when the configuration declares none: your open items by
/// state group.
pub fn default_columns() -> Vec<PlaneColumn> {
    [
        ("In progress", StateGroup::Started),
        ("To do", StateGroup::Unstarted),
        ("Backlog", StateGroup::Backlog),
    ]
    .into_iter()
    .map(|(name, group)| PlaneColumn {
        name: name.to_owned(),
        state_group: vec![group],
        ..PlaneColumn::default()
    })
    .collect()
}

/// What a column filter looks at.
struct Facts {
    group: String,
    state_name: String,
    assignees: Vec<String>,
    priority: String,
    project: String,
}

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
    /// Reads `PLANE_BASE_URL`, `PLANE_WORKSPACE_SLUG` (or `PLANE_WORKSPACE`),
    /// `PLANE_API_KEY` (or `PLANE_TOKEN`) and the
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
        // PLANE_WORKSPACE / PLANE_TOKEN are the names other Plane tools use.
        let workspace_slug = read("PLANE_WORKSPACE_SLUG").or_else(|| read("PLANE_WORKSPACE"));
        let api_key = read("PLANE_API_KEY").or_else(|| read("PLANE_TOKEN"));

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
    columns: Vec<PlaneColumn>,
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
            columns: default_columns(),
        }
    }

    /// Replaces the default columns; an empty list keeps the defaults.
    pub fn with_columns(mut self, columns: Vec<PlaneColumn>) -> Self {
        if !columns.is_empty() {
            self.columns = columns;
        }
        self
    }

    pub fn with_project_budget(mut self, budget: Duration) -> Self {
        self.project_budget = budget;
        self
    }
}

impl PendingSource for PlaneSource {
    fn columns(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.name.clone()).collect()
    }

    fn refresh(&self) -> BoxFuture<'_, Result<SourceBatch, SourceError>> {
        Box::pin(async move {
            match &self.settings {
                Ok(settings) => {
                    let api = Api {
                        client: self.client.clone(),
                        settings: settings.clone(),
                    };
                    api.refresh(self.project_budget, &self.columns)
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
    async fn refresh(
        self,
        project_budget: Duration,
        columns: &[PlaneColumn],
    ) -> Result<SourceBatch, String> {
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
                        match self.describe_issue(issue, project, &found.states, today) {
                            Described::Card(facts, card) => {
                                for column in columns.iter().filter(|c| c.matches(&facts, &me)) {
                                    batch.items.push(SourceItem {
                                        column: column.name.clone(),
                                        card: (*card).clone(),
                                    });
                                }
                            }
                            // Only items some column could show are worth a warning.
                            Described::UnknownState(facts)
                                if columns
                                    .iter()
                                    .any(|c| c.matches_ignoring_state(&facts, &me)) =>
                            {
                                unknown_state += 1
                            }
                            Described::UnknownState(_) => {}
                            Described::Malformed => {}
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

    /// The card for a work item plus the facts column filters look at.
    fn describe_issue(
        &self,
        issue: &Value,
        project: &Project,
        states: &States,
        today: NaiveDate,
    ) -> Described {
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
        let (Some(id), Some(sequence)) = (
            issue.get("id").and_then(Value::as_str),
            issue.get("sequence_id").and_then(Value::as_u64),
        ) else {
            return Described::Malformed;
        };

        let assignees: Vec<&Value> = issue
            .get("assignees")
            .and_then(Value::as_array)
            .map(|list| list.iter().collect())
            .unwrap_or_default();
        let assignee_ids: Vec<String> = assignees
            .iter()
            .filter_map(|a| a.as_str().or_else(|| a.get("id").and_then(Value::as_str)))
            .map(str::to_owned)
            .collect();
        let assignee_names: Vec<&str> = assignees
            .iter()
            .filter_map(|a| a.get("display_name").and_then(Value::as_str))
            .collect();

        let reference = format!("{}-{sequence}", project.identifier);
        let due = issue
            .get("target_date")
            .and_then(Value::as_str)
            .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok());
        let overdue = due.is_some_and(|due| due < today);
        let priority = issue
            .get("priority")
            .and_then(Value::as_str)
            .unwrap_or("none");

        let Some(group) = group else {
            return Described::UnknownState(Facts {
                group: String::new(),
                state_name: String::new(),
                assignees: assignee_ids,
                priority: priority.to_owned(),
                project: project.identifier.clone(),
            });
        };

        let severity = match priority {
            _ if overdue => CardSeverity::Critical,
            "urgent" => CardSeverity::Critical,
            "high" => CardSeverity::Warning,
            _ => CardSeverity::Info,
        };

        // The reference is the card title; the issue name leads the details.
        let mut body = format!("{} · {}", str_field(issue, "name"), project.name);
        if let Some(state_name) = state_name.filter(|name| !name.is_empty()) {
            body.push_str(&format!(" · {state_name}"));
        }
        if !assignee_names.is_empty() {
            let names: Vec<String> = assignee_names.iter().map(|n| format!("@{n}")).collect();
            body.push_str(&format!(" · {}", names.join(" ")));
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
        Described::Card(
            Facts {
                group: group.to_owned(),
                state_name: state_name.unwrap_or_default().to_owned(),
                assignees: assignee_ids,
                priority: priority.to_owned(),
                project: project.identifier.clone(),
            },
            Box::new(PendingCard {
                id: format!("plane:{reference}"),
                title: reference.clone(),
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
            }),
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

enum Described {
    Card(Facts, Box<PendingCard>),
    /// Its state group could not be determined; facts without the state.
    UnknownState(Facts),
    /// Missing id or sequence number.
    Malformed,
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
