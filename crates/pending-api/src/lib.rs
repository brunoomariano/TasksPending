use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::Context;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use pending_core::api::{DASHBOARD_HEADER, Health, REFRESH_COOLDOWN};
use pending_core::{DashboardSnapshot, sandbox_snapshot};
use pending_runtime::Dashboard;
use pending_runtime::config::{Origin, cache_path};
use pending_runtime::live::{Env, Live, Watch, process_env};
use pending_runtime::looks::Looks;
use pending_runtime::marks::{MarkError, Marks};
use pending_runtime::snoozes::{SnoozeError, Snoozes};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::time::Instant;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

pub mod update;
pub mod weather;

/// The router type, for callers that serve the API themselves.
pub use axum::Router;

#[derive(Debug)]
pub struct ServeOptions {
    pub listen: SocketAddr,
    /// Config file. Defaults to $TASKS_PENDING_CONFIG, then
    /// $XDG_CONFIG_HOME/tasks-pending/config.toml, then
    /// ~/.config/tasks-pending/config.toml.
    pub config: Option<PathBuf>,
}

/// The live dashboard behind the API: the routes, where the config came
/// from, and the watch that reloads the config file while it runs.
pub struct LiveServer {
    pub router: Router,
    pub origin: Origin,
    /// Keep it while the router serves: dropping it stops the config
    /// reloads.
    pub watch: Watch,
}

/// Starts the live dashboard (the sources, their cache and the config
/// watch) and returns its API routes, to serve on a listener of the
/// caller's choosing: the daemon's port, or one the app picks for itself
/// when no daemon is running. `env` is what the sources and the cache read
/// (XDG directories, tokens): the process environment, or a fake one in
/// tests. Needs a Tokio runtime: the sources, the config watch and the
/// release check are spawned on it.
pub fn start_live(config: Option<PathBuf>, env: Env) -> anyhow::Result<LiveServer> {
    let cache = cache_path(&*env);
    match &cache {
        Some(path) => info!(cache = %path.display(), "source cache enabled"),
        None => warn!("no XDG_STATE_HOME or HOME; the source cache is disabled"),
    }
    let (live, origin) = Live::start(config, env, cache)?;
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
    let watch = live.watch(CONFIG_WATCH_INTERVAL);
    Ok(LiveServer {
        router: app(live),
        origin,
        watch,
    })
}

/// The API over deterministic simulated cards, without loading user
/// configuration, credentials, cache, or provider integrations; the weather
/// is a fixed sample.
pub fn sandbox_app() -> Router {
    app_with(
        SandboxDashboard::default(),
        weather::WeatherService::sample(),
        update::UpdateCheck::off(),
    )
}

/// Runs the daemon: the live dashboard served on `options.listen` until the
/// process ends.
pub async fn serve(options: ServeOptions) -> anyhow::Result<()> {
    validate_listen_addr(options.listen)?;
    init_tracing();

    let server = start_live(options.config, process_env())?;
    let listener = TcpListener::bind(options.listen)
        .await
        .with_context(|| format!("binding {}", options.listen))?;
    info!(addr = %options.listen, "tasks-pending listening");
    serve_router(listener, server.router).await
}

