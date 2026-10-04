use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::Context;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header::CONTENT_TYPE};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use pending_core::{DashboardSnapshot, sandbox_snapshot};
use pending_runtime::Dashboard;
use pending_runtime::config::{Origin, cache_path};
use pending_runtime::live::{Live, process_env};
use pending_runtime::looks::Looks;
use pending_runtime::marks::{MarkError, Marks};
use pending_runtime::snoozes::{SnoozeError, Snoozes};
use serde::{Deserialize, Serialize};
use tokio::time::Instant;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

#[derive(Debug)]
pub struct ServeOptions {
    pub listen: SocketAddr,
    /// Config file. Defaults to $TASKS_PENDING_CONFIG, then
    /// $XDG_CONFIG_HOME/tasks-pending/config.toml, then
    /// ~/.config/tasks-pending/config.toml.
    pub config: Option<PathBuf>,
    pub static_dir: Option<PathBuf>,
}

#[derive(Debug)]
pub struct SandboxOptions {
    pub listen: SocketAddr,
    pub static_dir: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug)]
pub struct EmbeddedFile {
    pub path: &'static str,
    pub bytes: &'static [u8],
}

#[derive(Clone, Debug)]
pub enum Frontend {
    Directory(PathBuf),
    Embedded(&'static [EmbeddedFile]),
}

impl Frontend {
    pub fn directory(path: impl Into<PathBuf>) -> Self {
        Self::Directory(path.into())
    }

    pub const fn embedded(files: &'static [EmbeddedFile]) -> Self {
        Self::Embedded(files)
    }
}

pub mod weather;

mod embedded_frontend {
    use super::EmbeddedFile;

    include!(concat!(env!("OUT_DIR"), "/embedded_frontend.rs"));
}

#[derive(Debug, Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
}

pub async fn serve(options: ServeOptions) -> anyhow::Result<()> {
    validate_listen_addr(options.listen)?;
    init_tracing();

    let env = process_env();
    let cache = cache_path(&*env);
    match &cache {
        Some(path) => info!(cache = %path.display(), "source cache enabled"),
        None => warn!("no XDG_STATE_HOME or HOME; the source cache is disabled"),
    }
    let (live, origin) = Live::start(options.config, env, cache)?;
    let sources = live.snapshot().sources.len();
    match &origin {
        Origin::File(path) if sources == 0 => warn!(
            config = %path.display(),
            "config has no enabled sources; the dashboard will be empty"
        ),
        Origin::File(path) => info!(config = %path.display(), sources, "config loaded"),
        Origin::SampleDefault { searched } => warn!(
            searched = ?searched,
            "no config file found; serving the built-in sample source"
        ),
    }
    // Saving the config file is enough; no restart needed.
    let _watch = live.watch(CONFIG_WATCH_INTERVAL);

    let frontend = resolve_frontend(options.static_dir)?;
    match &frontend {
        Some(Frontend::Directory(dir)) => info!(dir = %dir.display(), "serving frontend directory"),
        Some(Frontend::Embedded(files)) => info!(files = files.len(), "serving embedded frontend"),
        None => info!("no embedded frontend; serving the API only"),
    }

    let listener = tokio::net::TcpListener::bind(options.listen)
        .await
        .with_context(|| format!("binding {}", options.listen))?;
    info!(addr = %options.listen, "tasks-pending listening");

    axum::serve(listener, app(live, frontend))
        .await
        .context("serving API")?;
    Ok(())
}

/// Serves deterministic simulated cards without loading user configuration,
/// credentials, cache, or provider integrations.
pub async fn serve_sandbox(options: SandboxOptions) -> anyhow::Result<()> {
    validate_listen_addr(options.listen)?;
    init_tracing();

    let frontend = resolve_frontend(options.static_dir)?;
    let listener = tokio::net::TcpListener::bind(options.listen)
        .await
        .with_context(|| format!("binding {}", options.listen))?;
    info!(addr = %options.listen, "TasksPending sandbox listening");

    let weather = weather::WeatherService::sample();
    let app = app_with_weather(SandboxDashboard::default(), frontend, weather);
    axum::serve(listener, app)
        .await
        .context("serving sandbox API")?;
    Ok(())
}

fn validate_listen_addr(listen: SocketAddr) -> anyhow::Result<()> {
    if listen.ip().is_loopback() {
        return Ok(());
    }
    anyhow::bail!(
        "refusing to listen on {listen}: TasksPending has no authentication; use a loopback address"
    )
}

