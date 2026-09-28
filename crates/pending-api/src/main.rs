use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::Context;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use pending_core::DashboardSnapshot;
use pending_runtime::Aggregator;
use pending_runtime::cache::Cache;
use pending_runtime::config::{Origin, cache_path, load_plan};
use serde::Serialize;
use tokio::time::Instant;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

#[derive(Debug, Parser)]
#[command(name = "pending-api", about = "TasksPending HTTP API")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: SocketAddr,
    /// Config file. Defaults to $TASKS_PENDING_CONFIG, then
    /// $XDG_CONFIG_HOME/tasks-pending/config.toml, then
    /// ~/.config/tasks-pending/config.toml.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Built frontend to serve at `/`. Defaults to `../frontend` next to the
    /// binary when it exists (release bundle layout).
    #[arg(long)]
    static_dir: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing();

    let (plan, origin) = load_plan(cli.config, &|key| std::env::var_os(key))?;
    match &origin {
        Origin::File(path) if plan.specs.is_empty() => warn!(
            config = %path.display(),
            "config has no enabled sources; the dashboard will be empty"
        ),
        Origin::File(path) => {
            info!(config = %path.display(), sources = plan.specs.len(), "config loaded")
        }
        Origin::SampleDefault { searched } => warn!(
            searched = ?searched,
            "no config file found; serving the built-in sample source"
        ),
    }
    let cache = cache_path(&|key| std::env::var_os(key)).map(Cache::new);
    match &cache {
        Some(cache) => info!(cache = ?cache, "source cache enabled"),
        None => warn!("no XDG_STATE_HOME or HOME; the source cache is disabled"),
    }
    let aggregator = Aggregator::start_with_cache(plan.specs, plan.timeout, cache);

    let static_dir = resolve_static_dir(cli.static_dir);
    match &static_dir {
        Some(dir) if !dir.join("index.html").is_file() => {
            anyhow::bail!(
                "{} has no index.html; build the frontend first",
                dir.display()
            )
        }
        Some(dir) => info!(dir = %dir.display(), "serving frontend"),
        None => info!("no frontend directory; serving the API only"),
    }

    let listener = tokio::net::TcpListener::bind(cli.listen)
        .await
        .with_context(|| format!("binding {}", cli.listen))?;
    info!(addr = %cli.listen, "pending-api listening");

    axum::serve(listener, app(aggregator, static_dir.as_deref()))
        .await
        .context("serving API")?;
    Ok(())
}

/// Minimum wait between refreshes requested over HTTP; each one queries every
/// source, and provider APIs rate-limit.
const REFRESH_COOLDOWN: Duration = Duration::from_secs(10);

/// Header the dashboard sends with state-changing requests. Another site open
/// in the browser cannot add it without a CORS preflight, which is refused.
const DASHBOARD_HEADER: (&str, &str) = ("x-requested-with", "tasks-pending");

#[derive(Clone)]
struct AppState {
    aggregator: Aggregator,
    last_refresh: Arc<Mutex<Option<Instant>>>,
}

