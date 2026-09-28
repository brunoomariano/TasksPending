use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use axum::extract::State;
use axum::{Json, Router, routing::get};
use clap::Parser;
use pending_core::DashboardSnapshot;
use pending_runtime::Aggregator;
use pending_runtime::config::{Origin, load_plan};
use serde::Serialize;
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
    let aggregator = Aggregator::start(plan.specs, plan.timeout);

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

/// API routes, plus the built frontend from `static_dir` for any other path.
fn app(aggregator: Aggregator, static_dir: Option<&Path>) -> Router {
    let router = Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/snapshot", get(snapshot));
    let router = match static_dir {
        Some(dir) => router.fallback_service(ServeDir::new(dir)),
        None => router,
    };
    router
        .layer(TraceLayer::new_for_http())
        .with_state(aggregator)
}

/// `--static-dir`, else `frontend/` next to the binary's `bin/` directory (the
/// release bundle layout), if it has an `index.html`.
fn resolve_static_dir(cli: Option<PathBuf>) -> Option<PathBuf> {
    if cli.is_some() {
        return cli;
    }
    let bundled = std::env::current_exe()
        .ok()?
        .parent()?
        .parent()?
        .join("frontend");
    bundled.join("index.html").is_file().then_some(bundled)
}

async fn healthz() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn snapshot(State(aggregator): State<Aggregator>) -> Json<DashboardSnapshot> {
    Json(aggregator.snapshot())
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
}