/// How often the config file is checked for changes.
const CONFIG_WATCH_INTERVAL: Duration = Duration::from_secs(2);

/// Minimum wait between refreshes requested over HTTP; each one queries every
/// source, and provider APIs rate-limit.
const REFRESH_COOLDOWN: Duration = Duration::from_secs(10);

/// Header the dashboard sends with state-changing requests. Another site open
/// in the browser cannot add it without a CORS preflight, which is refused.
const DASHBOARD_HEADER: (&str, &str) = ("x-requested-with", "tasks-pending");

#[derive(Clone)]
struct AppState {
    dashboard: Arc<dyn Dashboard>,
    last_refresh: Arc<Mutex<Option<Instant>>>,
}

/// Simulated cards; marks are kept in memory while the sandbox runs.
#[derive(Clone)]
struct SandboxDashboard {
    marks: Arc<Marks>,
    snoozes: Arc<Snoozes>,
    looks: Arc<Looks>,
}

impl Default for SandboxDashboard {
    fn default() -> Self {
        // The demo's one look is an hour before its data was "fetched",
        // so the newest simulated cards show as changed.
        let looks = Looks::load(None);
        looks.look_at(sandbox_snapshot().generated_at - chrono::Duration::hours(1));
        Self {
            marks: Arc::new(Marks::load(None)),
            snoozes: Arc::new(Snoozes::load(None)),
            looks: Arc::new(looks),
        }
    }
}

impl Dashboard for SandboxDashboard {
    fn snapshot(&self) -> DashboardSnapshot {
        let mut snapshot = sandbox_snapshot();
        self.marks.apply(&mut snapshot);
        self.snoozes.apply(&mut snapshot);
        self.looks.apply(&mut snapshot);
        snapshot
    }

    fn refresh_now(&self) {}

    fn set_mark(&self, id: &str, marked: bool) -> Result<(), MarkError> {
        // Like the live dashboard: a snoozed card is off the boards.
        self.marks.set(&self.snapshot(), id, marked)
    }