/// API routes, plus the built frontend from `static_dir` for any other path.
fn app(aggregator: Aggregator, static_dir: Option<&Path>) -> Router {
    let state = AppState {
        aggregator,
        last_refresh: Arc::new(Mutex::new(None)),
    };
    let router = Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/snapshot", get(snapshot))
        .route("/api/v1/refresh", post(refresh));
    let router = match static_dir {
        Some(dir) => router.fallback_service(ServeDir::new(dir)),
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

/// `--static-dir`, else `frontend/` next to the binary's `bin/` directory (the
/// release bundle layout), if it has an `index.html`.
fn resolve_static_dir(cli: Option<PathBuf>) -> Option<PathBuf> {
    cli.or_else(|| bundled_frontend(&std::env::current_exe().ok()?))
}

/// `<bundle>/frontend` for a binary at `<bundle>/bin/<name>`, when it holds an
/// `index.html`.
fn bundled_frontend(exe: &Path) -> Option<PathBuf> {
    let frontend = exe.parent()?.parent()?.join("frontend");
    frontend.join("index.html").is_file().then_some(frontend)
}

async fn healthz() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn snapshot(State(state): State<AppState>) -> Json<DashboardSnapshot> {
    Json(state.aggregator.snapshot())
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
    state.aggregator.refresh_now();
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

    /// A API serve o estado mantido pelo agregador: depois do primeiro
    /// refresh, o snapshot traz os cards e a saúde da fonte configurada.
    #[tokio::test(start_paused = true)]
    async fn snapshot_endpoint_serves_the_aggregated_state() {
        let aggregator = Aggregator::start(
            vec![SourceSpec {
                name: "sample".to_owned(),
                source: Arc::new(SampleSource),
                lane: "Inbox".to_owned(),
                interval: Duration::from_secs(300),
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
        assert_eq!(snapshot.lanes[0].name, "Inbox");
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

    /// Com o frontend compilado disponível, a API serve a página, os assets e
    /// continua respondendo as rotas da API.
    #[tokio::test]
    async fn serves_the_built_frontend_next_to_the_api() {
        let dir = std::env::temp_dir().join(format!("tasks-pending-static-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("index.html"), "<main id=app></main>").unwrap();
        std::fs::write(dir.join("assets/app.js"), "console.log(1)").unwrap();
        let app = app(idle_aggregator(), Some(&dir));

        assert_eq!(
            get(app.clone(), "/").await,
            (StatusCode::OK, "<main id=app></main>".to_owned())
        );
        assert_eq!(get(app.clone(), "/assets/app.js").await.0, StatusCode::OK);
        assert_eq!(get(app.clone(), "/healthz").await.0, StatusCode::OK);
        assert_eq!(get(app, "/api/v1/snapshot").await.0, StatusCode::OK);
    }

    /// Sem frontend configurado, só as rotas da API existem.
    #[tokio::test]
    async fn without_a_frontend_only_api_routes_exist() {
        let app = app(idle_aggregator(), None);

        assert_eq!(get(app.clone(), "/").await.0, StatusCode::NOT_FOUND);
        assert_eq!(get(app, "/healthz").await.0, StatusCode::OK);
    }

    struct Counting(Arc<std::sync::atomic::AtomicUsize>);

    impl pending_core::PendingSource for Counting {
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

    /// O botão de atualizar da web pede um refresh imediato de todas as
    /// fontes; um segundo pedido logo em seguida é recusado com o tempo de
    /// espera, para não estourar limites de taxa das APIs.
    #[tokio::test(start_paused = true)]
    async fn refresh_endpoint_wakes_sources_with_a_cooldown() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let aggregator = Aggregator::start(
            vec![SourceSpec {
                name: "counting".to_owned(),
                source: Arc::new(Counting(calls.clone())),
                lane: "Inbox".to_owned(),
                interval: Duration::from_secs(300),
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

    /// Outro site aberto no navegador não consegue disparar o refresh: sem o
    /// cabeçalho que só o dashboard envia, o pedido é recusado.
    #[tokio::test]
    async fn refresh_requires_the_dashboard_header() {
        let (status, _) = post(app(idle_aggregator(), None), "/api/v1/refresh", false).await;

        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    /// No bundle de release (`bin/` ao lado de `frontend/`), a API acha o
    /// frontend sozinha; sem `index.html`, não serve nada; a flag vence.
    #[test]
    fn static_dir_comes_from_flag_or_release_bundle() {
        let bundle =
            std::env::temp_dir().join(format!("tasks-pending-bundle-{}", std::process::id()));
        std::fs::create_dir_all(bundle.join("bin")).unwrap();
        std::fs::create_dir_all(bundle.join("frontend")).unwrap();
        let exe = bundle.join("bin/pending-api");

        assert_eq!(bundled_frontend(&exe), None, "no index.html yet");
        std::fs::write(bundle.join("frontend/index.html"), "<main></main>").unwrap();
        assert_eq!(bundled_frontend(&exe), Some(bundle.join("frontend")));

        let flag = PathBuf::from("/explicit/dist");
        assert_eq!(resolve_static_dir(Some(flag.clone())), Some(flag));
        let _ = std::fs::remove_dir_all(&bundle);
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

    /// Só nomes locais chegam à API: um site que aponta o próprio domínio para
    /// 127.0.0.1 (DNS rebinding) não lê o snapshot nem dispara refresh.
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
