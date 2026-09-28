use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::extract::State;
use axum::{Json, Router, routing::get};
use clap::Parser;
use pending_core::{DEFAULT_LANE, DashboardSnapshot, SampleSource};
use pending_runtime::{Aggregator, SourceSpec};
use serde::Serialize;
use tower_http::trace::TraceLayer;
use tracing::info;

#[derive(Debug, Parser)]
#[command(name = "pending-api", about = "TasksPending HTTP API")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: SocketAddr,
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

    // Until config loading exists, the API serves the built-in sample source.
    let aggregator = Aggregator::start(
        vec![SourceSpec {
            source: Arc::new(SampleSource),
            lane: DEFAULT_LANE.to_owned(),
            interval: Duration::from_secs(300),
        }],
        Duration::from_secs(30),
    );

    let listener = tokio::net::TcpListener::bind(cli.listen)
        .await
        .with_context(|| format!("binding {}", cli.listen))?;
    info!(addr = %cli.listen, "pending-api listening");

    axum::serve(listener, app(aggregator))
        .await
        .context("serving API")?;
    Ok(())
}

fn app(aggregator: Aggregator) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/snapshot", get(snapshot))
        .layer(TraceLayer::new_for_http())
        .with_state(aggregator)
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
                source: Arc::new(SampleSource),
                lane: "Inbox".to_owned(),
                interval: Duration::from_secs(300),
            }],
            Duration::from_secs(30),
        );
        tokio::time::sleep(Duration::from_millis(10)).await;

        let response = app(aggregator)
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
}