    fn snooze(
        &self,
        id: &str,
        until: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(), SnoozeError> {
        self.snoozes.snooze(&self.snapshot(), id, until)
    }

    fn wake(&self, id: &str) -> Result<(), SnoozeError> {
        self.snoozes.wake(id)
    }

    /// The simulated cards have fixed dates in the past: a real look would
    /// start a sitting today and no card would ever be newer than it. The
    /// demo keeps its one look, and with it the changed dots.
    fn look(&self) {}
}

/// API routes, plus the configured frontend for any other path.
pub fn app(dashboard: impl Dashboard + 'static, frontend: Option<Frontend>) -> Router {
    app_with_weather(dashboard, frontend, weather::WeatherService::omarchy())
}

/// [`app`] with the weather route answered by `weather`.
pub fn app_with_weather(
    dashboard: impl Dashboard + 'static,
    frontend: Option<Frontend>,
    weather: weather::WeatherService,
) -> Router {
    let state = AppState {
        dashboard: Arc::new(dashboard),
        last_refresh: Arc::new(Mutex::new(None)),
    };
    let router = Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/snapshot", get(snapshot))
        .route("/api/v1/refresh", post(refresh))
        .route("/api/v1/marks", post(set_mark))
        .route("/api/v1/snooze", post(set_snooze))
        .route("/api/v1/look", post(look))
        .merge(weather::routes(weather));
    let router = match frontend {
        Some(Frontend::Directory(dir)) => router.fallback_service(ServeDir::new(dir)),
        Some(Frontend::Embedded(files)) => {
            router.fallback(move |uri: Uri| async move { serve_embedded_frontend(uri, files) })
        }
        None => router,
    };
    router
        .layer(axum::middleware::from_fn(local_hosts_only))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Serves only requests addressed to a local name. The API has no
/// authentication; without this, a web page could point its own domain at
/// 127.0.0.1 (DNS rebinding) and read the dashboard as same-origin. Requests
/// without a Host header (non-browser clients) pass.
async fn local_hosts_only(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let host = request
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok());
    let local = host.is_none_or(|host| {
        let name = match host.strip_prefix('[') {
            Some(rest) => rest.split(']').next().unwrap_or_default(),
            None => host.split(':').next().unwrap_or_default(),
        };
        matches!(name, "localhost" | "127.0.0.1" | "::1")
    });
    if local {
        next.run(request).await
    } else {
        (StatusCode::MISDIRECTED_REQUEST, "unknown host").into_response()
    }
}

fn resolve_frontend(static_dir: Option<PathBuf>) -> anyhow::Result<Option<Frontend>> {
    if let Some(dir) = static_dir {
        if !dir.join("index.html").is_file() {
            anyhow::bail!(
                "{} has no index.html; build the frontend first",
                dir.display()
            );
        }
        return Ok(Some(Frontend::directory(dir)));
    }

    Ok(embedded_frontend::FILES
        .iter()
        .any(|file| file.path == "index.html")
        .then(|| Frontend::embedded(embedded_frontend::FILES)))
}

fn serve_embedded_frontend(uri: Uri, files: &'static [EmbeddedFile]) -> axum::response::Response {
    let requested = match uri.path().trim_start_matches('/') {
        "" => "index.html",
        path => path,
    };
    let Some(file) = files.iter().find(|file| file.path == requested) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let mut response = (StatusCode::OK, axum::body::Body::from(file.bytes)).into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static(content_type(file.path)),
    );
    response
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("css") => "text/css; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("ico") => "image/x-icon",
        Some("jpeg" | "jpg") => "image/jpeg",
        Some("js" | "mjs") => "application/javascript; charset=utf-8",
        Some("json" | "map") => "application/json; charset=utf-8",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

async fn healthz() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn snapshot(State(state): State<AppState>) -> Json<DashboardSnapshot> {
    Json(state.dashboard.snapshot())
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum RefreshReply {
    Accepted { accepted: bool },
    Wait { retry_after_secs: u64 },
    Forbidden { error: &'static str },
}

#[derive(Debug, Deserialize)]
struct MarkRequest {
    id: String,
    marked: bool,
}

/// Marks or unmarks a card as in progress.
async fn set_mark(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<MarkRequest>,
) -> StatusCode {
    let (name, value) = DASHBOARD_HEADER;
    if headers.get(name).and_then(|v| v.to_str().ok()) != Some(value) {
        return StatusCode::FORBIDDEN;
    }
    match state.dashboard.set_mark(&request.id, request.marked) {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(MarkError::UnknownCard) => StatusCode::NOT_FOUND,
        Err(MarkError::Unsupported) => StatusCode::NOT_IMPLEMENTED,
        Err(MarkError::NotSaved) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Records that the user is looking at the dashboard, so the next visit
/// can flag what changed since.
async fn look(State(state): State<AppState>, headers: HeaderMap) -> StatusCode {
    let (name, value) = DASHBOARD_HEADER;
    if headers.get(name).and_then(|v| v.to_str().ok()) != Some(value) {
        return StatusCode::FORBIDDEN;
    }
    state.dashboard.look();
    StatusCode::NO_CONTENT
}

#[derive(Debug, Deserialize)]
struct SnoozeRequest {
    id: String,
    /// `false` wakes the card.
    snoozed: bool,
    /// When the card comes back by itself; absent or `null` waits for the
    /// item to change.
    #[serde(default)]
    until: Option<chrono::DateTime<chrono::Utc>>,
}

/// Snoozes a card (hiding it until a time or until the item changes) or
/// wakes it.
async fn set_snooze(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SnoozeRequest>,
) -> StatusCode {
    let (name, value) = DASHBOARD_HEADER;
    if headers.get(name).and_then(|v| v.to_str().ok()) != Some(value) {
        return StatusCode::FORBIDDEN;
    }
    let result = match request.snoozed {
        true => state.dashboard.snooze(&request.id, request.until),
        false => state.dashboard.wake(&request.id),
    };
    match result {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(SnoozeError::UnknownCard) => StatusCode::NOT_FOUND,
        Err(SnoozeError::PastTime) => StatusCode::BAD_REQUEST,
        Err(SnoozeError::Unsupported) => StatusCode::NOT_IMPLEMENTED,
        Err(SnoozeError::NotSaved) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Refreshes every source now, at most once per [`REFRESH_COOLDOWN`].
async fn refresh(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let (name, value) = DASHBOARD_HEADER;
    if headers.get(name).and_then(|v| v.to_str().ok()) != Some(value) {
        return (
            StatusCode::FORBIDDEN,
            Json(RefreshReply::Forbidden {
                error: "missing dashboard header",
            }),
        );
    }

    let now = Instant::now();
    let mut last = state
        .last_refresh
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(elapsed) = last.map(|at| now.saturating_duration_since(at))
        && elapsed < REFRESH_COOLDOWN
    {
        // Rounded up, so waiting exactly that long is enough.
        let wait = (REFRESH_COOLDOWN - elapsed).as_secs_f64().ceil() as u64;
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(RefreshReply::Wait {
                retry_after_secs: wait,
            }),
        );
    }
    *last = Some(now);
    state.dashboard.refresh_now();
    (
        StatusCode::ACCEPTED,
        Json(RefreshReply::Accepted { accepted: true }),
    )
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "pending_api=info,pending_runtime=info,tower_http=info".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use pending_core::{SampleSource, SourceStatus};
    use pending_runtime::{Aggregator, SourceSpec};
    use tower::ServiceExt;

    use super::*;

    /// The API serves the aggregator's state: after the first refresh, the
    /// snapshot carries the cards and health of the configured source.
    #[tokio::test(start_paused = true)]
    async fn snapshot_endpoint_serves_the_aggregated_state() {
        let aggregator = Aggregator::start(
            vec![SourceSpec {
                name: "sample".to_owned(),
                source: Arc::new(SampleSource),
                board: "Inbox".to_owned(),
                interval: Duration::from_secs(300),
                timeout: None,
                icon: None,
                sorts: Default::default(),
                excludes: Default::default(),
            }],
            Duration::from_secs(30),
        );
        tokio::time::sleep(Duration::from_millis(10)).await;

        let response = app(aggregator, None)
            .oneshot(
                Request::get("/api/v1/snapshot")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let snapshot: DashboardSnapshot = serde_json::from_slice(&body).unwrap();
        assert_eq!(snapshot.sources[0].name, "sample");
        assert_eq!(snapshot.sources[0].status, SourceStatus::Ready);
        assert_eq!(snapshot.boards[0].name, "Inbox");
    }

    /// The sandbox uses the regular API route but never reads configuration or
    /// contacts a provider before returning its simulated cards.
    #[tokio::test]
    async fn sandbox_snapshot_endpoint_serves_simulated_cards() {
        let response = app(SandboxDashboard::default(), None)
            .oneshot(
                Request::get("/api/v1/snapshot")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let snapshot: DashboardSnapshot = serde_json::from_slice(&body).unwrap();
        assert!(
            snapshot
                .boards
                .iter()
                .flat_map(|board| &board.groups)
                .flat_map(|group| &group.columns)
                .flat_map(|column| &column.cards)
                .any(|card| card.title == "Review the release checklist")
        );
    }

    /// The sandbox shows some cards as changed since the last look, and
    /// keeps showing them however the page is used: activity reported by
    /// the page does not start a new sitting there.
    #[tokio::test]
    async fn sandbox_keeps_its_changed_cards() {
        let dashboard = SandboxDashboard::default();
        let changed = dashboard.snapshot().changed;
        assert!(
            changed.contains(&"github:review:214".to_owned()),
            "{changed:?}"
        );
        let cards = dashboard
            .snapshot()
            .boards
            .iter()
            .flat_map(|board| &board.groups)
            .flat_map(|group| &group.columns)
            .map(|column| column.cards.len())
            .sum::<usize>();
        assert!(changed.len() < cards, "only the newest cards are flagged");

        let app = app(dashboard, None);
        let status = post_json(app.clone(), "/api/v1/look", "", true).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = get(app, "/api/v1/snapshot").await;
        let snapshot: DashboardSnapshot = serde_json::from_str(&body).unwrap();
        assert_eq!(snapshot.changed, changed);
    }

    async fn get(app: Router, path: &str) -> (StatusCode, String) {
        let response = app
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    fn idle_aggregator() -> Aggregator {
        Aggregator::start(Vec::new(), Duration::from_secs(30))
    }

    #[test]
    fn server_rejects_non_loopback_addresses() {
        assert!(validate_listen_addr("127.0.0.1:8080".parse().unwrap()).is_ok());
        assert!(validate_listen_addr("[::1]:8080".parse().unwrap()).is_ok());

        let error = validate_listen_addr("0.0.0.0:8080".parse().unwrap()).unwrap_err();
        assert!(error.to_string().contains("has no authentication"));
    }

    /// With the built frontend available, the API serves the page and assets
    /// and still answers the API routes.
    #[tokio::test]
    async fn serves_the_built_frontend_next_to_the_api() {
        let dir = std::env::temp_dir().join(format!("tasks-pending-static-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("index.html"), "<main id=app></main>").unwrap();
        std::fs::write(dir.join("assets/app.js"), "console.log(1)").unwrap();
        let app = app(idle_aggregator(), Some(Frontend::directory(&dir)));

        assert_eq!(
            get(app.clone(), "/").await,
            (StatusCode::OK, "<main id=app></main>".to_owned())
        );
        assert_eq!(get(app.clone(), "/assets/app.js").await.0, StatusCode::OK);
        assert_eq!(get(app.clone(), "/healthz").await.0, StatusCode::OK);
        assert_eq!(get(app, "/api/v1/snapshot").await.0, StatusCode::OK);
    }

    /// Without a frontend configured, only the API routes exist.
    #[tokio::test]
    async fn without_a_frontend_only_api_routes_exist() {
        let app = app(idle_aggregator(), None);

        assert_eq!(get(app.clone(), "/").await.0, StatusCode::NOT_FOUND);
        assert_eq!(get(app, "/healthz").await.0, StatusCode::OK);
    }

    struct Counting(Arc<std::sync::atomic::AtomicUsize>);

    impl pending_core::PendingSource for Counting {
        fn columns(&self) -> Vec<String> {
            Vec::new()
        }

        fn refresh(
            &self,
        ) -> pending_core::BoxFuture<'_, Result<pending_core::SourceBatch, pending_core::SourceError>>
        {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok(pending_core::SourceBatch::default()) })
        }
    }

    async fn post(app: Router, path: &str, marked: bool) -> (StatusCode, String) {
        let mut request = Request::post(path);
        if marked {
            request = request.header("x-requested-with", "tasks-pending");
        }
        let response = app
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    /// The web refresh button asks for an immediate refresh of every source; a
    /// second request right after is refused with the wait time, so provider
    /// rate limits aren't exceeded.
    #[tokio::test(start_paused = true)]
    async fn refresh_endpoint_wakes_sources_with_a_cooldown() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let aggregator = Aggregator::start(
            vec![SourceSpec {
                name: "counting".to_owned(),
                source: Arc::new(Counting(calls.clone())),
                board: "Inbox".to_owned(),
                interval: Duration::from_secs(300),
                timeout: None,
                icon: None,
                sorts: Default::default(),
                excludes: Default::default(),
            }],
            Duration::from_secs(30),
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
        let app = app(aggregator, None);

        let (status, _) = post(app.clone(), "/api/v1/refresh", true).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);

        let (status, body) = post(app.clone(), "/api/v1/refresh", true).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert!(body.contains("retry_after_secs"), "{body}");

        tokio::time::sleep(Duration::from_secs(10)).await;
        let (status, _) = post(app, "/api/v1/refresh", true).await;
        assert_eq!(status, StatusCode::ACCEPTED);
    }

    /// Another site open in the browser can't trigger a refresh: without the
    /// header only the dashboard sends, the request is refused.
    #[tokio::test]
    async fn refresh_requires_the_dashboard_header() {
        let (status, _) = post(app(idle_aggregator(), None), "/api/v1/refresh", false).await;

        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    async fn post_json(app: Router, path: &str, body: &str, marked: bool) -> StatusCode {
        let mut request = Request::post(path).header("content-type", "application/json");
        if marked {
            request = request.header("x-requested-with", "tasks-pending");
        }
        app.oneshot(request.body(Body::from(body.to_owned())).unwrap())
            .await
            .unwrap()
            .status()
    }

    async fn marked_ids(app: Router) -> Vec<String> {
        let (_, body) = get(app, "/api/v1/snapshot").await;
        let snapshot: DashboardSnapshot = serde_json::from_str(&body).unwrap();
        snapshot.marked
    }

    /// The page marks a card as in progress and unmarks it again; the
    /// snapshot lists the marked cards. Unknown cards are refused.
    #[tokio::test]
    async fn marks_endpoint_marks_and_unmarks_cards() {
        let app = app(SandboxDashboard::default(), None);
        let id = sandbox_snapshot().boards[0].groups[0].columns[0].cards[0]
            .id
            .clone();
        let mark = |marked: bool| format!(r#"{{"id":{id:?},"marked":{marked}}}"#);

        let status = post_json(app.clone(), "/api/v1/marks", &mark(true), true).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(marked_ids(app.clone()).await, std::slice::from_ref(&id));

        let status = post_json(app.clone(), "/api/v1/marks", &mark(false), true).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(marked_ids(app.clone()).await.is_empty());

        let ghost = r#"{"id":"ghost","marked":true}"#;
        let status = post_json(app.clone(), "/api/v1/marks", ghost, true).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// The page reports activity so the next visit can flag what changed;
    /// only the dashboard itself may do so.
    #[tokio::test]
    async fn look_endpoint_requires_the_dashboard_header() {
        let app = app(SandboxDashboard::default(), None);

        let (status, _) = post(app.clone(), "/api/v1/look", false).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = post(app, "/api/v1/look", true).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    /// Whether the card is on the boards, and how many cards are snoozed.
    async fn board_state(app: Router, id: &str) -> (bool, usize) {
        let (_, body) = get(app, "/api/v1/snapshot").await;
        let snapshot: DashboardSnapshot = serde_json::from_str(&body).unwrap();
        let on_board = snapshot
            .boards
            .iter()
            .flat_map(|board| &board.groups)
            .flat_map(|group| &group.columns)
            .flat_map(|column| &column.cards)
            .any(|card| card.id == id);
        (on_board, snapshot.snoozed.len())
    }

    /// The page snoozes a card until a time or until it changes, and wakes
    /// it again; the snapshot leaves snoozed cards out of the boards and
    /// lists them. Unknown cards and past times are refused, and so is a
    /// request without the dashboard header.
    #[tokio::test]
    async fn snooze_endpoint_hides_and_wakes_cards() {
        let app = app(SandboxDashboard::default(), None);
        let id = sandbox_snapshot().boards[0].groups[0].columns[0].cards[0]
            .id
            .clone();
        let until = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        let snooze = format!(r#"{{"id":{id:?},"snoozed":true,"until":{until:?}}}"#);

        let status = post_json(app.clone(), "/api/v1/snooze", &snooze, false).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(board_state(app.clone(), &id).await, (true, 0));

        let status = post_json(app.clone(), "/api/v1/snooze", &snooze, true).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(board_state(app.clone(), &id).await, (false, 1));

        // A page that still shows the card (or wants another time) may
        // snooze it again: the time is replaced, not refused.
        let status = post_json(app.clone(), "/api/v1/snooze", &snooze, true).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let wake = format!(r#"{{"id":{id:?},"snoozed":false}}"#);
        let status = post_json(app.clone(), "/api/v1/snooze", &wake, true).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(board_state(app.clone(), &id).await, (true, 0));

        let ghost = r#"{"id":"ghost","snoozed":true}"#;
        let status = post_json(app.clone(), "/api/v1/snooze", ghost, true).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let past = format!(r#"{{"id":{id:?},"snoozed":true,"until":"2020-01-01T00:00:00Z"}}"#);
        let status = post_json(app, "/api/v1/snooze", &past, true).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// Another site open in the browser can't mark cards: without the header
    /// only the dashboard sends, the request is refused and nothing changes.
    #[tokio::test]
    async fn marks_require_the_dashboard_header() {
        let app = app(SandboxDashboard::default(), None);
        let id = sandbox_snapshot().boards[0].groups[0].columns[0].cards[0]
            .id
            .clone();
        let body = format!(r#"{{"id":{id:?},"marked":true}}"#);

        let status = post_json(app.clone(), "/api/v1/marks", &body, false).await;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(marked_ids(app).await.is_empty());
    }

    /// An explicit directory remains a development override, but it must have
    /// the page entry point rather than silently replacing the embedded page.
    #[test]
    fn static_dir_requires_an_index_page() {
        let directory = std::env::temp_dir().join(format!(
            "tasks-pending-static-missing-index-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();

        let error = resolve_frontend(Some(directory.clone())).unwrap_err();
        assert!(error.to_string().contains("has no index.html"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    async fn get_with_host(app: Router, path: &str, host: &str) -> StatusCode {
        app.oneshot(
            Request::get(path)
                .header("host", host)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
    }

    /// Only local host names reach the API: a site pointing its own domain at
    /// 127.0.0.1 (DNS rebinding) can't read the snapshot or trigger a refresh.
    #[tokio::test]
    async fn only_local_host_names_are_served() {
        let app = app(idle_aggregator(), None);

        for host in [
            "127.0.0.1:8080",
            "localhost:8080",
            "[::1]:8080",
            "localhost",
        ] {
            assert_eq!(
                get_with_host(app.clone(), "/api/v1/snapshot", host).await,
                StatusCode::OK,
                "{host}"
            );
        }
        assert_eq!(
            get_with_host(app, "/api/v1/snapshot", "evil.example:8080").await,
            StatusCode::MISDIRECTED_REQUEST
        );
    }
}
