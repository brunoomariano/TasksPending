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
use pending_core::DashboardSnapshot;
use pending_runtime::Dashboard;
use pending_runtime::config::{Origin, cache_path};
use pending_runtime::live::{Live, process_env};
use serde::Serialize;
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

/// API routes, plus the configured frontend for any other path.
pub fn app(dashboard: impl Dashboard + 'static, frontend: Option<Frontend>) -> Router {
    let state = AppState {
        dashboard: Arc::new(dashboard),
        last_refresh: Arc::new(Mutex::new(None)),
    };
    let router = Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/snapshot", get(snapshot))
        .route("/api/v1/refresh", post(refresh));
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