/// Serves `router` on `listener` until the process ends. The listener
/// must be bound to a loopback address, like the daemon's own.
pub async fn serve_router(listener: TcpListener, router: Router) -> anyhow::Result<()> {
    validate_listen_addr(
        listener
            .local_addr()
            .context("reading the listener's address")?,
    )?;
    axum::serve(listener, router).await.context("serving API")?;
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

#[derive(Clone)]
struct AppState {
    dashboard: Arc<dyn Dashboard>,
    last_refresh: Arc<Mutex<Option<Instant>>>,
}

/// Simulated cards; marks, snoozes and looks are kept in memory while the
/// sandbox runs.
#[derive(Clone)]
pub(crate) struct SandboxDashboard {
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

/// The API routes over `dashboard`, with the weather Omarchy reports and
/// the daily look at the latest release.
pub fn app(dashboard: impl Dashboard + 'static) -> Router {
    app_with(
        dashboard,
        weather::WeatherService::omarchy(),
        update::UpdateCheck::start(),
    )
}

/// [`app`] with the weather route answered by `weather`.
pub fn app_with_weather(
    dashboard: impl Dashboard + 'static,
    weather: weather::WeatherService,
) -> Router {
    app_with(dashboard, weather, update::UpdateCheck::off())
}

/// The API routes over `dashboard`, with the given weather and update
/// services.
pub fn app_with(
    dashboard: impl Dashboard + 'static,
    weather: weather::WeatherService,
    update: update::UpdateCheck,
) -> Router {
    let state = AppState {
        dashboard: Arc::new(dashboard),
        last_refresh: Arc::new(Mutex::new(None)),
    };
    Router::new()
        .route("/", get(root))
        .route("/healthz", get(healthz))
        .route("/api/v1/snapshot", get(snapshot))
        .route("/api/v1/refresh", post(refresh))
        .route("/api/v1/marks", post(set_mark))
        .route("/api/v1/snooze", post(set_snooze))
        .route("/api/v1/look", post(look))
        .merge(weather::routes(weather))
        .merge(update::routes(update))
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

/// A line for whoever opens the daemon's address in a browser: the page
/// moved into the app.
async fn root() -> String {
    format!(
        "TasksPending {} daemon. Open the dashboard with `tasks-pending` (the desktop app).\n",
        env!("CARGO_PKG_VERSION")
    )
}

async fn healthz() -> Json<Health> {
    Json(Health {
        status: "ok".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
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

fn has_dashboard_header(headers: &HeaderMap) -> bool {
    let (name, value) = DASHBOARD_HEADER;
    headers.get(name).and_then(|v| v.to_str().ok()) == Some(value)
}

/// Marks or unmarks a card as in progress.
async fn set_mark(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<MarkRequest>,
) -> StatusCode {
    if !has_dashboard_header(&headers) {
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
    if !has_dashboard_header(&headers) {
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
    if !has_dashboard_header(&headers) {
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
    if !has_dashboard_header(&headers) {
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

/// Logs to stderr at the level `RUST_LOG` asks for, info by default.
pub fn init_tracing() {
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

    /// [`app`] without the release check, which would reach GitHub from a
    /// test that runs long enough.
    fn test_app(dashboard: impl Dashboard + 'static) -> Router {
        app_with(
            dashboard,
            weather::WeatherService::omarchy(),
            update::UpdateCheck::off(),
        )
    }

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

        let response = test_app(aggregator)
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
        let response = test_app(SandboxDashboard::default())
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

        let app = test_app(dashboard);
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

    /// The daemon's address in a browser explains where the page went.
    #[tokio::test]
    async fn the_root_names_the_daemon_and_the_app() {
        let (status, body) = get(test_app(idle_aggregator()), "/").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("TasksPending"), "{body}");
        assert!(body.contains("tasks-pending"), "{body}");
    }

    /// The version route names the running version; with the check off
    /// nothing is available.
    #[tokio::test]
    async fn the_version_route_names_the_daemon_version() {
        let (status, body) = get(
            app_with_weather(idle_aggregator(), weather::WeatherService::sample()),
            "/api/v1/version",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let report: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(report["current"], env!("CARGO_PKG_VERSION"));
        assert!(report["available"].is_null());
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

    /// Whoever serves a router (the daemon, the window's built-in API)
    /// cannot do so on a listener open to the network.
    #[tokio::test]
    async fn serve_router_refuses_a_non_loopback_listener() {
        let listener = TcpListener::bind("0.0.0.0:0").await.unwrap();
        let error = serve_router(listener, Router::new()).await.unwrap_err();
        assert!(
            error.to_string().contains("has no authentication"),
            "{error}"
        );
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
        let app = test_app(aggregator);

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
        let (status, _) = post(test_app(idle_aggregator()), "/api/v1/refresh", false).await;

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
        let app = test_app(SandboxDashboard::default());
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
        let app = test_app(SandboxDashboard::default());

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
        let app = test_app(SandboxDashboard::default());
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
        let app = test_app(SandboxDashboard::default());
        let id = sandbox_snapshot().boards[0].groups[0].columns[0].cards[0]
            .id
            .clone();
        let body = format!(r#"{{"id":{id:?},"marked":true}}"#);

        let status = post_json(app.clone(), "/api/v1/marks", &body, false).await;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(marked_ids(app).await.is_empty());
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
        let app = test_app(idle_aggregator());

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
